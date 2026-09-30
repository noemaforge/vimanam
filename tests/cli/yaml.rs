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
