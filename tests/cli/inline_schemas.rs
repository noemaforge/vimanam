//! `--inline-schemas` on densely cyclic specs (#177): inline expansion
//! re-expands every acyclic path through the component graph at each use
//! site, so without a bound a whole-document render never finishes. Past the
//! inline row budget it is bounded at `--schema-depth 2`, with a notice;
//! renders under the budget, and every explicit `--schema-depth`, are
//! unchanged.
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::common::write_spec;

/// Mutually recursive components `C0..C{n-1}`: each has a scalar and `k`
/// references to the next `k` components. 40 × 4 renders more than a gigabyte
/// of Markdown inline on 2.2.0 without finishing; 8 × 3 stays small.
fn cyclic_spec(n: usize, k: usize) -> Value {
    tagged_cyclic_spec(n, k, 1)
}

/// [`cyclic_spec`] with its one operation tagged `t0..t{tags-1}`, so the
/// default service view renders it `tags` times.
fn tagged_cyclic_spec(n: usize, k: usize, tags: usize) -> Value {
    let mut schemas = Map::new();
    for i in 0..n {
        let mut properties = Map::new();
        properties.insert("id".into(), json!({"type": "string"}));
        for j in 1..=k {
            properties.insert(
                format!("r{j}"),
                json!({"$ref": format!("#/components/schemas/C{}", (i + j) % n)}),
            );
        }
        schemas.insert(
            format!("C{i}"),
            json!({"type": "object", "properties": properties}),
        );
    }
    json!({
        "openapi": "3.0.0",
        "info": {"title": "Cyclic", "version": "1"},
        "paths": {"/c": {"get": {
            "tags": (0..tags).map(|t| if tags == 1 { "c".to_string() } else { format!("t{t}") }).collect::<Vec<_>>(),
            "responses": {"200": {
                "description": "ok",
                "content": {"application/json": {"schema": {"$ref": "#/components/schemas/C0"}}}
            }}
        }}},
        "components": {"schemas": schemas},
    })
}

/// Runs vimanam, killing it at `limit` instead of waiting for it to exit.
/// Returns `(exit code, stdout, stderr)`.
fn bounded(args: &[&str], limit: Duration) -> (i32, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("stdout");
    let err = dir.path().join("stderr");
    let mut child = Command::new(assert_cmd::cargo::cargo_bin("vimanam"))
        .args(args)
        .stdout(std::fs::File::create(&out).unwrap())
        .stderr(std::fs::File::create(&err).unwrap())
        .stdin(Stdio::null())
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
    (
        status.code().unwrap(),
        std::fs::read_to_string(&out).unwrap(),
        std::fs::read_to_string(&err).unwrap(),
    )
}

/// Generous for an unoptimised build; 2.2.0 did not finish in minutes.
const LIMIT: Duration = Duration::from_secs(60);

const BLOCKQUOTE: &str = "> Schema expansion is bounded at --schema-depth 2: --inline-schemas would expand more than 2000000 schema rows. Follow the per-row retrieval commands, pass --schema-depth N (0..=24) to choose another bound, or drop --inline-schemas to render each component schema once.\n";
const NOTICE: &str = "vimanam: --inline-schemas would expand more than 2000000 schema rows and --schema-depth is not set; bounding schema expansion at --schema-depth 2 (pass --schema-depth 0..=24 to choose, or drop --inline-schemas to render each component schema once)";

#[test]
fn whole_document_inline_expansion_is_bounded_on_cyclic_specs() {
    let dir = tempfile::tempdir().unwrap();
    let spec = write_spec(&dir, "cyclic.json", &cyclic_spec(40, 4));
    let full = ["--detail", "full", "--include-schemas", "--no-report"];

    let mut args = vec![spec.as_str()];
    args.extend(full);
    args.push("--inline-schemas");
    let (code, bounded_out, stderr) = bounded(&args, LIMIT);
    assert_eq!(code, 0);
    assert!(stderr.starts_with(NOTICE), "stderr: {stderr}");

    // The bound is exactly an explicit `--schema-depth 2` plus the blockquote
    // under the title: nothing else about the render changes.
    args.extend(["--schema-depth", "2"]);
    let (code, explicit, explicit_err) = bounded(&args, LIMIT);
    assert_eq!(code, 0);
    assert!(!explicit.contains(BLOCKQUOTE));
    assert!(!explicit_err.contains(NOTICE));
    let title = "# Cyclic\n";
    assert!(explicit.starts_with(title));
    let rest = &explicit[title.len()..];
    let gap = if rest.starts_with('\n') { "" } else { "\n" };
    let expected = format!("{title}\n{BLOCKQUOTE}{gap}{rest}");
    assert_eq!(bounded_out, expected);
    assert!(bounded_out.contains("Omitted nested expansion at schema depth 2"));
    assert!(bounded_out.contains("--schema 'C1' --schema-depth 2 --no-report"));
}

#[test]
fn explicit_schema_depth_is_honored_as_given() {
    let dir = tempfile::tempdir().unwrap();
    let spec = write_spec(&dir, "cyclic.json", &cyclic_spec(40, 4));
    let (code, out, err) = bounded(
        &[
            &spec,
            "--detail",
            "full",
            "--include-schemas",
            "--inline-schemas",
            "--schema-depth",
            "5",
            "--no-report",
        ],
        LIMIT,
    );
    assert_eq!(code, 0);
    assert!(!out.contains("> Schema expansion is bounded"));
    assert!(!err.contains("would expand more than"));
    assert!(out.contains("Omitted nested expansion at schema depth 5"));
}

#[test]
fn budget_stats_and_costs_stay_bounded_on_cyclic_specs() {
    let dir = tempfile::tempdir().unwrap();
    let spec = write_spec(&dir, "cyclic.json", &cyclic_spec(40, 4));
    let full = [spec.as_str(), "--detail", "full", "--include-schemas"];

    let with = |extra: &[&'static str]| {
        let mut args: Vec<&str> = full.to_vec();
        args.extend(extra);
        bounded(&args, LIMIT)
    };

    // `--max-tokens` re-renders per detail level; each decides the bound.
    let (code, out, err) = with(&["--inline-schemas", "--max-tokens", "100000", "--no-report"]);
    assert_eq!(code, 0);
    assert!(out.contains(BLOCKQUOTE));
    assert!(err.contains(NOTICE));

    let (code, out, err) = with(&["--inline-schemas", "--stats"]);
    assert_eq!(code, 0);
    assert!(out.contains("TOTAL"));
    assert!(err.contains(NOTICE));

    let (code, out, err) = with(&["--inline-schemas", "--costs"]);
    assert_eq!(code, 0);
    assert!(out.contains(
        "Schema depth: automatic --schema-depth 2 (--inline-schemas would expand more than 2000000 schema rows); every row is measured at this depth."
    ));
    assert!(err.contains(NOTICE));

    // Linked mode renders each component once, but the `--costs` inline
    // model expands each schema alone: past its per-schema budget only a
    // prefix is measured, marked as a lower bound in the table and hotspots.
    let (code, out, err) = with(&["--costs"]);
    assert_eq!(code, 0);
    assert!(!err.contains(NOTICE));
    assert!(out.contains("cycle, inline-bounded"), "{out}");
    assert!(out.contains(
        "inline-bounded: one inline expansion would render more than 10000 schema rows, so INLINE/USE measures only its first 10000 rows; it, INLINE TOTAL and AMPLIF. are lower bounds."
    ));
    assert!(out.contains("tokens inline vs ~"), "{out}");
    assert!(out.contains(", at least ~"), "{out}");
    assert!(out.contains("read once (cycle, inline-bounded)"), "{out}");
}

/// The service view renders a multi-tag operation once per service, so the
/// estimate counts every rendering: one rendering of 18 × 3 fits the budget
/// (about 260 thousand rows, 25 MB on 2.2.0), ten do not.
#[test]
fn multi_tag_operations_count_once_per_service() {
    let dir = tempfile::tempdir().unwrap();
    let spec = write_spec(&dir, "tagged.json", &tagged_cyclic_spec(18, 3, 10));
    let full = [
        spec.as_str(),
        "--detail",
        "full",
        "--include-schemas",
        "--inline-schemas",
    ];

    let mut args = full.to_vec();
    args.push("--no-report");
    let (code, out, err) = bounded(&args, LIMIT);
    assert_eq!(code, 0);
    assert!(err.starts_with(NOTICE), "stderr: {err}");
    assert!(out.contains(BLOCKQUOTE));
    for tag in 0..10 {
        assert!(out.contains(&format!("## t{tag} {{#t{tag}}}")), "t{tag}");
    }

    let mut args = full.to_vec();
    args.push("--stats");
    let (code, out, err) = bounded(&args, LIMIT);
    assert_eq!(code, 0);
    assert!(out.contains("TOTAL"));
    assert!(err.contains(NOTICE));
}

#[test]
fn inline_expansion_under_the_budget_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let spec = write_spec(&dir, "cyclic.json", &cyclic_spec(8, 3));
    let args = [
        spec.as_str(),
        "--detail",
        "full",
        "--include-schemas",
        "--inline-schemas",
        "--no-report",
    ];
    let (code, out, err) = bounded(&args, LIMIT);
    assert_eq!(code, 0);
    assert!(err.is_empty(), "stderr: {err}");
    assert!(!out.contains("> Schema expansion is bounded"));
    assert!(!out.contains("Omitted nested expansion"));
    // Fully expanded down to the cycle guard, deterministically.
    assert!(out.contains("Cycle detected while expanding schema reference"));
    assert_eq!(bounded(&args, LIMIT).1, out);

    let (_, costs, _) = bounded(
        &[&spec, "--detail", "full", "--include-schemas", "--costs"],
        LIMIT,
    );
    assert!(!costs.contains("inline-bounded"));
}

/// `C0`'s `--costs` row: `(INLINE/USE, inline-bounded)`.
fn c0_inline_per_use(spec: &str, extra: &[&str]) -> (u64, bool) {
    let mut args = vec![spec, "--detail", "full", "--include-schemas", "--costs"];
    args.extend(extra);
    let (code, out, _) = bounded(&args, LIMIT);
    assert_eq!(code, 0);
    let row = out
        .lines()
        .find(|line| line.starts_with("C0 "))
        .unwrap_or_else(|| panic!("no C0 row: {out}"));
    let columns: Vec<&str> = row.split_whitespace().collect();
    (columns[3].parse().unwrap(), row.contains("inline-bounded"))
}

/// A bounded `--costs` measurement covers exactly the per-schema budget of
/// rows, more than any unbounded one, so growing the graph past the budget
/// never makes a schema look cheaper: at 9 × 3, C0's expansion fits (about
/// 4,700 rows); from 10 × 3 it is bounded, and the figure keeps growing.
#[test]
fn bounded_costs_figures_never_rank_below_unbounded_ones() {
    let dir = tempfile::tempdir().unwrap();
    let mut last = 0;
    for (n, expect_bounded) in [(9, false), (10, true), (12, true)] {
        let spec = write_spec(&dir, &format!("c{n}.json"), &cyclic_spec(n, 3));
        let (inline, is_bounded) = c0_inline_per_use(&spec, &[]);
        assert_eq!(is_bounded, expect_bounded, "{n} x 3");
        assert!(inline >= last, "{n} x 3: {inline} < {last}");
        last = inline;
    }
}

/// The per-schema `--costs` budget holds at any explicit `--schema-depth`:
/// depth 24 does not cut a dense cyclic graph's expansion in practice, and
/// the linked document itself renders in milliseconds.
#[test]
fn costs_budget_holds_at_an_explicit_schema_depth() {
    let dir = tempfile::tempdir().unwrap();
    let spec = write_spec(&dir, "cyclic.json", &cyclic_spec(40, 4));
    let (_, is_bounded) = c0_inline_per_use(&spec, &["--schema-depth", "24"]);
    assert!(is_bounded);
}
