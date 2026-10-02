use crate::common::vimanam;

// --- --costs token-cost analysis (#47) ---

const COSTS: &str = "tests/fixtures/costs_oas3.json";

fn costs_output(args: &[&str]) -> String {
    let output = vimanam().args(args).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "costs wrote to stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// The lines of the ENDPOINTS table: (tokens, share, operation).
fn endpoint_rows(report: &str) -> Vec<(usize, String, String)> {
    let mut lines = report.lines();
    // Skip to the header row of the endpoint table.
    let header = lines
        .find(|line| line.starts_with("~TOKENS") && line.contains("OPERATION"))
        .expect("endpoint table header");
    assert!(header.contains("SHARE"), "{header}");
    assert!(header.contains("OPERATION"), "{header}");
    lines
        .take_while(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.split_whitespace();
            let tokens: usize = fields.next().unwrap().parse().unwrap();
            let share = fields.next().unwrap().to_string();
            let operation = fields.collect::<Vec<_>>().join(" ");
            (tokens, share, operation)
        })
        .collect()
}

/// The lines of the SCHEMAS table: (name, uses, notes).
fn schema_rows(report: &str) -> Vec<(String, usize, String)> {
    let mut lines = report.lines();
    let header = lines
        .find(|line| line.starts_with("SCHEMA "))
        .expect("schema table header");
    assert!(header.contains("DEF ~TOKENS"), "{header}");
    assert!(header.contains("AMPLIF."), "{header}");
    lines
        .take_while(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next().unwrap().to_string();
            let uses: usize = fields.next().unwrap().parse().unwrap();
            let _definition: usize = fields.next().unwrap().parse().unwrap();
            let _per_use: usize = fields.next().unwrap().parse().unwrap();
            let _inline_total: usize = fields.next().unwrap().parse().unwrap();
            let _amplification = fields.next().unwrap().to_string();
            let notes = fields.collect::<Vec<_>>().join(" ");
            (name, uses, notes)
        })
        .collect()
}

// The shared User schema is linked from five rows: the POST /users request and
// response (the operation is multi-tag, so it renders under both of its
// services) and the GET /users/{id} response. The Node schema references
// itself, so its inline model is flagged.
#[test]
fn costs_report_lists_endpoints_schemas_and_cycle_notes() {
    let report = costs_output(&[COSTS, "--costs", "--detail", "full", "--include-schemas"]);

    assert!(report.starts_with("TOKEN COST ANALYSIS\n"), "{report}");
    assert!(report.contains("chars/4 estimates"), "{report}");
    assert!(report.contains("TOTAL: ~"), "{report}");
    // The inline total is labeled a separate analysis, not the linked output.
    assert!(report.contains("not the cost of the output in linked mode"));

    let endpoints = endpoint_rows(&report);
    let operations: Vec<&str> = endpoints
        .iter()
        .map(|(_, _, operation)| operation.as_str())
        .collect();
    assert_eq!(
        operations,
        vec![
            "POST /chains",
            "GET /items",
            "POST /users (+items)",
            "GET /users/{id}"
        ],
        "{report}"
    );

    let schemas = schema_rows(&report);
    let uses = |name: &str| {
        schemas
            .iter()
            .find(|(schema, _, _)| schema == name)
            .unwrap_or_else(|| panic!("no {name} row in {report}"))
            .1
    };
    assert_eq!(uses("User"), 5, "{report}");
    // Profile is linked only from User's single Schema Definitions entry, so
    // one row — the use sites of User do not multiply it.
    assert_eq!(uses("Profile"), 1, "{report}");
    assert_eq!(uses("Item"), 1, "{report}");
    // 2 body rows plus the self-link row inside Node's own definition.
    assert_eq!(uses("Node"), 3, "{report}");

    let node_notes = &schemas
        .iter()
        .find(|(schema, _, _)| schema == "Node")
        .unwrap()
        .2;
    assert_eq!(node_notes, "cycle", "{report}");
    // Only Node is cyclic.
    assert!(
        schemas
            .iter()
            .all(|(schema, _, notes)| schema == "Node" || notes.is_empty()),
        "{report}"
    );

    assert!(report.contains("HOTSPOTS"), "{report}");
}

// The POST /users operation is tagged `users` and `items`; the report measures
// it once, not once per tag, and a ` (+tag)` suffix names the extra tags.
#[test]
fn costs_measures_multi_tag_endpoints_once() {
    let report = costs_output(&[COSTS, "--costs", "--detail", "full", "--include-schemas"]);
    let post_users: Vec<String> = endpoint_rows(&report)
        .into_iter()
        .map(|(_, _, operation)| operation)
        .filter(|operation| operation.starts_with("POST /users"))
        .collect();
    assert_eq!(post_users, vec!["POST /users (+items)"], "{report}");
}

// Repeated runs produce byte-identical reports.
#[test]
fn costs_output_is_deterministic() {
    let first = costs_output(&[COSTS, "--costs", "--detail", "full", "--include-schemas"]);
    let second = costs_output(&[COSTS, "--costs", "--detail", "full", "--include-schemas"]);
    assert_eq!(first, second);
}

// Filters narrow the analyzed set exactly as they narrow the document.
#[test]
fn costs_honors_filters() {
    let report = costs_output(&[
        COSTS,
        "--costs",
        "--detail",
        "full",
        "--include-schemas",
        "--method-filter",
        "get",
    ]);
    let operations: Vec<String> = endpoint_rows(&report)
        .into_iter()
        .map(|(_, _, operation)| operation)
        .collect();
    assert_eq!(
        operations,
        vec!["GET /items", "GET /users/{id}"],
        "{report}"
    );

    let report = costs_output(&[
        COSTS,
        "--costs",
        "--detail",
        "full",
        "--include-schemas",
        "--service-filter",
        "users",
    ]);
    let operations: Vec<String> = endpoint_rows(&report)
        .into_iter()
        .map(|(_, _, operation)| operation)
        .collect();
    assert_eq!(
        operations,
        vec!["POST /users (+items)", "GET /users/{id}"],
        "{report}"
    );
}

// The inline schema mode is honored and labeled in the mode line.
#[test]
fn costs_labels_inline_schema_mode() {
    let report = costs_output(&[
        COSTS,
        "--costs",
        "--detail",
        "full",
        "--include-schemas",
        "--inline-schemas",
    ]);
    assert!(
        report.contains("schemas inline (every use site expands; no definitions section)"),
        "{report}"
    );
    // Inline use sites are counted exactly like linked ones, including the
    // multi-tag duplicate render of POST /users.
    let schemas = schema_rows(&report);
    assert_eq!(
        schemas
            .iter()
            .find(|(schema, _, _)| schema == "User")
            .unwrap()
            .1,
        5,
        "{report}"
    );
}

// At the default summary detail no schema tables render, so the schema section
// is empty and the report says so instead of showing zero rows.
#[test]
fn costs_at_summary_detail_has_no_schema_rows() {
    let report = costs_output(&[COSTS, "--costs"]);
    assert!(
        report.contains("(no component schemas rendered"),
        "{report}"
    );
    assert!(schema_rows_is_empty(&report), "{report}");
}

fn schema_rows_is_empty(report: &str) -> bool {
    !report.lines().any(|line| line.starts_with("SCHEMA "))
}

// TOTAL is the chars/4 estimate of the real conversion body, so the report
// agrees with what a full render (without the hygiene report) measures.
#[test]
fn costs_total_matches_an_actual_render() {
    let report = costs_output(&[COSTS, "--costs", "--detail", "full", "--include-schemas"]);
    let total: usize = report
        .lines()
        .find_map(|line| {
            line.strip_prefix("Mode:")
                .and_then(|_| line.split("TOTAL: ~").nth(1))
                .and_then(|rest| rest.split(" tokens").next())
                .and_then(|number| number.parse().ok())
        })
        .expect("TOTAL in {report}");

    let output = vimanam()
        .args([
            COSTS,
            "--detail",
            "full",
            "--include-schemas",
            "--no-report",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body = String::from_utf8(output.stdout).unwrap();
    assert_eq!(total, body.chars().count().div_ceil(4));
}

// Usage conflicts mirror --stats: no output, no budget, no tree, no schema
// selection.
#[test]
fn costs_conflicts_with_other_output_modes() {
    for conflicting in [
        vec!["--costs", "-o", "costs.md"],
        vec!["--costs", "--max-tokens", "100"],
        vec!["--costs", "--stats"],
        vec!["--costs", "--schema", "User"],
        vec!["--costs", "--schema-field", "User#/properties/id"],
    ] {
        let mut args = vec![COSTS];
        args.extend_from_slice(&conflicting);
        let output = vimanam().args(&args).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(2),
            "expected usage error for {conflicting:?}: {output:?}"
        );
    }
}

// An all-filters-exclude-everything spec yields an empty, still-deterministic
// report rather than an error.
#[test]
fn costs_with_nothing_visible_is_headers_and_zero_total() {
    let report = costs_output(&[
        COSTS,
        "--costs",
        "--detail",
        "full",
        "--include-schemas",
        "--exclude-deprecated",
        "--path-filter",
        "/nonexistent",
    ]);
    assert!(
        report.contains("(no endpoints visible after filters)"),
        "{report}"
    );
    assert!(
        report.contains("(no component schemas rendered"),
        "{report}"
    );
    assert!(report.contains("TOTAL: ~0 tokens"), "{report}");
}

// `--schema-depth` bounds the analyzed render like any other: the report keeps
// exiting 0 and stays deterministic at both extremes. At depth 2 the schema
// table is present; at depth 0 every reference edge is cut, no use sites are
// recorded, and the schema section says so instead of showing zero rows.
#[test]
fn costs_locks_schema_depth() {
    for depth in ["0", "2"] {
        let args = [
            COSTS,
            "--costs",
            "--detail",
            "full",
            "--include-schemas",
            "--schema-depth",
            depth,
        ];
        let first = costs_output(&args);
        let second = costs_output(&args);
        assert_eq!(first, second, "--schema-depth {depth}");
    }

    let deep = costs_output(&[
        COSTS,
        "--costs",
        "--detail",
        "full",
        "--include-schemas",
        "--schema-depth",
        "2",
    ]);
    assert!(
        !schema_rows_is_empty(&deep),
        "no schema table at depth 2: {deep}"
    );

    let shallow = costs_output(&[
        COSTS,
        "--costs",
        "--detail",
        "full",
        "--include-schemas",
        "--schema-depth",
        "0",
    ]);
    assert!(
        shallow.contains("(no component schemas rendered"),
        "{shallow}"
    );
    assert!(schema_rows_is_empty(&shallow), "{shallow}");
}

const MUTUAL: &str = "tests/fixtures/costs_mutual_oas3.json";

// Mutual recursion A -> B -> A pins the definition-internal row accounting:
// A is linked twice from POST /a's body and once from inside B's definition
// entry; B's only use is the reference row inside A's entry; C is reached once
// from B's entry and once from GET /c's response. A and B participate in the
// reference cycle and carry the cycle note; C does not.
#[test]
fn costs_counts_definition_internal_reference_rows() {
    let report = costs_output(&[MUTUAL, "--costs", "--detail", "full", "--include-schemas"]);
    let schemas = schema_rows(&report);
    let row = |name: &str| {
        schemas
            .iter()
            .find(|(schema, _, _)| schema == name)
            .unwrap_or_else(|| panic!("no {name} row in {report}"))
    };
    assert_eq!(row("A").1, 3, "{report}");
    assert_eq!(row("B").1, 1, "{report}");
    assert_eq!(row("C").1, 2, "{report}");
    assert_eq!(row("A").2, "cycle", "{report}");
    assert_eq!(row("B").2, "cycle", "{report}");
    assert_eq!(row("C").2, "", "{report}");
}

// `--operation` narrows the analysis to exactly the selected endpoint.
#[test]
fn costs_with_operation_selector_shows_one_endpoint() {
    let report = costs_output(&[
        COSTS,
        "--costs",
        "--detail",
        "full",
        "--include-schemas",
        "--operation",
        "GET /users/{id}",
    ]);
    let operations: Vec<String> = endpoint_rows(&report)
        .into_iter()
        .map(|(_, _, operation)| operation)
        .collect();
    assert_eq!(operations, vec!["GET /users/{id}"], "{report}");
}

// A selected operation that another filter removes is warned about on stderr
// while the report itself still succeeds.
#[test]
fn costs_warns_when_a_filter_removes_the_selected_operation() {
    let output = vimanam()
        .args([
            COSTS,
            "--costs",
            "--detail",
            "full",
            "--include-schemas",
            "--operation",
            "POST /users",
            "--method-filter",
            "get",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--operation \"POST /users\""), "{stderr}");
    assert!(stderr.contains("removed by --method-filter"), "{stderr}");
    assert!(
        String::from_utf8_lossy(&output.stdout).starts_with("TOKEN COST ANALYSIS\n"),
        "stdout missing the report: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}
