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
const OAS2_YAML: &str = "tests/fixtures/petstore_oas2.yaml";
const OAS3_SCHEMA_REFS_YAML: &str = "tests/fixtures/schema_refs_oas3.yaml";

// Parse-layer correctness cluster (issues #48, #50, #51, #54, #56, #60).
const MULTI_TAG: &str = "tests/fixtures/multi_tag_oas3.json";
const OAS2_HTTP_SCHEME: &str = "tests/fixtures/http_scheme_oas2.json";
const REF_PARAMETER: &str = "tests/fixtures/ref_parameter.json";
const REF_PATH_ITEM: &str = "tests/fixtures/ref_path_item.json";
const TYPE_ARRAY: &str = "tests/fixtures/type_array_nullable.json";
const MISSING_RESPONSES: &str = "tests/fixtures/missing_responses.json";
const OVERRIDE_PARAM: &str = "tests/fixtures/override_param.json";
const UNKNOWN_TAG: &str = "tests/fixtures/unknown_tag.json";

// --stats alignment with a service name wider than the SERVICE header (#42).
const STATS_LONG_SERVICE: &str = "tests/fixtures/stats_long_service_oas3.json";

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

// The OAS2 `schemes` field names the transfer protocol; a plain-HTTP spec must
// not be rendered with an assumed `https://` prefix. (Specs without `schemes`
// still default to https — covered by `oas2_spec_is_supported` above.)
#[test]
fn oas2_http_scheme_is_respected() {
    vimanam()
        .arg(OAS2_HTTP_SCHEME)
        .arg("--include-auth")
        .assert()
        .success()
        .stdout(predicate::str::contains("http://internal.example.com/v1"))
        .stdout(predicate::str::contains("https://internal.example.com").not());
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

// `--required-only` only filters the parameters table, which basic/summary
// detail never renders — so it warns there like the other no-effect flags.
#[test]
fn required_only_at_basic_detail_warns() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "basic", "--required-only"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "--required-only has no effect at --detail basic; use --detail standard or full.",
        ));
}

// At `--detail standard` the flag takes effect, so no warning is emitted.
#[test]
fn required_only_at_standard_detail_emits_no_warning() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "standard", "--required-only"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no effect").not());
}

// `--toc` is the explicit opposite of `--no-toc`; the TOC is on by default, and
// when both flags are given the later one wins.
#[test]
fn toc_flag_is_accepted_and_last_one_wins() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "basic", "--toc"])
        .assert()
        .success()
        .stdout(predicate::str::contains("## Services"));

    vimanam()
        .arg(OAS3)
        .args(["--detail", "basic", "--no-toc", "--toc"])
        .assert()
        .success()
        .stdout(predicate::str::contains("## Services"));

    vimanam()
        .arg(OAS3)
        .args(["--detail", "basic", "--toc", "--no-toc"])
        .assert()
        .success()
        .stdout(predicate::str::contains("## Services").not());
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

// Shell completions (#41): `vimanam completions <SHELL>` prints a completion
// script for every shell clap_complete supports, without needing a spec file.
#[test]
fn completions_are_generated_for_each_supported_shell() {
    // Each shell is paired with a marker unique to its script format, so the
    // test fails if every shell were to emit the same (e.g. bash) script.
    let shells = [
        ("bash", "complete -F _vimanam"),
        ("zsh", "#compdef vimanam"),
        ("fish", "complete -c vimanam"),
        ("powershell", "Register-ArgumentCompleter"),
        ("elvish", "edit:completion:arg-completer[vimanam]"),
    ];
    for (shell, marker) in shells {
        vimanam()
            .args(["completions", shell])
            .assert()
            .success()
            .stdout(predicate::str::contains(marker))
            .stderr(predicate::str::is_empty());
    }

    // The script must actually cover the CLI's options.
    vimanam()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--group-by"));
}

#[test]
fn completions_rejects_unsupported_shell() {
    vimanam()
        .args(["completions", "nushell"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("invalid value 'nushell'"));
}

#[test]
fn completions_requires_a_shell() {
    vimanam()
        .arg("completions")
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("<SHELL>"));
}

// The subcommand must not loosen the normal path: with no subcommand the spec
// file is still required.
#[test]
fn no_arguments_still_requires_input_file() {
    vimanam()
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("<FILE>"));
}

// Conversion flags and the spec file are meaningless alongside a subcommand,
// so mixing them is an error rather than being silently ignored.
#[test]
fn completions_conflicts_with_conversion_arguments() {
    vimanam()
        .args([OAS3, "completions", "bash"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

// --- Spec hygiene report (#44) ---

// A spec that trips every hygiene check at least once.
const HYGIENE: &str = "tests/fixtures/hygiene_oas3.json";
// One operation whose undescribed requestBody offers two media types.
const HYGIENE_MULTI_BODY: &str = "tests/fixtures/hygiene_multi_body_oas3.json";

// The report is appended after the body by default, separated by a rule.
#[test]
fn hygiene_report_is_appended_by_default() {
    vimanam()
        .arg(OAS3)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\n---\n\n## Spec Hygiene Report\n",
        ))
        .stdout(predicate::str::contains(
            "**4 endpoints** across **2 services**",
        ))
        .stdout(predicate::str::contains("| Deprecated | 1 |"))
        .stdout(predicate::str::contains(
            "### Deprecated (1)\n- `GET /store/orders`\n",
        ));
}

#[test]
fn no_report_suppresses_hygiene_report() {
    vimanam()
        .arg(OAS3)
        .arg("--no-report")
        .assert()
        .success()
        .stdout(predicate::str::contains("# Petstore API"))
        .stdout(predicate::str::contains("Spec Hygiene Report").not())
        .stdout(predicate::str::contains("\n---\n").not());
}

// Every check fires on the hygiene fixture; the whole report is asserted
// byte-for-byte so its shape (table order, list order, list item format) is
// pinned down.
#[test]
fn hygiene_report_lists_every_check() {
    let output = String::from_utf8(
        vimanam()
            .arg(HYGIENE)
            .args(["--detail", "standard"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    // A plain multi-line literal (no `\` continuations) so the two-space
    // indentation of the nested duplicate-operationId items is preserved.
    let expected = "
---

## Spec Hygiene Report

**6 endpoints** across **2 services**

| Check | Count |
|-------|------:|
| Missing description | 1 |
| Missing operationId | 1 |
| No responses documented | 1 |
| Deprecated | 2 |
| Untagged (no service tag) | 1 |
| Duplicate operationIds | 1 |
| Parameters without description | 3 |

### Missing description (1)
- `GET /health`

### Missing operationId (1)
- `GET /health`

### No responses documented (1)
- `POST /users`

### Deprecated (2)
- `GET /ping`
- `DELETE /users/{id}`

### Untagged (no service tag) (1)
- `GET /health`

### Duplicate operationIds (1)
- `getUser`
  - `DELETE /users/{id}`
  - `GET /users/{id}`

### Parameters without description (3)
- `GET /users` — `limit`
- `POST /users` — `requestBody`
- `DELETE /users/{id}` — `id`
";

    assert!(
        output.ends_with(expected),
        "report did not match.\nexpected tail:\n{expected}\nactual output:\n{output}"
    );
    // The body precedes the report.
    assert!(output.starts_with("# Hygiene API\n"), "{output}");
}

// The report analyzes the same endpoint set the body rendered: a service
// filter narrows both the counts and the service total.
#[test]
fn hygiene_report_respects_service_filter() {
    vimanam()
        .arg(HYGIENE)
        .args(["--service-filter", "Health"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "**1 endpoint** across **1 service**",
        ))
        .stdout(predicate::str::contains("| Deprecated | 1 |"))
        .stdout(predicate::str::contains(
            "| Untagged (no service tag) | 0 |",
        ))
        .stdout(predicate::str::contains("| Duplicate operationIds | 0 |"))
        .stdout(predicate::str::contains(
            "### Deprecated (1)\n- `GET /ping`\n",
        ))
        .stdout(predicate::str::contains("GET /health").not());
}

// `--exclude-deprecated` removes the deprecated DELETE, which also dissolves
// the duplicate operationId pair and drops its undescribed parameter; the
// deprecated GET /ping was the only Health endpoint, so one service remains.
#[test]
fn hygiene_report_respects_exclude_deprecated() {
    vimanam()
        .arg(HYGIENE)
        .arg("--exclude-deprecated")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "**4 endpoints** across **1 service**",
        ))
        .stdout(predicate::str::contains("| Deprecated | 0 |"))
        .stdout(predicate::str::contains("| Duplicate operationIds | 0 |"))
        .stdout(predicate::str::contains(
            "| Parameters without description | 2 |",
        ))
        .stdout(predicate::str::contains("### Deprecated").not())
        .stdout(predicate::str::contains("### Duplicate operationIds").not());
}

// `--max-tokens` budgets the body only; the report is still appended.
#[test]
fn hygiene_report_is_appended_under_max_tokens() {
    vimanam()
        .arg(OAS3)
        .args(["--detail", "full", "--max-tokens", "40"])
        .assert()
        .success()
        .stderr(predicate::str::contains("token budget"))
        .stdout(predicate::str::contains("## Spec Hygiene Report"));
}

// The file output path appends the report just like stdout does.
#[test]
fn hygiene_report_is_written_to_output_file() {
    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("out.md");

    vimanam()
        .arg(HYGIENE)
        .args(["-o", out_path.to_str().unwrap()])
        .assert()
        .success();

    let content = std::fs::read_to_string(&out_path).unwrap();
    assert!(content.starts_with("# Hygiene API\n"), "{content}");
    assert!(
        content.contains("\n---\n\n## Spec Hygiene Report\n"),
        "{content}"
    );
    assert!(
        content.contains("| Duplicate operationIds | 1 |"),
        "{content}"
    );
}

// The report is deterministic even with `--sort none` (spec order), where
// nothing but stable collections keeps the lists in a fixed order.
#[test]
fn hygiene_report_is_deterministic() {
    let run = || {
        vimanam()
            .arg(HYGIENE)
            .args(["--detail", "full", "--sort", "none"])
            .output()
            .unwrap()
            .stdout
    };

    let first = run();
    for _ in 0..4 {
        assert_eq!(first, run(), "report differed between identical runs");
    }
}

// The parser emits one synthetic body parameter per requestBody media type,
// all sharing the request body's description; an undescribed body is reported
// once per endpoint, not once per media type.
#[test]
fn hygiene_report_counts_multi_media_type_body_once() {
    let output = String::from_utf8(
        vimanam()
            .arg(HYGIENE_MULTI_BODY)
            .args(["--detail", "standard"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    // The body still documents both media types...
    assert!(
        output.contains("`requestBody (application/json)` | body |"),
        "{output}"
    );
    assert!(
        output.contains("`requestBody (application/xml)` | body |"),
        "{output}"
    );
    // ...but the report flags the body once.
    assert!(
        output.contains("| Parameters without description | 1 |"),
        "{output}"
    );
    assert!(
        output.ends_with(
            "### Parameters without description (1)\n- `POST /items` — `requestBody (application/json)`\n"
        ),
        "{output}"
    );
    assert_eq!(
        output.matches("- `POST /items` — `requestBody").count(),
        1,
        "{output}"
    );
}

// Exactly one blank line separates the body from the report at every detail
// level, even though the views themselves end with differing trailing
// whitespace (one newline at summary, a blank line otherwise).
#[test]
fn hygiene_report_is_separated_from_body_by_one_blank_line() {
    for detail in ["summary", "basic", "standard", "full"] {
        vimanam()
            .arg(OAS3)
            .args(["--detail", detail])
            .assert()
            .success()
            .stdout(predicate::str::contains(
                "\n\n---\n\n## Spec Hygiene Report",
            ))
            .stdout(predicate::str::contains("\n\n\n---").not());
    }
}

// A multi-tag operation is rendered under each of its services but analyzed
// once; the service count still reflects every service it appears in.
#[test]
fn hygiene_report_counts_multi_tag_endpoint_once() {
    vimanam()
        .arg(MULTI_TAG)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "**1 endpoint** across **2 services**",
        ));
}

// Filters that leave nothing visible produce an all-zero report with no
// detail sections, and no services (only services with visible endpoints
// are counted).
#[test]
fn hygiene_report_on_empty_filtered_set_is_all_zero() {
    let output = String::from_utf8(
        vimanam()
            .arg(OAS3)
            .args(["--path-filter", "/nope"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    let expected = "\
## Spec Hygiene Report

**0 endpoints** across **0 services**

| Check | Count |
|-------|------:|
| Missing description | 0 |
| Missing operationId | 0 |
| No responses documented | 0 |
| Deprecated | 0 |
| Untagged (no service tag) | 0 |
| Duplicate operationIds | 0 |
| Parameters without description | 0 |
";
    assert!(output.ends_with(expected), "{output}");
    assert!(!output.contains("\n### "), "{output}");
}

// --- --stats token-budget dry-run (#42) ---

fn stats_output(args: &[&str]) -> String {
    let output = vimanam().args(args).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "stats wrote to stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

// Splits a stats table into (service, endpoints, ~tokens) rows, header excluded.
fn stats_rows(table: &str) -> Vec<(String, usize, usize)> {
    let mut lines = table.lines();
    assert_eq!(
        lines
            .next()
            .map(|line| line.split_whitespace().collect::<Vec<_>>()),
        Some(vec!["SERVICE", "ENDPOINTS", "~TOKENS"]),
        "{table}"
    );
    lines
        .map(|line| {
            let mut fields = line.split_whitespace().rev();
            let tokens: usize = fields.next().unwrap().parse().unwrap();
            let endpoints: usize = fields.next().unwrap().parse().unwrap();
            let name: Vec<&str> = fields.rev().collect();
            (name.join(" "), endpoints, tokens)
        })
        .collect()
}

// Petstore declares Pets (GET /pets, POST /pets, GET /pets/{petId}) and Store
// (GET /store/orders): one row each in declared order, then a TOTAL of 4.
#[test]
fn stats_lists_each_service_with_endpoint_counts_and_total() {
    let table = stats_output(&[OAS3, "--stats"]);
    let rows = stats_rows(&table);

    assert_eq!(rows.len(), 3, "{table}");
    assert_eq!((rows[0].0.as_str(), rows[0].1), ("Pets", 3));
    assert_eq!((rows[1].0.as_str(), rows[1].1), ("Store", 1));
    assert_eq!((rows[2].0.as_str(), rows[2].1), ("TOTAL", 4));
    for (name, _, tokens) in &rows {
        assert!(*tokens > 0, "{name} has no token estimate: {table}");
    }

    // Plain text only: no blank lines, no Markdown, one trailing newline.
    assert!(table.ends_with('\n') && !table.ends_with("\n\n"), "{table}");
    assert!(!table.contains("\n\n"), "{table}");
    assert!(!table.contains('#') && !table.contains('|'), "{table}");
}

// The exact layout: SERVICE padded to the widest name (the header here),
// numeric columns right-aligned under their headers, three spaces between.
#[test]
fn stats_columns_are_aligned() {
    let table = stats_output(&[OAS3, "--stats"]);
    let lines: Vec<&str> = table.lines().collect();

    assert_eq!(lines[0], "SERVICE   ENDPOINTS   ~TOKENS");
    assert!(lines[1].starts_with("Pets              3   "), "{table}");
    assert!(lines[2].starts_with("Store             1   "), "{table}");
    assert!(lines[3].starts_with("TOTAL             4   "), "{table}");
    let width = lines[0].len();
    assert!(lines.iter().all(|line| line.len() == width), "{table}");
}

#[test]
fn stats_is_deterministic() {
    let first = stats_output(&[OAS3, "--stats", "--detail", "full", "--include-schemas"]);
    let second = stats_output(&[OAS3, "--stats", "--detail", "full", "--include-schemas"]);
    assert_eq!(first, second);
}

// More detail renders more text, so the estimate for the same service grows.
#[test]
fn stats_tokens_grow_with_detail_level() {
    let summary = stats_rows(&stats_output(&[OAS3, "--stats", "--detail", "summary"]));
    let full = stats_rows(&stats_output(&[
        OAS3,
        "--stats",
        "--detail",
        "full",
        "--include-schemas",
    ]));

    for (row, summary_row) in full.iter().zip(&summary) {
        assert_eq!(row.0, summary_row.0);
        assert!(
            row.2 > summary_row.2,
            "{}: full {} <= summary {}",
            row.0,
            row.2,
            summary_row.2
        );
    }
}

#[test]
fn stats_respects_service_filter() {
    let rows = stats_rows(&stats_output(&[
        OAS3,
        "--stats",
        "--service-filter",
        "store",
    ]));

    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0].0.as_str(), rows[0].1), ("Store", 1));
    assert_eq!((rows[1].0.as_str(), rows[1].1), ("TOTAL", 1));
}

// A multi-tag operation is counted in each of its service rows but once in
// the TOTAL, so the TOTAL is not the sum of the rows.
#[test]
fn stats_counts_multi_tag_endpoint_in_each_service_but_once_in_total() {
    let rows = stats_rows(&stats_output(&[MULTI_TAG, "--stats"]));

    assert_eq!(rows.len(), 3);
    assert_eq!((rows[0].0.as_str(), rows[0].1), ("Pets", 1));
    assert_eq!((rows[1].0.as_str(), rows[1].1), ("Admin", 1));
    assert_eq!((rows[2].0.as_str(), rows[2].1), ("TOTAL", 1));
}

// The hygiene fixture's Users service has 5 endpoints (one deprecated) and
// Health has only a deprecated one; excluding deprecated drops Users to 4 and
// removes the Health row entirely.
#[test]
fn stats_respects_exclude_deprecated() {
    let all = stats_rows(&stats_output(&[HYGIENE, "--stats"]));
    assert_eq!(all.len(), 3);
    assert_eq!((all[0].0.as_str(), all[0].1), ("Users", 5));
    assert_eq!((all[1].0.as_str(), all[1].1), ("Health", 1));
    assert_eq!((all[2].0.as_str(), all[2].1), ("TOTAL", 6));

    let live = stats_rows(&stats_output(&[HYGIENE, "--stats", "--exclude-deprecated"]));
    assert_eq!(live.len(), 2);
    assert_eq!((live[0].0.as_str(), live[0].1), ("Users", 4));
    assert_eq!((live[1].0.as_str(), live[1].1), ("TOTAL", 4));
}

// Filters that leave nothing visible still print the header and a zero TOTAL.
#[test]
fn stats_on_empty_filtered_set_is_header_and_zero_total() {
    let table = stats_output(&[OAS3, "--stats", "--path-filter", "/nope"]);
    assert_eq!(
        table,
        "\
SERVICE   ENDPOINTS   ~TOKENS
TOTAL             0         0
"
    );
}

// The hygiene report is never part of stats output, whatever --no-report says.
#[test]
fn stats_never_includes_hygiene_report() {
    let table = stats_output(&[OAS3, "--stats"]);
    assert!(!table.contains("Spec Hygiene Report"), "{table}");
    assert!(!table.contains("---"), "{table}");

    let with_flag = stats_output(&[OAS3, "--stats", "--no-report"]);
    assert_eq!(table, with_flag);
}

// Renders the document with `args` and returns the chars/4 token estimate of
// the resulting Markdown, exactly as `--max-tokens` and `--stats` compute it.
fn rendered_tokens(args: &[&str]) -> usize {
    let output = vimanam().args(args).arg("--no-report").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout)
        .unwrap()
        .chars()
        .count()
        .div_ceil(4)
}

// Each row's estimate must equal a real render of that service alone under the
// same flags, and TOTAL a real render of the whole document, so the table can
// be trusted as a menu for `--service-filter`.
fn assert_stats_match_real_render(render_args: &[&str]) {
    let mut stats_args = vec![OAS3, "--stats"];
    stats_args.extend_from_slice(render_args);
    let table = stats_output(&stats_args);
    let rows = stats_rows(&table);
    assert!(rows.len() > 1, "{table}");

    for (name, _, tokens) in &rows {
        let mut args = vec![OAS3];
        args.extend_from_slice(render_args);
        if name != "TOTAL" {
            args.extend_from_slice(&["--service-filter", name]);
        }
        assert_eq!(
            *tokens,
            rendered_tokens(&args),
            "{name} estimate disagrees with a real render: {table}"
        );
    }
}

#[test]
fn stats_row_tokens_match_real_render() {
    assert_stats_match_real_render(&["--detail", "full", "--include-schemas"]);
}

// The flat view renders differently from the service view; the estimate must
// follow the grouping flag rather than always sizing the service view.
#[test]
fn stats_row_tokens_match_real_render_under_flat() {
    assert_stats_match_real_render(&["--flat", "--detail", "standard"]);
}

// A service name wider than the SERVICE header widens the first column: the
// header is padded to the name, and every line still has the same length.
#[test]
fn stats_pads_service_column_to_a_long_name() {
    let table = stats_output(&[STATS_LONG_SERVICE, "--stats"]);
    let lines: Vec<&str> = table.lines().collect();
    let name = "Long Service Name";

    assert_eq!(lines.len(), 3, "{table}");
    assert!(
        lines[0].starts_with(&format!(
            "{:<width$}   ENDPOINTS",
            "SERVICE",
            width = name.len()
        )),
        "{table}"
    );
    assert!(lines[1].starts_with(&format!("{name}   ")), "{table}");
    assert!(
        lines[2].starts_with(&format!("{:<width$}   ", "TOTAL", width = name.len())),
        "{table}"
    );
    let width = lines[0].chars().count();
    assert!(
        lines.iter().all(|line| line.chars().count() == width),
        "{table}"
    );

    let rows = stats_rows(&table);
    assert_eq!((rows[0].0.as_str(), rows[0].1), (name, 2));
    assert_eq!((rows[1].0.as_str(), rows[1].1), ("TOTAL", 2));
}

// Writing a stats table to a file and budgeting a dry run are both
// meaningless; clap rejects the combinations with its usage error.
#[test]
fn stats_conflicts_with_output() {
    vimanam()
        .args([OAS3, "--stats", "-o", "stats.txt"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn stats_conflicts_with_max_tokens() {
    vimanam()
        .args([OAS3, "--stats", "--max-tokens", "100"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("cannot be used with"));
}

// --- diff subcommand (#45) ---

// Two versions of one spec. `new` adds a required `pricing` property to the
// shared `Widget` schema while leaving the `GET /widgets` operation object
// byte-identical, removes the deprecated `GET /legacy`, adds
// `GET /widgets/{id}/history`, makes the `fields` query parameter required and
// drops the 404 response on `GET /widgets/{id}`, renames and deprecates the
// `DELETE /widgets/{id}` operation, changes `WidgetInput.weight` from float
// to double, and rewords descriptions only (the `X-Trace` header, the
// `WidgetInput.name` property).
const DIFF_OLD: &str = "tests/fixtures/diff_old_oas3.json";
const DIFF_NEW: &str = "tests/fixtures/diff_new_oas3.json";

const DIFF_SUMMARY: &str =
    "**Summary:** 1 endpoint added, 1 removed, 4 changed; 4 breaking, 8 non-breaking, 1 to review";

/// Runs `vimanam diff` with `args` and returns `(stdout, exit code)`.
fn diff_run(args: &[&str]) -> (String, i32) {
    let output = vimanam().arg("diff").args(args).output().unwrap();
    (
        String::from_utf8(output.stdout).unwrap(),
        output.status.code().unwrap(),
    )
}

fn load_json(path: &str) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Writes `spec` to `<dir>/<name>` so an edited copy of a fixture can be diffed.
fn write_spec(dir: &tempfile::TempDir, name: &str, spec: &serde_json::Value) -> String {
    let path = dir.path().join(name);
    std::fs::write(&path, serde_json::to_string_pretty(spec).unwrap()).unwrap();
    path.to_str().unwrap().to_string()
}

#[test]
fn diff_reports_added_removed_changed() {
    let (stdout, code) = diff_run(&[DIFF_OLD, DIFF_NEW]);
    assert_eq!(code, 0, "{stdout}");

    assert!(
        stdout.starts_with("# API Diff: Widgets API 1.0.0 → 1.1.0\n\n"),
        "{stdout}"
    );
    assert!(stdout.contains(&format!("\n{DIFF_SUMMARY}\n")), "{stdout}");

    let breaking = "\
## Breaking changes (4)

| Change | Endpoint | Detail |
|--------|----------|--------|
| Parameter newly required | `GET /widgets/{id}` | `fields` (query) |
| Response removed | `GET /widgets/{id}` | 404 |
| operationId changed | `DELETE /widgets/{id}` | `Widgets_DeleteWidget` → `Widgets_RemoveWidget` |
| Endpoint removed | `GET /legacy` | was deprecated |
";
    assert!(stdout.contains(breaking), "{stdout}");

    let non_breaking = "\
## Non-breaking changes (8)

| Change | Endpoint | Detail |
|--------|----------|--------|
| Response schema changed | `GET /widgets` | 200 `/properties/pricing` added |
| Response schema changed | `GET /widgets` | 200 `/properties/pricing` added to `required` |
| Response schema changed | `POST /widgets` | 201 `/properties/pricing` added |
| Response schema changed | `POST /widgets` | 201 `/properties/pricing` added to `required` |
| Response schema changed | `GET /widgets/{id}` | 200 `/properties/pricing` added |
| Response schema changed | `GET /widgets/{id}` | 200 `/properties/pricing` added to `required` |
| Marked deprecated | `DELETE /widgets/{id}` | - |
| Endpoint added | `GET /widgets/{id}/history` | - |
";
    assert!(stdout.contains(non_breaking), "{stdout}");

    // Without --report there is no Deltas section.
    assert!(!stdout.contains("## Deltas"), "{stdout}");
}

// nrynss's requirement on #45: a change behind a shared `$ref` must surface on
// every endpoint whose operation object did not change at all.
#[test]
fn diff_detects_schema_drift_behind_shared_ref() {
    let old = load_json(DIFF_OLD);
    let new = load_json(DIFF_NEW);
    assert_eq!(
        old.pointer("/paths/~1widgets/get"),
        new.pointer("/paths/~1widgets/get"),
        "fixture precondition: the GET /widgets operation object must be identical"
    );
    assert_ne!(
        old.pointer("/components/schemas/Widget"),
        new.pointer("/components/schemas/Widget"),
        "fixture precondition: the shared Widget schema must differ"
    );

    let (stdout, _) = diff_run(&[DIFF_OLD, DIFF_NEW]);
    assert!(
        stdout.contains(
            "| Response schema changed | `GET /widgets` | 200 `/properties/pricing` added |"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "| Response schema changed | `GET /widgets` | 200 `/properties/pricing` added to `required` |"
        ),
        "{stdout}"
    );
}

#[test]
fn diff_classifies_format_change_as_review() {
    let (stdout, _) = diff_run(&[DIFF_OLD, DIFF_NEW]);
    let review = "\
## Needs review (1)

| Change | Endpoint | Detail |
|--------|----------|--------|
| Request schema changed | `POST /widgets` | `/properties/weight/format` changed `float` → `double` |
";
    assert!(stdout.contains(review), "{stdout}");
}

// Reworded descriptions (on a parameter and inside a schema) are not changes.
#[test]
fn diff_ignores_description_only_changes() {
    let old = load_json(DIFF_OLD);
    let new = load_json(DIFF_NEW);
    assert_ne!(
        old.pointer("/paths/~1widgets/post/parameters/0/description"),
        new.pointer("/paths/~1widgets/post/parameters/0/description"),
        "fixture precondition: X-Trace description differs"
    );

    let (stdout, _) = diff_run(&[DIFF_OLD, DIFF_NEW]);
    assert!(!stdout.contains("X-Trace"), "{stdout}");
    assert!(!stdout.contains("/properties/name"), "{stdout}");
    assert!(!stdout.contains("description"), "{stdout}");
}

#[test]
fn diff_fail_on_breaking_exits_3() {
    // Without the flag, breaking changes are reported but the exit is 0.
    let (_, code) = diff_run(&[DIFF_OLD, DIFF_NEW]);
    assert_eq!(code, 0);

    // With the flag the full report is still written before exiting 3.
    let (stdout, code) = diff_run(&[DIFF_OLD, DIFF_NEW, "--fail-on-breaking"]);
    assert_eq!(code, 3, "{stdout}");
    assert!(stdout.contains("## Breaking changes (4)"), "{stdout}");
    assert!(stdout.contains("## Needs review (1)"), "{stdout}");
}

// Review-level and non-breaking changes never trip --fail-on-breaking.
#[test]
fn diff_fail_on_breaking_passes_without_breaking_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mut edited = load_json(DIFF_OLD);
    // A format change (review) and a new optional parameter (non-breaking).
    *edited
        .pointer_mut("/components/schemas/WidgetInput/properties/weight/format")
        .unwrap() = serde_json::json!("double");
    edited
        .pointer_mut("/paths/~1widgets/get/parameters")
        .unwrap()
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "name": "offset",
            "in": "query",
            "schema": {"type": "integer"}
        }));
    let edited_path = write_spec(&dir, "edited.json", &edited);

    let (stdout, code) = diff_run(&[DIFF_OLD, &edited_path, "--fail-on-breaking"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(!stdout.contains("## Breaking changes"), "{stdout}");
    assert!(
        stdout.contains("| Parameter added | `GET /widgets` | `offset` (query), optional |"),
        "{stdout}"
    );
    assert!(stdout.contains("## Needs review (1)"), "{stdout}");
}

#[test]
fn diff_no_changes_exits_0_and_says_so() {
    let (stdout, code) = diff_run(&[DIFF_OLD, DIFF_OLD]);
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "# API Diff: Widgets API 1.0.0 → 1.0.0\n\n**Summary:** No changes.\n"
    );

    // Self-referential schemas (Node -> Node, Pet -> Pet) must resolve and
    // compare without looping.
    let (stdout, code) = diff_run(&[OAS3_SCHEMA_REFS, OAS3_SCHEMA_REFS, "--fail-on-breaking"]);
    assert_eq!(code, 0);
    assert!(stdout.ends_with("**Summary:** No changes.\n"), "{stdout}");
}

#[test]
fn diff_report_shows_hygiene_and_token_deltas() {
    let (stdout, code) = diff_run(&[DIFF_OLD, DIFF_NEW, "--report"]);
    assert_eq!(code, 0, "{stdout}");

    let deltas_header = "\
## Deltas

| Check | Old | New | Δ |
|-------|----:|----:|--:|
";
    assert!(stdout.contains(deltas_header), "{stdout}");
    // The added history endpoint has no summary; the deprecated count moves
    // from GET /legacy to DELETE /widgets/{id}.
    assert!(
        stdout.contains("| Missing description | 0 | 1 | +1 |"),
        "{stdout}"
    );
    assert!(stdout.contains("| Deprecated | 1 | 1 | 0 |"), "{stdout}");
    assert!(
        stdout.contains("| Parameters without description | 0 | 0 | 0 |"),
        "{stdout}"
    );

    let token_line = stdout
        .lines()
        .find(|line| line.starts_with("Token estimate (--detail full --include-schemas): "))
        .unwrap_or_else(|| panic!("no token estimate line in:\n{stdout}"));
    let numbers: Vec<usize> = token_line
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap())
        .collect();
    let (old_tokens, new_tokens) = (numbers[0], numbers[1]);
    assert!(new_tokens > old_tokens, "{token_line}");
    assert!(
        token_line.ends_with(&format!("(+{})", new_tokens - old_tokens)),
        "{token_line}"
    );
    assert!(stdout.ends_with(&format!("{token_line}\n")), "{stdout}");

    // With no changes the Deltas section still follows the summary.
    let (stdout, _) = diff_run(&[DIFF_OLD, DIFF_OLD, "--report"]);
    assert!(
        stdout.contains("**Summary:** No changes.\n\n## Deltas\n"),
        "{stdout}"
    );
}

#[test]
fn diff_output_is_deterministic() {
    let first = diff_run(&[DIFF_OLD, DIFF_NEW, "--report"]);
    for _ in 0..4 {
        assert_eq!(first, diff_run(&[DIFF_OLD, DIFF_NEW, "--report"]));
    }
}

#[test]
fn diff_writes_to_output_file() {
    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("diff.md");

    vimanam()
        .args(["diff", DIFF_OLD, DIFF_NEW, "-o", out_path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    let content = std::fs::read_to_string(&out_path).unwrap();
    let (stdout, _) = diff_run(&[DIFF_OLD, DIFF_NEW]);
    assert_eq!(content, stdout);
    assert!(content.starts_with("# API Diff: Widgets API"), "{content}");

    // --fail-on-breaking still exits 3 when writing to a file.
    vimanam()
        .args([
            "diff",
            DIFF_OLD,
            DIFF_NEW,
            "--fail-on-breaking",
            "-o",
            out_path.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(3);
}

// The conversion arguments are meaningless alongside the subcommand.
#[test]
fn diff_rejects_conversion_flags() {
    vimanam()
        .args([OAS3, "diff", DIFF_OLD, DIFF_NEW])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("cannot be used with"));

    vimanam()
        .args(["--detail", "full", "diff", DIFF_OLD, DIFF_NEW])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn diff_requires_two_specs() {
    vimanam()
        .args(["diff", DIFF_OLD])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("<NEW>"));
}

// A spec that fails to parse is a runtime error (1), distinct from breaking (3).
#[test]
fn diff_unparseable_spec_exits_1() {
    vimanam()
        .args([
            "diff",
            DIFF_OLD,
            "tests/fixtures/does_not_exist.json",
            "--fail-on-breaking",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("Failed to parse OpenAPI file"));
}

// Swagger 2.0: `#/definitions/` refs resolve, OAS2 `schema` responses are
// compared, and a non-body parameter's inline `type` is compared.
#[test]
fn diff_works_for_oas2() {
    let (stdout, code) = diff_run(&[OAS2, OAS2]);
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "# API Diff: Petstore Legacy API 2.0.0 → 2.0.0\n\n**Summary:** No changes.\n"
    );

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
        "properties": {"name": {"type": "string"}}
    });
    let old_path = write_spec(&dir, "old.json", &old);
    let new_path = write_spec(&dir, "new.json", &new);

    let (stdout, code) = diff_run(&[&old_path, &new_path, "--fail-on-breaking"]);
    assert_eq!(code, 3, "{stdout}");
    assert!(
        stdout.contains(
            "| Parameter schema changed | `POST /pets` | `limit` (query) `/type` changed `integer` → `string` |"
        ),
        "{stdout}"
    );
    // The body parameter and the 200 response both reference #/definitions/Pet.
    assert!(
        stdout.contains("| Request schema changed | `POST /pets` | `/properties/name` added |"),
        "{stdout}"
    );
    assert!(
        stdout
            .contains("| Response schema changed | `POST /pets` | 200 `/properties/name` added |"),
        "{stdout}"
    );
}

// ── Markdown byte-stability (#98) ───────────────────────────────────────────
//
// The captured Markdown must stay byte-identical; JSON output was added
// alongside it without touching the Markdown path.

#[test]
fn diff_markdown_is_byte_identical_to_captured_fixture() {
    let expected = std::fs::read_to_string("tests/fixtures/diff_expected.md").unwrap();
    let (stdout, code) = diff_run(&[DIFF_OLD, DIFF_NEW]);
    assert_eq!(code, 0);
    assert_eq!(stdout, expected);
}

#[test]
fn diff_markdown_with_report_is_byte_identical_to_captured_fixture() {
    let expected = std::fs::read_to_string("tests/fixtures/diff_expected_report.md").unwrap();
    let (stdout, code) = diff_run(&[DIFF_OLD, DIFF_NEW, "--report"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, expected);
}

#[test]
fn diff_default_format_is_markdown() {
    let (default, _) = diff_run(&[DIFF_OLD, DIFF_NEW]);
    let (explicit, _) = diff_run(&[DIFF_OLD, DIFF_NEW, "--format", "markdown"]);
    assert_eq!(default, explicit);
    assert_eq!(
        default,
        std::fs::read_to_string("tests/fixtures/diff_expected.md").unwrap()
    );
}

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

// ---------------------------------------------------------------------------
// Exact operation selection: --operation / --operation-id (#99)
// ---------------------------------------------------------------------------

const OP_SELECT: &str = "tests/fixtures/operation_select_oas3.json";
const DIFF_OLD_OAS3: &str = "tests/fixtures/diff_old_oas3.json";
const DIFF_NEW_OAS3: &str = "tests/fixtures/diff_new_oas3.json";
const DIFF_OLD_OAS2: &str = "tests/fixtures/diff_old_oas2.json";
const DIFF_NEW_OAS2: &str = "tests/fixtures/diff_new_oas2.json";

/// Runs a conversion and returns (exit code, stdout, stderr).
fn run_select(args: &[&str]) -> (i32, String, String) {
    let output = vimanam().args(args).output().unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// The `**Operation:** METHOD /path` lines of a `--detail basic` or richer render.
fn rendered_operations(markdown: &str) -> Vec<&str> {
    markdown
        .lines()
        .filter_map(|line| line.strip_prefix("**Operation:** "))
        .collect()
}

#[test]
fn operation_selects_exactly_one_endpoint() {
    let (code, stdout, _) = run_select(&[
        OP_SELECT,
        "--operation",
        "GET /users",
        "--flat",
        "--detail",
        "basic",
    ]);
    assert_eq!(code, 0);
    assert_eq!(rendered_operations(&stdout), ["GET /users"]);
}

#[test]
fn path_filter_remains_a_substring_match() {
    let (code, stdout, _) = run_select(&[
        OP_SELECT,
        "--path-filter",
        "/users",
        "--method-filter",
        "GET",
        "--flat",
        "--detail",
        "basic",
    ]);
    assert_eq!(code, 0);
    assert_eq!(
        rendered_operations(&stdout),
        [
            "GET /admin/users",
            "GET /users",
            "GET /users/{id}",
            "GET /users/{id}/keys"
        ]
    );
}

#[test]
fn operation_method_is_case_insensitive() {
    let upper = run_select(&[OP_SELECT, "--operation", "GET /users", "--detail", "basic"]);
    let lower = run_select(&[OP_SELECT, "--operation", "get /users", "--detail", "basic"]);
    assert_eq!(upper.0, 0);
    assert_eq!(rendered_operations(&upper.1), ["GET /users"]);
    assert_eq!(upper, lower);
}

#[test]
fn operation_path_template_and_trailing_slash_are_literal() {
    for value in ["GET /users/{userId}", "GET /users/"] {
        let (code, stdout, stderr) = run_select(&[OP_SELECT, "--operation", value]);
        assert_eq!(code, 1, "{value} should not match");
        assert!(stdout.is_empty());
        assert!(
            stderr.contains(&format!("--operation matched no endpoint: {value:?}")),
            "stderr: {stderr}"
        );
    }
}

#[test]
fn operations_and_operation_ids_render_their_union_once_under_flat() {
    let (code, stdout, _) = run_select(&[
        OP_SELECT,
        "--operation",
        "GET /users",
        "--operation",
        "DELETE /users/{id}",
        "--operation-id",
        "getUser",
        // Selected twice (by path and ID): still rendered once.
        "--operation-id",
        "listUsers",
        "--flat",
        "--detail",
        "basic",
    ]);
    assert_eq!(code, 0);
    assert_eq!(
        rendered_operations(&stdout),
        ["GET /users", "DELETE /users/{id}", "GET /users/{id}"]
    );
}

#[test]
fn multi_tag_operation_appears_under_each_service_unless_flat() {
    let args = [OP_SELECT, "--operation-id", "getUser", "--detail", "basic"];
    let (_, grouped, _) = run_select(&args);
    assert_eq!(rendered_operations(&grouped).len(), 2);
    let (_, flat, _) = run_select(&[&args[..], &["--flat"]].concat());
    assert_eq!(rendered_operations(&flat), ["GET /users/{id}"]);
}

#[test]
fn duplicate_operation_id_selects_every_carrier() {
    let (code, stdout, _) = run_select(&[
        OP_SELECT,
        "--operation-id",
        "export",
        "--flat",
        "--detail",
        "basic",
    ]);
    assert_eq!(code, 0);
    assert_eq!(
        rendered_operations(&stdout),
        ["GET /exports/a", "GET /exports/b"]
    );
    // The hygiene report still flags the duplicate, scoped to the selection.
    assert!(stdout.contains("**2 endpoints** across **1 service**"));
    assert!(stdout.contains("| Duplicate operationIds | 1 |"));
}

#[test]
fn operation_id_is_case_sensitive() {
    let (code, _, stderr) = run_select(&[OP_SELECT, "--operation-id", "listusers"]);
    assert_eq!(code, 1);
    assert!(stderr.contains(r#"--operation-id matched no endpoint: "listusers""#));
}

#[test]
fn unmatched_selectors_fail_listing_every_value_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.md");
    let (code, stdout, stderr) = run_select(&[
        OP_SELECT,
        "--operation",
        "GET /user",
        "--operation",
        "GET /users",
        "--operation",
        "PATCH /nowhere/at/all",
        "--operation-id",
        "nope",
        "-o",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(!out.exists(), "no output file on a selector error");
    assert!(
        stderr.contains(
            r#"Error: --operation matched no endpoint: "GET /user" (did you mean "GET /users"?), "PATCH /nowhere/at/all""#
        ),
        "stderr: {stderr}"
    );
    assert!(stderr.contains(r#"--operation-id matched no endpoint: "nope""#));
}

#[test]
fn selector_removed_by_another_filter_warns_but_succeeds() {
    let (code, stdout, stderr) = run_select(&[
        OP_SELECT,
        "--operation",
        "GET /legacy",
        "--exclude-deprecated",
        "--detail",
        "basic",
    ]);
    assert_eq!(code, 0);
    assert!(rendered_operations(&stdout).is_empty());
    assert!(
        stderr.contains(
            r#"--operation "GET /legacy" matched an operation removed by --exclude-deprecated"#
        ),
        "stderr: {stderr}"
    );

    // No warning when the selection survives.
    let (_, _, stderr) = run_select(&[OP_SELECT, "--operation", "GET /legacy"]);
    assert!(!stderr.contains("removed by"), "stderr: {stderr}");
}

#[test]
fn malformed_operation_values_are_usage_errors() {
    for value in ["GET", "/users", "GET users"] {
        let (code, stdout, stderr) = run_select(&[OP_SELECT, "--operation", value]);
        assert_eq!(code, 2, "{value:?} should be a usage error");
        assert!(stdout.is_empty());
        assert!(stderr.contains("METHOD /path"), "stderr: {stderr}");
    }
}

/// For every change in `vimanam diff --format json`, `--operation "<method>
/// <path>"` renders exactly that endpoint from the spec it exists in.
fn assert_diff_round_trips(old: &str, new: &str) {
    let output = vimanam()
        .args(["diff", old, new, "--format", "json"])
        .output()
        .unwrap();
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let changes = document["changes"].as_array().unwrap();
    assert!(!changes.is_empty());
    for change in changes {
        let method = change["endpoint"]["method"].as_str().unwrap();
        let path = change["endpoint"]["path"].as_str().unwrap();
        let spec = if change["kind"] == "endpoint_removed" {
            old
        } else {
            new
        };
        let selector = format!("{method} {path}");
        let (code, stdout, stderr) = run_select(&[
            spec,
            "--operation",
            &selector,
            "--flat",
            "--detail",
            "basic",
        ]);
        assert_eq!(code, 0, "{selector}: {stderr}");
        assert_eq!(rendered_operations(&stdout), [selector.as_str()]);
    }
}

#[test]
fn diff_json_endpoints_round_trip_to_operation_oas3() {
    assert_diff_round_trips(DIFF_OLD_OAS3, DIFF_NEW_OAS3);
}

// Swagger 2 paths are stored (and diffed) without `basePath`, so the same
// string works for both commands.
#[test]
fn diff_json_endpoints_round_trip_to_operation_oas2() {
    assert_diff_round_trips(DIFF_OLD_OAS2, DIFF_NEW_OAS2);
    let (code, _, _) = run_select(&[DIFF_NEW_OAS2, "--operation", "GET /v1/gadgets"]);
    assert_eq!(code, 1, "basePath is not part of the path template");
}

#[test]
fn operation_composes_with_stats() {
    let (code, stdout, _) = run_select(&[
        OP_SELECT,
        "--operation",
        "GET /users/{id}",
        "--operation-id",
        "export",
        "--stats",
    ]);
    assert_eq!(code, 0);
    let total = stdout.lines().find(|l| l.starts_with("TOTAL")).unwrap();
    assert_eq!(total.split_whitespace().nth(1), Some("3"));
}

#[test]
fn operation_stats_with_unmatched_selector_fails() {
    let (code, stdout, _) = run_select(&[OP_SELECT, "--operation", "GET /nope", "--stats"]);
    assert_eq!(code, 1);
    assert!(stdout.is_empty());
}

#[test]
fn operation_composes_with_full_detail_schemas_examples_and_budget() {
    let (code, stdout, _) = run_select(&[
        OP_SELECT,
        "--operation",
        "GET /users",
        "--detail",
        "full",
        "--include-schemas",
        "--include-examples",
        "--flat",
    ]);
    assert_eq!(code, 0);
    assert_eq!(rendered_operations(&stdout), ["GET /users"]);
    assert!(stdout.contains("## Schema Definitions"));
    assert!(stdout.contains("Ada"), "example should render");
    assert!(stdout.contains("**1 endpoint** across **1 service**"));

    // --inline-schemas and --max-tokens keep the same single-endpoint scope.
    // The budget fits the selection at --detail full (so the inline schema rows
    // render) but not the whole spec.
    let budget_args = [
        OP_SELECT,
        "--detail",
        "full",
        "--include-schemas",
        "--inline-schemas",
        "--max-tokens",
        "200",
        "--flat",
        "--no-report",
    ];
    let (code, stdout, stderr) =
        run_select(&[&budget_args[..], &["--operation", "GET /users"]].concat());
    assert_eq!(code, 0);
    assert!(stderr.is_empty(), "no detail reduction expected: {stderr}");
    assert_eq!(rendered_operations(&stdout), ["GET /users"]);
    assert!(
        stdout.contains("`response[].id`"),
        "inline schema rows: {stdout}"
    );
    assert!(!stdout.contains("## Schema Definitions"));
    // Without the selector the same budget forces the detail down.
    let (_, _, stderr) = run_select(&budget_args);
    assert!(!stderr.is_empty(), "whole spec should not fit 200 tokens");
}

#[test]
fn operation_output_is_deterministic() {
    let args = [
        OP_SELECT,
        "--operation",
        "GET /users",
        "--operation",
        "DELETE /users/{id}",
        "--operation-id",
        "export",
        "--detail",
        "full",
        "--include-schemas",
        "--include-examples",
        "--sort",
        "none",
    ];
    let first = run_select(&args);
    for _ in 0..4 {
        assert_eq!(first, run_select(&args));
    }
}

#[test]
fn completions_offer_the_operation_flags() {
    let output = vimanam().args(["completions", "bash"]).output().unwrap();
    let script = String::from_utf8(output.stdout).unwrap();
    assert!(script.contains("--operation"));
    assert!(script.contains("--operation-id"));
}

#[test]
fn selector_warning_names_every_removing_filter() {
    let cases: [(&[&str], &str); 4] = [
        (
            &["--service-filter", "Admin"],
            "removed by --service-filter",
        ),
        (&["--method-filter", "POST"], "removed by --method-filter"),
        (&["--path-filter", "/admin"], "removed by --path-filter"),
        (
            &["--method-filter", "POST", "--path-filter", "/admin"],
            "removed by --method-filter, --path-filter",
        ),
    ];
    for (filters, expected) in cases {
        let args = [&[OP_SELECT, "--operation", "GET /users"][..], filters].concat();
        let (code, _, stderr) = run_select(&args);
        assert_eq!(code, 0);
        assert!(
            stderr.contains(&format!(
                r#"--operation "GET /users" matched an operation {expected};"#
            )),
            "{filters:?}: {stderr}"
        );
    }
}

#[test]
fn duplicate_id_with_one_surviving_carrier_does_not_warn() {
    let (code, stdout, stderr) = run_select(&[
        OP_SELECT,
        "--operation-id",
        "export",
        "--path-filter",
        "/exports/a",
        "--flat",
        "--detail",
        "basic",
    ]);
    assert_eq!(code, 0);
    assert_eq!(rendered_operations(&stdout), ["GET /exports/a"]);
    assert!(!stderr.contains("removed by"), "stderr: {stderr}");
}

#[test]
fn selection_omits_services_it_leaves_empty() {
    for detail in ["summary", "basic"] {
        let (code, stdout, _) = run_select(&[
            OP_SELECT,
            "--operation",
            "GET /admin/users",
            "--detail",
            detail,
            "--no-report",
        ]);
        assert_eq!(code, 0);
        assert!(stdout.contains("Admin"), "{detail}: {stdout}");
        // The Users service (as a list entry, TOC link or section) is gone.
        assert!(!stdout.contains("- Users"), "{detail}: {stdout}");
        assert!(!stdout.contains("[Users]"), "{detail}: {stdout}");
        assert!(!stdout.contains("## Users"), "{detail}: {stdout}");
        assert!(!stdout.contains("No endpoints found"), "{detail}: {stdout}");
    }
    // Without a selector, empty services are still listed as before.
    let (_, stdout, _) = run_select(&[
        OP_SELECT,
        "--path-filter",
        "/admin",
        "--detail",
        "basic",
        "--no-report",
    ]);
    assert!(stdout.contains("No endpoints found for this service."));
}

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
