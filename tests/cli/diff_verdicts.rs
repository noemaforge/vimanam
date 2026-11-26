//! Verdict regressions found by the Lean models (`formal/`): a boolean keyword
//! written or dropped at its default (#184), and component references outside
//! the schema grammar the severity table reads (#186).
use crate::common::{vimanam, write_spec};
use serde_json::{Value, json};

fn r(name: &str) -> Value {
    json!({"$ref": format!("#/components/schemas/{name}")})
}

/// One `POST /a` with a JSON request body and a 200 response.
fn spec(request: Value, response: Value, components: Value) -> Value {
    json!({
        "openapi": "3.0.3",
        "info": {"title": "Verdicts", "version": "1"},
        "paths": {"/a": {"post": {
            "requestBody": {"content": {"application/json": {"schema": request}}},
            "responses": {"200": {"description": "ok",
                "content": {"application/json": {"schema": response}}}}
        }}},
        "components": {"schemas": components}
    })
}

/// `vimanam diff old new --fail-on-breaking --format json --contract 1`:
/// the v1 records as `(pointer, severity, component)` and the exit code.
fn diff_records(old: &Value, new: &Value) -> (Vec<(String, String, Option<String>)>, i32) {
    let dir = tempfile::tempdir().unwrap();
    let old = write_spec(&dir, "old.json", old);
    let new = write_spec(&dir, "new.json", new);
    let output = vimanam()
        .args(["diff", &old, &new, "--fail-on-breaking"])
        .args(["--format", "json", "--contract", "1"])
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&output.stdout).unwrap();
    let records = doc["changes"]
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
                change["details"]["component"].as_str().map(str::to_string),
            )
        })
        .collect();
    (records, output.status.code().unwrap())
}

fn object() -> Value {
    json!({"type": "object", "properties": {"a": {"type": "string"}}})
}

fn with(mut schema: Value, key: &str, value: Value) -> Value {
    schema[key] = value;
    schema
}

// ── #184: a boolean keyword at its default ─────────────────────────────────

#[test]
fn request_nullable_false_added_is_a_no_op() {
    let old = spec(object(), object(), json!({}));
    let new = spec(
        with(object(), "nullable", json!(false)),
        object(),
        json!({}),
    );
    let (records, code) = diff_records(&old, &new);
    assert_eq!(code, 0, "{records:?}");
    assert_eq!(
        records,
        [("/nullable".to_string(), "non_breaking".to_string(), None)]
    );
}

#[test]
fn request_additional_properties_true_removed_is_a_no_op() {
    let old = spec(
        with(object(), "additionalProperties", json!(true)),
        object(),
        json!({}),
    );
    let new = spec(object(), object(), json!({}));
    let (records, code) = diff_records(&old, &new);
    assert_eq!(code, 0, "{records:?}");
    assert_eq!(
        records,
        [(
            "/additionalProperties".to_string(),
            "non_breaking".to_string(),
            None
        )]
    );
}

#[test]
fn response_nullable_false_removed_is_a_no_op() {
    let old = spec(
        object(),
        with(object(), "nullable", json!(false)),
        json!({}),
    );
    let new = spec(object(), object(), json!({}));
    let (records, code) = diff_records(&old, &new);
    assert_eq!(code, 0, "{records:?}");
    assert_eq!(
        records,
        [("/nullable".to_string(), "non_breaking".to_string(), None)]
    );
}

#[test]
fn a_shared_component_written_at_its_default_is_a_no_op_both_ways() {
    let old = spec(r("C"), r("C"), json!({"C": object()}));
    let new = spec(
        r("C"),
        r("C"),
        json!({"C": with(with(object(), "nullable", json!(false)), "additionalProperties", json!(true))}),
    );
    let (records, code) = diff_records(&old, &new);
    assert_eq!(code, 0, "{records:?}");
    assert!(!records.is_empty());
    assert!(
        records
            .iter()
            .all(|(_, severity, component)| severity == "non_breaking"
                && component.as_deref() == Some("C")),
        "{records:?}"
    );
}

#[test]
fn moving_off_the_default_keeps_its_direction() {
    // Dropping `nullable: true` from a request: clients sending `null` fail.
    let old = spec(with(object(), "nullable", json!(true)), object(), json!({}));
    let new = spec(object(), object(), json!({}));
    let (records, code) = diff_records(&old, &new);
    assert_eq!(code, 3, "{records:?}");
    assert_eq!(
        records,
        [("/nullable".to_string(), "breaking".to_string(), None)]
    );
    // Writing `additionalProperties: false` on a request narrows it.
    let old = spec(object(), object(), json!({}));
    let new = spec(
        with(object(), "additionalProperties", json!(false)),
        object(),
        json!({}),
    );
    let (records, code) = diff_records(&old, &new);
    assert_eq!(code, 3, "{records:?}");
    assert_eq!(records[0].1, "breaking");
}

// ── #186: references outside the schema grammar ────────────────────────────

/// A response schema holding `P` at a position the severity table does not
/// rate, with `P` going from `string` to `integer`.
fn outside(schema: Value) -> (Vec<(String, String, Option<String>)>, i32) {
    let old = spec(object(), schema.clone(), json!({"P": {"type": "string"}}));
    let new = spec(object(), schema, json!({"P": {"type": "integer"}}));
    diff_records(&old, &new)
}

#[test]
fn a_component_under_default_is_inlined_and_reviewed() {
    let (records, code) = outside(json!({"type": "object", "default": r("P")}));
    assert_eq!(code, 0, "{records:?}");
    assert_eq!(
        records,
        [("/default/type".to_string(), "review".to_string(), None)]
    );
}

#[test]
fn a_component_under_if_is_inlined_and_reviewed() {
    let (records, code) = outside(json!({"if": r("P"), "then": {"type": "object"}}));
    assert_eq!(code, 0, "{records:?}");
    assert_eq!(
        records,
        [("/if/type".to_string(), "review".to_string(), None)]
    );
}

#[test]
fn a_component_under_prefix_items_is_inlined_and_reviewed() {
    let (records, code) = outside(json!({"type": "array", "prefixItems": [r("P")]}));
    assert_eq!(code, 0, "{records:?}");
    assert_eq!(
        records,
        [(
            "/prefixItems/0/type".to_string(),
            "review".to_string(),
            None
        )]
    );
}

#[test]
fn a_component_under_defs_is_inlined_and_reviewed() {
    let (records, code) = outside(json!({"type": "object", "$defs": {"d": {"not": r("P")}}}));
    assert_eq!(code, 0, "{records:?}");
    assert_eq!(
        records,
        [("/$defs/d/not/type".to_string(), "review".to_string(), None)]
    );
}

#[test]
fn a_component_also_used_through_the_grammar_keeps_its_verdict_there() {
    let (records, code) = outside(json!({
        "type": "object",
        "properties": {"p": r("P")},
        "default": r("P")
    }));
    assert_eq!(code, 3, "{records:?}");
    assert_eq!(
        records,
        [
            ("/default/type".to_string(), "review".to_string(), None),
            (
                "/type".to_string(),
                "breaking".to_string(),
                Some("P".to_string())
            ),
        ]
    );
}

#[test]
fn an_unchanged_component_outside_the_grammar_reports_nothing() {
    let schema = json!({"type": "object", "default": r("P"), "if": r("P")});
    let old = spec(object(), schema.clone(), json!({"P": {"type": "string"}}));
    let (records, code) = diff_records(&old, &old);
    assert_eq!(code, 0);
    assert!(records.is_empty(), "{records:?}");
}

#[test]
fn a_self_referencing_component_under_default_terminates() {
    let p = |ty: &str| json!({"type": ty, "default": r("P")});
    let old = spec(object(), r("P"), json!({"P": p("object")}));
    let new = spec(object(), r("P"), json!({"P": p("array")}));
    let (records, code) = diff_records(&old, &new);
    assert_eq!(code, 3, "{records:?}");
    assert_eq!(
        records,
        [
            (
                "/default/type".to_string(),
                "review".to_string(),
                Some("P".to_string())
            ),
            (
                "/type".to_string(),
                "breaking".to_string(),
                Some("P".to_string())
            ),
        ]
    );
}

/// `P` going from `string` to `integer` under `schema`, used as the request
/// body (`request`) or the response.
fn outside_at(schema: Value, request: bool) -> (Vec<(String, String, Option<String>)>, i32) {
    let side = |p: &str| {
        let comps = json!({"P": {"type": p}});
        if request {
            spec(schema.clone(), object(), comps)
        } else {
            spec(object(), schema.clone(), comps)
        }
    };
    diff_records(&side("string"), &side("integer"))
}

const MEMBER: &str = r##"/enum/{"$ref":"#~1components~1schemas~1P"}/type"##;

#[test]
fn an_enum_member_holding_a_component_is_reviewed_not_a_member_change() {
    // An `enum` member is a value: the set still holds the same member, and
    // the referenced component's change is reported inside it, Review, in
    // either direction (it used to be a member removed plus one added,
    // Breaking for a request).
    for request in [true, false] {
        let (records, code) = outside_at(json!({"type": "object", "enum": ["x", r("P")]}), request);
        assert_eq!(code, 0, "{records:?}");
        assert_eq!(records, [(MEMBER.to_string(), "review".to_string(), None)]);
    }
}

#[test]
fn an_enum_member_added_or_removed_keeps_its_set_verdict() {
    let old = spec(
        json!({"enum": ["x", r("P")]}),
        object(),
        json!({"P": {"type": "string"}}),
    );
    let new = spec(
        json!({"enum": ["x"]}),
        object(),
        json!({"P": {"type": "string"}}),
    );
    let (records, code) = diff_records(&old, &new);
    assert_eq!(code, 3, "{records:?}");
    assert_eq!(
        records,
        [(
            r##"/enum/{"$ref":"#~1components~1schemas~1P"}"##.to_string(),
            "breaking".to_string(),
            None
        )]
    );
}

#[test]
fn a_component_under_const_is_inlined_and_reviewed() {
    for request in [true, false] {
        let (records, code) = outside_at(json!({"type": "object", "const": r("P")}), request);
        assert_eq!(code, 0, "{records:?}");
        assert_eq!(
            records,
            [("/const/type".to_string(), "review".to_string(), None)]
        );
    }
}

/// `enum: ["x", old]` → `enum: ["x", new]` as the request body or the
/// response, with `A` and `B` both strings.
fn rewritten(
    old: Value,
    new: Value,
    request: bool,
) -> (Vec<(String, String, Option<String>)>, i32) {
    let comps = json!({"A": {"type": "string"}, "B": {"type": "string"}});
    let side = |member: &Value| {
        let schema = json!({"type": "string", "enum": ["x", member]});
        if request {
            spec(schema, object(), comps.clone())
        } else {
            spec(object(), schema, comps.clone())
        }
    };
    diff_records(&side(&old), &side(&new))
}

const MEMBER_A: &str = r##"/enum/{"$ref":"#~1components~1schemas~1A"}"##;

#[test]
fn an_enum_member_retargeted_to_an_identical_component_is_one_review() {
    for request in [true, false] {
        let (records, code) = rewritten(r("A"), r("B"), request);
        assert_eq!(code, 0, "{records:?}");
        assert_eq!(
            records,
            [(MEMBER_A.to_string(), "review".to_string(), None)]
        );
    }
}

#[test]
fn an_enum_member_inlined_or_extracted_is_one_review() {
    for request in [true, false] {
        let (records, code) = rewritten(r("A"), json!({"type": "string"}), request);
        assert_eq!(code, 0, "{records:?}");
        assert_eq!(
            records,
            [(MEMBER_A.to_string(), "review".to_string(), None)]
        );
        let (records, code) = rewritten(json!({"type": "string"}), r("A"), request);
        assert_eq!(code, 0, "{records:?}");
        assert_eq!(
            records,
            [(
                r#"/enum/{"type":"string"}"#.to_string(),
                "review".to_string(),
                None
            )]
        );
    }
}

#[test]
fn an_enum_member_retargeted_to_a_different_component_is_replaced() {
    let comps = json!({"A": {"type": "string"}, "C": {"type": "integer"}});
    let old = spec(json!({"enum": [r("A")]}), object(), comps.clone());
    let new = spec(json!({"enum": [r("C")]}), object(), comps);
    let (records, code) = diff_records(&old, &new);
    assert_eq!(code, 3, "{records:?}");
    let severities: Vec<&str> = records.iter().map(|(_, s, _)| s.as_str()).collect();
    assert_eq!(severities, ["breaking", "non_breaking"], "{records:?}");
}

#[test]
fn an_enum_member_whose_target_is_deleted_is_reviewed() {
    let old = spec(
        json!({"enum": [r("A")]}),
        object(),
        json!({"A": {"type": "string"}}),
    );
    let new = spec(json!({"enum": [r("A")]}), object(), json!({}));
    let (records, code) = diff_records(&old, &new);
    assert_eq!(code, 0, "{records:?}");
    assert!(!records.is_empty());
    assert!(records.iter().all(|(_, s, _)| s == "review"), "{records:?}");
}
