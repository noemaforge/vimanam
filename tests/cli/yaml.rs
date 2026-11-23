use crate::common::OAS2;
use crate::common::OAS3;
use crate::common::OAS3_SCHEMA_REFS;
use crate::common::vimanam;
use predicates::prelude::*;

// --- YAML input support (#4) ---

// YAML twin of petstore_oas3.json — must parse to identical documentation (#4).
const OAS3_YAML: &str = "tests/fixtures/petstore_oas3.yaml";
const OAS2_YAML: &str = "tests/fixtures/petstore_oas2.yaml";
const OAS3_SCHEMA_REFS_YAML: &str = "tests/fixtures/schema_refs_oas3.yaml";

// A YAML OpenAPI 3 spec parses just like its JSON counterpart.
#[test]
fn yaml_spec_is_parsed() {
    vimanam()
        .arg(OAS3_YAML)
        .args(["--detail", "basic"])
        .assert()
        .success()
        .stdout(predicate::str::contains("# Petstore API"))
        .stdout(predicate::str::contains("### Pets_ListPets"))
        .stdout(predicate::str::contains("**Operation:** GET /pets"));
}

// The YAML twin and the JSON fixture must produce byte-identical documentation:
// format is an input detail, not a semantic one. Also guards key-order determinism
// (IndexMap) across the YAML deserializer.
#[test]
fn yaml_and_json_produce_identical_output() {
    let render = |spec: &str| {
        vimanam()
            .arg(spec)
            .args(["--detail", "full", "--include-schemas", "--include-auth"])
            .output()
            .unwrap()
            .stdout
    };
    assert_eq!(
        render(OAS3),
        render(OAS3_YAML),
        "YAML and JSON inputs produced different output"
    );
}

// The Swagger 2.0 (OAS2) YAML twin and its JSON counterpart must produce byte-identical documentation.
#[test]
fn yaml_and_json_produce_identical_output_oas2() {
    let render = |spec: &str| {
        vimanam()
            .arg(spec)
            .args(["--detail", "full", "--include-schemas", "--include-auth"])
            .output()
            .unwrap()
            .stdout
    };
    assert_eq!(
        render(OAS2),
        render(OAS2_YAML),
        "OAS2 YAML and JSON inputs produced different output"
    );
}

// The $ref-heavy YAML twin of schema_refs_oas3.json must produce identical output.
#[test]
fn yaml_and_json_produce_identical_output_schema_refs() {
    let render = |spec: &str| {
        vimanam()
            .arg(spec)
            .args(["--detail", "full", "--include-schemas"])
            .output()
            .unwrap()
            .stdout
    };
    assert_eq!(
        render(OAS3_SCHEMA_REFS),
        render(OAS3_SCHEMA_REFS_YAML),
        "schema_refs YAML and JSON inputs produced different output"
    );
}

// Extension detection is case-insensitive (`.YAML` routes to the YAML parser).
#[test]
fn yaml_extension_is_case_insensitive() {
    let yaml = std::fs::read_to_string(OAS3_YAML).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spec.YAML");
    std::fs::write(&path, yaml).unwrap();

    vimanam()
        .arg(&path)
        .assert()
        .success()
        .stdout(predicate::str::contains("# Petstore API"));
}

// A YAML spec with a non-YAML/JSON extension still parses: the JSON-first path
// falls back to the YAML parser.
#[test]
fn yaml_content_with_unknown_extension_falls_back() {
    let yaml = std::fs::read_to_string(OAS3_YAML).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spec.txt");
    std::fs::write(&path, yaml).unwrap();

    vimanam()
        .arg(&path)
        .assert()
        .success()
        .stdout(predicate::str::contains("# Petstore API"));
}

// Malformed YAML fails with an error rather than panicking.
#[test]
fn invalid_yaml_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.yaml");
    std::fs::write(&path, "openapi: \"3.0.0\"\n  bad: : indentation:").unwrap();

    vimanam()
        .arg(&path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error:"));
}

// A structurally-valid YAML document that isn't an OpenAPI spec reports the
// targeted missing-field error (and only that — no doubled fallback noise).
#[test]
fn yaml_without_openapi_fields_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notspec.yaml");
    std::fs::write(&path, "hello: world\n").unwrap();

    vimanam()
        .arg(&path)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Missing 'swagger' or 'openapi' field",
        ));
}

// YAML 1.2 strict booleans keep unquoted Norway-problem literals (NO/yes/on/off)
// and leading-zero/underscored scalars (007, 1_000) as strings in untyped JSON
// values (enum, examples, extensions), while numeric HTTP status keys (200:, 404:)
// coerce cleanly into response map keys.
#[test]
fn yaml_preserves_norway_strings_and_unquoted_status_codes() {
    let dir = tempfile::tempdir().unwrap();
    let yaml_path = dir.path().join("norway.yaml");
    let json_path = dir.path().join("norway.json");
    std::fs::write(
        &yaml_path,
        r#"openapi: 3.0.3
info:
  title: Country API
  version: 1.0.0
paths:
  /countries:
    get:
      summary: List countries
      responses:
        200:
          description: OK
          content:
            application/json:
              example:
                country: NO
                enabled_label: yes
                mode: on
                fallback: off
                zip: 007
                batch: 1_000
              schema:
                type: object
                properties:
                  code:
                    type: string
                    enum:
                      - NO
                      - SE
                      - yes
                      - on
                      - off
        404:
          description: Not found
"#,
    )
    .unwrap();
    std::fs::write(
        &json_path,
        serde_json::json!({
            "openapi": "3.0.3",
            "info": { "title": "Country API", "version": "1.0.0" },
            "paths": {
                "/countries": {
                    "get": {
                        "summary": "List countries",
                        "responses": {
                            "200": {
                                "description": "OK",
                                "content": {
                                    "application/json": {
                                        "example": {
                                            "country": "NO",
                                            "enabled_label": "yes",
                                            "mode": "on",
                                            "fallback": "off",
                                            "zip": "007",
                                            "batch": "1_000"
                                        },
                                        "schema": {
                                            "type": "object",
                                            "properties": {
                                                "code": {
                                                    "type": "string",
                                                    "enum": ["NO", "SE", "yes", "on", "off"]
                                                }
                                            }
                                        }
                                    }
                                }
                            },
                            "404": { "description": "Not found" }
                        }
                    }
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let render = |p: &std::path::Path| {
        String::from_utf8(
            vimanam()
                .arg(p)
                .args([
                    "--detail",
                    "full",
                    "--include-schemas",
                    "--include-examples",
                    "--no-report",
                ])
                .assert()
                .success()
                .get_output()
                .stdout
                .clone(),
        )
        .unwrap()
    };
    let yaml_md = render(&yaml_path);
    let json_md = render(&json_path);
    assert_eq!(yaml_md, json_md);
    assert!(yaml_md.contains("| 200 |"));
    assert!(yaml_md.contains("| 404 |"));
    assert!(yaml_md.contains("\"country\": \"NO\""));
    assert!(yaml_md.contains("\"zip\": \"007\""));
    assert!(yaml_md.contains("\"batch\": \"1_000\""));

    let diff_out = vimanam()
        .arg("diff")
        .arg(&json_path)
        .arg(&yaml_path)
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let diff: serde_json::Value = serde_json::from_slice(&diff_out).unwrap();
    assert_eq!(
        diff["changes"],
        serde_json::json!([]),
        "YAML and JSON twins must have zero semantic diff: {diff}"
    );
}

// YAML anchors and `<<:` merge keys expand cleanly into OpenAPI structures.
#[test]
fn yaml_merge_keys_expand_anchors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("merge.yaml");
    std::fs::write(
        &path,
        r#"openapi: 3.0.3
info:
  title: Merge API
  version: 1.0.0
x-shared-ok: &shared_ok
  description: Shared OK response
paths:
  /ping:
    get:
      summary: Ping
      responses:
        200:
          <<: *shared_ok
"#,
    )
    .unwrap();

    vimanam()
        .arg(&path)
        .args(["--detail", "full", "--no-report"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Shared OK response"));
}

// Duplicate YAML mapping keys are rejected instead of silently overwriting.
#[test]
fn yaml_duplicate_keys_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dup.yaml");
    std::fs::write(
        &path,
        r#"openapi: 3.0.3
info:
  title: First
  title: Second
  version: 1.0.0
paths: {}
"#,
    )
    .unwrap();

    vimanam()
        .arg(&path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("duplicate"));
}

// Reusing one shared anchor via `<<:` across 120 endpoints must not trip
// serde-saphyr's alias-to-anchor ratio heuristic while keeping alias-bomb
// bounds active.
#[test]
fn yaml_high_alias_to_anchor_ratio_succeeds() {
    use std::fmt::Write as _;

    let mut yaml = String::from(
        "openapi: 3.0.3\ninfo:\n  title: Shared Anchor API\n  version: 1.0.0\nx-shared-ok: &shared_ok\n  description: Shared OK response\npaths:\n",
    );
    for i in 0..120 {
        writeln!(
            yaml,
            "  /items/{i}:\n    get:\n      summary: Item {i}\n      responses:\n        200:\n          <<: *shared_ok"
        )
        .unwrap();
    }

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("many_aliases.yaml");
    std::fs::write(&path, yaml).unwrap();

    vimanam()
        .arg(&path)
        .args(["--detail", "full", "--no-report"])
        .assert()
        .success()
        .stdout(predicate::str::contains("/items/0"))
        .stdout(predicate::str::contains("/items/119"))
        .stdout(predicate::str::contains("Shared OK response"));
}
