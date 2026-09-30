use crate::common::DIFF_NEW;
use crate::common::DIFF_OLD;
use crate::common::vimanam;

// ---------------------------------------------------------------------------
// Exact operation selection: --operation / --operation-id (#99)
// ---------------------------------------------------------------------------

const OP_SELECT: &str = "tests/fixtures/operation_select_oas3.json";
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
    assert_diff_round_trips(DIFF_OLD, DIFF_NEW);
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
