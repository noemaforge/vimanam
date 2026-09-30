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

// --- diff subcommand (#45) ---

const DIFF_SUMMARY: &str =
    "**Summary:** 1 endpoint added, 1 removed, 4 changed; 4 breaking, 8 non-breaking, 1 to review";

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
