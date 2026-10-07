//! Exercises the public API as a downstream crate, also without CLI features.

use serde_json::Value;
use vimanam::diff::json::{JsonDiff, JsonPresence, change_id, sha256_hex, to_json};
use vimanam::diff::{ChangeKind, compute_deltas, diff};
use vimanam::markdown::{estimate_tokens, generate_markdown, generate_markdown_with_notices};
use vimanam::{DocConfig, OperationRef, OperationSelector, parse_openapi, parse_openapi_bytes};

const OLD: &str = "tests/fixtures/diff_old_oas3.json";
const NEW: &str = "tests/fixtures/diff_new_oas3.json";

#[test]
fn bytes_and_file_parsers_produce_identical_diff_documents() {
    let old_bytes = std::fs::read(OLD).unwrap();
    let new_bytes = std::fs::read(NEW).unwrap();
    let old = parse_openapi(OLD).unwrap();
    let new = parse_openapi(NEW).unwrap();
    let bytes_old = parse_openapi_bytes(&old_bytes, "json", None).unwrap();
    let bytes_new = parse_openapi_bytes(&new_bytes, "JSON", None).unwrap();
    let delta = diff(&old, &new);
    assert_eq!(delta, diff(&bytes_old, &bytes_new));
    assert!(delta.has_breaking());
    // Growing enums are usable from another crate with a wildcard arm.
    assert!(
        delta
            .changes
            .iter()
            .any(|change| matches!(change.kind, ChangeKind::EndpointAdded))
    );
    let document = to_json(
        &delta,
        Some(&compute_deltas(&old, &new).unwrap()),
        &sha256_hex(&old_bytes),
        &sha256_hex(&new_bytes),
    );
    let bytes = serde_json::to_vec_pretty(&document).unwrap();
    let decoded: JsonDiff = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(bytes, serde_json::to_vec_pretty(&decoded).unwrap());
    assert_eq!(decoded.schema_version, 1);
    for change in decoded.changes {
        assert_eq!(change.id, change_id(&change));
    }
}

#[test]
fn presence_roundtrips_absent_null_and_non_null_values() {
    for json in [
        r#"{"present":false}"#,
        r#"{"present":true,"value":null}"#,
        r#"{"present":true,"value":{"field":null}}"#,
    ] {
        let presence: JsonPresence = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_string(&presence).unwrap(), json);
        assert_eq!(presence.value.is_some(), presence.present);
    }
    let null: JsonPresence = serde_json::from_str(r#"{"present":true,"value":null}"#).unwrap();
    assert_eq!(null.value, Some(Value::Null));
}

#[test]
fn focused_operations_and_schema_subtrees_are_validated_before_writes() {
    let doc = parse_openapi("tests/fixtures/operation_select_oas3.json").unwrap();
    let mut config = DocConfig::unfiltered();
    config.operation_selector = Some(OperationSelector {
        operations: [OperationRef {
            method: "GET".into(),
            path: "/users".into(),
        }]
        .into_iter()
        .collect(),
        operation_ids: ["createUser".to_string()].into_iter().collect(),
    });
    vimanam::selection::validate(&doc, &config).unwrap();
    let mut output = Vec::new();
    generate_markdown(&mut output, &doc, &config).unwrap();
    let body = String::from_utf8(output).unwrap();
    assert!(body.contains("listUsers"));
    assert!(body.contains("createUser"));
    assert!(!body.contains("getUser"));

    config
        .operation_selector
        .as_mut()
        .unwrap()
        .operation_ids
        .insert("missingOperation".into());
    let mut output = Vec::new();
    assert!(generate_markdown(&mut output, &doc, &config).is_err());
    assert!(output.is_empty());

    let doc = parse_openapi("tests/fixtures/schema_selection_oas3.json").unwrap();
    let mut config = DocConfig::unfiltered();
    config.schema_fields = vec!["Root#/properties/selected".into()];
    config.max_tokens = Some(0);
    config.schema_depth = Some(0);
    let mut output = Vec::new();
    let mut notices = Vec::new();
    generate_markdown_with_notices(&mut output, &doc, &config, &mut |s| {
        notices.push(s.to_string())
    })
    .unwrap();
    assert!(estimate_tokens(&output) > 0);
    let body = String::from_utf8(output).unwrap();
    assert!(body.contains("selected"));
    assert!(body.contains("Root metadata"));
    assert!(!body.contains("secret"));
    assert!(
        notices
            .iter()
            .any(|s| s.contains("preserving requested selection"))
    );
    config.schema_fields = vec!["Root#/properties/missing".into()];
    let mut output = Vec::new();
    assert!(generate_markdown(&mut output, &doc, &config).is_err());
    assert!(output.is_empty());
}

#[test]
fn notices_are_returned_without_console_writes() {
    // A subprocess lets us observe the actual process stdout and stderr.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "quiet_library_probe", "--nocapture", "--quiet"])
        .env("VIMANAM_LIBRARY_QUIET_PROBE", "1")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Only the test harness may write stdout. Any library text adds a line or
    // changes one of these exact harness lines.
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<_> = stdout.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), 3, "unexpected stdout: {stdout}");
    assert_eq!(lines[0], "running 1 test");
    assert_eq!(lines[1], ".");
    assert!(lines[2].starts_with("test result: ok. 1 passed; 0 failed;"));
}

#[test]
fn quiet_library_probe() {
    if std::env::var_os("VIMANAM_LIBRARY_QUIET_PROBE").is_none() {
        return;
    }
    let doc = parse_openapi("tests/fixtures/schema_selection_oas3.json").unwrap();
    let mut config = DocConfig::unfiltered();
    config.schema_fields = vec!["Root#/properties/selected".into()];
    config.schema_depth = Some(0);
    config.max_tokens = Some(0);
    generate_markdown(&mut Vec::new(), &doc, &config).unwrap();
    config.schema_fields.clear();
    config.max_tokens = None;
    generate_markdown(&mut Vec::new(), &doc, &config).unwrap();
    config.schema_depth = None;
    config.max_tokens = Some(0);
    generate_markdown(&mut Vec::new(), &doc, &config).unwrap();
}

#[cfg(feature = "cli")]
#[test]
fn cli_json_matches_library_bytes_and_deserializes_into_the_same_contract() {
    for (old_path, new_path) in [
        (OLD, NEW),
        (
            "tests/fixtures/diff_old_oas2.json",
            "tests/fixtures/diff_new_oas2.json",
        ),
    ] {
        for report in [false, true] {
            let old_bytes = std::fs::read(old_path).unwrap();
            let new_bytes = std::fs::read(new_path).unwrap();
            let old = parse_openapi_bytes(&old_bytes, "json", None).unwrap();
            let new = parse_openapi_bytes(&new_bytes, "json", None).unwrap();
            let delta = diff(&old, &new);
            let deltas = report.then(|| compute_deltas(&old, &new).unwrap());
            let document = to_json(
                &delta,
                deltas.as_ref(),
                &sha256_hex(&old_bytes),
                &sha256_hex(&new_bytes),
            );
            let mut expected = serde_json::to_vec_pretty(&document).unwrap();
            expected.push(b'\n');
            let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_vimanam"));
            command.args(["diff", old_path, new_path, "--format", "json"]);
            if report {
                command.arg("--report");
            }
            let output = command.output().unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, expected);
            let from_binary: JsonDiff = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                serde_json::to_vec_pretty(&from_binary).unwrap(),
                expected[..expected.len() - 1]
            );
        }
    }
}

#[cfg(feature = "cli")]
#[test]
fn focused_markdown_and_notices_match_the_cli() {
    let path = "tests/fixtures/schema_selection_oas3.json";
    let doc = parse_openapi(path).unwrap();
    let mut config = DocConfig::unfiltered();
    config.source_path = Some(path.into());
    config.schema_fields = vec!["Root#/properties/selected".into()];
    config.max_tokens = Some(0);
    config.schema_depth = Some(0);
    let mut body = Vec::new();
    let mut notices = Vec::new();
    generate_markdown_with_notices(&mut body, &doc, &config, &mut |s| {
        notices.push(s.to_string())
    })
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_vimanam"))
        .args([
            path,
            "--schema-field",
            "Root#/properties/selected",
            "--schema-depth",
            "0",
            "--max-tokens",
            "0",
            "--no-report",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(body, output.stdout);
    assert_eq!(
        format!("{}\n", notices.join("\n")).as_bytes(),
        output.stderr
    );
}
