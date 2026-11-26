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
            "tags": ["c"],
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
    // model expands each schema alone: past the budget it is measured at
    // depth 2 and marked as a lower bound.
    let (code, out, err) = with(&["--costs"]);
    assert_eq!(code, 0);
    assert!(!err.contains(NOTICE));
    assert!(out.contains("cycle, inline-bounded"), "{out}");
    assert!(out.contains(
        "inline-bounded: one inline expansion would render more than 2000000 schema rows, so INLINE/USE was measured at --schema-depth 2 and is a lower bound."
    ));
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
