use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;

const OAS3: &str = "tests/fixtures/petstore_oas3.json";
const OAS2: &str = "tests/fixtures/petstore_oas2.json";

// Fixtures for ref-aware, tolerant parsing (issues #48, #50, #51, #56, #49).
const REF_PARAM_BODY: &str = "tests/fixtures/ref_param_and_body.json";
const REF_PATH_ITEM: &str = "tests/fixtures/ref_path_item.json";
const TYPE_ARRAY: &str = "tests/fixtures/type_array_nullable.json";
const MISSING_RESPONSES: &str = "tests/fixtures/missing_responses.json";
const MULTI_AUTH: &str = "tests/fixtures/multi_auth.json";

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

// #48: a parameter declared as a component `$ref` is resolved instead of
// failing the whole parse (regression — bare `$ref` params used to crash).
#[test]
fn ref_parameter_is_resolved() {
    vimanam()
        .arg(REF_PARAM_BODY)
        .args(["--detail", "standard"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "| `limit` | query | No | Max results |",
        ));
}

// #48: a requestBody declared as a component `$ref` is resolved.
#[test]
fn ref_request_body_is_resolved() {
    vimanam()
        .arg(REF_PARAM_BODY)
        .args(["--detail", "standard"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "| `requestBody` | body | No | Item payload |",
        ));
}

// #50: a path item declared as a `$ref` yields its operations instead of being
// silently dropped.
#[test]
fn path_item_ref_yields_operation() {
    vimanam()
        .arg(REF_PATH_ITEM)
        .args(["--detail", "basic"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Things_ListThings"))
        .stdout(predicate::str::contains("**Operation:** GET /things"));
}

// #51: OpenAPI 3.1 `type` arrays (e.g. ["string","null"]) parse instead of
// failing on "invalid type: sequence".
#[test]
fn type_array_parameter_parses() {
    vimanam()
        .arg(TYPE_ARRAY)
        .args(["--detail", "standard"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "| `q` | query | No | Search term |",
        ));
}

// #56: an operation missing its `responses` block no longer fails the entire
// document; both operations are still rendered.
#[test]
fn operation_missing_responses_still_parses() {
    vimanam()
        .arg(MISSING_RESPONSES)
        .args(["--detail", "basic"])
        .assert()
        .success()
        .stdout(predicate::str::contains("A_NoResponses"))
        .stdout(predicate::str::contains("B_HasResponses"));
}

// #49: all security schemes are listed.
#[test]
fn multi_auth_lists_all_schemes() {
    vimanam()
        .arg(MULTI_AUTH)
        .arg("--include-auth")
        .assert()
        .success()
        .stdout(predicate::str::contains("alphaAuth"))
        .stdout(predicate::str::contains("middleAuth"))
        .stdout(predicate::str::contains("zebraAuth"));
}

// #49: with multiple security schemes, the Authentication section must come out
// in a stable order across runs (previously backed by a HashMap).
#[test]
fn multi_auth_order_is_deterministic() {
    let run = || {
        vimanam()
            .arg(MULTI_AUTH)
            .args(["--include-auth", "--detail", "standard"])
            .output()
            .unwrap()
            .stdout
    };

    let first = run();
    for _ in 0..4 {
        assert_eq!(first, run(), "auth section order differed between runs");
    }
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
