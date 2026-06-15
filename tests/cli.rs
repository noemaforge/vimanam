use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;

const OAS3: &str = "tests/fixtures/petstore_oas3.json";
const OAS2: &str = "tests/fixtures/petstore_oas2.json";
const OAS3_SCHEMA_REFS: &str = "tests/fixtures/schema_refs_oas3.json";

fn vimanam() -> Command {
    Command::cargo_bin("vimanam").unwrap()
}

#[test]
fn version_flag_reports_crate_version() {
    vimanam()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn summary_lists_services_and_operations() {
    vimanam()
        .arg(OAS3)
        .assert()
        .success()
        .stdout(predicate::str::contains("# Petstore API"))
        .stdout(predicate::str::contains("- Pets"))
        .stdout(predicate::str::contains("- Store"))
        // Service prefix is stripped from operation IDs in the summary view
        .stdout(predicate::str::contains("* ListPets"));
}

#[test]
fn basic_detail_writes_endpoint_sections() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "basic"])
        .assert()
        .success()
        .stdout(predicate::str::contains("### Pets_ListPets"))
        .stdout(predicate::str::contains("**Operation:** GET /pets"))
        .stdout(predicate::str::contains("**Operation:** POST /pets"));
}

// Regression test: optional request bodies (no `required: true`) used to be
// dropped from the parameter table entirely.
#[test]
fn optional_request_body_is_documented() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "standard"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "| `requestBody` | body | No | Pet to add |",
        ));
}

#[test]
fn required_path_param_is_documented() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "standard"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "| `petId` | path | Yes | ID of the pet |",
        ));
}

#[test]
fn exclude_deprecated_hides_endpoint() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "basic"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Store_ListOrders"));

    vimanam()
        .arg(OAS3)
        .args(["--detail", "basic", "--exclude-deprecated"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Store_ListOrders").not());
}

#[test]
fn method_filter_excludes_other_methods() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "basic", "--method-filter", "GET"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Pets_ListPets"))
        .stdout(predicate::str::contains("Pets_CreatePet").not());
}

#[test]
fn path_filter_excludes_other_paths() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "basic", "--path-filter", "/store"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Store_ListOrders"))
        .stdout(predicate::str::contains("Pets_ListPets").not());
}

#[test]
fn include_auth_shows_servers_and_schemes() {
    vimanam()
        .arg(OAS3)
        .arg("--include-auth")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "https://api.petstore.example.com/v1",
        ))
        .stdout(predicate::str::contains("apiKeyAuth"));
}

#[test]
fn flat_grouping_lists_all_endpoints() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "basic", "--flat"])
        .assert()
        .success()
        .stdout(predicate::str::contains("## Endpoints"))
        .stdout(predicate::str::contains("### Pets_ListPets"))
        .stdout(predicate::str::contains("### Store_ListOrders"));
}

#[test]
fn oas2_spec_is_supported() {
    vimanam()
        .arg(OAS2)
        .args(["--detail", "standard", "--include-auth"])
        .assert()
        .success()
        .stdout(predicate::str::contains("# Petstore Legacy API"))
        // host + basePath are combined into a server URL
        .stdout(predicate::str::contains(
            "https://legacy.petstore.example.com/v2",
        ))
        .stdout(predicate::str::contains("Pets_CreatePet"))
        // OpenAPI 2.0 body responses infer application/json
        .stdout(predicate::str::contains(
            "| 200 | application/json | Created |",
        ));
}

#[test]
fn output_flag_writes_file() {
    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("out.md");

    vimanam()
        .arg(OAS3)
        .args(["-o", out_path.to_str().unwrap()])
        .assert()
        .success();

    let content = std::fs::read_to_string(&out_path).unwrap();
    assert!(content.contains("# Petstore API"));
}

#[test]
fn invalid_json_fails() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    write!(file, "this is not json").unwrap();

    vimanam().arg(file.path()).assert().failure();
}

#[test]
fn json_without_openapi_fields_fails() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    write!(file, "{{\"hello\": \"world\"}}").unwrap();

    vimanam().arg(file.path()).assert().failure();
}

// Output must be byte-identical across runs, even with sorting disabled.
// Guards the IndexMap-based ordering of paths, responses, and content types.
#[test]
fn output_is_deterministic() {
    let run = || {
        vimanam()
            .arg(OAS3)
            .args([
                "--detail",
                "full",
                "--include-schemas",
                "--include-auth",
                "--sort",
                "none",
            ])
            .output()
            .unwrap()
            .stdout
    };

    let first = run();
    for _ in 0..4 {
        assert_eq!(first, run(), "output differed between identical runs");
    }
}

#[test]
fn full_detail_expands_schema_refs_into_tables() {
    vimanam()
        .arg(OAS3_SCHEMA_REFS)
        .args(["--detail", "full", "--include-schemas"])
        .assert()
        .success()
        .stdout(predicate::str::contains("#### Request Schema"))
        .stdout(predicate::str::contains(
            "| `request.name` | string | Yes | Pet name |",
        ))
        .stdout(predicate::str::contains(
            "| `request.category.id` | string | Yes | Category identifier |",
        ))
        .stdout(predicate::str::contains(
            "| `response.allOf[2].id` | string | Yes | Pet identifier |",
        ))
        .stdout(predicate::str::contains("request.variant.oneOf[1]"));
}

#[test]
fn full_detail_schema_expansion_detects_ref_cycles() {
    vimanam()
        .arg(OAS3_SCHEMA_REFS)
        .args(["--detail", "full", "--include-schemas"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Cycle detected while expanding schema reference",
        ));
}
