use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;

const OAS3: &str = "tests/fixtures/petstore_oas3.json";
const OAS2: &str = "tests/fixtures/petstore_oas2.json";
const OAS3_SCHEMA_REFS: &str = "tests/fixtures/schema_refs_oas3.json";
const OAS3_MULTI_AUTH: &str = "tests/fixtures/multi_auth_oas3.json";
const OAS2_MULTI_AUTH: &str = "tests/fixtures/multi_auth_oas2.json";
const OAS3_EXAMPLES: &str = "tests/fixtures/examples_oas3.json";

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

// Regression test for #13: methods are stored uppercase, so a lowercase
// `--method-filter` value used to match nothing and silently empty the output.
#[test]
fn method_filter_is_case_insensitive() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "basic", "--method-filter", "get"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Pets_ListPets"))
        .stdout(predicate::str::contains("Pets_CreatePet").not());
}

// Regression test for #19: a case-mismatched `--service-filter` used to
// silently omit all endpoints.
#[test]
fn service_filter_is_case_insensitive() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "basic", "--service-filter", "pets"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Pets_ListPets"))
        .stdout(predicate::str::contains("Store_ListOrders").not());
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

    vimanam()
        .arg(file.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error:"));
}

#[test]
fn json_without_openapi_fields_fails() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    write!(file, "{{\"hello\": \"world\"}}").unwrap();

    vimanam()
        .arg(file.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error:"));
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
            "| `response.allOf[1].id` | string | Yes | Pet identifier |",
        ))
        .stdout(predicate::str::contains("request.variant.oneOf[0]"));
}

// Regression test for #16: the Authentication section is emitted in spec
// (file) order, not the random order of a HashMap, and is stable across runs.
#[test]
fn multiple_security_schemes_preserve_spec_order() {
    let run = || {
        String::from_utf8(
            vimanam()
                .arg(OAS3_MULTI_AUTH)
                .arg("--include-auth")
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
    };

    let output = run();

    let zebra = output.find("zebraAuth").expect("zebraAuth missing");
    let api_key = output.find("apiKeyAuth").expect("apiKeyAuth missing");
    let middle = output.find("middleAuth").expect("middleAuth missing");

    // Schemes appear in the order they are declared in the spec file.
    assert!(
        zebra < api_key && api_key < middle,
        "security schemes not in spec order: {output}"
    );

    // And that order is deterministic across runs.
    for _ in 0..4 {
        assert_eq!(output, run(), "authentication order differed between runs");
    }
}

// Companion to #16 for OpenAPI 2.0: `securityDefinitions` are read through the
// extensions map, so they only preserve spec order with serde_json's
// `preserve_order` feature (otherwise they sort alphabetically). The schemes
// are declared zebra/apiKey/middle, which is not alphabetical.
#[test]
fn oas2_security_schemes_preserve_spec_order() {
    let output = String::from_utf8(
        vimanam()
            .arg(OAS2_MULTI_AUTH)
            .arg("--include-auth")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    let zebra = output.find("zebraAuth").expect("zebraAuth missing");
    let api_key = output.find("apiKey").expect("apiKey missing");
    let middle = output.find("middleAuth").expect("middleAuth missing");

    assert!(
        zebra < api_key && api_key < middle,
        "OAS2 security schemes not in spec order: {output}"
    );
}

// Regression test for #20: `--group-by method` must behave like `--method`,
// producing HTTP-method sections rather than service sections.
#[test]
fn group_by_method_groups_by_http_method() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "basic", "--group-by", "method"])
        .assert()
        .success()
        .stdout(predicate::str::contains("## GET"))
        .stdout(predicate::str::contains("## POST"));
}

// Regression test for #18: under alphabetical sort the TOC operation links must
// appear in the same order as the endpoint sections in the body.
#[test]
fn toc_order_matches_body_order() {
    let output = String::from_utf8(
        vimanam()
            .arg(OAS3)
            .args(["--detail", "basic", "--sort", "alpha"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    // The Pets service has CreatePet (POST /pets) and ListPets (GET /pets);
    // sorted by path then method, GET sorts before POST, so ListPets precedes
    // CreatePet in both the TOC and the body.
    let toc_list = output
        .find("[Pets_ListPets]")
        .expect("ListPets TOC link missing");
    let toc_create = output
        .find("[Pets_CreatePet]")
        .expect("CreatePet TOC link missing");
    let body_list = output
        .find("### Pets_ListPets")
        .expect("ListPets section missing");
    let body_create = output
        .find("### Pets_CreatePet")
        .expect("CreatePet section missing");

    assert!(toc_list < toc_create, "TOC order unexpected: {output}");
    assert!(body_list < body_create, "body order unexpected: {output}");
}

// #6: `--include-examples` at `--detail full` renders the request body's inline
// example and the response example resolved from a `$ref` into
// `components/examples`.
#[test]
fn include_examples_renders_request_and_response() {
    vimanam()
        .arg(OAS3_EXAMPLES)
        .args(["--detail", "full", "--include-examples"])
        .assert()
        .success()
        .stdout(predicate::str::contains("#### Examples"))
        // Inline request body example.
        .stdout(predicate::str::contains("**Request**"))
        .stdout(predicate::str::contains("\"name\": \"Fluffy\""))
        // Response example resolved through #/components/examples/CreatedPet.
        .stdout(predicate::str::contains("Response `201`"))
        .stdout(predicate::str::contains("\"id\": 7"));
}

// Examples only render at `--detail full`, matching `--include-schemas`.
#[test]
fn include_examples_only_at_full_detail() {
    vimanam()
        .arg(OAS3_EXAMPLES)
        .args(["--detail", "standard", "--include-examples"])
        .assert()
        .success()
        .stdout(predicate::str::contains("#### Examples").not());
}

// #8: `--group-by path` produces one section per path with its operations
// underneath.
#[test]
fn group_by_path_groups_by_path() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "basic", "--group-by", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains("## Paths"))
        .stdout(predicate::str::contains("## /pets/{petId}"))
        .stdout(predicate::str::contains("## /store/orders"))
        .stdout(predicate::str::contains("### Pets_ListPets"))
        .stdout(predicate::str::contains("### Pets_CreatePet"));
}

// #7: a tiny `--max-tokens` budget forces a full-detail request down to a lower
// detail level and reports the reduction on stderr.
#[test]
fn max_tokens_steps_down_detail_level() {
    vimanam()
        .arg(OAS3)
        .args([
            "--detail",
            "full",
            "--include-schemas",
            "--max-tokens",
            "40",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("token budget"))
        .stderr(predicate::str::contains("--detail summary"));
}

// A generous `--max-tokens` budget leaves the requested detail untouched and
// emits no stderr note.
#[test]
fn max_tokens_keeps_detail_when_it_fits() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "basic", "--max-tokens", "100000"])
        .assert()
        .success()
        .stdout(predicate::str::contains("### Pets_ListPets"))
        .stderr(predicate::str::is_empty());
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
