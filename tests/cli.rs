use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;

const OAS3: &str = "tests/fixtures/petstore_oas3.json";
const OAS2: &str = "tests/fixtures/petstore_oas2.json";
const OAS3_SCHEMA_REFS: &str = "tests/fixtures/schema_refs_oas3.json";
const OAS3_MULTI_AUTH: &str = "tests/fixtures/multi_auth_oas3.json";
const OAS2_MULTI_AUTH: &str = "tests/fixtures/multi_auth_oas2.json";
const OAS3_EXAMPLES: &str = "tests/fixtures/examples_oas3.json";
const OAS3_REF_BODY: &str = "tests/fixtures/ref_request_body_oas3.json";

// YAML twin of petstore_oas3.json — must parse to identical documentation (#4).
const OAS3_YAML: &str = "tests/fixtures/petstore_oas3.yaml";

// Parse-layer correctness cluster (issues #48, #50, #51, #54, #56, #60).
const MULTI_TAG: &str = "tests/fixtures/multi_tag_oas3.json";
const REF_PARAMETER: &str = "tests/fixtures/ref_parameter.json";
const REF_PATH_ITEM: &str = "tests/fixtures/ref_path_item.json";
const TYPE_ARRAY: &str = "tests/fixtures/type_array_nullable.json";
const MISSING_RESPONSES: &str = "tests/fixtures/missing_responses.json";
const OVERRIDE_PARAM: &str = "tests/fixtures/override_param.json";
const UNKNOWN_TAG: &str = "tests/fixtures/unknown_tag.json";

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

// `--required-only` drops parameters that are not required (explicit
// `required: false` or unspecified), keeping required ones.
#[test]
fn required_only_excludes_non_required_parameters() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "standard", "--required-only"])
        .assert()
        .success()
        // Required path parameter is kept.
        .stdout(predicate::str::contains("| `petId` | path | Yes |"))
        // Optional query parameter is dropped.
        .stdout(predicate::str::contains("| `limit` |").not());
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

// --- YAML input support (#4) ---

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

// By default (#58) component schemas are linked from their use site and expanded
// once in a trailing "Schema Definitions" section, rather than re-inlined.
#[test]
fn full_detail_links_schema_refs_to_definitions() {
    vimanam()
        .arg(OAS3_SCHEMA_REFS)
        .args(["--detail", "full", "--include-schemas"])
        .assert()
        .success()
        // The use site is a single linked row, not a re-inlined subtree.
        .stdout(predicate::str::contains(
            "| `request` | [CreatePetRequest](#schema-createpetrequest) | - | - |",
        ))
        .stdout(predicate::str::contains(
            "| `response` | [Pet](#schema-pet) | - | - |",
        ))
        // The shared schemas are expanded once in the definitions section.
        .stdout(predicate::str::contains("## Schema Definitions"))
        .stdout(predicate::str::contains(
            "### CreatePetRequest {#schema-createpetrequest}",
        ))
        .stdout(predicate::str::contains(
            "| `CreatePetRequest.name` | string | Yes | Pet name |",
        ))
        .stdout(predicate::str::contains(
            "| `CreatePetRequest.category` | [Category](#schema-category) | Yes |",
        ))
        .stdout(predicate::str::contains(
            "| `Category.id` | string | Yes | Category identifier |",
        ))
        .stdout(predicate::str::contains(
            "| `Pet.allOf[1].id` | string | Yes | Pet identifier |",
        ));
}

// CreatePetRequest is referenced from the /pets request body and again from
// Pet's `allOf`; it must be expanded exactly once (the #58 win).
#[test]
fn shared_schema_is_expanded_once() {
    let output = String::from_utf8(
        vimanam()
            .arg(OAS3_SCHEMA_REFS)
            .args(["--detail", "full", "--include-schemas"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    let definitions = output
        .matches("### CreatePetRequest {#schema-createpetrequest}")
        .count();
    assert_eq!(
        definitions, 1,
        "CreatePetRequest expanded {definitions} times"
    );
}

// A self-referential schema (Node.next -> Node) renders once and links back to
// itself instead of looping or printing a "cycle detected" row.
#[test]
fn linked_mode_handles_self_reference_with_a_link() {
    vimanam()
        .arg(OAS3_SCHEMA_REFS)
        .args(["--detail", "full", "--include-schemas"])
        .assert()
        .success()
        .stdout(predicate::str::contains("### Node {#schema-node}"))
        .stdout(predicate::str::contains(
            "| `Node.next` | [Node](#schema-node) | No |",
        ))
        .stdout(predicate::str::contains("Cycle detected").not());
}

// `--inline-schemas` restores the fully self-contained output: every `$ref` is
// expanded inline at each use site, with no shared definitions section.
#[test]
fn inline_schemas_expands_refs_at_each_use_site() {
    vimanam()
        .arg(OAS3_SCHEMA_REFS)
        .args(["--detail", "full", "--include-schemas", "--inline-schemas"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "| `request.name` | string | Yes | Pet name |",
        ))
        .stdout(predicate::str::contains(
            "| `request.category.id` | string | Yes | Category identifier |",
        ))
        .stdout(predicate::str::contains(
            "| `response.allOf[1].id` | string | Yes | Pet identifier |",
        ))
        .stdout(predicate::str::contains("request.variant.oneOf[0]"))
        .stdout(predicate::str::contains("## Schema Definitions").not());
}

// #69 follow-up: the "no effect" warning reports the current detail level in the
// same lowercase spelling the user types (`standard`), not the Debug-derived
// `Standard`.
#[test]
fn include_schemas_warning_uses_lowercase_detail_name() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "standard", "--include-schemas"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "--include-schemas has no effect at --detail standard; use --detail full.",
        ));
}

// At `--detail full` the flag takes effect, so no warning is emitted.
#[test]
fn include_schemas_at_full_detail_emits_no_warning() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "full", "--include-schemas"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no effect").not());
}

// `--inline-schemas` only changes how schemas render, so it warns when used
// without `--include-schemas`.
#[test]
fn inline_schemas_without_include_schemas_warns() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "full", "--inline-schemas"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "--inline-schemas has no effect without --include-schemas.",
        ));
}

// #70 follow-up: an operation carrying multiple tags is rendered under each
// service section, so its heading anchor must be scoped per service to stay
// unique — and each TOC link must point at the matching copy.
#[test]
fn multi_tag_endpoint_gets_unique_anchors_per_service() {
    let output = String::from_utf8(
        vimanam()
            .arg(MULTI_TAG)
            .args(["--detail", "basic"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    // Distinct, service-scoped heading anchors (no duplicate `#delete-pets-petid`).
    assert!(
        output.contains("### DeletePet {#pets-delete-pets-petid}"),
        "missing Pets-scoped anchor:\n{output}"
    );
    assert!(
        output.contains("### DeletePet {#admin-delete-pets-petid}"),
        "missing Admin-scoped anchor:\n{output}"
    );

    // Each TOC entry links to the copy under its own service.
    assert!(
        output.contains("* [DeletePet](#pets-delete-pets-petid)")
            && output.contains("* [DeletePet](#admin-delete-pets-petid)"),
        "TOC links do not match per-service anchors:\n{output}"
    );
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

// A `requestBody` given as a `$ref` into `components/requestBodies` is resolved
// during parsing: its description/required surface in the parameter table, and
// at `--detail full` its referenced schema expands. Before resolution such a
// spec failed to parse at all.
#[test]
fn ref_request_body_is_resolved() {
    vimanam()
        .arg(OAS3_REF_BODY)
        .args(["--detail", "standard"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "| `requestBody` | body | Yes | Pet to add |",
        ));

    vimanam()
        .arg(OAS3_REF_BODY)
        .args(["--detail", "full", "--include-schemas"])
        .assert()
        .success()
        // The resolved body schema is linked and expanded in the definitions
        // section.
        .stdout(predicate::str::contains("## Schema Definitions"))
        .stdout(predicate::str::contains(
            "| `Pet.name` | string | Yes | Pet name |",
        ));
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

// Under `--inline-schemas` the recursive expansion still guards against `$ref`
// cycles, breaking the chain with a "cycle detected" row.
#[test]
fn inline_schema_expansion_detects_ref_cycles() {
    vimanam()
        .arg(OAS3_SCHEMA_REFS)
        .args(["--detail", "full", "--include-schemas", "--inline-schemas"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Cycle detected while expanding schema reference",
        ));
}

// #48: a parameter declared as a component `$ref` is resolved instead of
// failing the whole parse (a bare `$ref` param used to crash on `missing field name`).
#[test]
fn ref_parameter_is_resolved() {
    vimanam()
        .arg(REF_PARAMETER)
        .args(["--detail", "standard"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "| `limit` | query | No | Max results |",
        ));
}

// #50: a path item declared as a `$ref` yields its operation instead of being
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

// #56: an operation missing its `responses` block no longer fails the whole
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

// #54: an operation-level parameter overrides a path-level one of the same
// (name, in) — it should appear exactly once.
#[test]
fn duplicate_parameter_is_deduplicated() {
    let output = vimanam()
        .arg(OVERRIDE_PARAM)
        .args(["--detail", "standard"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();
    let id_rows = text.matches("| `id` | path |").count();
    assert_eq!(id_rows, 1, "expected a single `id` row, got {id_rows}");
    // The operation-level definition wins.
    assert!(text.contains("Operation-level id wins"));
}

// #60: an operation tagged with a value not in the declared `tags` list gets its
// own service section instead of being silently reassigned to the first service.
#[test]
fn unknown_operation_tag_keeps_its_own_service() {
    vimanam()
        .arg(UNKNOWN_TAG)
        .args(["--detail", "basic"])
        .assert()
        .success()
        // The undeclared tag Gamma becomes its own service section (under the
        // bug it would not exist — the endpoint was dumped under Alpha)...
        .stdout(predicate::str::contains("## Gamma"))
        .stdout(predicate::str::contains("W_Get"))
        // ...and the first declared service ends up with no endpoints.
        .stdout(predicate::str::contains(
            "## Alpha {#alpha}\n\nNo endpoints found for this service.",
        ));
}
