use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use vimanam::gitrefs::Repository;

const SPEC: &str = r#"{"openapi":"3.0.3","info":{"title":"Test API","version":"1"},"paths":{}}"#;

struct Repo(TempDir);
impl Repo {
    fn new() -> Option<Self> {
        match Command::new("git").arg("--version").output() {
            Ok(output) if output.status.success() => {}
            _ => {
                eprintln!("Skipping Git integration test: git is unavailable on PATH");
                return None;
            }
        }
        let repo = Self(tempfile::tempdir().unwrap());
        repo.git(&["init", "-q"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.invalid"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        Some(repo)
    }
    fn root(&self) -> &Path {
        self.0.path()
    }
    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(self.root())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
    fn write(&self, path: &str, contents: &str) {
        let path = self.root().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    fn commit(&self, message: &str) -> String {
        self.git(&["add", "--all"]);
        self.git(&["commit", "-qm", message]);
        self.git(&["rev-parse", "HEAD"])
    }
    fn api(&self) -> Repository {
        Repository::discover(self.root()).unwrap()
    }
}

#[test]
fn discovery_and_candidate_sniff_are_committed_sorted_and_lightweight() {
    let Some(repo) = Repo::new() else {
        return;
    };
    repo.write("nested/z.JSON", SPEC);
    repo.write(
        "a.yaml",
        "swagger: '2.0'\ninfo: {title: YAML, version: '1'}\npaths: {}\n",
    );
    repo.write("odd.yml", SPEC);
    repo.write("shallow.json", r#"{"openapi":"3.1.0","info":{"title":"x","version":"1"},"paths":{"/x":{"get":"not an operation"}},"components":{"schemas":42}}"#);
    repo.write("package.json", "{\"name\":\"not a spec\"}");
    repo.write("broken.json", "{\"openapi\":\"3.0.0\"");
    repo.write(
        "wrong.json",
        r#"{"openapi":"3.0.3","info":{"title":"x","version":"1"},"paths":[]}"#,
    );
    repo.write(
        "not-marker.json",
        r#"{"info":{"title":"x","version":"1"},"paths":{}}"#,
    );
    repo.write(".gitignore", "ignored.json\n");
    repo.write("ignored.json", SPEC);
    let commit = repo.commit("files");
    repo.write("untracked.json", SPEC);
    repo.write("a.yaml", "working copy is broken");
    let api = Repository::discover(repo.root().join("nested/missing/deep.json")).unwrap();
    assert_eq!(api.root(), repo.root().canonicalize().unwrap());
    assert_eq!(
        api.candidates(&commit).unwrap(),
        ["a.yaml", "nested/z.JSON", "odd.yml", "shallow.json"].map(PathBuf::from)
    );
    assert_eq!(
        api.candidates(&commit).unwrap(),
        api.candidates(&commit).unwrap()
    );
    let spec = api.materialize("HEAD", "nested/z.JSON").unwrap();
    assert_eq!(spec.commit, commit);
    assert_eq!(spec.bytes, SPEC.as_bytes());
    assert_eq!(
        vimanam::parse_openapi_bytes(&spec.bytes, "JSON", Some(&spec.path))
            .unwrap()
            .title,
        "Test API"
    );
    assert!(
        api.materialize("HEAD", "ignored.json")
            .unwrap_err()
            .to_string()
            .contains("missing")
    );
}

#[test]
fn literal_refs_paths_and_diagnostics() {
    let Some(repo) = Repo::new() else {
        return;
    };
    let paths: &[&str] = if cfg!(unix) {
        &[
            "-spec.json",
            ":spec.json",
            "[star]*.json",
            "dir space/api\nfile.json",
        ]
    } else {
        &["-spec.json", "dir space/api file.json"]
    };
    for path in paths {
        repo.write(path, SPEC);
    }
    repo.commit("punctuation");
    repo.git(&["tag", "release-good"]);
    repo.git(&["update-ref", "refs/tags/-odd", "HEAD"]);
    let api = repo.api();
    for path in paths {
        assert_eq!(
            api.materialize("release-good", path).unwrap().bytes,
            SPEC.as_bytes()
        );
    }
    assert_eq!(
        api.materialize("-odd", "-spec.json").unwrap().bytes,
        SPEC.as_bytes()
    );
    assert!(
        api.materialize("--help", "-spec.json")
            .unwrap_err()
            .to_string()
            .contains("ref")
    );
    assert!(
        api.materialize("not-a-ref", "-spec.json")
            .unwrap_err()
            .to_string()
            .contains("ref")
    );
    assert!(
        api.materialize("HEAD", "missing.json")
            .unwrap_err()
            .to_string()
            .contains("missing")
    );
    for path in ["../escape.json", "/absolute.json", ""] {
        assert!(
            api.materialize("HEAD", path)
                .unwrap_err()
                .to_string()
                .contains("repository-relative")
        );
    }
    let outside = tempfile::tempdir().unwrap();
    assert!(
        Repository::discover(outside.path())
            .unwrap_err()
            .to_string()
            .contains("Not a Git repository")
    );
    let bare = tempfile::tempdir().unwrap();
    let status = Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(bare.path())
        .status()
        .unwrap();
    assert!(status.success());
    assert!(
        Repository::discover(bare.path())
            .unwrap_err()
            .to_string()
            .contains("Bare")
    );
}

#[test]
fn follow_renames_in_both_directions_and_ignore_reused_names() {
    let Some(repo) = Repo::new() else {
        return;
    };
    repo.write("old.json", SPEC);
    let old = repo.commit("old");
    repo.git(&["mv", "old.json", "temporary.txt"]);
    repo.commit("first rename");
    std::fs::create_dir(repo.root().join("dir space")).unwrap();
    repo.git(&["mv", "temporary.txt", "dir space/new.yaml"]);
    repo.write("old.json", &SPEC.replace("Test API", "Unrelated"));
    let new = repo.commit("second rename and reuse old name");
    let api = repo.api();
    let followed = api.materialize_follow("old.json", &old, &new).unwrap();
    assert_eq!(followed.path, Path::new("dir space/new.yaml"));
    assert_eq!(followed.bytes, SPEC.as_bytes());
    assert_eq!(
        api.materialize_follow("dir space/new.yaml", &new, &old)
            .unwrap()
            .path,
        Path::new("old.json")
    );
    assert!(
        api.materialize_follow("old.json", &new, &old)
            .unwrap_err()
            .to_string()
            .contains("lineage")
    );
    assert!(
        String::from_utf8(api.materialize(&new, "old.json").unwrap().bytes)
            .unwrap()
            .contains("Unrelated")
    );
}

#[test]
fn linear_history_follows_rename_before_a_major_later_rewrite() {
    let Some(repo) = Repo::new() else {
        return;
    };
    repo.write("old.json", SPEC);
    let old = repo.commit("old release");
    repo.git(&["mv", "old.json", "new.json"]);
    repo.commit("detectable rename");
    for i in 0..8 {
        repo.write("unrelated.txt", &i.to_string());
        repo.commit("unrelated changes");
    }
    let rewritten = format!(
        "{{\"openapi\":\"3.0.3\",\"info\":{{\"title\":\"Rewritten\",\"version\":\"2\"}},\"paths\":{{}},\"description\":\"{}\"}}",
        "major later rewrite ".repeat(200)
    );
    repo.write("new.json", &rewritten);
    let new = repo.commit("rewrite after rename");
    let aggregate = repo.git(&["diff", "--name-status", "-M50%", &old, &new]);
    assert!(aggregate.contains("D\told.json"), "{aggregate}");
    assert!(aggregate.contains("A\tnew.json"), "{aggregate}");
    // User config must not turn the machine history into decorated/copy output.
    repo.git(&["config", "log.decorate", "full"]);
    repo.git(&["config", "diff.renames", "copies"]);
    repo.git(&["config", "diff.renameLimit", "1"]);
    let followed = repo
        .api()
        .materialize_follow("new.json", &new, &old)
        .unwrap();
    assert_eq!(followed.path, Path::new("old.json"));
    assert_eq!(followed.bytes, SPEC.as_bytes());
    assert_eq!(
        repo.api().materialize(&new, "new.json").unwrap().bytes,
        rewritten.as_bytes()
    );
}

#[test]
fn a_copy_and_later_source_deletion_do_not_join_file_lifetimes() {
    let Some(repo) = Repo::new() else {
        return;
    };
    repo.write("old.json", SPEC);
    let old = repo.commit("source");
    repo.write("new.json", SPEC);
    repo.commit("copy while source exists");
    repo.git(&["rm", "old.json"]);
    let new = repo.commit("remove source later");
    assert!(
        repo.api()
            .materialize_follow("new.json", &new, &old)
            .is_err()
    );
    assert_eq!(
        repo.api().materialize(&new, "new.json").unwrap().bytes,
        SPEC.as_bytes()
    );
}

#[cfg(unix)]
#[test]
fn linear_history_parses_newline_and_hash_like_names_literally() {
    let Some(repo) = Repo::new() else {
        return;
    };
    let name = "odd\nR100\n.json";
    repo.write(name, SPEC);
    let old = repo.commit("original");
    let intermediate = "0123456789012345678901234567890123456789";
    repo.git(&["mv", "--", name, intermediate]);
    repo.commit("hash-like path");
    repo.git(&["mv", "--", intermediate, "[star]*.yaml"]);
    let new = repo.commit("literal glob");
    let followed = repo
        .api()
        .materialize_follow("[star]*.yaml", &new, &old)
        .unwrap();
    assert_eq!(followed.path, Path::new(name));
    assert_eq!(followed.bytes, SPEC.as_bytes());
}

#[test]
fn sibling_branches_use_common_lineage_and_merged_duplicates_are_ambiguous() {
    let Some(repo) = Repo::new() else {
        return;
    };
    repo.write("base.json", SPEC);
    let base = repo.commit("base");
    repo.git(&["checkout", "-qb", "left"]);
    repo.git(&["mv", "base.json", "left.json"]);
    let left = repo.commit("left rename");
    repo.git(&["checkout", "-qb", "right", &base]);
    repo.git(&["mv", "base.json", "right.json"]);
    let right = repo.commit("right rename");
    let api = repo.api();
    assert_eq!(
        api.materialize_follow("left.json", &left, &right)
            .unwrap()
            .path,
        Path::new("right.json")
    );
    // Make a merge tree containing both branch results without relying on
    // platform-specific conflict resolution or Git's merge strategy defaults.
    repo.write("left.json", SPEC);
    repo.git(&["add", "left.json"]);
    let tree = repo.git(&["write-tree"]);
    let merge = repo.git(&[
        "commit-tree",
        &tree,
        "-p",
        &left,
        "-p",
        &right,
        "-m",
        "keep both",
    ]);
    assert!(
        api.materialize_follow("base.json", &base, &merge)
            .unwrap_err()
            .to_string()
            .contains("Ambiguous spec paths")
    );
    assert_eq!(
        api.materialize_follow("left.json", &merge, &base)
            .unwrap()
            .path,
        Path::new("base.json")
    );
    assert_eq!(
        api.materialize_follow("right.json", &merge, &left)
            .unwrap()
            .path,
        Path::new("left.json")
    );
}

#[test]
fn merge_cannot_hide_a_deleted_and_reintroduced_file_lifetime() {
    let Some(repo) = Repo::new() else {
        return;
    };
    repo.git(&["checkout", "-qb", "main"]);
    repo.write("api.json", SPEC);
    let base = repo.commit("original spec");
    repo.git(&["checkout", "-qb", "side"]);
    repo.git(&["rm", "api.json"]);
    repo.commit("delete original");
    let unrelated = SPEC.replace("Test API", "Unrelated API");
    repo.write("api.json", &unrelated);
    repo.commit("reintroduce same name");
    repo.git(&["checkout", "main"]);
    repo.write("README.txt", "main branch change\n");
    repo.commit("unrelated main change");
    repo.git(&["merge", "--no-ff", "-m", "merge reintroduced spec", "side"]);
    let merge = repo.git(&["rev-parse", "HEAD"]);
    let api = repo.api();

    for (from, to) in [(&base, &merge), (&merge, &base)] {
        let error = api
            .materialize_follow("api.json", from, to)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("lineage") || error.contains("Ambiguous"),
            "{error}"
        );
    }
    assert_eq!(
        api.materialize(&merge, "api.json").unwrap().bytes,
        unrelated.as_bytes()
    );
}

#[test]
fn deletion_and_readdition_break_lineage_including_unrelated_histories() {
    let Some(repo) = Repo::new() else {
        return;
    };
    repo.write("api.json", SPEC);
    let old = repo.commit("base");
    repo.git(&["rm", "api.json"]);
    repo.commit("delete");
    repo.write("api.json", SPEC);
    let new = repo.commit("reintroduce");
    let api = repo.api();
    for (from, to) in [(&old, &new), (&new, &old)] {
        assert!(
            api.materialize_follow("api.json", from, to)
                .unwrap_err()
                .to_string()
                .contains("lineage")
        );
    }
    repo.git(&["checkout", "--orphan", "unrelated"]);
    let unrelated = repo.commit("unrelated");
    assert!(
        api.materialize_follow("api.json", &old, &unrelated)
            .unwrap_err()
            .to_string()
            .contains("common history")
    );
}

#[test]
fn linked_worktree_discovery_and_non_regular_paths() {
    let Some(repo) = Repo::new() else {
        return;
    };
    repo.write("api.json", SPEC);
    #[cfg(unix)]
    std::os::unix::fs::symlink("api.json", repo.root().join("alias.json")).unwrap();
    repo.commit("spec");
    let other = tempfile::tempdir().unwrap();
    let checkout = other.path().join("linked worktree");
    repo.git(&[
        "worktree",
        "add",
        "--detach",
        checkout.to_str().unwrap(),
        "HEAD",
    ]);
    let api = Repository::discover(checkout.join("api.json")).unwrap();
    assert_eq!(api.root(), checkout.canonicalize().unwrap());
    assert_eq!(api.candidates("HEAD").unwrap(), [PathBuf::from("api.json")]);
    assert_eq!(
        api.materialize("HEAD", "api.json").unwrap().bytes,
        SPEC.as_bytes()
    );
    #[cfg(unix)]
    assert!(
        api.materialize("HEAD", "alias.json")
            .unwrap_err()
            .to_string()
            .contains("not a regular file")
    );
}

#[cfg(unix)]
#[test]
fn missing_git_is_a_contextual_library_error_without_console_output() {
    let Some(repo) = Repo::new() else {
        return;
    };
    // Change PATH only in a child test process, never in this parallel runner.
    let binary = std::env::current_exe().unwrap();
    let output = Command::new(binary)
        .args(["--exact", "no_git_child", "--nocapture"])
        .env("PATH", "")
        .env("VIMANAM_TEST_NO_GIT", repo.root())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stderr.is_empty());
}

#[cfg(unix)]
#[test]
fn no_git_child() {
    if let Some(root) = std::env::var_os("VIMANAM_TEST_NO_GIT") {
        assert!(
            Repository::discover(root)
                .unwrap_err()
                .to_string()
                .contains("Git executable not found on PATH")
        );
    }
}

#[test]
fn shallow_and_unborn_refs_return_errors_instead_of_reading_working_files() {
    let Some(repo) = Repo::new() else {
        return;
    };
    assert!(
        repo.api()
            .candidates("HEAD")
            .unwrap_err()
            .to_string()
            .contains("ref")
    );
    repo.write("api.json", SPEC);
    let old = repo.commit("old");
    repo.write("api.json", &SPEC.replace("Test API", "Updated"));
    repo.commit("new");
    let clone = tempfile::tempdir().unwrap();
    let checkout = clone.path().join("shallow");
    let source = if cfg!(windows) {
        format!(
            "file:///{}",
            repo.root().display().to_string().replace('\\', "/")
        )
    } else {
        format!("file://{}", repo.root().display())
    };
    let output = Command::new("git")
        .args(["clone", "-q", "--depth", "1", "--"])
        .arg(source)
        .arg(&checkout)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let api = Repository::discover(&checkout).unwrap();
    assert_eq!(api.candidates("HEAD").unwrap(), [PathBuf::from("api.json")]);
    assert!(
        api.materialize_follow("api.json", "HEAD", &old)
            .unwrap_err()
            .to_string()
            .contains("shallow")
    );
}

#[test]
fn shallow_history_with_an_available_ancestor_retains_rename_behavior() {
    let Some(repo) = Repo::new() else {
        return;
    };
    repo.write("old.json", SPEC);
    let old = repo.commit("old");
    repo.git(&["mv", "old.json", "new.json"]);
    repo.commit("rename");
    let clone = tempfile::tempdir().unwrap();
    let checkout = clone.path().join("shallow");
    let source = if cfg!(windows) {
        format!(
            "file:///{}",
            repo.root().display().to_string().replace('\\', "/")
        )
    } else {
        format!("file://{}", repo.root().display())
    };
    let output = Command::new("git")
        .args(["clone", "-q", "--depth", "2", "--"])
        .arg(source)
        .arg(&checkout)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let followed = Repository::discover(checkout)
        .unwrap()
        .materialize_follow("new.json", "HEAD", &old)
        .unwrap();
    assert_eq!(followed.path, Path::new("old.json"));
    assert_eq!(followed.bytes, SPEC.as_bytes());
}

#[cfg(unix)]
#[test]
fn byte_paths_survive_git_nul_delimited_output() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let Some(repo) = Repo::new() else {
        return;
    };
    let path = PathBuf::from(OsString::from_vec(b"bad-\xff.json".to_vec()));
    // Some Unix filesystems (including macOS APFS) reject byte filenames.
    // Construct the committed tree directly so the Git API is still tested.
    use std::io::Write;
    use std::process::Stdio;
    let input = |args: &[&str], bytes: &[u8]| {
        let mut child = Command::new("git")
            .current_dir(repo.root())
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(bytes).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    let oid = input(&["hash-object", "-w", "--stdin"], SPEC.as_bytes());
    let mut record = format!("100644 blob {oid}\t").into_bytes();
    record.extend_from_slice(b"bad-\xff.json\0");
    let tree = input(&["mktree", "-z"], &record);
    let commit = repo.git(&["commit-tree", &tree, "-m", "byte filename"]);
    repo.git(&["update-ref", "HEAD", &commit]);
    let api = repo.api();
    assert_eq!(api.candidates("HEAD").unwrap(), std::slice::from_ref(&path));
    assert_eq!(
        api.materialize("HEAD", path).unwrap().bytes,
        SPEC.as_bytes()
    );
}
