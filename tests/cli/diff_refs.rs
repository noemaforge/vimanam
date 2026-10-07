//! CLI adapter coverage; all specs and histories are synthetic scratch repositories.
use std::path::Path;
use std::process::{Command, Output};

use super::common::{DIFF_NEW, DIFF_OLD, vimanam};
use tempfile::TempDir;

struct Repo(TempDir);

impl Repo {
    fn new() -> Option<Self> {
        match Command::new("git").arg("--version").output() {
            Ok(output) if output.status.success() => {}
            _ => {
                eprintln!("Skipping Git CLI integration test: git is unavailable on PATH");
                return None;
            }
        }
        let repo = Self(tempfile::tempdir().unwrap());
        repo.git(&["init", "-q"]);
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "test@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
        ] {
            repo.git(&["config", key, value]);
        }
        Some(repo)
    }

    fn root(&self) -> &Path {
        self.0.path()
    }

    fn git(&self, args: &[&str]) -> Vec<u8> {
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
        output.stdout
    }

    fn write(&self, path: &str, bytes: &[u8]) {
        let path = self.root().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn fixture(&self, path: &str, fixture: &str) {
        self.write(
            path,
            &std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join(fixture)).unwrap(),
        );
    }

    fn commit(&self, tag: &str) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "-qm", tag]);
        self.git(&["tag", tag]);
    }

    fn run(&self, args: &[&str]) -> Output {
        vimanam()
            .current_dir(self.root())
            .arg("diff")
            .args(args)
            .output()
            .unwrap()
    }

    fn pair(&self) {
        self.fixture("api/spec.json", DIFF_OLD);
        self.commit("old");
        self.fixture("api/spec.json", DIFF_NEW);
        self.commit("new");
    }

    fn extracted(&self, old_path: &str, new_path: &str, flags: &[&str]) -> Output {
        // git show's exact committed bytes, not worktree contents.
        self.write(
            "extracted-old.json",
            &self.git(&["show", &format!("old:{old_path}")]),
        );
        self.write(
            "extracted-new.yaml",
            &self.git(&["show", &format!("new:{new_path}")]),
        );
        let mut args = vec!["extracted-old.json", "extracted-new.yaml"];
        args.extend_from_slice(flags);
        self.run(&args)
    }
}

fn failure(output: Output, code: i32, needle: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(code), "{stderr}");
    assert!(
        output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(stderr.contains(needle), "{stderr}");
}

#[test]
fn refs_equal_committed_file_pairs_for_markdown_json_hashes_and_deltas() {
    let Some(repo) = Repo::new() else { return };
    repo.pair();
    // Local edits and untracked candidates must never affect ref selection or hashes.
    repo.write("api/spec.json", b"not committed");
    repo.fixture("untracked.json", DIFF_NEW);
    for flags in [
        vec![],
        vec!["--report"],
        vec!["--format", "json"],
        vec!["--format", "json", "--report"],
    ] {
        let expected = repo.extracted("api/spec.json", "api/spec.json", &flags);
        let mut args = vec!["--from-ref", "old", "--to-ref", "new"];
        args.extend_from_slice(&flags);
        let actual = repo.run(&args);
        assert!(
            actual.status.success(),
            "{}",
            String::from_utf8_lossy(&actual.stderr)
        );
        assert_eq!(actual.stdout, expected.stdout);
    }
    // Discovery works from nested worktree directories; paths remain root-relative.
    let nested = vimanam()
        .current_dir(repo.root().join("api"))
        .args([
            "diff",
            "--from-ref",
            "old",
            "--to-ref",
            "new",
            "--spec",
            "api/spec.json",
        ])
        .output()
        .unwrap();
    assert_eq!(
        nested.stdout,
        repo.extracted("api/spec.json", "api/spec.json", &[]).stdout
    );
    assert!(nested.status.success());
}

#[test]
fn refs_follow_rename_chain_before_major_rewrite_and_parse_historical_formats() {
    let Some(repo) = Repo::new() else { return };
    repo.fixture("api/original.JSON", DIFF_OLD);
    repo.commit("old");
    repo.git(&["mv", "api/original.JSON", "api/intermediate.txt"]);
    repo.commit("rename-one");
    repo.git(&["mv", "api/intermediate.txt", "api/current.YAML"]);
    repo.commit("rename-two");
    let new: serde_json::Value = serde_json::from_slice(
        &std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join(DIFF_NEW)).unwrap(),
    )
    .unwrap();
    repo.write(
        "api/current.YAML",
        serde_norway::to_string(&new).unwrap().as_bytes(),
    );
    repo.commit("new");
    let flags = ["--format", "json", "--report"];
    let expected = repo.extracted("api/original.JSON", "api/current.YAML", &flags);
    for selection in [vec![], vec!["--spec", "api/current.YAML"]] {
        let mut args = vec!["--from-ref", "old", "--to-ref", "new"];
        args.extend_from_slice(&selection);
        args.extend_from_slice(&flags);
        let output = repo.run(&args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected.stdout);
    }
}

#[test]
fn literal_paths_compare_move_and_rewrite_without_similarity_or_history_guessing() {
    let Some(repo) = Repo::new() else { return };
    repo.fixture("before.json", DIFF_OLD);
    repo.commit("old");
    repo.git(&["rm", "before.json"]);
    // No detectable rename, including a new, entirely unrelated filename and bytes.
    repo.write(
        "after.yaml",
        b"openapi: 3.0.3\ninfo: {title: Rebuilt, version: '9'}\npaths: {}\n",
    );
    repo.commit("new");
    failure(
        repo.run(&["--from-ref", "old", "--to-ref", "new"]),
        1,
        "--from-spec PATH --to-spec PATH",
    );
    let expected = repo.extracted("before.json", "after.yaml", &["--format", "json"]);
    let actual = repo.run(&[
        "--from-ref",
        "old",
        "--to-ref",
        "new",
        "--from-spec",
        "before.json",
        "--to-spec",
        "after.yaml",
        "--format",
        "json",
    ]);
    assert!(
        actual.status.success(),
        "{}",
        String::from_utf8_lossy(&actual.stderr)
    );
    assert_eq!(actual.stdout, expected.stdout);
}

#[test]
fn candidate_errors_name_ref_and_all_sorted_paths_and_allow_explicit_selection() {
    let Some(repo) = Repo::new() else { return };
    repo.write("readme.txt", b"no spec");
    repo.commit("empty");
    failure(
        repo.run(&["--from-ref", "empty", "--to-ref", "empty"]),
        1,
        "No tracked OpenAPI candidates at Git ref \"empty\"",
    );
    repo.fixture("z/spec.json", DIFF_OLD);
    repo.fixture("a/spec.yaml", DIFF_OLD);
    repo.commit("multiple");
    let output = repo.run(&["--from-ref", "multiple", "--to-ref", "multiple"]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    failure(
        output,
        1,
        "Ambiguous OpenAPI candidates at Git ref \"multiple\"",
    );
    assert!(stderr.find("a/spec.yaml").unwrap() < stderr.find("z/spec.json").unwrap());
    assert!(
        repo.run(&[
            "--from-ref",
            "multiple",
            "--to-ref",
            "multiple",
            "--spec",
            "z/spec.json"
        ])
        .status
        .success()
    );
}

#[test]
fn git_input_failures_leave_output_untouched() {
    let Some(repo) = Repo::new() else { return };
    repo.pair();
    let cases = [
        (vec!["--from-ref", "unknown", "--to-ref", "new"], "unknown"),
        (vec!["--from-ref", "old", "--to-ref", "unknown"], "unknown"),
        (
            vec![
                "--from-ref",
                "old",
                "--to-ref",
                "new",
                "--spec",
                "missing.json",
            ],
            "missing.json",
        ),
        (
            vec![
                "--from-ref",
                "old",
                "--to-ref",
                "new",
                "--from-spec",
                "missing-old.json",
                "--to-spec",
                "api/spec.json",
            ],
            "missing-old.json",
        ),
        (
            vec![
                "--from-ref",
                "old",
                "--to-ref",
                "new",
                "--from-spec",
                "api/spec.json",
                "--to-spec",
                "missing-new.json",
            ],
            "missing-new.json",
        ),
        (
            vec![
                "--from-ref",
                "old",
                "--to-ref",
                "new",
                "--spec",
                "../escape.json",
            ],
            "repository-relative",
        ),
    ];
    for (mut args, needle) in cases {
        args.extend_from_slice(&["-o", "result.md"]);
        failure(repo.run(&args), 1, needle);
        assert!(!repo.root().join("result.md").exists());
        repo.write("result.md", b"keep me");
        failure(repo.run(&args), 1, needle);
        assert_eq!(
            std::fs::read(repo.root().join("result.md")).unwrap(),
            b"keep me"
        );
        std::fs::remove_file(repo.root().join("result.md")).unwrap();
    }
    repo.write(
        "invalid.yaml",
        b"openapi: 3.0.3\ninfo: {title: Invalid, version: '1'}\npaths: {bad: [}\n",
    );
    repo.commit("invalid");
    failure(
        repo.run(&[
            "--from-ref",
            "old",
            "--to-ref",
            "invalid",
            "--from-spec",
            "api/spec.json",
            "--to-spec",
            "invalid.yaml",
            "-o",
            "result.md",
        ]),
        1,
        "Failed to parse OpenAPI spec at Git ref \"invalid\", path \"invalid.yaml\"",
    );
    assert!(!repo.root().join("result.md").exists());
}

#[test]
fn unresolved_lineage_never_falls_back_to_an_unrelated_spec_or_new_endpoint() {
    let Some(repo) = Repo::new() else { return };
    repo.fixture("unrelated.json", DIFF_OLD);
    repo.commit("old");
    repo.fixture("selected.json", DIFF_NEW);
    repo.commit("new");
    failure(
        repo.run(&[
            "--from-ref",
            "old",
            "--to-ref",
            "new",
            "--spec",
            "selected.json",
            "-o",
            "result.md",
        ]),
        1,
        "Failed to follow spec",
    );
    assert!(!repo.root().join("result.md").exists());
}

#[test]
fn ref_diff_writes_complete_file_before_breaking_exit_three() {
    let Some(repo) = Repo::new() else { return };
    repo.pair();
    for (format, name) in [("markdown", "result.md"), ("json", "result.json")] {
        let expected = repo.extracted(
            "api/spec.json",
            "api/spec.json",
            &["--format", format, "--report", "--fail-on-breaking"],
        );
        assert_eq!(expected.status.code(), Some(3));
        let actual = repo.run(&[
            "--from-ref",
            "old",
            "--to-ref",
            "new",
            "--format",
            format,
            "--report",
            "--fail-on-breaking",
            "-o",
            name,
        ]);
        assert_eq!(
            actual.status.code(),
            Some(3),
            "{}",
            String::from_utf8_lossy(&actual.stderr)
        );
        assert!(actual.stdout.is_empty());
        assert_eq!(
            std::fs::read(repo.root().join(name)).unwrap(),
            expected.stdout
        );
    }
    failure(
        repo.run(&[
            "--from-ref",
            "old",
            "--to-ref",
            "new",
            "-o",
            "missing/result.md",
        ]),
        1,
        "Failed to create output file",
    );
}

#[test]
fn missing_git_and_non_repository_have_clear_errors() {
    let dir = tempfile::tempdir().unwrap();
    let output = vimanam()
        .current_dir(dir.path())
        .env("PATH", "")
        .args(["diff", "--from-ref", "old", "--to-ref", "new"])
        .output()
        .unwrap();
    failure(output, 1, "Git executable not found on PATH");
    if Command::new("git").arg("--version").output().is_ok() {
        let output = vimanam()
            .current_dir(dir.path())
            .args(["diff", "--from-ref", "old", "--to-ref", "new"])
            .output()
            .unwrap();
        failure(output, 1, "Not a Git repository");
    }
}

#[test]
fn invalid_input_forms_are_usage_errors_before_output_creation() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec![],
        vec!["old.json"],
        vec!["--from-ref", "old"],
        vec!["--to-ref", "new"],
        vec![
            "old.json",
            "new.json",
            "--from-ref",
            "old",
            "--to-ref",
            "new",
        ],
        vec!["old.json", "--from-ref", "old", "--to-ref", "new"],
        vec!["--spec", "spec.json"],
        vec!["old.json", "new.json", "--spec", "spec.json"],
        vec![
            "--from-ref",
            "old",
            "--to-ref",
            "new",
            "--from-spec",
            "old.json",
        ],
        vec![
            "--from-ref",
            "old",
            "--to-ref",
            "new",
            "--to-spec",
            "new.json",
        ],
        vec![
            "--from-ref",
            "old",
            "--to-ref",
            "new",
            "--spec",
            "spec.json",
            "--from-spec",
            "old.json",
            "--to-spec",
            "new.json",
        ],
        vec![
            "old.json",
            "new.json",
            "--from-spec",
            "old.json",
            "--to-spec",
            "new.json",
        ],
    ] {
        let output = vimanam()
            .current_dir(dir.path())
            .arg("diff")
            .args(&args)
            .args(["-o", "result.md"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!dir.path().join("result.md").exists());
    }
}
