#![cfg(feature = "cli")]
//! GraphQL through the CLI: diff report/JSON/gates, focused context, Git refs,
//! directory merging and native Markdown/Skill rendering.
use assert_cmd::Command as AssertCommand;
use predicates::{prelude::PredicateBooleanExt, str::contains};
use serde_json::Value;
use std::{fs, path::Path, process::Command};

fn fixture(n: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/graphql")
        .join(n)
}
fn binary() -> AssertCommand {
    AssertCommand::new(assert_cmd::cargo::cargo_bin!("vimanam"))
}
const OLD: &str = "spacex-land-64a7fa9.graphql";
const NEW: &str = "spacex-land-0a482d1.graphql";

#[test]
fn markdown_report_groups_by_verdict() {
    let out = binary()
        .args(["diff"])
        .arg(fixture(OLD))
        .arg(fixture(NEW))
        .arg("--report")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    assert!(text.starts_with("# GraphQL schema changes\n"));
    assert!(text.contains("## Breaking (1)\n\n- `Result.count` — `FIELD_REMOVED` (field_removed)"));
    assert!(text.contains("## Non-breaking (8)"));
    assert!(text.contains("| Root fields | 33 | 37 |"), "{text}");
    let hist = text.find("`HistoriesResult`").unwrap();
    let ships = text.find("`ShipsResult`").unwrap();
    assert!(hist < ships, "rows sorted by coordinate");
}

#[test]
fn json_is_contract_v2_and_fail_on_breaking_exits_3() {
    let out = binary()
        .args(["diff"])
        .arg(fixture(OLD))
        .arg(fixture(NEW))
        .args(["--format", "json", "--fail-on-breaking"])
        .assert()
        .code(3)
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(json["schema_version"], 2);
    assert_eq!(json["new"]["contract"]["format"], "graphql");
    assert_eq!(json["summary"]["breaking"], 1);
    assert_eq!(
        json["old"]["file_sha256"],
        vimanam::diff::json::sha256_hex(&fs::read(fixture(OLD)).unwrap())
    );
    // Non-breaking direction passes the gate.
    binary()
        .args(["diff"])
        .arg(fixture(NEW))
        .arg(fixture(NEW))
        .arg("--fail-on-breaking")
        .assert()
        .success()
        .stdout(contains("No contract changes."));
}

#[test]
fn introspection_json_inputs_are_sniffed() {
    let out = binary()
        .args(["diff"])
        .arg(fixture("spacex-land-64a7fa9.introspection.json"))
        .arg(fixture(NEW))
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(json["changes"].as_array().unwrap().len(), 9);
}

#[test]
fn unsupported_options_fail_before_output_creation() {
    let t = tempfile::tempdir().unwrap();
    for (args, message) in [
        (vec!["--format", "json", "--contract", "1"], "OpenAPI-only"),
        (vec!["--breaking", "wire"], "protobuf only"),
        (vec!["--format", "json", "--context"], "cannot be combined"),
    ] {
        let output = t.path().join("out");
        binary()
            .args(["diff"])
            .arg(fixture(OLD))
            .arg(fixture(NEW))
            .args(&args)
            .arg("-o")
            .arg(&output)
            .assert()
            .failure()
            .stderr(contains(message));
        assert!(!output.exists(), "{args:?}");
    }
    // Mixed formats are refused.
    binary()
        .args(["diff"])
        .arg(fixture(OLD))
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/petstore_oas3.json"))
        .assert()
        .failure()
        .stderr(contains("different contract formats"));
    // --context is GraphQL-only.
    binary()
        .args(["diff"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/diff_old_oas3.json"))
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/diff_new_oas3.json"))
        .arg("--context")
        .assert()
        .failure()
        .stderr(contains("GraphQL-only"));
    // --max-tokens requires --context.
    binary()
        .args(["diff"])
        .arg(fixture(OLD))
        .arg(fixture(NEW))
        .args(["--max-tokens", "100"])
        .assert()
        .code(2);
}

#[test]
fn focused_context_respects_max_tokens() {
    let full = binary()
        .args(["diff"])
        .arg(fixture(OLD))
        .arg(fixture(NEW))
        .arg("--context")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let full = String::from_utf8(full).unwrap();
    let context = &full[full.find("## Focused context\n").unwrap()..];
    assert!(context.contains("### Query.launchesPastResult"));
    assert!(context.contains("totalCount"));
    assert!(!context.contains("### Query.launches "));
    let budgeted = binary()
        .args(["diff"])
        .arg(fixture(OLD))
        .arg(fixture(NEW))
        .args(["--context", "--max-tokens", "500"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let budgeted = String::from_utf8(budgeted).unwrap();
    let section = &budgeted[budgeted.find("## Focused context\n\n").unwrap() + 20..];
    assert!(vimanam::markdown::estimate_tokens(section.as_bytes()) <= 500);
    assert!(section.contains("Query.historiesResult"));
}

#[test]
fn directory_inputs_merge_sdl_files() {
    let t = tempfile::tempdir().unwrap();
    for (version, extra) in [("old", ""), ("new", "\nextend type Query { pong: Int! }\n")] {
        let dir = t.path().join(version).join("nested");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.graphqls"), "type Query { ping: Int }").unwrap();
        fs::write(
            t.path().join(version).join("b.graphql"),
            format!("extend type Query {{ x: String }}{extra}"),
        )
        .unwrap();
        fs::write(t.path().join(version).join("notes.txt"), "ignored").unwrap();
    }
    let out = binary()
        .args(["diff"])
        .arg(t.path().join("old"))
        .arg(t.path().join("new"))
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(json["summary"]["operations_added"], 1);
    assert_eq!(json["changes"][0]["native"]["coordinate"], "Query.pong");
    binary()
        .arg(t.path().join("new"))
        .args(["--operation", "QUERY Query.pong", "--no-report"])
        .assert()
        .success()
        .stdout(contains("* Query.pong"))
        .stdout(contains("Query.ping").not());
}

fn git(root: &Path, args: &[&str]) {
    let o = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

#[test]
fn git_refs_use_committed_schema_bytes() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.name", "Public test"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    fs::create_dir(root.join("api")).unwrap();
    fs::copy(fixture(OLD), root.join("api/schema.graphql")).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "old"]);
    git(root, &["tag", "old"]);
    fs::copy(fixture(NEW), root.join("api/schema.graphql")).unwrap();
    git(root, &["commit", "-qam", "new"]);
    git(root, &["tag", "new"]);
    fs::write(root.join("api/schema.graphql"), "broken worktree bytes").unwrap();
    let out = binary()
        .current_dir(root)
        .args([
            "diff",
            "--from-ref",
            "old",
            "--to-ref",
            "new",
            "--spec",
            "api/schema.graphql",
            "--format",
            "json",
            "--fail-on-breaking",
        ])
        .assert()
        .code(3)
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(json["changes"].as_array().unwrap().len(), 9);
    // Committed introspection JSON is recognised from its bytes.
    fs::copy(
        fixture("spacex-land-64a7fa9.introspection.json"),
        root.join("api/schema.json"),
    )
    .unwrap();
    git(root, &["checkout", "-q", "api/schema.graphql"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "introspection"]);
    binary()
        .current_dir(root)
        .args([
            "diff",
            "--from-ref",
            "HEAD",
            "--to-ref",
            "HEAD",
            "--from-spec",
            "api/schema.json",
            "--to-spec",
            "api/schema.graphql",
        ])
        .assert()
        .success()
        .stdout(contains("## Breaking (1)"));
}

#[test]
fn native_markdown_budget_and_skill_tree() {
    let out = binary()
        .arg(fixture(NEW))
        .args([
            "--operation",
            "QUERY Query.shipsResult",
            "--detail",
            "full",
            "--include-schemas",
            "--no-report",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("**Operation:** QUERY Query.shipsResult"));
    assert!(text.contains("#### Arguments"));
    assert!(text.contains("| `limit` | argument | No | `Int` |"));
    assert!(text.contains("#### Result"));
    assert!(text.contains("| result | graphql | `ShipsResult` |"));
    assert!(text.contains("ShipsResult"));
    assert!(!text.contains("| 200 |"));
    assert!(!text.contains("### Query.rockets "));
    binary()
        .arg(fixture("spacex-land-0a482d1.introspection.json"))
        .args([
            "--operation-id",
            "Query.shipsResult",
            "--max-tokens",
            "100",
            "--no-report",
        ])
        .assert()
        .success();
    binary()
        .arg(fixture(NEW))
        .args(["--schema", "ShipsResult", "--no-report"])
        .assert()
        .success()
        .stdout(contains("GraphQL object"));
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().canonicalize().unwrap().join("skill");
    binary()
        .arg(fixture(NEW))
        .args([
            "--output-mode",
            "skill",
            "--detail",
            "full",
            "--include-schemas",
            "-o",
        ])
        .arg(&dir)
        .assert()
        .success();
    assert!(
        fs::read_to_string(dir.join("SKILL.md"))
            .unwrap()
            .contains("GraphQL schema")
    );
    assert!(dir.join("schemas").is_dir());
}

#[test]
fn group_by_method_keeps_graphql_operations() {
    binary()
        .arg(fixture(NEW))
        .args([
            "--group-by",
            "method",
            "--detail",
            "standard",
            "--no-report",
        ])
        .assert()
        .success()
        .stdout(contains("## QUERY {#query}"))
        .stdout(contains("### Query.shipsResult"));
}

#[test]
fn focused_context_overage_notice_goes_to_stderr() {
    binary()
        .args(["diff"])
        .arg(fixture(OLD))
        .arg(fixture(NEW))
        .args(["--context", "--max-tokens", "5"])
        .assert()
        .success()
        .stderr(contains("5-token budget"));
}

#[test]
fn missing_graphql_file_is_labelled_graphql() {
    binary()
        .args(["diff", "missing-old.graphql", "missing-new.graphql"])
        .assert()
        .failure()
        .stderr(contains("GraphQL schema"));
}
