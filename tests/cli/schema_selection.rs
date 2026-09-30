use crate::common::vimanam;
use crate::common::write_spec;
use predicates::prelude::*;
use std::io::Write;

const SCHEMA_SELECTION: &str = "tests/fixtures/schema_selection_oas3.json";

#[test]
fn schema_field_retains_array_requiredness_description_enum_and_excludes_siblings() {
    vimanam()
        .args([
            SCHEMA_SELECTION,
            "--schema-field",
            "Root#/properties/selected/items/properties/kind",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "| `Root.selected` | array<object> | Yes | Chosen tags |",
        ))
        .stdout(predicate::str::contains(
            "| `Root.selected[].kind` | string | Yes | Tag kind; Enum: \"FIRST\", \"SECOND\" |",
        ))
        .stdout(predicate::str::contains("Root metadata"))
        .stdout(predicate::str::contains("Tag metadata"))
        .stdout(predicate::str::contains("Unrelated").not())
        .stdout(predicate::str::contains("Root.other").not())
        .stdout(predicate::str::contains("Schema Definitions").not());
}

#[test]
fn schema_selectors_are_repeatable_and_read_unreachable_schemas() {
    vimanam()
        .args([
            SCHEMA_SELECTION,
            "--schema",
            "Unused",
            "--schema-field",
            "Root#/properties/a~1b~0c",
            "--schema",
            "Unused",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Not referenced by any endpoint; Enum: \"X\", \"Y\"",
        ))
        .stdout(predicate::str::contains("Root.a/b~c"))
        .stdout(predicate::str::contains("Enum: \"alpha\", \"beta\""));
}

#[test]
fn schema_selection_crosses_recursive_refs_with_consumed_pointer() {
    vimanam()
        .args([
            SCHEMA_SELECTION,
            "--schema-field",
            "Node#/properties/next/properties/next/properties/value",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Node.next.next.value` | integer | Yes",
        ));
}

#[test]
fn schema_selection_composition_and_map_keep_source_context() {
    vimanam()
        .args([
            SCHEMA_SELECTION,
            "--schema-field",
            "Root#/properties/choice/oneOf/1/properties/kind",
            "--schema-field",
            "Root#/properties/map/additionalProperties/properties/kind",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Root.choice.oneOf[1].kind"))
        .stdout(predicate::str::contains("Root.map.*.kind"))
        .stdout(predicate::str::contains("Root.choice.oneOf[0]").not());
}

#[test]
fn schema_invalid_selectors_fail_before_creating_output() {
    let temp = tempfile::tempdir().unwrap();
    for selector in [
        "Root",
        "Root#properties/selected",
        "Root#/properties/nope",
        "Root#/properties/a~2b",
        "Root#/properties/choice/oneOf/09",
        "Root#/description",
        "Missing#/properties/a",
    ] {
        let output = temp.path().join("missing").join("out.md");
        vimanam()
            .args([SCHEMA_SELECTION, "--schema-field", selector, "-o"])
            .arg(&output)
            .assert()
            .failure()
            .stderr(predicate::str::contains("Error:"));
        assert!(!output.exists(), "{selector} created output");
        assert!(!output.parent().unwrap().exists());
    }
}

#[test]
fn schema_selection_preserves_selected_leaf_even_at_depth_and_budget_zero() {
    vimanam()
        .args([
            SCHEMA_SELECTION,
            "--schema-field",
            "Root#/properties/selected/items/properties/kind",
            "--schema-depth",
            "0",
            "--max-tokens",
            "0",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Root.selected[].kind` | string | Yes | Tag kind; Enum: \"FIRST\", \"SECOND\"",
        ))
        .stdout(predicate::str::contains(
            "Selected schemas exceed the approximate 0-token budget",
        ))
        .stderr(predicate::str::contains(
            "preserving requested selection and metadata",
        ));
}

#[test]
fn schema_selection_composes_with_exact_operation_context() {
    vimanam()
        .args([
            SCHEMA_SELECTION,
            "--schema",
            "Unused",
            "--operation-id",
            "GetRoot",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("- GET /roots"))
        .stdout(predicate::str::contains(
            "Operation selectors provide context",
        ))
        .stdout(predicate::str::contains("Root.selected").not());
}

#[test]
fn schema_depth_bounds_deferred_graph_and_reports_retrieval() {
    vimanam()
        .args([
            SCHEMA_SELECTION,
            "--operation-id",
            "GetRoot",
            "--detail",
            "full",
            "--include-schemas",
            "--schema-depth",
            "3",
            "--no-report",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("### Root {#schema-root}"))
        .stdout(predicate::str::contains("### Shared {#schema-shared}"))
        .stdout(predicate::str::contains("### Tag").not())
        .stdout(predicate::str::contains("### Tail").not())
        .stdout(predicate::str::contains(
            "Omitted nested expansion at schema depth 3",
        ))
        .stdout(predicate::str::contains("--schema 'Tag' --no-report"))
        .stderr(predicate::str::contains("omitted nested schema expansion"));
}

#[test]
fn schema_depth_preserves_inline_root_metadata_at_zero() {
    vimanam()
        .args([
            SCHEMA_SELECTION,
            "--schema",
            "Root",
            "--inline-schemas",
            "--schema-depth",
            "0",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Root` | object | - | Root metadata; Omitted",
        ))
        .stdout(predicate::str::contains("Root.selected").not());
}

#[test]
fn schema_depth_keeps_enums_without_selectors() {
    vimanam()
        .args([
            SCHEMA_SELECTION,
            "--detail",
            "full",
            "--include-schemas",
            "--schema-depth",
            "2",
            "--no-report",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "| `Root.a/b~c` | string | No | Escaped name; Enum: \"alpha\", \"beta\" |",
        ));
}

#[test]
fn schema_depth_field_selector_retrieval_uses_same_schema_field() {
    vimanam()
        .args([
            SCHEMA_SELECTION,
            "--schema-field",
            "Root#/properties/selected",
            "--schema-depth",
            "1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "--schema-field 'Root#/properties/selected' --no-report",
        ))
        .stderr(predicate::str::contains(
            "--schema-field 'Root#/properties/selected' --no-report",
        ));
}

// Schema-less media types ahead of a schema-bearing one must not suppress the
// request schema table (mirrors response_schema's first-with-schema rule).
#[test]
fn request_schema_skips_schema_less_media_type() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_spec(
        &dir,
        "octet_then_json.json",
        &serde_json::json!({
            "openapi": "3.0.3",
            "info": {"title": "Upload", "version": "1"},
            "paths": {
                "/upload": {
                    "post": {
                        "operationId": "Upload",
                        "tags": ["Files"],
                        "requestBody": {
                            "required": true,
                            "content": {
                                "application/octet-stream": {},
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "required": ["name"],
                                        "properties": {
                                            "name": {
                                                "type": "string",
                                                "description": "File name"
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        "responses": {
                            "204": {"description": "ok"}
                        }
                    }
                }
            }
        }),
    );

    let assert_json_table = |extra: &[&str]| {
        let mut args = vec![
            path.as_str(),
            "--detail",
            "full",
            "--include-schemas",
            "--no-report",
        ];
        args.extend_from_slice(extra);
        let output = vimanam().args(&args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            !text.contains("*No request schema available*"),
            "must not skip the JSON body when octet-stream is first: {text}"
        );
        assert!(
            text.contains("| `request.name` | string | Yes | File name |"),
            "JSON request schema table missing: {text}"
        );
    };

    assert_json_table(&[]);
    assert_json_table(&["--schema-depth", "2"]);
}

#[test]
fn schema_depth_cutoff_links_to_emitted_schema_not_missing_ones() {
    vimanam()
        .args([
            SCHEMA_SELECTION,
            "--operation-id",
            "GetRoot",
            "--detail",
            "full",
            "--include-schemas",
            "--schema-depth",
            "3",
            "--no-report",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "| `Root.deep.hop` | [Shared](#schema-shared) |",
        ))
        .stdout(predicate::str::contains("| `Root.selected[]` | ref Tag |"))
        .stdout(predicate::str::contains("### Tag").not());
}

fn order_shared_schema_spec(dir: &tempfile::TempDir) -> String {
    write_spec(
        dir,
        "order.json",
        &serde_json::json!({
            "openapi": "3.0.3",
            "info": {"title": "Order", "version": "1"},
            "paths": {
                "/early": {
                    "get": {
                        "operationId": "Early",
                        "tags": ["A"],
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {
                                            "type": "object",
                                            "properties": {
                                                "l1": {
                                                    "type": "object",
                                                    "properties": {
                                                        "l2": {
                                                            "type": "object",
                                                            "properties": {
                                                                "hop": {
                                                                    "$ref": "#/components/schemas/Shared"
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                "/late": {
                    "get": {
                        "operationId": "Late",
                        "tags": ["A"],
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {"$ref": "#/components/schemas/Shared"}
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "components": {
                "schemas": {
                    "Shared": {
                        "type": "object",
                        "properties": {"x": {"type": "string"}}
                    }
                }
            }
        }),
    )
}

#[test]
fn schema_depth_cutoff_links_shared_schema_discovered_by_later_endpoint() {
    // Early hits Shared only at the depth cutoff; Late reaches it inside the
    // limit. Pre-discovery must make Early's cutoff link regardless of write order.
    let dir = tempfile::tempdir().unwrap();
    let path = order_shared_schema_spec(&dir);

    let output = vimanam()
        .args([
            &path,
            "--flat",
            "--detail",
            "full",
            "--include-schemas",
            "--schema-depth",
            "3",
            "--no-report",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    let early = text
        .find("### Early")
        .and_then(|start| {
            text[start..]
                .find("### Late")
                .map(|end| &text[start..start + end])
        })
        .expect("Early section before Late");
    assert!(
        early.contains("| `response.l1.l2.hop` | [Shared](#schema-shared) |"),
        "Early cutoff must link to Shared discovered via Late: {early}"
    );
    assert!(text.contains("### Shared {#schema-shared}"), "{text}");
}

#[test]
fn schema_depth_inline_cutoff_stays_plain_ref_without_schema_link() {
    // Inline mode never emits Schema Definitions; pre-discovery must not
    // invent #schema- anchors that would leave broken cutoff links.
    let dir = tempfile::tempdir().unwrap();
    let path = order_shared_schema_spec(&dir);

    let output = vimanam()
        .args([
            &path,
            "--flat",
            "--detail",
            "full",
            "--include-schemas",
            "--inline-schemas",
            "--schema-depth",
            "3",
            "--no-report",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    let early = text
        .find("### Early")
        .and_then(|start| {
            text[start..]
                .find("### Late")
                .map(|end| &text[start..start + end])
        })
        .expect("Early section before Late");
    assert!(
        early.contains("| `response.l1.l2.hop` | ref Shared |"),
        "inline cutoff must stay a plain ref: {early}"
    );
    assert!(
        !text.contains("#schema-"),
        "inline mode must not invent schema definition links: {text}"
    );
}

#[test]
fn schema_depth_ignores_schemas_reachable_only_via_non_2xx() {
    // Renderer expands only the first 2xx body. A schema referenced solely from
    // a 4xx response must stay unlinked and unemitted under depth limiting.
    let dir = tempfile::tempdir().unwrap();
    let path = write_spec(
        &dir,
        "error_only.json",
        &serde_json::json!({
            "openapi": "3.0.3",
            "info": {"title": "Errors", "version": "1"},
            "paths": {
                "/item": {
                    "get": {
                        "operationId": "GetItem",
                        "tags": ["Items"],
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {
                                            "type": "object",
                                            "properties": {
                                                "l1": {
                                                    "type": "object",
                                                    "properties": {
                                                        "l2": {
                                                            "type": "object",
                                                            "properties": {
                                                                "hop": {
                                                                    "$ref": "#/components/schemas/ErrorOnly"
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            },
                            "400": {
                                "description": "bad",
                                "content": {
                                    "application/json": {
                                        "schema": {"$ref": "#/components/schemas/ErrorOnly"}
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "components": {
                "schemas": {
                    "ErrorOnly": {
                        "type": "object",
                        "properties": {"message": {"type": "string"}}
                    }
                }
            }
        }),
    );

    vimanam()
        .args([
            &path,
            "--detail",
            "full",
            "--include-schemas",
            "--schema-depth",
            "3",
            "--no-report",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "| `response.l1.l2.hop` | ref ErrorOnly |",
        ))
        .stdout(predicate::str::contains("### ErrorOnly").not())
        .stdout(predicate::str::contains("[ErrorOnly]").not());
}

#[test]
fn schema_depth_max_tokens_trials_do_not_leak_omission_stderr() {
    let trial = vimanam()
        .args([
            SCHEMA_SELECTION,
            "--detail",
            "full",
            "--include-schemas",
            "--schema-depth",
            "2",
            "--max-tokens",
            "60",
            "--no-report",
        ])
        .output()
        .unwrap();
    assert!(trial.status.success());
    let trial_err = String::from_utf8(trial.stderr).unwrap();
    assert!(
        trial_err.contains("reduced to --detail basic"),
        "expected budget reduction: {trial_err}"
    );
    assert!(
        !trial_err.contains("omitted nested schema expansion"),
        "discarded full-detail trial leaked omissions: {trial_err}"
    );

    let real = vimanam()
        .args([
            SCHEMA_SELECTION,
            "--detail",
            "full",
            "--include-schemas",
            "--schema-depth",
            "2",
            "--no-report",
        ])
        .output()
        .unwrap();
    assert!(real.status.success());
    let real_err = String::from_utf8(real.stderr).unwrap();
    assert!(
        real_err.contains("omitted nested schema expansion"),
        "real depth-limited render should report omissions: {real_err}"
    );
}

#[test]
fn default_maximum_schema_depth_message_unchanged_without_new_flags() {
    // Build a 25-deep property chain so the safety limit (24) fires without
    // --schema/--schema-field/--schema-depth.
    let mut properties = serde_json::json!({"leaf": {"type": "boolean"}});
    for depth in (0..25).rev() {
        properties = serde_json::json!({
            "type": "object",
            "properties": {
                format!("n{depth}"): properties
            }
        });
    }
    let spec = serde_json::json!({
        "openapi": "3.0.3",
        "info": {"title": "Deep", "version": "1"},
        "paths": {
            "/deep": {
                "get": {
                    "operationId": "GetDeep",
                    "responses": {
                        "200": {
                            "description": "ok",
                            "content": {
                                "application/json": {
                                    "schema": properties
                                }
                            }
                        }
                    }
                }
            }
        }
    });
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(serde_json::to_string(&spec).unwrap().as_bytes())
        .unwrap();
    let output = vimanam()
        .args([
            file.path().to_str().unwrap(),
            "--detail",
            "full",
            "--include-schemas",
            "--inline-schemas",
            "--no-report",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("Maximum schema depth reached; nested expansion stopped"),
        "safety cutoff missing: {text}"
    );
    assert!(
        !text.contains("Omitted nested expansion at schema depth"),
        "new depth-limit wording must not appear without --schema-depth: {text}"
    );

    // Absent the deep nest, the same flags must not invent the safety message.
    vimanam()
        .args([
            SCHEMA_SELECTION,
            "--detail",
            "full",
            "--include-schemas",
            "--no-report",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Maximum schema depth reached").not());
}

#[test]
fn schema_selectors_reject_tree_stats_and_ineffective_depth_options() {
    let cases: &[(&[&str], &str)] = &[
        (
            &["--schema", "Root", "--stats"],
            "cannot be used with '--stats'",
        ),
        (
            &["--schema", "Root", "--split", "endpoint", "-o", "unused"],
            "cannot be used with '--split",
        ),
        (
            &[
                "--schema-field",
                "Root#",
                "--output-mode",
                "skill",
                "-o",
                "unused",
            ],
            "cannot be used with '--output-mode",
        ),
        (
            &["--schema-depth", "1"],
            "Error: --schema-depth requires --detail full --include-schemas, or --schema/--schema-field",
        ),
        (
            &["--schema", "Root", "--schema-depth", "25"],
            "Error: --schema-depth supports 0..=24 (the schema recursion safety limit)",
        ),
    ];
    for (args, expected) in cases {
        vimanam()
            .arg(SCHEMA_SELECTION)
            .args(*args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(*expected));
    }
}

#[test]
fn schema_selection_supports_swagger_definitions() {
    let mut spec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(SCHEMA_SELECTION).unwrap()).unwrap();
    spec["swagger"] = serde_json::json!("2.0");
    spec.as_object_mut().unwrap().remove("openapi");
    spec["definitions"] = spec["components"]["schemas"].take();
    spec.as_object_mut().unwrap().remove("components");
    spec["paths"] = serde_json::json!({});
    let text = serde_json::to_string(&spec)
        .unwrap()
        .replace("#/components/schemas/", "#/definitions/");
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(text.as_bytes()).unwrap();
    vimanam()
        .arg(file.path())
        .args([
            "--schema-field",
            "Root#/properties/selected/items/properties/kind",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Root.selected[].kind"))
        .stdout(predicate::str::contains("Enum: \"FIRST\", \"SECOND\""));
}

#[test]
fn schema_depth_shared_definition_uses_shortest_path_and_is_deterministic() {
    let args = [
        SCHEMA_SELECTION,
        "--operation-id",
        "GetRoot",
        "--detail",
        "full",
        "--include-schemas",
        "--schema-depth",
        "5",
        "--no-report",
    ];
    let first = vimanam().args(args).output().unwrap();
    let second = vimanam().args(args).output().unwrap();
    assert!(first.status.success());
    assert_eq!(first.stdout, second.stdout);
    let text = String::from_utf8(first.stdout).unwrap();
    let shared = text
        .split("### Shared {#schema-shared}")
        .nth(1)
        .unwrap()
        .split("### ")
        .next()
        .unwrap();
    assert!(shared.contains("[Tail](#schema-tail)"));
    assert!(text.contains("### Tail {#schema-tail}"));
    assert!(!text.contains("Maximum schema depth"));
}

#[test]
fn schema_depth_stats_estimate_matches_real_render() {
    let args = [
        SCHEMA_SELECTION,
        "--operation-id",
        "GetRoot",
        "--detail",
        "full",
        "--include-schemas",
        "--schema-depth",
        "3",
    ];
    let document = vimanam().args(args).arg("--no-report").output().unwrap();
    let statistics = vimanam().args(args).arg("--stats").output().unwrap();
    assert!(document.status.success() && statistics.status.success());
    let tokens = String::from_utf8(document.stdout)
        .unwrap()
        .chars()
        .count()
        .div_ceil(4);
    let statistics = String::from_utf8(statistics.stdout).unwrap();
    let total = statistics
        .lines()
        .find(|line| line.starts_with("TOTAL"))
        .unwrap();
    assert_eq!(
        total
            .split_whitespace()
            .last()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        tokens
    );
}
