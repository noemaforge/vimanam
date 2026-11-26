use crate::common::DIFF_NEW;
use crate::common::DIFF_OLD;
use crate::common::OAS2;
use crate::common::OAS3;
use crate::common::OAS3_SCHEMA_REFS;
use crate::common::diff_run;
use crate::common::load_json;
use crate::common::vimanam;
use crate::common::write_spec;
use predicates::prelude::*;
use serde_json::json;

// --- diff subcommand (#45) ---

const DIFF_SUMMARY: &str =
    "**Summary:** 1 endpoint added, 1 removed, 4 changed; 4 breaking, 4 non-breaking, 1 to review";

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

    // The shared `Widget` change is reported once (#181), not once per
    // response that reaches it; the uses are listed under the tables.
    let non_breaking = "\
## Non-breaking changes (4)

| Change | Endpoint | Detail |
|--------|----------|--------|
| Marked deprecated | `DELETE /widgets/{id}` | - |
| Endpoint added | `GET /widgets/{id}/history` | - |
| Component schema changed | component `Widget` | response `/properties/pricing` added (3 operations) |
| Component schema changed | component `Widget` | response `/properties/pricing` added to `required` (3 operations) |
";
    assert!(stdout.contains(non_breaking), "{stdout}");

    // Without --report there is no Deltas section.
    assert!(!stdout.contains("## Deltas"), "{stdout}");
}

// nrynss's requirement on #45: a change behind a shared `$ref` must surface on
// every endpoint whose operation object did not change at all. Since #181 it
// is reported once, at the component, with every such endpoint listed.
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
            "| Component schema changed | component `Widget` | response `/properties/pricing` added (3 operations) |"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "| Component schema changed | component `Widget` | response `/properties/pricing` added to `required` (3 operations) |"
        ),
        "{stdout}"
    );
    let affected = "\
## Affected operations

### `Widget`

- `GET /widgets` response 200
- `POST /widgets` response 201
- `GET /widgets/{id}` response 200
";
    assert!(stdout.contains(affected), "{stdout}");
    assert_eq!(
        stdout.matches("`/properties/pricing` added").count(),
        2,
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
| Component schema changed | component `WidgetInput` | request `/properties/weight/format` changed `float` → `double` (1 operation) |
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
    // The body parameter and the 200 response both reference #/definitions/Pet:
    // the change is reported once per usage direction (#181).
    assert!(
        stdout.contains(
            "| Component schema changed | component `Pet` | request `/properties/name` added (1 operation) |"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "| Component schema changed | component `Pet` | response `/properties/name` added (1 operation) |"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("### `Pet`\n\n- `POST /pets` request body\n- `POST /pets` response 200\n"),
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

// --- OpenAPI 3.1 type arrays (#166) and response enum narrowing (#169) ---

const TYPE_ARRAY: &str = "tests/fixtures/type_array_nullable.json";

/// Diffs `old_edit(fixture)` against `new_edit(fixture)` with
/// `--fail-on-breaking`.
fn type_array_diff(
    old_edit: impl FnOnce(&mut serde_json::Value),
    new_edit: impl FnOnce(&mut serde_json::Value),
) -> (String, i32) {
    let dir = tempfile::tempdir().unwrap();
    let mut old = load_json(TYPE_ARRAY);
    let mut new = old.clone();
    old_edit(&mut old);
    new_edit(&mut new);
    let old_path = write_spec(&dir, "old.json", &old);
    let new_path = write_spec(&dir, "new.json", &new);
    diff_run(&[&old_path, &new_path, "--fail-on-breaking"])
}

fn set(spec: &mut serde_json::Value, pointer: &str, value: serde_json::Value) {
    *spec.pointer_mut(pointer).unwrap() = value;
}

const ITEM: &str = "/components/schemas/Item/properties";
const ITEM_INPUT: &str = "/components/schemas/ItemInput/properties";

#[test]
fn diff_type_array_null_on_request_is_non_breaking_and_reverse_breaking() {
    let pointer = &format!("{ITEM_INPUT}/note/type");
    let (stdout, code) =
        type_array_diff(|_| {}, |new| set(new, pointer, json!(["string", "null"])));
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains(
            "| Component schema changed | component `ItemInput` | request `/properties/note/nullable` added (`true`) (1 operation) |"
        ),
        "{stdout}"
    );
    assert!(!stdout.contains("## Breaking changes"), "{stdout}");

    let (stdout, code) =
        type_array_diff(|old| set(old, pointer, json!(["string", "null"])), |_| {});
    assert_eq!(code, 3, "{stdout}");
    assert!(
        stdout.contains(
            "| Component schema changed | component `ItemInput` | request `/properties/note/nullable` removed"
        ),
        "{stdout}"
    );
}

#[test]
fn diff_type_array_null_on_response_is_breaking_and_reverse_non_breaking() {
    let pointer = &format!("{ITEM}/label/type");
    // ["string", "null"] -> "string": the server stops sending null.
    let (stdout, code) = type_array_diff(|_| {}, |new| set(new, pointer, json!("string")));
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains(
            "| Component schema changed | component `Item` | response `/properties/label/nullable` removed"
        ),
        "{stdout}"
    );

    // "string" -> ["string", "null"]: clients may now receive null.
    let (stdout, code) = type_array_diff(|old| set(old, pointer, json!("string")), |_| {});
    assert_eq!(code, 3, "{stdout}");
    assert!(stdout.contains("## Breaking changes (1)"), "{stdout}");
    assert!(
        stdout.contains(
            "| Component schema changed | component `Item` | response `/properties/label/nullable` added (`true`) (1 operation) |"
        ),
        "{stdout}"
    );
}

#[test]
fn diff_type_array_equivalent_spellings_are_unchanged() {
    // Reordered union members, a duplicate member, and the OAS 3.0 spelling of
    // nullability all describe the same schema.
    let (stdout, code) = type_array_diff(
        |_| {},
        |new| {
            set(
                new,
                &format!("{ITEM}/key/type"),
                json!(["integer", "string"]),
            );
            set(
                new,
                &format!("{ITEM}/count/type"),
                json!(["null", "integer"]),
            );
            set(
                new,
                &format!("{ITEM}/label"),
                json!({"type": "string", "nullable": true}),
            );
        },
    );
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("**Summary:** No changes."), "{stdout}");
}

#[test]
fn diff_response_enum_narrowing_is_not_breaking() {
    let (stdout, code) = type_array_diff(
        |_| {},
        |new| set(new, &format!("{ITEM}/state/enum"), json!(["active"])),
    );
    assert_eq!(code, 0, "{stdout}");
    assert!(!stdout.contains("## Breaking changes"), "{stdout}");
    assert!(
        stdout.contains(
            "| Component schema changed | component `Item` | response `/properties/state/enum"
        ),
        "{stdout}"
    );

    // The same narrowing on the request side still breaks callers.
    let (stdout, code) = type_array_diff(
        |_| {},
        |new| set(new, &format!("{ITEM_INPUT}/status/enum"), json!(["active"])),
    );
    assert_eq!(code, 3, "{stdout}");
    assert!(stdout.contains("## Breaking changes (1)"), "{stdout}");
}

// #175 review: a missing `enum` allows every value, so dropping the keyword is
// not a series of member removals.
#[test]
fn diff_whole_enum_keyword_follows_direction() {
    let drop_enum = |pointer: String| {
        move |spec: &mut serde_json::Value| {
            spec.pointer_mut(&pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove("enum");
        }
    };

    // Response loses its enum: the server may now return anything -> review.
    let (stdout, code) = type_array_diff(|_| {}, drop_enum(format!("{ITEM}/state")));
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("**Summary:** 0 endpoints added, 0 removed, 1 changed; 0 breaking, 0 non-breaking, 1 to review"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "| Component schema changed | component `Item` | response `/properties/state/enum` removed (1 operation) |"
        ),
        "{stdout}"
    );

    // Response gains an enum: narrower output -> non-breaking.
    let (stdout, code) = type_array_diff(drop_enum(format!("{ITEM}/state")), |_| {});
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("0 breaking, 1 non-breaking, 0 to review"),
        "{stdout}"
    );

    // Request loses its enum: accepts more input -> non-breaking.
    let (stdout, code) = type_array_diff(|_| {}, drop_enum(format!("{ITEM_INPUT}/status")));
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("0 breaking, 1 non-breaking, 0 to review"),
        "{stdout}"
    );

    // Request gains an enum: previously valid input now fails -> breaking.
    let (stdout, code) = type_array_diff(drop_enum(format!("{ITEM_INPUT}/status")), |_| {});
    assert_eq!(code, 3, "{stdout}");
    assert!(
        stdout.contains("1 breaking, 0 non-breaking, 0 to review"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "| Component schema changed | component `ItemInput` | request `/properties/status/enum` added (1 operation) |"
        ),
        "{stdout}"
    );
}

// `not` reverses every direction, so changes beneath it are left to review.
#[test]
fn diff_changes_under_not_are_review() {
    let pointer = format!("{ITEM_INPUT}/note");
    let excluded = json!({"type": "string", "not": {"type": "string", "enum": ["a"]}});
    // Dropping the excluded enum makes `not` match every string: no longer a
    // safe non-breaking widening, so it must not pass silently either way.
    let (stdout, code) = type_array_diff(
        |old| set(old, &pointer, excluded.clone()),
        |new| {
            set(
                new,
                &pointer,
                json!({"type": "string", "not": {"type": "string"}}),
            )
        },
    );
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("0 breaking, 0 non-breaking, 1 to review"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "| Component schema changed | component `ItemInput` | request `/properties/note/not/enum` removed (1 operation) |"
        ),
        "{stdout}"
    );
}

/// A spec whose components all reference each other: fully inlining any one
/// of them visits every simple path through the graph (exponential), which
/// made `diff` hang on Stripe's spec (#152).
fn dense_ref_graph(components: usize, leaf_type: &str) -> serde_json::Value {
    let names: Vec<String> = (0..components).map(|i| format!("N{i}")).collect();
    let mut schemas = serde_json::Map::new();
    for name in &names {
        let properties: serde_json::Map<String, serde_json::Value> = names
            .iter()
            .filter(|other| *other != name)
            .map(|other| {
                (
                    other.to_lowercase(),
                    json!({"$ref": format!("#/components/schemas/{other}")}),
                )
            })
            .chain([("id".to_string(), json!({"type": "string"}))])
            .collect();
        schemas.insert(
            name.clone(),
            json!({"type": "object", "properties": properties}),
        );
    }
    schemas.insert("Leaf".into(), json!({"type": leaf_type}));
    let paths: serde_json::Map<String, serde_json::Value> = names
        .iter()
        .map(|name| {
            let response = json!({"$ref": format!("#/components/schemas/{name}")});
            (
                format!("/{}", name.to_lowercase()),
                json!({"get": {
                    "operationId": format!("get{name}"),
                    "parameters": [{"name": "limit", "in": "query", "schema": {"type": "integer"}}],
                    "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": response}}}}
                }}),
            )
        })
        .chain([(
            "/leaf".to_string(),
            json!({"get": {"responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Leaf"}}}}}}}),
        )])
        .collect();
    json!({
        "openapi": "3.0.3",
        "info": {"title": "Dense", "version": "1"},
        "paths": paths,
        "components": {"schemas": schemas}
    })
}

#[test]
fn diff_skips_resolving_provably_unchanged_cross_linked_schemas() {
    let dir = tempfile::tempdir().unwrap();
    let old = write_spec(&dir, "old.json", &dense_ref_graph(12, "string"));
    let new = write_spec(&dir, "new.json", &dense_ref_graph(12, "integer"));
    let run = |format: &str| {
        let output = vimanam()
            .timeout(std::time::Duration::from_secs(60))
            .args(["diff", &old, &new, "--format", format])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    };
    // Only the unlinked Leaf changed; none of the dense graph is resolved.
    let markdown = run("markdown");
    assert!(markdown.contains("`GET /leaf`"), "{markdown}");
    assert!(!markdown.contains("`GET /n"), "{markdown}");
    let json: serde_json::Value = serde_json::from_str(&run("json")).unwrap();
    let changes = json["changes"].as_array().unwrap();
    assert!(!changes.is_empty(), "{json}");
    for change in changes {
        // Operation facts are all on /leaf; the named-type fact has no path.
        let path = &change["native"]["path"];
        assert!(path.is_null() || path == "/leaf", "{change}");
    }
}

// --- media types (#154) ---

/// A one-operation spec: `POST /a` with `request` as its request-body
/// `content` and `response` as its 200 `content` (each omitted when `null`),
/// plus a `W` component of the given `id` type.
fn media_spec(
    request: serde_json::Value,
    response: serde_json::Value,
    id: &str,
) -> serde_json::Value {
    let mut op = json!({"operationId": "a", "responses": {"200": {"description": "ok"}}});
    if !request.is_null() {
        op["requestBody"] = json!({"content": request});
    }
    if !response.is_null() {
        op["responses"]["200"]["content"] = response;
    }
    json!({
        "openapi": "3.0.3",
        "info": {"title": "t", "version": "1"},
        "paths": {"/a": {"post": op}},
        "components": {"schemas": {"W": {"type": "object", "properties": {"id": {"type": id}}}}}
    })
}

fn media(schema: serde_json::Value) -> serde_json::Value {
    json!({"schema": schema})
}

fn w() -> serde_json::Value {
    json!({"$ref": "#/components/schemas/W"})
}

fn obj(id: &str) -> serde_json::Value {
    json!({"type": "object", "properties": {"id": {"type": id}}})
}

/// Runs `diff --fail-on-breaking` (plus `extra`) on two specs and returns
/// `(stdout, exit code)`.
fn media_diff(old: &serde_json::Value, new: &serde_json::Value, extra: &[&str]) -> (String, i32) {
    let dir = tempfile::tempdir().unwrap();
    let old = write_spec(&dir, "old.json", old);
    let new = write_spec(&dir, "new.json", new);
    let mut args = vec![old.as_str(), new.as_str(), "--fail-on-breaking"];
    args.extend_from_slice(extra);
    diff_run(&args)
}

#[test]
fn diff_reports_a_removed_request_media_type_as_breaking() {
    // Before #154 only the first media type was compared, so dropping the
    // second one passed `--fail-on-breaking`.
    let old = media_spec(
        json!({"application/json": media(w()), "application/xml": media(w())}),
        json!(null),
        "string",
    );
    let new = media_spec(
        json!({"application/json": media(w())}),
        json!(null),
        "string",
    );
    let (stdout, code) = media_diff(&old, &new, &[]);
    assert_eq!(code, 3, "{stdout}");
    assert!(
        stdout.contains("| Request media type removed | `POST /a` | `application/xml` |"),
        "{stdout}"
    );
    let (stdout, code) = media_diff(&new, &old, &[]);
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("## Non-breaking changes (1)")
            && stdout.contains("| Request media type added | `POST /a` | `application/xml` |"),
        "{stdout}"
    );
}

#[test]
fn diff_compares_the_schema_of_every_response_media_type() {
    let old = media_spec(
        json!(null),
        json!({"application/json": media(obj("string")), "application/xml": media(obj("string"))}),
        "string",
    );
    let new = media_spec(
        json!(null),
        json!({"application/json": media(obj("string")), "application/xml": media(obj("integer"))}),
        "string",
    );
    let (stdout, code) = media_diff(&old, &new, &[]);
    assert_eq!(code, 3, "{stdout}");
    assert!(
        stdout.contains(
            "| Response schema changed | `POST /a` | 200 `application/xml` `/properties/id/type` changed `string` → `integer` |"
        ),
        "{stdout}"
    );
    // A response media type dropped breaks the clients that negotiate it.
    let fewer = media_spec(
        json!(null),
        json!({"application/json": media(obj("string"))}),
        "string",
    );
    let (stdout, code) = media_diff(&old, &fewer, &[]);
    assert_eq!(code, 3, "{stdout}");
    assert!(
        stdout.contains("| Response media type removed | `POST /a` | 200 `application/xml` |"),
        "{stdout}"
    );
}

#[test]
fn diff_lists_each_media_type_use_of_a_shared_component() {
    let content = json!({"application/json": media(w()), "application/xml": media(w())});
    let old = media_spec(json!(null), content.clone(), "string");
    let new = media_spec(json!(null), content, "integer");
    let (stdout, code) = media_diff(&old, &new, &[]);
    assert_eq!(code, 3, "{stdout}");
    // One record for the direction, both media types listed as uses.
    assert!(
        stdout.contains(
            "| Component schema changed | component `W` | response `/properties/id/type` changed `string` → `integer` (1 operation) |"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "- `POST /a` response 200 `application/json`\n- `POST /a` response 200 `application/xml`\n"
        ),
        "{stdout}"
    );
    // v1 JSON: one record per media-type use, with distinct IDs.
    let (stdout, _) = media_diff(
        &media_spec(
            json!(null),
            json!({"application/json": media(w()), "application/xml": media(w())}),
            "string",
        ),
        &media_spec(
            json!(null),
            json!({"application/json": media(w()), "application/xml": media(w())}),
            "integer",
        ),
        &["--format", "json", "--contract", "1"],
    );
    let doc: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let records = doc["changes"].as_array().unwrap();
    assert_eq!(records.len(), 2, "{stdout}");
    assert_eq!(records[0]["details"]["media_type"], "application/json");
    assert_eq!(records[1]["details"]["media_type"], "application/xml");
    assert_eq!(records[0]["details"]["component"], "W");
    assert_ne!(records[0]["id"], records[1]["id"]);
}

#[test]
fn diff_matches_media_types_by_exact_key() {
    let old = media_spec(
        json!({"application/json": media(obj("string"))}),
        json!(null),
        "string",
    );
    let new = media_spec(
        json!({"application/json; charset=utf-8": media(obj("string"))}),
        json!(null),
        "string",
    );
    let (stdout, code) = media_diff(&old, &new, &[]);
    assert_eq!(code, 3, "{stdout}");
    assert!(
        stdout.contains("| Request media type removed | `POST /a` | `application/json` |")
            && stdout.contains(
                "| Request media type added | `POST /a` | `application/json; charset=utf-8` |"
            ),
        "{stdout}"
    );
    // The same media types in another order are the same media types.
    let a = media_spec(
        json!(null),
        json!({"application/json": media(obj("string")), "*/*": media(obj("string"))}),
        "string",
    );
    let b = media_spec(
        json!(null),
        json!({"*/*": media(obj("string")), "application/json": media(obj("string"))}),
        "string",
    );
    let (stdout, code) = media_diff(&a, &b, &[]);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("**Summary:** No changes."), "{stdout}");
}

#[test]
fn diff_keeps_single_media_type_records_without_a_media_type() {
    // One media type on each side, the same one: compared as before #154,
    // so the v1 record has no `media_type` and keeps its ID.
    let old = media_spec(
        json!({"application/json": media(obj("string"))}),
        json!(null),
        "string",
    );
    let new = media_spec(
        json!({"application/json": media(obj("integer"))}),
        json!(null),
        "string",
    );
    let (stdout, _) = media_diff(&old, &new, &["--format", "json", "--contract", "1"]);
    let doc: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let record = &doc["changes"][0];
    assert_eq!(record["kind"], "request_schema_changed");
    assert!(record["details"].get("media_type").is_none(), "{record}");
    // A second media type makes the body compared per media type: the
    // first one's records name it from then on.
    let two = media_spec(
        json!({"application/json": media(obj("integer")), "application/xml": media(obj("string"))}),
        json!(null),
        "string",
    );
    let (stdout, code) = media_diff(&old, &two, &["--format", "json", "--contract", "1"]);
    assert_eq!(code, 3, "{stdout}");
    let doc: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let kinds: Vec<(&str, &str)> = doc["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["kind"].as_str().unwrap(),
                c["details"]["media_type"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ("request_schema_changed", "application/json"),
            ("request_media_type_added", "application/xml"),
        ],
        "{stdout}"
    );
}

#[test]
fn diff_compares_swagger2_bodies_with_the_first_openapi3_media_type() {
    // Swagger 2 names no media type per body or response (`consumes` and
    // `produces` are not compared), so a 2.0 -> 3.x migration compares
    // schemas as before #154, without media-type records.
    let old = json!({
        "swagger": "2.0",
        "info": {"title": "t", "version": "1"},
        "paths": {"/a": {"post": {
            "parameters": [{"name": "body", "in": "body", "schema": obj("string")}],
            "responses": {"200": {"description": "ok", "schema": obj("string")}}
        }}}
    });
    let new = media_spec(
        json!({"application/json": media(obj("integer")), "application/xml": media(obj("string"))}),
        json!({"application/json": media(obj("string")), "application/xml": media(obj("string"))}),
        "string",
    );
    let (stdout, code) = media_diff(&old, &new, &[]);
    assert_eq!(code, 3, "{stdout}");
    assert!(!stdout.contains("media type"), "{stdout}");
    assert!(
        stdout.contains("| Request schema changed | `POST /a` | `/properties/id/type` changed"),
        "{stdout}"
    );
}

#[test]
fn diff_treats_a_respelled_media_type_as_the_same_one() {
    // RFC 9110 §8.3.1: type, subtype and parameter names are
    // case-insensitive, so `Application/JSON` is a respelling, not a removal
    // plus an addition (#154 review).
    for (old, new) in [
        (
            json!({"application/json": media(obj("string"))}),
            json!(null),
        ),
        (
            json!(null),
            json!({"application/json": media(obj("string"))}),
        ),
    ] {
        let respelled = |v: &serde_json::Value| {
            if v.is_null() {
                v.clone()
            } else {
                json!({"Application/JSON": media(obj("string"))})
            }
        };
        let a = media_spec(old.clone(), new.clone(), "string");
        let b = media_spec(respelled(&old), respelled(&new), "string");
        for (x, y) in [(&a, &b), (&b, &a)] {
            let (stdout, code) = media_diff(x, y, &[]);
            assert_eq!(code, 0, "{stdout}");
            assert!(stdout.contains("**Summary:** No changes."), "{stdout}");
        }
    }
    // Among several media types, a respelled one is matched and named as the
    // old side writes it; whitespace around `;` and parameter-name case are
    // spelling too, a parameter value is not.
    let old = media_spec(
        json!(null),
        json!({
            "Application/JSON": media(obj("string")),
            "text/plain;charset=utf-8": media(obj("string")),
            "a/b; q=1": media(obj("string"))
        }),
        "string",
    );
    let new = media_spec(
        json!(null),
        json!({
            "application/json": media(obj("integer")),
            "TEXT/Plain ; Charset=utf-8": media(obj("string")),
            "a/b; q=2": media(obj("string"))
        }),
        "string",
    );
    let (stdout, code) = media_diff(&old, &new, &["--format", "json", "--contract", "1"]);
    assert_eq!(code, 3, "{stdout}");
    let doc: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let records: Vec<(&str, &str)> = doc["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["kind"].as_str().unwrap(),
                c["details"]["media_type"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        records,
        [
            ("response_schema_changed", "Application/JSON"),
            ("response_media_type_removed", "a/b; q=1"),
            ("response_media_type_added", "a/b; q=2"),
        ],
        "{stdout}"
    );
}
