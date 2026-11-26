//! Found by the Lean `$ref` reference model (`formal/`): a one-sided expansion
//! of a recursive component cut by component name (#187), and a `definitions`
//! entry that does not deserialise dropped from the comparison (#188). The
//! inputs are the pinned cases in `formal/refdiff/cases/r8-*`, whose model
//! output is pinned in `formal/refdiff/expected/`.
use crate::common::{vimanam, write_spec};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

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
    // Valid on one side, `nullable: "yes"` on the other. The `nullable`
    // change is one the table leaves to review; inside an invalid entry it
    // fails closed.
    assert_case("r8-bad-def-oneside", |_, _| {
        (
            vec![
                ("/properties/a/nullable", "breaking"),
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
    let warning = "`/definitions/Bad` in the new spec is not a valid schema \
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

#[test]
fn a_change_the_table_cannot_classify_inside_an_unparseable_definition_fails_closed() {
    // `properties` turned into an array (only on one side), and an entry
    // that is a number (5 -> 6): Review for a valid schema, Breaking here.
    assert_case("r9-bad-def-props-array", |_, _| {
        (vec![("/properties", "breaking")], 3)
    });
    assert_case("r9-bad-def-nonobj", |_, _| (vec![("", "breaking")], 3));
}

#[test]
fn the_v2_report_lists_unparseable_definitions_only_when_there_are_any() {
    let run = |name: &str| -> Value {
        let output = vimanam()
            .arg("diff")
            .arg(format!("{CASES}/{name}.old.json"))
            .arg(format!("{CASES}/{name}.new.json"))
            .args(["--format", "json"])
            .output()
            .unwrap();
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let doc = run("r8-bad-def-oneside-resp");
    assert_eq!(doc["warnings"][0]["code"], "unparsed_definition");
    assert_eq!(doc["warnings"][0]["spec"], "new");
    assert_eq!(doc["warnings"][0]["pointer"], "/definitions/Bad");
    assert_eq!(doc["warnings"].as_array().unwrap().len(), 1);
    let kinds: Vec<&str> = doc["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["native_kind"].as_str().unwrap())
        // The named-type surface lists valid schemas only, so it also sees
        // `Bad` leave it (`surface_changed`, `type_removed`).
        .filter(|kind| *kind != "surface_changed")
        .collect();
    assert_eq!(kinds, ["unparsed_definition_changed"; 2]);
    assert!(run("r8-unroll-resp").get("warnings").is_none());
    // `--omit-surfaces` keeps them.
    let output = vimanam()
        .arg("diff")
        .arg(format!("{CASES}/r8-bad-def-oneside-resp.old.json"))
        .arg(format!("{CASES}/r8-bad-def-oneside-resp.new.json"))
        .args(["--format", "json", "--omit-surfaces"])
        .output()
        .unwrap();
    let omitted: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(omitted["warnings"], doc["warnings"]);
}

#[test]
fn an_unparseable_definition_name_is_encoded_and_fenced() {
    let dir = tempfile::tempdir().unwrap();
    let spec = json!({
        "openapi": "3.0.0", "info": {"title": "t", "version": "1"},
        "paths": {"/a": {"post": {"responses": {"200": {"description": "ok",
            "content": {"application/json": {"schema": {"$ref": "#/definitions/a~1b`c"}}}}}}}},
        "definitions": {"a/b`c\nd": {"required": "x"}, "a/b`c": {"required": "x"}}
    });
    let path = write_spec(&dir, "s.json", &spec);
    let output = vimanam().args(["diff", &path, &path]).output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("> **Warning:** ``/definitions/a~1b`c`` in the old spec"),
        "{stdout}"
    );
    assert!(
        stdout.contains("``/definitions/a~1b`c\\nd`` in the new spec"),
        "{stdout}"
    );
    // Every warning stays on one line.
    assert_eq!(
        stdout.lines().filter(|line| line.starts_with("> ")).count(),
        4,
        "{stdout}"
    );
}

/// `gen_blow` from the #195 review: the old body is `$ref A0`, each `Ai` a
/// two-level object whose `p0..p{width}` lead to `A(i+1 mod old)`; the new
/// body is a one-level inline object leading to `B0`, each `Bj` the same
/// shape leading to `B(j+1 mod new)`. The two expansions branch until the
/// compared pair recurs, `old * new` levels down.
fn blow(old: usize, new: usize, width: usize) -> (Value, Value) {
    let two = |next: usize, prefix: &str| {
        let props = |inner: Value| -> Value {
            (0..width)
                .map(|i| (format!("p{i}"), inner.clone()))
                .collect()
        };
        let reference = json!({"$ref": format!("#/components/schemas/{prefix}{next}")});
        let inner = json!({"type": "object", "properties": props(reference)});
        json!({"type": "object", "properties": props(inner)})
    };
    let spec = |schemas: Value, body: Value| {
        json!({"openapi": "3.0.0", "info": {"title": "t", "version": "1"},
            "paths": {"/x": {"post": {
                "requestBody": {"content": {"application/json": {"schema": body}}},
                "responses": {"200": {"description": "ok"}}}}},
            "components": {"schemas": schemas}})
    };
    let olds: serde_json::Map<String, Value> = (0..old)
        .map(|i| (format!("A{i}"), two((i + 1) % old, "A")))
        .collect();
    let news: serde_json::Map<String, Value> = (0..new)
        .map(|j| (format!("B{j}"), two((j + 1) % new, "B")))
        .collect();
    let body: serde_json::Map<String, Value> = (0..width)
        .map(|i| (format!("p{i}"), json!({"$ref": "#/components/schemas/B0"})))
        .collect();
    (
        spec(
            Value::Object(olds),
            json!({"$ref": "#/components/schemas/A0"}),
        ),
        spec(
            Value::Object(news),
            json!({"type": "object", "properties": body}),
        ),
    )
}

/// Runs `vimanam args`, killing it and failing the test once `limit` has
/// passed (a regression must fail fast, not hang the suite): the exit code
/// and stdout.
fn bounded(args: &[&str], limit: Duration) -> (i32, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("stdout");
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("vimanam"))
        .args(args)
        .stdout(std::fs::File::create(&out).unwrap())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "`vimanam {}` did not finish within {limit:?}",
                args.join(" ")
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    (status.code().unwrap(), std::fs::read(&out).unwrap())
}

#[test]
fn facing_reference_cycles_stay_within_the_expansion_budget() {
    // Without a budget 3/4 cycles (width 2) did not finish in a minute and
    // 9/10 grows without bound; with it each is a fraction of a second in a
    // release build. Past the budget the reference is compared as written,
    // so the cut shows as a Breaking `$ref` record (fails closed), never as
    // nothing.
    for (old, new) in [(3, 4), (9, 10)] {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = blow(old, new, 2);
        let (a, b) = (
            write_spec(&dir, "old.json", &a),
            write_spec(&dir, "new.json", &b),
        );
        let (code, stdout) = bounded(
            &[
                "diff",
                &a,
                &b,
                "--fail-on-breaking",
                "--format",
                "json",
                "--contract",
                "1",
            ],
            Duration::from_secs(30),
        );
        assert_eq!(code, 3, "{old}/{new}");
        let doc: Value = serde_json::from_slice(&stdout).unwrap();
        assert!(
            doc["changes"].as_array().unwrap().iter().any(|change| {
                change["details"]["schema_change"]["pointer"]
                    .as_str()
                    .is_some_and(|pointer| pointer.ends_with("/$ref"))
            }),
            "{old}/{new}: no visible cut"
        );
    }
    // Under the budget, the same shapes unrolled differently are no change.
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = blow(2, 5, 1);
    let (a, b) = (
        write_spec(&dir, "old.json", &a),
        write_spec(&dir, "new.json", &b),
    );
    let output = vimanam()
        .args(["diff", &a, &b, "--fail-on-breaking"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn past_the_expansion_cap_the_reference_is_compared_as_written() {
    // 5/7 cycles of width 1: a no-op unroll whose pair recurs only past 64
    // one-sided expansions. Compared as written there, as the model does,
    // and failing closed: the `$ref` record is Breaking, never Review.
    let (records, code) = case("r9-cap");
    assert_eq!(code, 3);
    assert!(
        records
            .iter()
            .any(|(pointer, severity)| pointer.ends_with("/$ref") && severity == "breaking")
    );
    assert!(records.iter().all(|(_, severity)| severity != "review"));
}

#[test]
fn one_sided_expansion_of_an_unparseable_definition_fails_closed() {
    // The use writes `$ref Bad` on one side and an inline schema on the
    // other, so `Bad` (not a valid schema) is expanded in place and its
    // changes belong to the use: they fail closed there too (#195 round 2).
    for stem in [
        "r10-reqstr-add",
        "r10-propsarr",
        "r10-enum-str",
        "r10-nonobj5",
        "r10-nonobjarr",
    ] {
        for position in ["req", "resp"] {
            for reversed in ["", "-rev"] {
                let name = format!("{stem}-{position}{reversed}");
                let (records, code) = case(&name);
                assert_eq!(code, 3, "{name}: {records:?}");
                assert!(
                    records.iter().all(|(_, severity)| severity != "review"),
                    "{name}: {records:?}"
                );
            }
        }
    }
    let (records, _) = case("r10-reqstr-add-req");
    assert_eq!(records, [("/required".to_string(), "breaking".to_string())]);
}

#[test]
fn a_change_hidden_past_the_expansion_budget_fails_closed() {
    // `a` spends the budget on a no-op (4094 or 4095 one-sided expansions
    // and the one into `W`); `z` is `$ref Z` against an inline copy whose
    // `f` changed type. One under the budget the real change is seen; at
    // the budget `z` is refused and compared as written, and its `$ref`
    // record is Breaking, so the gate still fails (main and this PR's
    // round-1 head exited 0 there).
    let (records, code) = case("r9-budget-edge-4094");
    assert_eq!(code, 3);
    assert_eq!(
        records,
        [(
            "/properties/z/properties/f/type".to_string(),
            "breaking".to_string()
        )]
    );
    for name in ["r9-budget-edge-4095", "r9-budget-edge-4095-rev"] {
        let (records, code) = case(name);
        assert_eq!(code, 3, "{name}");
        assert!(
            records.contains(&("/properties/z/$ref".to_string(), "breaking".to_string())),
            "{name}: {records:?}"
        );
    }
}
