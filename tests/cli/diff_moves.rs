//! `vimanam diff` across operations moved to a new version prefix (#137).
use crate::common::{vimanam, write_spec};
use serde_json::{Value, json};

fn spec(path: &str, operation_id: &str, paging: bool) -> Value {
    let mut properties =
        json!({"items": {"type": "array", "items": {"$ref": "#/components/schemas/Item"}}});
    let mut operation = json!({"operationId": operation_id});
    if paging {
        properties["next_cursor"] = json!({"type": "string"});
        operation["parameters"] = json!([
            {"name": "cursor", "in": "query", "schema": {"type": "string"}},
            {"name": "limit", "in": "query", "schema": {"type": "integer"}}
        ]);
    }
    operation["responses"] = json!({"200": {"description": "ok", "content": {"application/json":
        {"schema": {"type": "object", "properties": properties}}}}});
    json!({
        "openapi": "3.0.3",
        "info": {"title": "Items", "version": if paging { "2.0.0" } else { "1.0.0" }},
        "paths": {path: {"get": operation}},
        "components": {"schemas": {"Item": {"type": "object", "properties": {"id": {"type": "string"}}}}}
    })
}

fn pair(dir: &tempfile::TempDir) -> (String, String) {
    (
        write_spec(dir, "v1.json", &spec("/v1/items", "listItems", false)),
        write_spec(dir, "v2.json", &spec("/v2/items", "exportItems", true)),
    )
}

fn run(args: &[&str]) -> (Vec<u8>, i32) {
    let output = vimanam().arg("diff").args(args).output().unwrap();
    (output.stdout, output.status.code().unwrap())
}

fn kinds(stdout: &[u8]) -> Vec<String> {
    let document: Value = serde_json::from_slice(stdout).unwrap();
    document["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["kind"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn markdown_reports_the_move_and_its_inner_changes() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = pair(&dir);
    let (stdout, code) = run(&[&old, &new]);
    let stdout = String::from_utf8(stdout).unwrap();
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains(
            "**Summary:** 0 endpoints added, 0 removed, 1 moved, 1 changed; \
             2 breaking, 3 non-breaking, 0 to review\n"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "| Endpoint moved | `GET /v2/items` | from `GET /v1/items` (matched by version segment) |\n\
             | operationId changed | `GET /v2/items` | `listItems` → `exportItems` |\n"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("| Parameter added | `GET /v2/items` | `cursor` (query), optional |"),
        "{stdout}"
    );
    // A move is breaking for --fail-on-breaking.
    assert_eq!(run(&[&old, &new, "--fail-on-breaking"]).1, 3);
}

#[test]
fn exact_paths_restores_removed_plus_added() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = pair(&dir);
    let (stdout, _) = run(&[&old, &new, "--exact-paths"]);
    let stdout = String::from_utf8(stdout).unwrap();
    assert!(
        stdout.contains("**Summary:** 1 endpoint added, 1 removed, 0 changed;"),
        "{stdout}"
    );
    assert!(!stdout.contains("Endpoint moved"), "{stdout}");
    let (json, _) = run(&[&old, &new, "--format", "json", "--exact-paths"]);
    assert_eq!(kinds(&json), ["operation_removed", "operation_added"]);
}

#[test]
fn v2_json_reports_operation_moved_and_pagination() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = pair(&dir);
    let (json, code) = run(&[&old, &new, "--format", "json"]);
    assert_eq!(code, 0);
    assert_eq!(
        kinds(&json),
        [
            "operation_moved",
            "operation_renamed",
            "argument_added",
            "argument_added",
            "field_added",
            "pagination_changed"
        ]
    );
    let document: Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(document["summary"]["operations_changed"], 1);
    assert_eq!(document["summary"]["operations_added"], 0);
    let moved = &document["changes"][0];
    assert_eq!(moved["severity"], "breaking");
    assert_eq!(moved["native"]["path"], "/v2/items");
    assert_eq!(moved["details"]["old_path"], "/v1/items");
    assert_eq!(moved["details"]["matched_by"], "version_segment");
    // Deterministic across runs.
    assert_eq!(json, run(&[&old, &new, "--format", "json"]).0);
}

#[test]
fn v1_json_never_matches_moves() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = pair(&dir);
    let (v1, _) = run(&[&old, &new, "--format", "json", "--contract", "1"]);
    assert_eq!(kinds(&v1), ["endpoint_removed", "endpoint_added"]);
    let (exact, _) = run(&[
        &old,
        &new,
        "--format",
        "json",
        "--contract",
        "1",
        "--exact-paths",
    ]);
    assert_eq!(v1, exact);
}

#[test]
fn exact_paths_is_openapi_only() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("a.graphql");
    let new = dir.path().join("b.graphql");
    std::fs::write(&old, "type Query { a: Int }").unwrap();
    std::fs::write(&new, "type Query { a: Int }").unwrap();
    let output = vimanam()
        .args([
            "diff",
            old.to_str().unwrap(),
            new.to_str().unwrap(),
            "--exact-paths",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--exact-paths is OpenAPI-only"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let help = vimanam().args(["diff", "--help"]).output().unwrap();
    assert!(String::from_utf8_lossy(&help.stdout).contains("--exact-paths"));
}
