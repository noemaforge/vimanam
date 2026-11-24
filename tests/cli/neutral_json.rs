use crate::common::vimanam;
use serde_json::Value;
const OLD: &str = "tests/fixtures/diff_old_oas3.json";
const NEW: &str = "tests/fixtures/diff_new_oas3.json";
fn run(extra: &[&str]) -> std::process::Output {
    vimanam()
        .args(["diff", OLD, NEW, "--format", "json"])
        .args(extra)
        .output()
        .unwrap()
}
#[test]
fn default_is_v2_and_explicit_v2_matches_bytes() {
    let default = run(&[]);
    let explicit = run(&["--contract", "2"]);
    assert_eq!(default.stdout, explicit.stdout);
    let document: Value = serde_json::from_slice(&default.stdout).unwrap();
    assert_eq!(document["schema_version"], 2);
    assert!(
        document["new"]["contract"]["entities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "type")
    );
    assert!(
        document["changes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["id"].as_str().unwrap().starts_with("vc2_"))
    );
    let breaking = document["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["severity"] == "breaking")
        .count();
    assert_eq!(document["summary"]["breaking"], breaking);
    let gated = run(&["--fail-on-breaking"]);
    assert_eq!(gated.status.code(), Some(if breaking > 0 { 3 } else { 0 }));
    assert_eq!(default.stdout, gated.stdout);
}
#[test]
fn explicit_v1_retains_legacy_envelope_and_ids() {
    let output = run(&["--contract", "1"]);
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["schema_version"], 1);
    assert!(
        document["changes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["id"].as_str().unwrap().starts_with("vc1_"))
    );
}
#[test]
fn invalid_contract_is_usage_error_and_report_file_matches_stdout() {
    assert_eq!(run(&["--contract", "3"]).status.code(), Some(2));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diff.json");
    let file = run(&["--report", "-o", path.to_str().unwrap()]);
    assert!(file.status.success());
    let stdout = run(&["--report"]);
    assert_eq!(std::fs::read(path).unwrap(), stdout.stdout);
    let document: Value = serde_json::from_slice(&stdout.stdout).unwrap();
    assert!(document["deltas"].is_object());
}

/// Swagger 2 keeps non-body parameter keywords and unmodelled schema keywords
/// in `extensions` `HashMap`s; v2 JSON used to serialize them in random key
/// order, so identical runs differed byte-for-byte.
#[test]
fn swagger_2_v2_json_is_byte_deterministic() {
    let spec = |page_type: &str| {
        serde_json::json!({
            "swagger": "2.0",
            "info": {"title": "Items", "version": "1"},
            "paths": {"/items": {"get": {
                "operationId": "listItems",
                "parameters": [
                    {"name": "page_token", "in": "query", "type": "string",
                     "format": "byte", "minLength": 1, "maxLength": 64,
                     "pattern": "^[a-z]+$", "default": "a", "x-order": 1},
                    {"name": "page_size", "in": "query", "type": page_type,
                     "format": "int32", "minimum": 1, "maximum": 100,
                     "default": 10, "multipleOf": 1, "x-order": 2}
                ],
                "responses": {"200": {"description": "ok", "schema": {
                    "$ref": "#/definitions/Page"}}}
            }}},
            "definitions": {"Page": {"type": "object", "properties": {
                "items": {"type": "array", "items": {"type": "string",
                    "readOnly": true, "minLength": 1, "maxLength": 9,
                    "pattern": "^x", "x-name": "Item", "uniqueItems": false}},
                "nextPageToken": {"type": "string", "readOnly": true,
                    "minLength": 1, "maxLength": 64, "pattern": "^[a-z]+$",
                    "x-name": "Token", "x-order": 3}
            }}}
        })
    };
    let dir = tempfile::tempdir().unwrap();
    let old = crate::common::write_spec(&dir, "old.json", &spec("integer"));
    let new = crate::common::write_spec(&dir, "new.json", &spec("number"));
    let run = || {
        vimanam()
            .args(["diff", &old, &new, "--format", "json"])
            .output()
            .unwrap()
            .stdout
    };
    let first = run();
    for _ in 0..7 {
        assert_eq!(first, run(), "v2 JSON differed between identical runs");
    }
    // Extension-derived keys serialize sorted.
    let document: Value = serde_json::from_slice(&first).unwrap();
    let argument = document["new"]["contract"]["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "argument" && e["name"] == "page_token")
        .unwrap();
    let keys: Vec<&String> = argument["attributes"]["schema"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
}

// --- OpenAPI 3.1 type arrays (#166) and response enum narrowing (#169) ---

const TYPE_ARRAY: &str = "tests/fixtures/type_array_nullable.json";

/// `(kind, native_kind, pointer, severity)` of one v2 change.
type ChangeRow = (String, String, String, String);

/// v2 JSON changes between the type-array fixture and an edited copy, plus the exit code under
/// `--fail-on-breaking`.
fn type_array_changes(edit: impl FnOnce(&mut Value)) -> (Vec<ChangeRow>, Option<i32>) {
    let dir = tempfile::tempdir().unwrap();
    let mut spec: Value =
        serde_json::from_str(&std::fs::read_to_string(TYPE_ARRAY).unwrap()).unwrap();
    edit(&mut spec);
    let new = dir.path().join("new.json");
    std::fs::write(&new, serde_json::to_string(&spec).unwrap()).unwrap();
    let output = vimanam()
        .args(["diff", TYPE_ARRAY])
        .arg(&new)
        .args(["--format", "json", "--fail-on-breaking"])
        .output()
        .unwrap();
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let changes = document["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                text(&c["kind"]),
                text(&c["native_kind"]),
                text(&c["native"]["pointer"]),
                text(&c["severity"]),
            )
        })
        .collect();
    (changes, output.status.code())
}

fn operation_change<'a>(changes: &'a [ChangeRow], native_kind: &str) -> Vec<&'a ChangeRow> {
    changes.iter().filter(|c| c.1 == native_kind).collect()
}

#[test]
fn v2_type_array_reorder_and_duplicates_are_unchanged() {
    let (changes, code) = type_array_changes(|spec| {
        spec["components"]["schemas"]["Item"]["properties"]["key"]["type"] =
            serde_json::json!(["integer", "string"]);
        spec["components"]["schemas"]["Item"]["properties"]["count"]["type"] =
            serde_json::json!(["null", "integer"]);
    });
    assert_eq!(changes, vec![]);
    assert_eq!(code, Some(0));
}

#[test]
fn v2_type_array_null_follows_nullable_direction() {
    // Response: ["string", "null"] -> "string" is non-breaking.
    let (changes, code) = type_array_changes(|spec| {
        spec["components"]["schemas"]["Item"]["properties"]["label"]["type"] =
            serde_json::json!("string");
    });
    let response = operation_change(&changes, "response_schema_changed");
    assert_eq!(response.len(), 1, "{changes:?}");
    assert_eq!(response[0].0, "nullability_changed");
    assert_eq!(response[0].2, "/properties/label/nullable");
    assert_eq!(response[0].3, "non_breaking");
    assert_eq!(code, Some(0));

    // Request: "string" -> ["string", "null"] is non-breaking too.
    let (changes, code) = type_array_changes(|spec| {
        spec["components"]["schemas"]["ItemInput"]["properties"]["note"]["type"] =
            serde_json::json!(["string", "null"]);
    });
    let request = operation_change(&changes, "request_schema_changed");
    assert_eq!(request.len(), 1, "{changes:?}");
    assert_eq!(request[0].0, "nullability_changed");
    assert_eq!(request[0].3, "non_breaking");
    assert_eq!(code, Some(0));
}

#[test]
fn v2_response_enum_value_removed_is_not_breaking() {
    let (changes, code) = type_array_changes(|spec| {
        spec["components"]["schemas"]["Item"]["properties"]["state"]["enum"] =
            serde_json::json!(["active"]);
    });
    let response = operation_change(&changes, "response_schema_changed");
    assert_eq!(response.len(), 1, "{changes:?}");
    assert_eq!(response[0].0, "enum_value_removed");
    assert_eq!(response[0].3, "non_breaking");
    assert_eq!(code, Some(0));

    let (changes, code) = type_array_changes(|spec| {
        spec["components"]["schemas"]["ItemInput"]["properties"]["status"]["enum"] =
            serde_json::json!(["active"]);
    });
    let request = operation_change(&changes, "request_schema_changed");
    assert_eq!(request.len(), 1, "{changes:?}");
    assert_eq!(request[0].0, "enum_value_removed");
    assert_eq!(request[0].3, "breaking");
    assert_eq!(code, Some(3));
}

#[test]
fn v2_whole_enum_keyword_follows_direction() {
    let drop = |schema: &'static str, field: &'static str| {
        move |spec: &mut Value| {
            spec["components"]["schemas"][schema]["properties"][field]
                .as_object_mut()
                .unwrap()
                .remove("enum");
        }
    };

    let (changes, code) = type_array_changes(drop("Item", "state"));
    let response = operation_change(&changes, "response_schema_changed");
    assert_eq!(response.len(), 1, "{changes:?}");
    assert_eq!(response[0].2, "/properties/state/enum");
    assert_eq!(response[0].3, "review");
    assert_eq!(code, Some(0));

    let (changes, code) = type_array_changes(drop("ItemInput", "status"));
    let request = operation_change(&changes, "request_schema_changed");
    assert_eq!(request.len(), 1, "{changes:?}");
    assert_eq!(request[0].2, "/properties/status/enum");
    assert_eq!(request[0].3, "non_breaking");
    assert_eq!(code, Some(0));
}
