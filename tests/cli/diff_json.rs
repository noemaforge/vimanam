use crate::common::DIFF_NEW;
use crate::common::DIFF_OLD;
use crate::common::OAS2;
use crate::common::diff_run;
use crate::common::load_json;
use crate::common::vimanam;
use crate::common::write_spec;
use predicates::prelude::*;

// ── JSON output (#98) ───────────────────────────────────────────────────────
//
/// Runs `vimanam diff --format json` and returns the parsed document plus the
/// exit code. Fails unless stdout is exactly one parseable JSON document.
fn diff_json_run(args: &[&str]) -> (serde_json::Value, i32) {
    let output = vimanam()
        .arg("diff")
        .args(args)
        .args(["--format", "json"])
        .output()
        .unwrap();
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "stdout must be a single JSON document ({error}): {}",
                String::from_utf8_lossy(&output.stdout)
            )
        });
    (document, output.status.code().unwrap())
}

/// A copy of `value` with every object's keys reversed, recursively.
fn with_reversed_keys(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .rev()
                .map(|(key, item)| (key.clone(), with_reversed_keys(item)))
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(with_reversed_keys).collect())
        }
        other => other.clone(),
    }
}

/// Lowercase-hex SHA-256 of a file's raw bytes, for `file_sha256` checks.
fn sha256_of_file(path: &str) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(std::fs::read(path).unwrap());
    let mut hex = String::new();
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// The `Change` column of the Markdown tables for a JSON record (the inverse
/// of `change_label`, including its `now`-dependent rows).
fn markdown_label(record: &serde_json::Value) -> String {
    let kind = record["kind"].as_str().unwrap();
    let details = &record["details"];
    match kind {
        "endpoint_added" => "Endpoint added",
        "endpoint_removed" => "Endpoint removed",
        "parameter_added" => "Parameter added",
        "parameter_removed" => "Parameter removed",
        "parameter_required_changed" => {
            if details["now_required"].as_bool().unwrap() {
                "Parameter newly required"
            } else {
                "Parameter made optional"
            }
        }
        "parameter_location_changed" => "Parameter location changed",
        "parameter_schema_changed" => "Parameter schema changed",
        "response_added" => "Response added",
        "response_removed" => "Response removed",
        "operation_id_changed" => "operationId changed",
        "deprecated_changed" => {
            if details["now"].as_bool().unwrap() {
                "Marked deprecated"
            } else {
                "Deprecation removed"
            }
        }
        "request_schema_changed" => "Request schema changed",
        "response_schema_changed" => "Response schema changed",
        other => panic!("unknown kind: {other}"),
    }
    .to_string()
}

/// Every `(section, first cell, second cell)` of the Markdown tables.
fn markdown_rows(markdown: &str) -> Vec<(String, String, String)> {
    let mut rows = Vec::new();
    let mut section = String::new();
    for line in markdown.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            section = heading.split(' ').next().unwrap().to_string();
        } else if line.starts_with("| ") && !line.contains("|-") {
            let cells: Vec<&str> = line.split('|').collect();
            if cells[1].trim() == "Check" || cells[1].trim() == "Change" {
                continue; // header rows
            }
            rows.push((
                section.clone(),
                cells[1].trim().to_string(),
                cells[2].trim().trim_matches('`').to_string(),
            ));
        }
    }
    rows
}

/// The change-table rows only (Breaking / Non-breaking / Needs review).
fn markdown_change_rows(markdown: &str) -> Vec<(String, String, String)> {
    markdown_rows(markdown)
        .into_iter()
        .filter(|(section, _, _)| matches!(section.as_str(), "Breaking" | "Non-breaking" | "Needs"))
        .collect()
}

/// The one `schema_change` at `pointer`, or panic.
fn schema_change_of<'a>(document: &'a serde_json::Value, pointer: &str) -> &'a serde_json::Value {
    let mut matches = document["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["details"]["schema_change"]["pointer"].as_str() == Some(pointer))
        .map(|record| &record["details"]["schema_change"]);
    let first = matches.next();
    let second = matches.next();
    assert!(
        second.is_none(),
        "expected exactly one schema_change at {pointer}"
    );
    first.unwrap_or_else(|| panic!("no schema_change at {pointer}"))
}

/// A minimal spec: `GET /things` uses the `Thing` schema for its 200 response
/// and `params` as its operation parameters, so edge cases can vary one part.
fn probe_spec(thing: serde_json::Value, params: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "openapi": "3.0.0",
        "info": {"title": "Probe", "version": "1.0.0"},
        "paths": {
            "/things": {
                "get": {
                    "operationId": "listThings",
                    "parameters": params,
                    "responses": {
                        "200": {
                            "description": "ok",
                            "content": {
                                "application/json": {
                                    "schema": {"$ref": "#/components/schemas/Thing"}
                                }
                            }
                        }
                    }
                }
            }
        },
        "components": {"schemas": {"Thing": thing}}
    })
}

#[test]
fn diff_json_parses_and_summary_matches_the_markdown_line() {
    let (markdown, md_code) = diff_run(&[DIFF_OLD, DIFF_NEW]);
    assert_eq!(md_code, 0);
    let (document, code) = diff_json_run(&[DIFF_OLD, DIFF_NEW]);
    assert_eq!(code, 0);

    // The six numbers of the Markdown summary line, in order.
    let line = markdown
        .lines()
        .find(|line| line.starts_with("**Summary:**"))
        .unwrap();
    let numbers: Vec<u64> = line
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().unwrap())
        .collect();
    let summary = &document["summary"];
    assert_eq!(
        numbers,
        vec![
            summary["endpoints_added"].as_u64().unwrap(),
            summary["endpoints_removed"].as_u64().unwrap(),
            summary["endpoints_changed"].as_u64().unwrap(),
            summary["breaking"].as_u64().unwrap(),
            summary["non_breaking"].as_u64().unwrap(),
            summary["review"].as_u64().unwrap(),
        ]
    );
}

#[test]
fn diff_json_records_match_the_markdown_tables() {
    let (markdown, _) = diff_run(&[DIFF_OLD, DIFF_NEW]);
    let (document, _) = diff_json_run(&[DIFF_OLD, DIFF_NEW]);

    let section_of = |severity: &str| match severity {
        "breaking" => "Breaking",
        "non_breaking" => "Non-breaking",
        "review" => "Needs",
        other => panic!("unknown severity: {other}"),
    };

    let mut expected: Vec<(String, String, String)> = document["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| {
            (
                section_of(record["severity"].as_str().unwrap()).to_string(),
                markdown_label(record),
                format!(
                    "{} {}",
                    record["endpoint"]["method"].as_str().unwrap(),
                    record["endpoint"]["path"].as_str().unwrap()
                ),
            )
        })
        .collect();
    expected.sort();

    let mut actual = markdown_change_rows(&markdown);
    actual.sort();
    assert_eq!(expected, actual);
}

#[test]
fn diff_json_contract_fields_are_well_formed() {
    let (document, code) = diff_json_run(&[DIFF_OLD, DIFF_NEW]);
    assert_eq!(code, 0);

    assert_eq!(document["schema_version"].as_u64().unwrap(), 1);
    assert_eq!(document["generator"]["name"], "vimanam");
    assert_eq!(document["generator"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(document["old"]["title"], "Widgets API");
    assert_eq!(document["old"]["version"], "1.0.0");
    assert_eq!(document["new"]["version"], "1.1.0");
    assert_eq!(document["old"]["file_sha256"], sha256_of_file(DIFF_OLD));
    assert_eq!(document["new"]["file_sha256"], sha256_of_file(DIFF_NEW));
    // 64 lowercase hex characters.
    for side in ["old", "new"] {
        let sha = document[side]["file_sha256"].as_str().unwrap();
        assert_eq!(sha.len(), 64, "{side} file_sha256 must be 64 hex chars");
        assert!(
            sha.bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "{side} file_sha256 must be lowercase hex: {sha}"
        );
    }

    let changes = document["changes"].as_array().unwrap();
    assert!(!changes.is_empty());
    let mut ids: Vec<&str> = Vec::new();
    for record in changes {
        assert!(
            matches!(
                record["severity"].as_str(),
                Some("breaking" | "non_breaking" | "review")
            ),
            "{record}"
        );
        let id = record["id"].as_str().unwrap();
        assert!(id.starts_with("vc1_"), "{id}");
        assert_eq!(id.len(), 68, "{id}");
        assert!(
            id[4..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "ID must be lowercase hex: {id}"
        );
        ids.push(id);
    }
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        changes.len(),
        "IDs must be unique in the document"
    );
}

#[test]
fn diff_json_of_identical_specs_is_empty_and_consistent() {
    let (document, code) = diff_json_run(&[DIFF_OLD, DIFF_OLD]);
    assert_eq!(code, 0);
    assert!(document["changes"].as_array().unwrap().is_empty());
    for key in [
        "endpoints_added",
        "endpoints_removed",
        "endpoints_changed",
        "breaking",
        "non_breaking",
        "review",
    ] {
        assert_eq!(document["summary"][key].as_u64().unwrap(), 0, "{key}");
    }
    assert_eq!(document["old"]["title"], document["new"]["title"]);
    assert_eq!(
        document["old"]["file_sha256"],
        document["new"]["file_sha256"]
    );
    // deltas only under --report
    assert!(document.get("deltas").is_none());
}

#[test]
fn diff_json_report_adds_deltas_and_it_is_omitted_without() {
    let (plain, _) = diff_json_run(&[DIFF_OLD, DIFF_NEW]);
    assert!(
        plain.get("deltas").is_none(),
        "no null deltas key, no key at all"
    );

    let (with_report, _) = diff_json_run(&[DIFF_OLD, DIFF_NEW, "--report"]);
    let deltas = &with_report["deltas"];
    let hygiene = deltas["hygiene"].as_array().unwrap();
    assert!(!hygiene.is_empty());
    assert!(
        hygiene
            .iter()
            .all(|row| row["check"].is_string() && row["old"].is_u64() && row["new"].is_u64())
    );
    assert_eq!(deltas["tokens"]["estimate"], "chars/4");
    assert_eq!(deltas["tokens"]["detail"], "full+schemas");
    assert!(deltas["tokens"]["old"].is_u64() && deltas["tokens"]["new"].is_u64());

    // The hygiene rows keep the Markdown Deltas order.
    let (markdown, _) = diff_run(&[DIFF_OLD, DIFF_NEW, "--report"]);
    let md_checks: Vec<String> = markdown_rows(&markdown)
        .into_iter()
        .filter(|(section, _, _)| section == "Deltas")
        .map(|(_, check, _)| check)
        .collect();
    let json_checks: Vec<String> = hygiene
        .iter()
        .map(|row| row["check"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(json_checks, md_checks);
}

#[test]
fn diff_json_file_output_matches_stdout_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("diff.json");

    vimanam()
        .args([
            "diff",
            DIFF_OLD,
            DIFF_NEW,
            "--format",
            "json",
            "-o",
            out_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    let file_bytes = std::fs::read(&out_path).unwrap();
    let stdout_bytes = vimanam()
        .args(["diff", DIFF_OLD, DIFF_NEW, "--format", "json"])
        .output()
        .unwrap()
        .stdout;
    assert_eq!(file_bytes, stdout_bytes);
    // Pretty-printed with a trailing newline.
    assert!(file_bytes.ends_with(b"\n"));
    let text = String::from_utf8(file_bytes).unwrap();
    assert!(
        text.lines().nth(1).unwrap().starts_with("  \""),
        "expected 2-space indentation: {text:?}"
    );
    // And it parses.
    let _: serde_json::Value = serde_json::from_str(&text).unwrap();
}

#[test]
fn diff_json_fail_on_breaking_writes_the_complete_document_then_exits_3() {
    // stdout path
    let output = vimanam()
        .args([
            "diff",
            DIFF_OLD,
            DIFF_NEW,
            "--format",
            "json",
            "--fail-on-breaking",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code().unwrap(), 3);
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("complete document on stdout");
    assert!(!document["changes"].as_array().unwrap().is_empty());

    // -o path: the file holds the same document, stdout stays empty.
    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("diff.json");
    let output = vimanam()
        .args([
            "diff",
            DIFF_OLD,
            DIFF_NEW,
            "--format",
            "json",
            "--fail-on-breaking",
            "-o",
            out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code().unwrap(), 3);
    assert!(output.stdout.is_empty());
    let file: serde_json::Value = serde_json::from_slice(&std::fs::read(&out_path).unwrap())
        .expect("complete document in -o file");
    assert_eq!(file, document);
}

#[test]
fn diff_json_parse_failure_exits_1_with_empty_stdout() {
    vimanam()
        .args([
            "diff",
            DIFF_OLD,
            "tests/fixtures/does_not_exist.json",
            "--format",
            "json",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("Failed to parse OpenAPI file"));
}

#[test]
fn diff_json_rejects_invalid_format_with_usage_error() {
    vimanam()
        .args(["diff", DIFF_OLD, DIFF_NEW, "--format", "xml"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("invalid value"));
}

#[test]
fn diff_json_stdout_stays_parseable_when_diagnostics_fire() {
    let dir = tempfile::tempdir().unwrap();
    let mut warny = load_json(DIFF_OLD);
    warny["info"]["version"] = serde_json::json!(""); // triggers a parse warning
    let path = write_spec(&dir, "warny.json", &warny);

    let output = vimanam()
        .args(["diff", &path, DIFF_NEW, "--format", "json"])
        .env("RUST_LOG", "warn")
        .output()
        .unwrap();
    assert_eq!(output.status.code().unwrap(), 0);
    assert!(!output.stderr.is_empty(), "expected the warning on stderr");
    let _: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout stays one JSON document");
}

// ── change identity ─────────────────────────────────────────────────────────

#[test]
fn diff_json_ids_are_stable_across_runs() {
    let (first, _) = diff_json_run(&[DIFF_OLD, DIFF_NEW]);
    let (second, _) = diff_json_run(&[DIFF_OLD, DIFF_NEW]);
    assert_eq!(first["changes"], second["changes"]);
}

#[test]
fn diff_json_ids_ignore_object_key_order() {
    let dir = tempfile::tempdir().unwrap();
    let old = load_json(DIFF_OLD);
    let new = load_json(DIFF_NEW);
    let old_reordered = write_spec(&dir, "old_reordered.json", &with_reversed_keys(&old));
    let new_reordered = write_spec(&dir, "new_reordered.json", &with_reversed_keys(&new));

    let (baseline, _) = diff_json_run(&[DIFF_OLD, DIFF_NEW]);
    let (reordered, _) = diff_json_run(&[&old_reordered, &new_reordered]);

    let sort_by_id = |document: &serde_json::Value| {
        let mut changes = document["changes"].as_array().unwrap().clone();
        changes.sort_by_key(|record| record["id"].as_str().unwrap().to_string());
        changes
    };
    assert_eq!(
        sort_by_id(&baseline),
        sort_by_id(&reordered),
        "key reordering changes neither IDs nor the set of records"
    );
    // …but the raw-bytes hash moves.
    assert_ne!(
        baseline["old"]["file_sha256"],
        reordered["old"]["file_sha256"]
    );
    assert_ne!(
        baseline["new"]["file_sha256"],
        reordered["new"]["file_sha256"]
    );
}

#[test]
fn diff_json_ids_survive_unrelated_edits() {
    let dir = tempfile::tempdir().unwrap();
    let mut new_extended = load_json(DIFF_NEW);
    new_extended["paths"]["/ping"] = serde_json::json!({"get": {"operationId": "Ping", "responses": {"200": {"description": "ok"}}}});
    let new_extended_path = write_spec(&dir, "new_extended.json", &new_extended);

    let (baseline, _) = diff_json_run(&[DIFF_OLD, DIFF_NEW]);
    let (extended, _) = diff_json_run(&[DIFF_OLD, &new_extended_path]);

    let baseline_ids: Vec<String> = baseline["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["id"].as_str().unwrap().to_string())
        .collect();
    let extended_ids: Vec<String> = extended["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["id"].as_str().unwrap().to_string())
        .collect();

    assert!(extended_ids.len() > baseline_ids.len());
    // Existing records keep their IDs and their order; the unrelated
    // endpoint's records are appended (old-spec endpoints are diffed first).
    assert_eq!(
        &extended_ids[..baseline_ids.len()],
        baseline_ids.as_slice(),
        "an unrelated edit must not rewrite existing IDs"
    );
}

#[test]
fn diff_json_ids_distinguish_different_changes_at_the_same_pointer() {
    let dir = tempfile::tempdir().unwrap();
    let base = serde_json::json!({
        "openapi": "3.0.0",
        "info": {"title": "T", "version": "1"},
        "paths": {
            "/things": {
                "get": {
                    "operationId": "listThings",
                    "responses": {
                        "200": {
                            "description": "ok",
                            "content": {
                                "application/json": {
                                    "schema": {"$ref": "#/components/schemas/Thing"}
                                }
                            }
                        }
                    }
                }
            }
        },
        "components": {
            "schemas": {"Thing": {"type": "object", "properties": {"id": {"type": "string"}}}}
        }
    });
    let base_path = write_spec(&dir, "base.json", &base);

    let mut to_integer = base.clone();
    *to_integer
        .pointer_mut("/components/schemas/Thing/properties/id/type")
        .unwrap() = serde_json::json!("integer");
    let integer_path = write_spec(&dir, "integer.json", &to_integer);

    let mut to_boolean = base.clone();
    *to_boolean
        .pointer_mut("/components/schemas/Thing/properties/id/type")
        .unwrap() = serde_json::json!("boolean");
    let boolean_path = write_spec(&dir, "boolean.json", &to_boolean);

    let (integer_doc, _) = diff_json_run(&[&base_path, &integer_path]);
    let (boolean_doc, _) = diff_json_run(&[&base_path, &boolean_path]);

    let integer_record = &integer_doc["changes"].as_array().unwrap()[0];
    let boolean_record = &boolean_doc["changes"].as_array().unwrap()[0];
    assert_eq!(
        integer_record["details"]["schema_change"]["pointer"],
        "/properties/id/type"
    );
    assert_eq!(
        boolean_record["details"]["schema_change"]["pointer"],
        "/properties/id/type"
    );
    assert_ne!(
        integer_record["id"], boolean_record["id"],
        "string→integer and string→boolean at the same pointer must differ"
    );
}

#[test]
fn diff_json_pins_golden_change_ids() {
    // The identity construction is versioned (`vc1_` and `"v": 1`); this pin
    // makes any silent change to it visible. The value was verified
    // independently against sha256 of the canonical identity object.
    let (document, _) = diff_json_run(&[DIFF_OLD, DIFF_NEW]);
    let added = document["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| {
            record["kind"] == "endpoint_added"
                && record["endpoint"]["path"] == "/widgets/{id}/history"
        })
        .unwrap();
    assert_eq!(
        added["id"],
        "vc1_8efeff460f315b2a726ed12220d7dbf818a2721fadc4ff212f25edb2a91d4bce"
    );
}

#[test]
fn diff_json_ids_are_unique_in_every_fixture_diff() {
    let mut pairs: Vec<Vec<String>> = vec![vec![DIFF_OLD.to_string(), DIFF_NEW.to_string()]];

    // A second, non-empty diff on a Swagger 2.0 pair.
    let dir = tempfile::tempdir().unwrap();
    let mut old = load_json(OAS2);
    old.pointer_mut("/paths/~1pets/post/parameters")
        .unwrap()
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"name": "limit", "in": "query", "type": "integer"}));
    let mut new = old.clone();
    *new.pointer_mut("/paths/~1pets/post/parameters/1/type")
        .unwrap() = serde_json::json!("string");
    *new.pointer_mut("/definitions/Pet").unwrap() = serde_json::json!({
        "type": "object",
        "properties": {"name": {"type": "string"}, "tag": {"type": "string"}}
    });
    let old_path = write_spec(&dir, "oas2_old.json", &old);
    let new_path = write_spec(&dir, "oas2_new.json", &new);
    pairs.push(vec![old_path, new_path]);

    for pair in &pairs {
        let args: Vec<&str> = pair.iter().map(String::as_str).collect();
        let (document, _) = diff_json_run(&args);
        let mut ids: Vec<&str> = document["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|record| record["id"].as_str().unwrap())
            .collect();
        let len = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), len, "duplicate IDs in diff of {args:?}");
    }
}

#[test]
fn diff_json_shared_ref_reports_once_per_endpoint_with_distinct_ids() {
    let (document, _) = diff_json_run(&[DIFF_OLD, DIFF_NEW]);
    let pricing: Vec<(String, String, String)> = document["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| {
            record["kind"] == "response_schema_changed"
                && record["details"]["schema_change"]["pointer"] == "/properties/pricing"
        })
        .map(|record| {
            (
                record["endpoint"]["method"].as_str().unwrap().to_string(),
                record["endpoint"]["path"].as_str().unwrap().to_string(),
                record["id"].as_str().unwrap().to_string(),
            )
        })
        .collect();

    // The shared Widget schema drifts once but surfaces on every endpoint
    // whose response uses it.
    assert_eq!(
        pricing
            .iter()
            .map(|(method, path, _)| format!("{method} {path}"))
            .collect::<Vec<_>>(),
        vec!["GET /widgets", "POST /widgets", "GET /widgets/{id}"]
    );
    // …each with its own ID, because the endpoint is part of the hash input.
    let mut ids: Vec<&str> = pricing.iter().map(|(_, _, id)| id.as_str()).collect();
    let len = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), len, "records for distinct endpoints must differ");
}

// ── presence encoding, targets, members ─────────────────────────────────────

#[test]
fn diff_json_root_schema_absence_is_present_false_with_flipped_operation() {
    let dir = tempfile::tempdir().unwrap();
    let thing = serde_json::json!({"type": "object", "properties": {"id": {"type": "string"}}});
    let no_schema = serde_json::json!([{ "name": "limit", "in": "query" }]);
    let with_schema =
        serde_json::json!([{ "name": "limit", "in": "query", "schema": {"type": "integer"} }]);

    // Absent → present: pointer "", before {present:false}, operation "added".
    let old_path = write_spec(
        &dir,
        "a_old.json",
        &probe_spec(thing.clone(), no_schema.clone()),
    );
    let new_path = write_spec(
        &dir,
        "a_new.json",
        &probe_spec(thing.clone(), with_schema.clone()),
    );
    let (appeared, _) = diff_json_run(&[&old_path, &new_path]);
    let change = appeared["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["kind"] == "parameter_schema_changed")
        .map(|record| &record["details"]["schema_change"])
        .unwrap();
    assert_eq!(change["pointer"], "");
    assert_eq!(change["operation"], "added");
    assert_eq!(change["before"], serde_json::json!({ "present": false }));
    assert_eq!(
        change["after"],
        serde_json::json!({ "present": true, "value": {"type": "integer"} })
    );

    // Present → absent: operation "removed".
    let old_path = write_spec(
        &dir,
        "b_old.json",
        &probe_spec(thing.clone(), with_schema.clone()),
    );
    let new_path = write_spec(&dir, "b_new.json", &probe_spec(thing, no_schema.clone()));
    let (vanished, _) = diff_json_run(&[&old_path, &new_path]);
    let change = vanished["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["kind"] == "parameter_schema_changed")
        .map(|record| &record["details"]["schema_change"])
        .unwrap();
    assert_eq!(change["pointer"], "");
    assert_eq!(change["operation"], "removed");
    assert_eq!(change["after"], serde_json::json!({ "present": false }));
}

#[test]
fn diff_json_explicit_null_inside_a_value_is_present_but_absence_is_not() {
    let dir = tempfile::tempdir().unwrap();

    // A property ADDED whose value contains an explicit null…
    let old_thing = serde_json::json!({"type": "object", "properties": {"id": {"type": "string"}}});
    let new_thing = serde_json::json!({"type": "object", "properties": {
        "id": {"type": "string"},
        "token": {"type": "string", "default": null}
    }});
    let old_path = write_spec(
        &dir,
        "n_old.json",
        &probe_spec(old_thing.clone(), serde_json::json!([])),
    );
    let new_path = write_spec(
        &dir,
        "n_new.json",
        &probe_spec(new_thing.clone(), serde_json::json!([])),
    );
    let (added, _) = diff_json_run(&[&old_path, &new_path]);
    let change = schema_change_of(&added, "/properties/token");
    assert_eq!(change["operation"], "added");
    assert_eq!(change["before"], serde_json::json!({ "present": false }));
    assert_eq!(change["after"]["present"], true);
    // The null sits INSIDE the emitted value, which is present.
    assert_eq!(change["after"]["value"]["default"], serde_json::Value::Null);

    // …versus a property REMOVED.
    let old_path = write_spec(
        &dir,
        "r_old.json",
        &probe_spec(new_thing, serde_json::json!([])),
    );
    let new_path = write_spec(
        &dir,
        "r_new.json",
        &probe_spec(old_thing, serde_json::json!([])),
    );
    let (removed, _) = diff_json_run(&[&old_path, &new_path]);
    let change = schema_change_of(&removed, "/properties/token");
    assert_eq!(change["operation"], "removed");
    assert_eq!(change["after"], serde_json::json!({ "present": false }));
    assert_eq!(change["before"]["present"], true);
    assert_eq!(
        change["before"]["value"]["default"],
        serde_json::Value::Null
    );
}

#[test]
fn diff_json_changed_null_value_stays_present_away_from_the_root() {
    // `default: null` → `default: "x"`: the before-value is a real null.
    let dir = tempfile::tempdir().unwrap();
    let old_thing = serde_json::json!({"type": "object", "properties": {
        "status": {"enum": [null], "default": null}
    }});
    let new_thing = serde_json::json!({"type": "object", "properties": {
        "status": {"enum": [null], "default": "x"}
    }});
    let old_path = write_spec(
        &dir,
        "d_old.json",
        &probe_spec(old_thing, serde_json::json!([])),
    );
    let new_path = write_spec(
        &dir,
        "d_new.json",
        &probe_spec(new_thing, serde_json::json!([])),
    );
    let (document, _) = diff_json_run(&[&old_path, &new_path]);
    let change = schema_change_of(&document, "/properties/status/default");
    assert_eq!(change["operation"], "changed");
    assert_eq!(
        change["before"],
        serde_json::json!({ "present": true, "value": null })
    );
    assert_eq!(
        change["after"],
        serde_json::json!({ "present": true, "value": "x" })
    );
}

#[test]
fn diff_json_enum_null_set_gaining_a_member() {
    let dir = tempfile::tempdir().unwrap();
    let old_thing = serde_json::json!({"type": "object", "properties": {
        "status": {"enum": [null]}
    }});
    let new_thing = serde_json::json!({"type": "object", "properties": {
        "status": {"enum": [null, "custom"]}
    }});
    let old_path = write_spec(
        &dir,
        "e_old.json",
        &probe_spec(old_thing, serde_json::json!([])),
    );
    let new_path = write_spec(
        &dir,
        "e_new.json",
        &probe_spec(new_thing, serde_json::json!([])),
    );
    let (document, _) = diff_json_run(&[&old_path, &new_path]);
    let change = schema_change_of(&document, "/properties/status/enum/custom");
    assert_eq!(change["target"], "enum_member");
    assert_eq!(change["member"], "custom");
    assert_eq!(change["operation"], "added");
    assert_eq!(change["before"], serde_json::json!({ "present": false }));
    assert_eq!(
        change["after"],
        serde_json::json!({ "present": true, "value": "custom" })
    );
}

#[test]
fn diff_json_target_and_member_disambiguate_pointer_roles() {
    let dir = tempfile::tempdir().unwrap();
    let old_thing = serde_json::json!({"type": "object", "required": ["id"], "properties": {
        "id": {"type": "string"}
    }});
    let new_thing = serde_json::json!({"type": "object", "required": ["id", "name"], "properties": {
        "id": {"type": "integer"},
        "name": {"type": "string"},
        "type": {"type": "string"}
    }});
    let old_path = write_spec(
        &dir,
        "t_old.json",
        &probe_spec(old_thing, serde_json::json!([])),
    );
    let new_path = write_spec(
        &dir,
        "t_new.json",
        &probe_spec(new_thing, serde_json::json!([])),
    );
    let (document, _) = diff_json_run(&[&old_path, &new_path]);

    // A property NAMED `type` is a property …
    let added_property = schema_change_of(&document, "/properties/type");
    assert_eq!(added_property["target"], "property");
    assert_eq!(added_property["member"], "type");

    // … the keyword is `type` with no member …
    let keyword = schema_change_of(&document, "/properties/id/type");
    assert_eq!(keyword["target"], "type");
    assert_eq!(keyword["member"], serde_json::Value::Null);

    // … and a required-set element is a required_member.
    let required = schema_change_of(&document, "/required/name");
    assert_eq!(required["target"], "required_member");
    assert_eq!(required["member"], "name");
}

#[test]
fn completions_offer_the_diff_format_flag() {
    let output = vimanam().args(["completions", "bash"]).output().unwrap();
    let script = String::from_utf8(output.stdout).unwrap();
    assert!(
        script.contains("--format"),
        "bash completions should offer --format"
    );
}
