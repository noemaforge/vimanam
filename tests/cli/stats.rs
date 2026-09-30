use crate::common::HYGIENE;
use crate::common::MULTI_TAG;
use crate::common::OAS3;
use crate::common::vimanam;
use predicates::prelude::*;

// --- --stats token-budget dry-run (#42) ---

// --stats alignment with a service name wider than the SERVICE header (#42).
const STATS_LONG_SERVICE: &str = "tests/fixtures/stats_long_service_oas3.json";

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
