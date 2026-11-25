//! Shared-component attribution (#181): a change inside a `$ref` target is
//! reported once per usage direction, in every output, however many reference
//! paths reach it.
use crate::common::{vimanam, write_spec};
use serde_json::{Value, json};

fn r(name: &str) -> Value {
    json!({"$ref": format!("#/components/schemas/{name}")})
}

/// Runs `vimanam diff old new [args]` and returns `(stdout, exit code)`.
fn run(old: &str, new: &str, args: &[&str]) -> (String, i32) {
    let output = vimanam()
        .timeout(std::time::Duration::from_secs(60))
        .args(["diff", old, new])
        .args(args)
        .output()
        .unwrap();
    (
        String::from_utf8(output.stdout).unwrap(),
        output.status.code().unwrap(),
    )
}

fn json_run(old: &str, new: &str, args: &[&str]) -> (Value, i32) {
    let (stdout, code) = run(old, new, &[&["--format", "json"], args].concat());
    (serde_json::from_str(&stdout).unwrap(), code)
}

/// `count` components that each reference every other one and `Leaf`, one
/// GET per component: the shape of Stripe's reference cycle, where a single
/// inlined root reaches `Leaf` by exponentially many paths.
fn dense_cycle(count: usize, leaf: Value) -> Value {
    let names: Vec<String> = (0..count).map(|i| format!("N{i}")).collect();
    let mut schemas = serde_json::Map::new();
    for name in &names {
        let properties: serde_json::Map<String, Value> = names
            .iter()
            .filter(|other| *other != name)
            .map(|other| (other.to_lowercase(), r(other)))
            .chain([("leaf".to_string(), r("Leaf"))])
            .collect();
        schemas.insert(
            name.clone(),
            json!({"type": "object", "properties": properties}),
        );
    }
    schemas.insert("Leaf".into(), leaf);
    let paths: serde_json::Map<String, Value> = names
        .iter()
        .map(|name| {
            (
                format!("/{}", name.to_lowercase()),
                json!({"get": {"responses": {"200": {"description": "ok",
                    "content": {"application/json": {"schema": r(name)}}}}}}),
            )
        })
        .collect();
    json!({
        "openapi": "3.0.3",
        "info": {"title": "Dense", "version": "1"},
        "paths": paths,
        "components": {"schemas": schemas}
    })
}

#[test]
fn a_change_under_a_dense_reference_cycle_is_reported_once() {
    let dir = tempfile::tempdir().unwrap();
    let leaf = |values: Value| json!({"type": "object", "properties": {"state": {"type": "string", "enum": values}}});
    let old = write_spec(&dir, "old.json", &dense_cycle(14, leaf(json!(["a"]))));
    let new = write_spec(&dir, "new.json", &dense_cycle(14, leaf(json!(["a", "b"]))));

    let (markdown, code) = run(&old, &new, &[]);
    assert_eq!(code, 0, "{markdown}");
    assert!(
        markdown.contains(
            "0 endpoints added, 0 removed, 14 changed; 0 breaking, 0 non-breaking, 1 to review"
        ),
        "{markdown}"
    );
    assert!(markdown.contains(
        "| Component schema changed | component `Leaf` | response `/properties/state/enum` value `b` added (14 operations) |"
    ));
    assert_eq!(markdown.matches("- `GET /n").count(), 14, "{markdown}");

    // v2: one directional fact at the type, every response as context.
    let (v2, _) = json_run(&old, &new, &[]);
    let facts: Vec<&Value> = v2["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["native_kind"] == "component_schema_changed")
        .collect();
    assert_eq!(facts.len(), 1, "{v2}");
    assert_eq!(facts[0]["kind"], "enum_value_added");
    assert_eq!(facts[0]["details"]["direction"], "response");
    assert_eq!(facts[0]["uses"].as_array().unwrap().len(), 14);
    assert_eq!(v2["summary"]["operations_changed"], 14);

    // v1: one record per affected response.
    let (v1, _) = json_run(&old, &new, &["--contract", "1"]);
    let records = v1["changes"].as_array().unwrap();
    assert_eq!(records.len(), 14, "{v1}");
    for record in records {
        assert_eq!(record["kind"], "response_schema_changed");
        assert_eq!(record["details"]["component"], "Leaf");
        assert_eq!(
            record["details"]["schema_change"]["pointer"],
            "/properties/state/enum/b"
        );
    }
}

/// `Shared` is sent by `POST /orders` and returned by `GET /orders/{id}`.
fn shared_spec(values: Value, extra_first: bool) -> Value {
    let mut paths = serde_json::Map::new();
    if extra_first {
        // Sorts and is declared before the others: it is the first use.
        paths.insert(
            "/archive".into(),
            json!({"get": {"responses": {"200": {"description": "ok",
                "content": {"application/json": {"schema": r("Shared")}}}}}}),
        );
    }
    paths.insert(
        "/orders".into(),
        json!({"post": {
            "requestBody": {"content": {"application/json": {"schema": r("Shared")}}},
            "responses": {"201": {"description": "created"}}
        }}),
    );
    paths.insert(
        "/orders/{id}".into(),
        json!({"get": {
            "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok",
                "content": {"application/json": {"schema": {"type": "object", "properties": {"order": r("Shared")}}}}}}
        }}),
    );
    json!({
        "openapi": "3.0.3",
        "info": {"title": "Orders", "version": "1"},
        "paths": paths,
        "components": {"schemas": {
            "Shared": {"type": "object", "properties": {"status": {"type": "string", "enum": values}}}
        }}
    })
}

#[test]
fn a_component_used_both_ways_gets_one_record_per_direction_and_trips_the_gate() {
    let dir = tempfile::tempdir().unwrap();
    let old = write_spec(
        &dir,
        "old.json",
        &shared_spec(json!(["open", "closed"]), false),
    );
    let new = write_spec(&dir, "new.json", &shared_spec(json!(["open"]), false));

    let (markdown, code) = run(&old, &new, &["--fail-on-breaking"]);
    assert_eq!(code, 3, "{markdown}");
    assert!(markdown.contains(
        "| Component schema changed | component `Shared` | request `/properties/status/enum` value `closed` removed (1 operation) |"
    ), "{markdown}");
    assert!(markdown.contains(
        "| Component schema changed | component `Shared` | response `/properties/status/enum` value `closed` removed (1 operation) |"
    ), "{markdown}");
    assert!(
        markdown.contains("1 breaking, 1 non-breaking, 0 to review"),
        "{markdown}"
    );
    assert!(
        markdown.contains(
            "### `Shared`\n\n- `POST /orders` request body\n- `GET /orders/{id}` response 200\n"
        ),
        "{markdown}"
    );

    for contract in ["1", "2"] {
        let (document, code) =
            json_run(&old, &new, &["--contract", contract, "--fail-on-breaking"]);
        assert_eq!(code, 3, "contract {contract}: {document}");
        assert_eq!(document["summary"]["breaking"], 1, "contract {contract}");
    }

    // Widening the enum instead breaks nobody on the request side and only
    // needs review on the response side.
    let wider = write_spec(
        &dir,
        "wider.json",
        &shared_spec(json!(["open", "closed", "held"]), false),
    );
    let (markdown, code) = run(&old, &wider, &["--fail-on-breaking"]);
    assert_eq!(code, 0, "{markdown}");
    assert!(
        markdown.contains("0 breaking, 1 non-breaking, 1 to review"),
        "{markdown}"
    );
}

#[test]
fn component_change_ids_do_not_depend_on_which_operation_reaches_it_first() {
    let dir = tempfile::tempdir().unwrap();
    let old = write_spec(
        &dir,
        "old.json",
        &shared_spec(json!(["open", "closed"]), false),
    );
    let new = write_spec(&dir, "new.json", &shared_spec(json!(["open"]), false));
    let old_more = write_spec(
        &dir,
        "old_more.json",
        &shared_spec(json!(["open", "closed"]), true),
    );
    let new_more = write_spec(&dir, "new_more.json", &shared_spec(json!(["open"]), true));

    let facts = |document: &Value| -> Vec<(String, usize)> {
        document["changes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["native_kind"] == "component_schema_changed")
            .map(|c| {
                (
                    c["id"].as_str().unwrap().to_string(),
                    c["uses"].as_array().unwrap().len(),
                )
            })
            .collect()
    };
    let (before, _) = json_run(&old, &new, &[]);
    let (after, _) = json_run(&old_more, &new_more, &[]);
    let (before, after) = (facts(&before), facts(&after));
    assert_eq!(before.len(), 2);
    // Same IDs although a new first operation now reaches the component; only
    // the response fact's context grew.
    assert_eq!(
        before.iter().map(|f| &f.0).collect::<Vec<_>>(),
        after.iter().map(|f| &f.0).collect::<Vec<_>>()
    );
    assert_eq!((before[1].1, after[1].1), (1, 2));

    // v1 keeps one record per use; existing uses keep their IDs.
    let ids = |document: Value| -> Vec<String> {
        document["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_string())
            .collect()
    };
    let before = ids(json_run(&old, &new, &["--contract", "1"]).0);
    let after = ids(json_run(&old_more, &new_more, &["--contract", "1"]).0);
    assert_eq!(after.len(), before.len() + 1);
    assert!(before.iter().all(|id| after.contains(id)));
}

#[test]
fn a_moved_operation_lists_its_new_path_as_the_use() {
    let dir = tempfile::tempdir().unwrap();
    let spec = |path: &str, values: Value| {
        json!({
            "openapi": "3.0.3",
            "info": {"title": "Moves", "version": "1"},
            "paths": {path: {"get": {"operationId": "getItem", "responses": {"200": {"description": "ok",
                "content": {"application/json": {"schema": r("Item")}}}}}}},
            "components": {"schemas": {"Item": {"type": "string", "enum": values}}}
        })
    };
    let old = write_spec(&dir, "old.json", &spec("/v1/items", json!(["a"])));
    let new = write_spec(&dir, "new.json", &spec("/v2/items", json!(["a", "b"])));
    let (markdown, _) = run(&old, &new, &[]);
    assert!(
        markdown.contains("| Endpoint moved | `GET /v2/items` |"),
        "{markdown}"
    );
    assert!(
        markdown.contains("### `Item`\n\n- `GET /v2/items` response 200\n"),
        "{markdown}"
    );

    // With --exact-paths the operation is removed and added: nothing reaches
    // the component from a surviving operation.
    let (markdown, _) = run(&old, &new, &["--exact-paths"]);
    assert!(!markdown.contains("Component schema changed"), "{markdown}");
}
