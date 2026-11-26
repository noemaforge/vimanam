#![cfg(feature = "cli")]
//! `diff --format json --omit-surfaces` (#159): the v2 document without the
//! `old.contract` / `new.contract` surfaces must otherwise equal the default
//! document of the same comparison (hashes, summary, changes and every ID in
//! them, deltas), with the same exit code, for OpenAPI, GraphQL and protobuf,
//! from file pairs and Git refs. Elsewhere the flag is rejected before any
//! output exists.
use assert_cmd::Command as AssertCommand;
use serde_json::Value;
use std::{fs, path::Path, path::PathBuf, process::Command};

fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(path)
}

fn binary() -> AssertCommand {
    AssertCommand::new(assert_cmd::cargo::cargo_bin!("vimanam"))
}

/// Runs `vimanam diff <args> --format json`, with and without
/// `--omit-surfaces`, from `dir`; returns (exit code, stdout) of each.
fn run_both(dir: Option<&Path>, args: &[&str]) -> ((i32, Vec<u8>), (i32, Vec<u8>)) {
    let run = |omit: bool| {
        let mut command = binary();
        if let Some(dir) = dir {
            command.current_dir(dir);
        }
        command.arg("diff").args(args).args(["--format", "json"]);
        if omit {
            command.arg("--omit-surfaces");
        }
        let output = command.output().unwrap();
        assert!(
            matches!(output.status.code(), Some(0 | 3)),
            "{args:?} omit={omit}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        (output.status.code().unwrap(), output.stdout)
    };
    (run(false), run(true))
}

/// Every `ve2_` ID mentioned anywhere in `value`.
fn entity_ids(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) if s.starts_with("ve2_") => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|v| entity_ids(v, out)),
        Value::Object(map) => map.values().for_each(|v| entity_ids(v, out)),
        _ => {}
    }
}

/// Asserts the omitted document is the full one minus the surfaces, with the
/// marker, the documented key order and the same exit code. Returns the
/// full document.
fn assert_same_but_surfaces(dir: Option<&Path>, args: &[&str]) -> Value {
    let ((full_code, full_bytes), (omit_code, omit_bytes)) = run_both(dir, args);
    assert_eq!(full_code, omit_code, "{args:?}: exit codes differ");
    let mut full: Value = serde_json::from_slice(&full_bytes).unwrap();
    let omitted: Value = serde_json::from_slice(&omit_bytes).unwrap();
    assert!(
        full.get("surfaces_omitted").is_none(),
        "default gained a field"
    );
    for side in ["old", "new"] {
        assert!(full[side]["contract"]["entities"].is_array());
        let keys: Vec<_> = omitted[side].as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            ["file_sha256"],
            "{args:?}: {side} kept more than its hash"
        );
    }
    let keys: Vec<_> = omitted.as_object().unwrap().keys().cloned().collect();
    let mut expected = vec![
        "schema_version",
        "generator",
        "surfaces_omitted",
        "old",
        "new",
        "summary",
        "changes",
    ];
    if full.get("deltas").is_some() {
        expected.push("deltas");
    }
    assert_eq!(keys, expected);
    assert_eq!(omitted["surfaces_omitted"], Value::Bool(true));

    // Every entity a change refers to is a real entity of the full surfaces.
    let mut referenced = vec![];
    entity_ids(&full["changes"], &mut referenced);
    let known: std::collections::BTreeSet<_> = ["old", "new"]
        .iter()
        .flat_map(|side| full[side]["contract"]["entities"].as_array().unwrap())
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    assert!(referenced.iter().all(|id| known.contains(id)));

    let surfaces_free = {
        let mut stripped = full.clone();
        for side in ["old", "new"] {
            stripped[side].as_object_mut().unwrap().remove("contract");
        }
        let mut omitted = omitted.clone();
        omitted.as_object_mut().unwrap().remove("surfaces_omitted");
        (stripped, omitted)
    };
    assert_eq!(surfaces_free.0, surfaces_free.1, "{args:?}");
    full.as_object_mut().unwrap().remove("surfaces_omitted");
    full
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn openapi_file_pairs_match_the_full_document() {
    for (old, new) in [
        ("diff_old_oas3.json", "diff_new_oas3.json"),
        ("diff_old_oas2.json", "diff_new_oas2.json"),
    ] {
        let (old, new) = (fixture(old), fixture(new));
        for extra in [
            &[][..],
            &["--report"],
            &["--fail-on-breaking"],
            &["--exact-paths", "--report", "--fail-on-breaking"],
        ] {
            let mut args = vec![path(&old), path(&new)];
            args.extend_from_slice(extra);
            let full = assert_same_but_surfaces(None, &args);
            assert!(full["summary"]["breaking"].as_u64().unwrap() > 0);
        }
    }
    // A breaking diff exits 3 under the flag too.
    binary()
        .arg("diff")
        .args([fixture("diff_old_oas3.json"), fixture("diff_new_oas3.json")])
        .args(["--format", "json", "--omit-surfaces", "--fail-on-breaking"])
        .assert()
        .code(3);
}

#[test]
fn output_file_matches_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("diff.json");
    let args = |cmd: &mut AssertCommand| {
        cmd.arg("diff")
            .args([fixture("diff_old_oas3.json"), fixture("diff_new_oas3.json")])
            .args(["--format", "json", "--omit-surfaces", "--report"]);
    };
    let mut to_file = binary();
    args(&mut to_file);
    to_file.arg("-o").arg(&out).assert().success();
    let mut to_stdout = binary();
    args(&mut to_stdout);
    let stdout = to_stdout.assert().success().get_output().stdout.clone();
    assert_eq!(fs::read(&out).unwrap(), stdout);
}

#[test]
fn graphql_file_pairs_match_the_full_document() {
    for (old, new) in [
        ("spacex-land-64a7fa9.graphql", "spacex-land-0a482d1.graphql"),
        (
            "spacex-land-64a7fa9.introspection.json",
            "spacex-land-0a482d1.introspection.json",
        ),
    ] {
        let (old, new) = (
            fixture(&format!("graphql/{old}")),
            fixture(&format!("graphql/{new}")),
        );
        let full = assert_same_but_surfaces(None, &[path(&old), path(&new), "--fail-on-breaking"]);
        assert!(full["summary"]["breaking"].as_u64().unwrap() > 0);
    }
}

/// A protobuf pair with a wire-and-source break (field type) and a
/// source-only break (same-number rename), so each `--breaking` scope has a
/// different gate.
fn proto_pair(dir: &Path) -> (PathBuf, PathBuf) {
    let old = dir.join("old.proto");
    let new = dir.join("new.proto");
    fs::write(
        &old,
        "syntax = \"proto3\"; package p; message M { int32 id = 1; string name = 2; }\n\
         service S { rpc Get(M) returns (M); }\n",
    )
    .unwrap();
    fs::write(
        &new,
        "syntax = \"proto3\"; package p; message M { int32 id = 1; string title = 2; }\n\
         service S { rpc Get(M) returns (M); }\n",
    )
    .unwrap();
    (old, new)
}

#[test]
fn protobuf_file_pairs_match_the_full_document_under_every_gate() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = proto_pair(dir.path());
    let pb = (
        fixture("protobuf/health-v1.0.0.pb"),
        fixture("protobuf/health-v1.20.0.pb"),
    );
    let src = (
        fixture("protobuf/health-v1.0.0.proto"),
        fixture("protobuf/health-v1.20.0.proto"),
    );
    let mut codes = vec![];
    for scope in ["all", "wire", "source"] {
        for (old, new) in [(&old, &new), (&pb.0, &pb.1), (&src.0, &src.1)] {
            let args = [
                path(old),
                path(new),
                "--breaking",
                scope,
                "--fail-on-breaking",
            ];
            assert_same_but_surfaces(None, &args);
        }
        let ((code, _), _) = run_both(
            None,
            &[
                path(&old),
                path(&new),
                "--breaking",
                scope,
                "--fail-on-breaking",
            ],
        );
        codes.push(code);
    }
    // The rename is source-only, so the wire gate passes while the others trip.
    assert_eq!(codes, [3, 0, 3]);
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn git_refs_match_the_full_document() {
    if !Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        eprintln!("Skipping Git integration test: git is unavailable on PATH");
        return;
    }
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    for (key, value) in [
        ("user.name", "Test"),
        ("user.email", "test@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.autocrlf", "false"),
    ] {
        git(root, &["config", key, value]);
    }
    let scratch = tempfile::tempdir().unwrap();
    let (old_proto, new_proto) = proto_pair(scratch.path());
    let sides = [
        (
            "old",
            fixture("diff_old_oas3.json"),
            fixture("graphql/spacex-land-64a7fa9.graphql"),
            old_proto,
        ),
        (
            "new",
            fixture("diff_new_oas3.json"),
            fixture("graphql/spacex-land-0a482d1.graphql"),
            new_proto,
        ),
    ];
    for (tag, openapi, graphql, proto) in &sides {
        fs::copy(openapi, root.join("openapi.json")).unwrap();
        fs::copy(graphql, root.join("schema.graphql")).unwrap();
        fs::copy(proto, root.join("api.proto")).unwrap();
        git(root, &["add", "--all"]);
        git(root, &["commit", "-qm", tag]);
        git(root, &["tag", tag]);
    }
    // The worktree must never be read in Git-ref mode.
    for file in ["openapi.json", "schema.graphql", "api.proto"] {
        fs::write(root.join(file), "broken worktree bytes").unwrap();
    }
    let refs = ["--from-ref", "old", "--to-ref", "new", "--fail-on-breaking"];
    for spec in ["openapi.json", "schema.graphql"] {
        let mut args = refs.to_vec();
        args.extend(["--spec", spec]);
        assert_same_but_surfaces(Some(root), &args);
    }
    for scope in ["all", "wire", "source"] {
        let mut args = refs.to_vec();
        args.extend(["--spec", "api.proto", "--breaking", scope]);
        assert_same_but_surfaces(Some(root), &args);
    }
}

#[test]
fn rejected_outside_v2_json_before_any_output() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("diff.out");
    let (proto_old, proto_new) = proto_pair(dir.path());
    let pairs = [
        (fixture("diff_old_oas3.json"), fixture("diff_new_oas3.json")),
        (
            fixture("graphql/spacex-land-64a7fa9.graphql"),
            fixture("graphql/spacex-land-0a482d1.graphql"),
        ),
        (proto_old, proto_new),
    ];
    for (old, new) in &pairs {
        for (args, message) in [
            (
                &["--format", "markdown"][..],
                "--omit-surfaces requires --format json",
            ),
            (&[][..], "--omit-surfaces requires --format json"),
            (
                &["--format", "json", "--contract", "1"][..],
                "--omit-surfaces requires --contract 2",
            ),
        ] {
            let output = binary()
                .arg("diff")
                .args([old, new])
                .args(args)
                .args(["--omit-surfaces", "--fail-on-breaking", "-o"])
                .arg(&out)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(1), "{args:?}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(message), "{args:?}: {stderr}");
            assert!(output.stdout.is_empty());
            assert!(!out.exists(), "{args:?}: output created");
        }
    }
}

#[test]
fn library_without_surfaces_keeps_everything_else() {
    use vimanam::contract::json::{JsonDiffWithoutSurfaces, openapi};
    let old = vimanam::parse_openapi(fixture("diff_old_oas3.json")).unwrap();
    let new = vimanam::parse_openapi(fixture("diff_new_oas3.json")).unwrap();
    let deltas = vimanam::diff::compute_deltas(&old, &new).unwrap();
    let full = openapi(&old, &new, Some(&deltas), "a", "b").unwrap();
    let mut expected = serde_json::to_value(&full).unwrap();
    for side in ["old", "new"] {
        expected[side].as_object_mut().unwrap().remove("contract");
    }
    expected["surfaces_omitted"] = Value::Bool(true);
    let breaking = full.has_breaking();
    let omitted = full.without_surfaces();
    assert_eq!(omitted.has_breaking(), breaking);
    let value = serde_json::to_value(&omitted).unwrap();
    assert_eq!(value, expected);
    // The surface-free shape round-trips through its own type.
    let parsed: JsonDiffWithoutSurfaces = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), value);
}
