//! Found by the Lean `$ref` reference model (`formal/`): a one-sided expansion
//! of a recursive component cut by component name (#187), and a `definitions`
//! entry that does not deserialise dropped from the comparison (#188). The
//! inputs are the pinned cases in `formal/refdiff/cases/r8-*`, whose model
//! output is pinned in `formal/refdiff/expected/`.
use crate::common::vimanam;
use serde_json::Value;

const CASES: &str = "formal/refdiff/cases";

/// `vimanam diff --fail-on-breaking --format json --contract 1` on a pinned
/// case: the records as sorted `(pointer, severity)` and the exit code.
fn case(name: &str) -> (Vec<(String, String)>, i32) {
    let output = vimanam()
        .arg("diff")
        .arg(format!("{CASES}/{name}.old.json"))
        .arg(format!("{CASES}/{name}.new.json"))
        .args(["--fail-on-breaking", "--format", "json", "--contract", "1"])
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&output.stdout).unwrap();
    let mut records: Vec<(String, String)> = doc["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| {
            (
                change["details"]["schema_change"]["pointer"]
                    .as_str()
                    .unwrap()
                    .to_string(),
                change["severity"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    records.sort();
    (records, output.status.code().unwrap())
}

fn records(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = expected
        .iter()
        .map(|(pointer, severity)| (pointer.to_string(), severity.to_string()))
        .collect();
    out.sort();
    out
}

/// Asserts a case in every direction and position: `req`/`resp`, each also
/// reversed (`-rev`).
fn assert_case(
    stem: &str,
    expect: impl Fn(bool, bool) -> (Vec<(&'static str, &'static str)>, i32),
) {
    for request in [true, false] {
        for reversed in [false, true] {
            let name = format!(
                "{stem}-{}{}",
                if request { "req" } else { "resp" },
                if reversed { "-rev" } else { "" }
            );
            let (expected, code) = expect(request, reversed);
            assert_eq!(case(&name), (records(&expected), code), "{name}");
        }
    }
}

// ── #187: one-sided expansion is cut on a recurring pair, not a name ──────

#[test]
fn a_recursive_component_unrolled_inline_is_no_change() {
    // `B` is recursive through `data`; one side writes it as `$ref B`, the
    // other unrolls it inline once. The name cut reported `/type` added
    // (Breaking, exit 3) and a literal `$ref` removed.
    assert_case("r8-unroll", |_, _| (vec![], 0));
}

#[test]
fn a_change_beneath_the_old_name_cut_is_reported() {
    // As above, with `data.data.x` changed in the unrolled copy: the one
    // real change, where the name cut reported the cut site instead.
    assert_case("r8-unroll-deep", |_, _| {
        (
            vec![(
                "/properties/data/properties/data/properties/x/type",
                "breaking",
            )],
            3,
        )
    });
}

#[test]
fn a_break_beneath_the_old_name_cut_fails_the_gate() {
    // `B` has no `type`, so the cut site gave only Review and Non-breaking
    // records and `--fail-on-breaking` exited 0 in one direction.
    assert_case("r8-hidden", |_, _| {
        (vec![("/properties/data/properties/x/type", "breaking")], 3)
    });
}

#[test]
fn the_issue_repro_reports_the_changes_beneath_the_alias() {
    // #187's repro: `$ref Page` (an alias for the recursive `B`) to `$ref C`,
    // whose `data.data` is a string.
    assert_case("r8-cut", |_, reversed| {
        // Properties dropped from a request or a response are Breaking;
        // added ones (the reversed pairs) are not.
        let gone = if reversed { "non_breaking" } else { "breaking" };
        (
            vec![
                ("/$ref", "review"),
                ("/properties/data/properties/data/type", "breaking"),
                ("/properties/data/properties/data/properties/data", gone),
                ("/properties/data/properties/data/properties/x", gone),
                ("/properties/data/properties/x", gone),
            ],
            3,
        )
    });
}

// ── #188: an unparseable `definitions` entry is compared as written ──────

#[test]
fn a_change_inside_an_unparseable_definition_is_reported() {
    // `Bad.required` is a string, so `Bad` is not a valid schema; its
    // property `a` changes type. Before: "No changes.", exit 0.
    assert_case("r8-bad-def", |_, _| {
        (vec![("/properties/a/type", "breaking")], 3)
    });
}

#[test]
fn a_definition_unparseable_on_one_side_is_compared_as_written() {
    // Valid on one side, `nullable: "yes"` on the other.
    assert_case("r8-bad-def-oneside", |_, _| {
        (
            vec![
                ("/properties/a/nullable", "review"),
                ("/properties/a/type", "breaking"),
            ],
            3,
        )
    });
}

#[test]
fn the_diff_report_and_stderr_name_an_unparseable_definition() {
    let output = vimanam()
        .arg("diff")
        .arg(format!("{CASES}/r8-bad-def-oneside-resp.old.json"))
        .arg(format!("{CASES}/r8-bad-def-oneside-resp.new.json"))
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let warning = "`definitions/Bad` in the new spec is not a valid schema \
                   (invalid type: string \"yes\", expected a boolean); \
                   it was compared as written";
    assert!(
        stdout.starts_with(&format!(
            "# API Diff: t 1 → 1\n\n> **Warning:** {warning}.\n\n"
        )),
        "{stdout}"
    );
    assert!(!stdout.contains("in the old spec"), "{stdout}");
    assert_eq!(stderr, format!("vimanam: warning: {warning}\n"));
}

#[test]
fn rendering_still_leaves_an_unparseable_definition_out() {
    let output = vimanam()
        .arg(format!("{CASES}/r8-bad-def-resp.old.json"))
        .args(["--detail", "full", "--include-schemas", "--no-report"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("## Schema Definitions"), "{stdout}");
}
