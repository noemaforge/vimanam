//! OpenAPI operations moved across a path change (#137) and pagination
//! inference over continuation names, nested envelopes and end flags (#138).
use serde_json::{Value, json};
use vimanam::contract::{self, ChangeKind, NativeLocation, Severity};
use vimanam::diff::{self, EndpointMatching, MoveRule};

fn parse(value: &Value) -> vimanam::ApiDocumentation {
    vimanam::parse_openapi_bytes(&serde_json::to_vec(value).unwrap(), "json", None).unwrap()
}

/// A one-operation spec in the shape the #137/#138 reporter's generator uses:
/// `GET path` returning `{items: [Item], <continuation>}`, with optional query
/// parameters. A dotted continuation (`links.next`) is nested one level.
fn spec(path: &str, operation_id: &str, params: &[&str], continuation: Option<&str>) -> Value {
    let mut properties =
        json!({"items": {"type": "array", "items": {"$ref": "#/components/schemas/Item"}}});
    if let Some(name) = continuation {
        match name.split_once('.') {
            Some((outer, inner)) => {
                properties[outer] =
                    json!({"type": "object", "properties": {inner: {"type": "string"}}})
            }
            None => properties[name] = json!({"type": "string"}),
        }
    }
    let parameters: Vec<Value> = params
        .iter()
        .map(|name| {
            let ty = if ["limit", "page_size", "pageSize", "size"].contains(name) {
                "integer"
            } else {
                "string"
            };
            json!({"name": name, "in": "query", "schema": {"type": ty}})
        })
        .collect();
    json!({
        "openapi": "3.0.3",
        "info": {"title": "Items", "version": "1"},
        "paths": {path: {"get": {
            "operationId": operation_id,
            "parameters": parameters,
            "responses": {"200": {"description": "ok", "content": {"application/json": {
                "schema": {"type": "object", "properties": properties}}}}}
        }}},
        "components": {"schemas": {"Item": {"type": "object", "required": ["id"],
            "properties": {"id": {"type": "string"}}}}}
    })
}

/// Merges the `paths` of several single-operation specs into one spec.
fn merge(specs: &[Value]) -> Value {
    let mut merged = specs[0].clone();
    for other in &specs[1..] {
        for (path, item) in other["paths"].as_object().unwrap() {
            let entry = merged["paths"][path].as_object_mut().map(|m| m.clone());
            let mut entry = entry.unwrap_or_default();
            for (method, operation) in item.as_object().unwrap() {
                entry.insert(method.clone(), operation.clone());
            }
            merged["paths"][path] = Value::Object(entry);
        }
    }
    merged
}

fn neutral(old: &Value, new: &Value, matching: EndpointMatching) -> Vec<contract::Change> {
    contract::openapi::diff_with(&parse(old), &parse(new), matching).unwrap()
}

fn kinds(changes: &[contract::Change]) -> Vec<ChangeKind> {
    changes.iter().map(|c| c.kind).collect()
}

fn paging_old(path: &str, id: &str) -> Value {
    spec(path, id, &[], None)
}
fn paging_new(path: &str, id: &str) -> Value {
    spec(path, id, &["cursor", "limit"], Some("next_cursor"))
}

// ── #137: matrix 1 ─────────────────────────────────────────────────────────

#[test]
fn matrix_1_control_same_path_is_unchanged() {
    let changes = neutral(
        &paging_old("/items", "listItems"),
        &paging_new("/items", "listItems"),
        EndpointMatching::Moves,
    );
    assert_eq!(
        kinds(&changes),
        [
            ChangeKind::ArgumentAdded,
            ChangeKind::ArgumentAdded,
            ChangeKind::FieldAdded,
            ChangeKind::PaginationChanged
        ]
    );
    // Moves never alter an exact match.
    assert_eq!(
        changes,
        neutral(
            &paging_old("/items", "listItems"),
            &paging_new("/items", "listItems"),
            EndpointMatching::Exact,
        )
    );
}

#[test]
fn matrix_1_version_in_the_path_is_a_move_with_inner_changes() {
    for (old_path, new_path) in [
        ("/v1/items", "/v2/items"),
        ("/api/v1/items", "/api/v2/items"),
    ] {
        let changes = neutral(
            &paging_old(old_path, "listItems"),
            &paging_new(new_path, "listItems"),
            EndpointMatching::Moves,
        );
        assert_eq!(
            kinds(&changes),
            [
                ChangeKind::OperationMoved,
                ChangeKind::ArgumentAdded,
                ChangeKind::ArgumentAdded,
                ChangeKind::FieldAdded,
                ChangeKind::PaginationChanged
            ],
            "{old_path}"
        );
        let moved = &changes[0];
        assert_eq!(moved.severity, Severity::Breaking);
        assert_eq!(moved.native_kind, "endpoint_moved");
        assert_eq!(moved.details["old_path"], old_path);
        assert_eq!(moved.details["old_method"], "GET");
        // A unique, unchanged operationId pairs first.
        assert_eq!(moved.details["matched_by"], "operation_id");
        assert!(moved.details.get("old_operation_id").is_none());
        // Identity: the moved fact and every inner fact live at the new
        // coordinate; details link back to the old operation entity.
        let new_surface = contract::openapi::adapt(&parse(&paging_new(new_path, "listItems")));
        let old_surface = contract::openapi::adapt(&parse(&paging_old(old_path, "listItems")));
        let new_op = &new_surface
            .entities
            .iter()
            .find(|e| e.name == "listItems")
            .unwrap()
            .id;
        let old_op = &old_surface
            .entities
            .iter()
            .find(|e| e.name == "listItems")
            .unwrap()
            .id;
        assert_eq!(&moved.subject, new_op);
        assert_eq!(&moved.details["from_operation"], old_op);
        for change in &changes {
            assert!(
                matches!(&change.native, NativeLocation::Openapi { path: Some(p), .. } if p == new_path),
                "{change:?}"
            );
        }
    }
}

#[test]
fn matrix_1_version_in_the_path_with_renamed_operation_id_pairs_by_segment() {
    let changes = neutral(
        &paging_old("/v1/items", "listItems"),
        &paging_new("/v2/items", "exportItems"),
        EndpointMatching::Moves,
    );
    assert_eq!(
        kinds(&changes),
        [
            ChangeKind::OperationMoved,
            ChangeKind::OperationRenamed,
            ChangeKind::ArgumentAdded,
            ChangeKind::ArgumentAdded,
            ChangeKind::FieldAdded,
            ChangeKind::PaginationChanged
        ]
    );
    assert_eq!(changes[0].details["matched_by"], "version_segment");
    assert_eq!(changes[0].details["old_operation_id"], "listItems");
    assert_eq!(changes[0].details["new_operation_id"], "exportItems");
}

#[test]
fn matrix_1_version_in_the_server_url_is_an_exact_match() {
    let mut old = paging_old("/items", "listItems");
    let mut new = paging_new("/items", "listItems");
    old["servers"] = json!([{"url": "https://api.example.test/v1"}]);
    new["servers"] = json!([{"url": "https://api.example.test/v2"}]);
    let changes = neutral(&old, &new, EndpointMatching::Moves);
    assert!(!kinds(&changes).contains(&ChangeKind::OperationMoved));
    assert!(kinds(&changes).contains(&ChangeKind::PaginationChanged));
}

#[test]
fn exact_matching_keeps_removed_plus_added() {
    let changes = neutral(
        &paging_old("/v1/items", "listItems"),
        &paging_new("/v2/items", "listItems"),
        EndpointMatching::Exact,
    );
    assert_eq!(
        kinds(&changes),
        [ChangeKind::OperationRemoved, ChangeKind::OperationAdded]
    );
    // The default library entry point is exact.
    assert_eq!(
        changes,
        contract::openapi::diff(
            &parse(&paging_old("/v1/items", "listItems")),
            &parse(&paging_new("/v2/items", "listItems"))
        )
        .unwrap()
    );
}

// ── #137: ambiguity and no-match ───────────────────────────────────────────

#[test]
fn ambiguous_version_segment_pairs_nothing() {
    // Two old versions normalise to the same key: no pairing.
    let old = merge(&[
        paging_old("/v1/items", "listV1"),
        paging_old("/v2/items", "listV2"),
    ]);
    let new = paging_new("/v3/items", "listV3");
    let changes = neutral(&old, &new, EndpointMatching::Moves);
    assert_eq!(
        kinds(&changes),
        [
            ChangeKind::OperationRemoved,
            ChangeKind::OperationRemoved,
            ChangeKind::OperationAdded
        ]
    );
    // Likewise two new candidates for one old operation.
    let changes = neutral(&new, &old, EndpointMatching::Moves);
    assert!(!kinds(&changes).contains(&ChangeKind::OperationMoved));
}

#[test]
fn duplicate_operation_id_falls_back_to_the_version_segment() {
    // `dup` is not unique in the new document, so operationId pairing
    // refuses; `/v1/items` ↔ `/v2/items` still pairs by its segment.
    let old = paging_old("/v1/items", "dup");
    let new = merge(&[paging_new("/v2/items", "dup"), paging_new("/other", "dup")]);
    let changes = neutral(&old, &new, EndpointMatching::Moves);
    let moved: Vec<_> = changes
        .iter()
        .filter(|c| c.kind == ChangeKind::OperationMoved)
        .collect();
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].details["matched_by"], "version_segment");
    assert_eq!(
        changes
            .iter()
            .filter(|c| c.kind == ChangeKind::OperationAdded)
            .count(),
        1
    );
}

#[test]
fn non_version_segments_methods_and_unrelated_paths_do_not_pair() {
    // `v1beta` is not a whole `v<digits>` segment.
    let changes = neutral(
        &paging_old("/v1beta/items", "a"),
        &paging_new("/v2beta/items", "b"),
        EndpointMatching::Moves,
    );
    assert_eq!(
        kinds(&changes),
        [ChangeKind::OperationRemoved, ChangeKind::OperationAdded]
    );
    // Different methods never pair, even with one operationId.
    let mut post = paging_new("/v2/items", "listItems");
    let operation = post["paths"]["/v2/items"]["get"].take();
    post["paths"]["/v2/items"] = json!({"post": operation});
    let changes = neutral(
        &paging_old("/v1/items", "listItems"),
        &post,
        EndpointMatching::Moves,
    );
    assert!(!kinds(&changes).contains(&ChangeKind::OperationMoved));
    // Unrelated paths with different operationIds stay removed plus added.
    let changes = neutral(
        &paging_old("/users", "listUsers"),
        &paging_new("/accounts", "listAccounts"),
        EndpointMatching::Moves,
    );
    assert!(!kinds(&changes).contains(&ChangeKind::OperationMoved));
    // A unique shared operationId pairs even across unrelated paths.
    let changes = neutral(
        &paging_old("/users", "list"),
        &paging_new("/accounts", "list"),
        EndpointMatching::Moves,
    );
    assert_eq!(changes[0].kind, ChangeKind::OperationMoved);
    assert_eq!(changes[0].details["matched_by"], "operation_id");
}

#[test]
fn legacy_diff_reports_moves_only_when_asked() {
    let old = parse(&paging_old("/v1/items", "listItems"));
    let new = parse(&paging_new("/v2/items", "listItems"));
    let exact = diff::diff(&old, &new);
    assert_eq!(exact, diff::diff_with(&old, &new, EndpointMatching::Exact));
    assert_eq!((exact.endpoints_added(), exact.endpoints_removed()), (1, 1));
    let moved = diff::diff_with(&old, &new, EndpointMatching::Moves);
    assert_eq!(moved.endpoints_moved(), 1);
    assert_eq!(moved.endpoints_changed(), 1);
    assert_eq!((moved.endpoints_added(), moved.endpoints_removed()), (0, 0));
    let first = &moved.changes[0];
    assert_eq!(first.endpoint.path, "/v2/items");
    assert!(matches!(
        &first.kind,
        diff::ChangeKind::EndpointMoved { from, matched_by: MoveRule::OperationId, .. }
            if from.path == "/v1/items"
    ));
    assert_eq!(diff::severity(first), diff::Severity::Breaking);
    // Inner changes are attributed to the new endpoint.
    assert!(moved.changes.iter().all(|c| c.endpoint.path == "/v2/items"));
}

#[test]
fn moved_ids_are_deterministic_and_v2_summary_counts_the_move_as_changed() {
    let old = parse(&paging_old("/v1/items", "listItems"));
    let new = parse(&paging_new("/v2/items", "listItems"));
    let a =
        contract::json::openapi_with(&old, &new, None, "", "", EndpointMatching::Moves).unwrap();
    let b =
        contract::json::openapi_with(&old, &new, None, "", "", EndpointMatching::Moves).unwrap();
    let ids =
        |d: &contract::json::JsonDiff| d.changes.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&a), ids(&b));
    assert_eq!(a.summary.operations_added, 0);
    assert_eq!(a.summary.operations_removed, 0);
    assert_eq!(a.summary.operations_changed, 1);
    assert_eq!(a.summary.breaking, 1);
}

// ── #138: matrix 2 ─────────────────────────────────────────────────────────

fn pagination(old: &Value, new: &Value) -> Option<contract::Change> {
    neutral(old, new, EndpointMatching::Moves)
        .into_iter()
        .find(|c| c.kind == ChangeKind::PaginationChanged)
}

fn detected(params: &[&str], continuation: &str) -> Option<contract::Change> {
    pagination(
        &spec("/items", "listItems", &[], None),
        &spec("/items", "listItems", params, Some(continuation)),
    )
}

#[test]
fn matrix_2_every_row_is_detected_and_review_only() {
    let rows: &[(&str, &[&str], &str)] = &[
        ("next_cursor", &["cursor", "limit"], "next_cursor"),
        ("nextCursor", &["cursor", "limit"], "nextCursor"),
        (
            "next_page_token",
            &["page_token", "page_size"],
            "next_page_token",
        ),
        ("next_page", &["page"], "next_page"),
        ("cursor", &["cursor", "limit"], "cursor"),
        ("next", &["cursor", "limit"], "next"),
        ("after_cursor", &["cursor", "limit"], "after_cursor"),
        ("after", &["cursor", "limit"], "after"),
        (
            "continuation_token",
            &["cursor", "limit"],
            "continuation_token",
        ),
        (
            "continuationToken",
            &["cursor", "limit"],
            "continuationToken",
        ),
        ("nextLink", &["cursor", "limit"], "nextLink"),
        // The generator nests dotted names, so this is `@odata` / `nextLink`.
        ("@odata.nextLink", &["cursor", "limit"], "@odata/nextLink"),
        ("LastEvaluatedKey", &["cursor", "limit"], "LastEvaluatedKey"),
        (
            "response_metadata.next_cursor",
            &["cursor", "limit"],
            "response_metadata/next_cursor",
        ),
        ("links.next", &["cursor", "limit"], "links/next"),
        ("next_cursor", &["starting_after", "limit"], "next_cursor"),
        ("next_cursor", &["after", "limit"], "next_cursor"),
    ];
    for (continuation, params, path) in rows {
        let change = detected(params, continuation)
            .unwrap_or_else(|| panic!("{continuation} with {params:?} not detected"));
        assert_eq!(change.severity, Severity::Review, "{continuation}");
        assert_eq!(change.native_kind, "pagination_inferred");
        assert_eq!(change.details["confidence"], "inferred", "{continuation}");
        assert_eq!(
            change.details["after"]["responses"][0]["continuation"],
            json!([path]),
            "{continuation}"
        );
    }
}

#[test]
fn generic_names_record_their_corroboration() {
    let change = detected(&["cursor", "limit"], "cursor").unwrap();
    assert_eq!(
        change.details["evidence_rule"],
        "list+request_controls+generic_continuation+token_control"
    );
    assert_eq!(
        change.details["after"]["responses"][0]["generic_continuation"],
        json!(["cursor"])
    );
    let specific = detected(&["cursor", "limit"], "next_cursor").unwrap();
    assert_eq!(
        specific.details["evidence_rule"],
        "list+request_controls+continuation"
    );
    // Specific-only evidence keeps its 2.0.0 shape (and so its IDs).
    assert_eq!(
        specific.details["after"]["responses"],
        json!([{"status": "200", "continuation": ["next_cursor"]}])
    );
}

#[test]
fn top_level_odata_next_link_matches_ignoring_separators_and_case() {
    for name in ["@odata.nextLink", "NEXT-PAGE-TOKEN", "Next_Link"] {
        let mut new = spec("/items", "listItems", &["limit"], None);
        new["paths"]["/items"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
            ["properties"][name] = json!({"type": "string"});
        let change = pagination(&spec("/items", "listItems", &[], None), &new)
            .unwrap_or_else(|| panic!("{name}"));
        assert_eq!(
            change.details["after"]["responses"][0]["continuation"],
            json!([name])
        );
    }
}

#[test]
fn end_flags_corroborate_and_raise_confidence() {
    let with_flag = |continuation: &str, flag: &str, params: &[&str]| {
        let mut new = spec("/items", "listItems", params, Some(continuation));
        new["paths"]["/items"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
            ["properties"][flag] = json!({"type": "boolean"});
        pagination(&spec("/items", "listItems", &[], None), &new)
    };
    // A specific name 2.0.0 did not know plus an end flag is corroborated.
    let change = with_flag("nextLink", "has_more", &["cursor", "limit"]).unwrap();
    assert_eq!(change.details["confidence"], "corroborated");
    assert_eq!(
        change.details["evidence_rule"],
        "list+request_controls+continuation+end_flag"
    );
    assert_eq!(
        change.details["after"]["responses"][0]["end_flags"],
        json!(["has_more"])
    );
    // A generic name with only a size control needs the end flag.
    let change = with_flag("cursor", "hasMore", &["limit"]).unwrap();
    assert_eq!(change.details["confidence"], "inferred");
    assert_eq!(
        change.details["evidence_rule"],
        "list+request_controls+generic_continuation+end_flag"
    );
    for flag in ["end_of_stream", "is_last", "complete", "drained"] {
        assert!(with_flag("cursor", flag, &["limit"]).is_some(), "{flag}");
    }
    // The payouts export: generic `cursor`, token control and `drained`.
    let change = with_flag("cursor", "drained", &["cursor", "limit"]).unwrap();
    assert_eq!(change.details["confidence"], "corroborated");
    // A boolean not named as an end flag corroborates nothing.
    assert!(with_flag("cursor", "archived", &["limit"]).is_none());
    // A nested end flag counts too.
    let mut new = spec("/items", "listItems", &["limit"], Some("cursor"));
    new["paths"]["/items"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]["properties"]
        ["meta"] = json!({"type": "object", "properties": {"has_more": {"type": "boolean"}}});
    let change = pagination(&spec("/items", "listItems", &[], None), &new).unwrap();
    assert_eq!(
        change.details["after"]["responses"][0]["end_flags"],
        json!(["meta/has_more"])
    );
}

#[test]
fn generic_or_partial_evidence_alone_is_not_pagination() {
    // A lone `next` string with no request controls at all.
    assert!(detected(&[], "next").is_none());
    // `next` / `after` / `cursor` with only size controls and no end flag.
    for name in ["next", "after", "cursor", "links.next"] {
        assert!(detected(&["limit", "offset"], name).is_none(), "{name}");
    }
    // An unrelated `after` timestamp, even beside a token control.
    let mut new = spec("/items", "listItems", &["cursor", "limit"], None);
    new["paths"]["/items"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]["properties"]
        ["after"] = json!({"type": "string", "format": "date-time"});
    assert!(pagination(&spec("/items", "listItems", &[], None), &new).is_none());
    // A non-string `next` (a page number) is not a generic continuation.
    let mut new = spec("/items", "listItems", &["cursor", "limit"], None);
    new["paths"]["/items"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]["properties"]
        ["next"] = json!({"type": "integer"});
    assert!(pagination(&spec("/items", "listItems", &[], None), &new).is_none());
    // An end flag without a continuation field.
    let mut new = spec("/items", "listItems", &["cursor", "limit"], None);
    new["paths"]["/items"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]["properties"]
        ["has_more"] = json!({"type": "boolean"});
    assert!(pagination(&spec("/items", "listItems", &[], None), &new).is_none());
    // Continuations two levels down are out of reach.
    let mut new = spec("/items", "listItems", &["cursor", "limit"], None);
    new["paths"]["/items"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]["properties"]
        ["meta"] = json!({"type": "object", "properties": {"paging":
            {"type": "object", "properties": {"next_cursor": {"type": "string"}}}}});
    assert!(pagination(&spec("/items", "listItems", &[], None), &new).is_none());
}

#[test]
fn nested_continuations_resolve_refs() {
    let mut new = spec("/items", "listItems", &["cursor", "limit"], None);
    new["paths"]["/items"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]["properties"]
        ["pagination"] = json!({"$ref": "#/components/schemas/Paging"});
    new["components"]["schemas"]["Paging"] =
        json!({"type": "object", "properties": {"next_cursor": {"type": "string"}}});
    let change = pagination(&spec("/items", "listItems", &[], None), &new).unwrap();
    assert_eq!(
        change.details["after"]["responses"][0]["continuation"],
        json!(["pagination/next_cursor"])
    );
}

// ── #138: 2.0.0 stability ─────────────────────────────────────────────────

/// `GET /items` returning `items` plus `properties`, with query `params`.
fn paged(params: &[&str], properties: Value) -> Value {
    let mut spec = spec("/items", "listItems", params, None);
    let schema = &mut spec["paths"]["/items"]["get"]["responses"]["200"]["content"]["application/json"]
        ["schema"]["properties"];
    for (name, value) in properties.as_object().unwrap() {
        schema[name] = value.clone();
    }
    spec
}

/// Every fact of a v2 JSON diff as serialised, for byte-level comparison.
fn v2_changes(old: &Value, new: &Value) -> Value {
    let document = contract::json::openapi_with(
        &parse(old),
        &parse(new),
        None,
        "",
        "",
        EndpointMatching::Moves,
    )
    .unwrap();
    serde_json::to_value(document.changes).unwrap()
}

fn pagination_fact(old: &Value, new: &Value) -> Value {
    let changes = v2_changes(old, new);
    let facts: Vec<&Value> = changes
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "pagination_changed")
        .collect();
    assert_eq!(facts.len(), 1, "{changes:#}");
    facts[0].clone()
}

#[test]
fn evidence_2_0_0_detected_keeps_its_shape_and_id_with_end_flags() {
    // The expected fact is the 2.0.0 binary's output for this pair: the end
    // flag adds no evidence, confidence or rule suffix, so the ID is stable.
    let response = json!({"nextPageToken": {"type": "string"}, "hasMore": {"type": "boolean"}});
    let fact = pagination_fact(&paged(&[], response.clone()), &paged(&["limit"], response));
    assert_eq!(
        fact["id"],
        "vc2_ec029af7f092f956b2e3b22b436296ddfabeb85ad7a51a41050d5c7cce25e95a"
    );
    assert_eq!(
        fact["details"],
        json!({
            "before": null,
            "after": {
                "controls": [{"name": "limit", "schema": {"type": "integer"}}],
                "responses": [{"status": "200", "continuation": ["nextPageToken"]}]
            },
            "confidence": "inferred",
            "evidence_rule": "list+request_controls+continuation"
        })
    );
}

#[test]
fn evidence_2_0_0_detected_ignores_extra_nested_and_generic_names() {
    // 2.0.0 matched `nextCursor`; nested continuations, a generic `next`, a
    // nested end flag and a top-level one are all left out (2.0.0 output).
    let response = json!({
        "nextCursor": {"type": "string"},
        "next": {"type": "string"},
        "links": {"type": "object", "properties": {
            "next": {"type": "string"}, "nextLink": {"type": "string"}}},
        "meta": {"type": "object", "properties": {
            "has_more": {"type": "boolean"}, "next_cursor": {"type": "string"}}},
        "end_of_stream": {"type": "boolean"}
    });
    let fact = pagination_fact(
        &paged(&[], response.clone()),
        &paged(&["cursor", "limit"], response),
    );
    assert_eq!(
        fact["id"],
        "vc2_b61ce4b5543179675e559218b9fdabe815f1235572e49bca87625ae08228597e"
    );
    assert_eq!(
        fact["details"],
        json!({
            "before": null,
            "after": {
                "controls": [
                    {"name": "cursor", "schema": {"type": "string"}},
                    {"name": "limit", "schema": {"type": "integer"}}
                ],
                "responses": [{"status": "200", "continuation": ["nextCursor"]}]
            },
            "confidence": "inferred",
            "evidence_rule": "list+request_controls+continuation"
        })
    );
}

#[test]
fn evidence_2_0_0_detected_on_both_sides_ignores_new_end_flags() {
    // 2.0.0 saw identical evidence on both sides, so no fact; adding an end
    // flag and a nested `links/next` must not create one now.
    let old = paged(
        &["page_token", "page_size"],
        json!({"next_page_token": {"type": "string"}}),
    );
    let new = paged(
        &["page_token", "page_size"],
        json!({
            "next_page_token": {"type": "string"},
            "has_more": {"type": "boolean"},
            "links": {"type": "object", "properties": {"next": {"type": "string"}}}
        }),
    );
    let changes = v2_changes(&old, &new);
    assert!(
        !changes
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["kind"] == "pagination_changed"),
        "{changes:#}"
    );
}

#[test]
fn evidence_2_0_0_on_one_side_with_broader_evidence_on_the_other_changes_the_id() {
    // Intentional: stability covers a side the 2.0.0 rule matches, not a side
    // only the broader rules match. 2.0.0 saw pagination removed (`after:
    // null`, vc2_ae366567…) when `nextPageToken` became `next_token` or moved
    // under `meta`; the broader rules now record the new side's evidence, so
    // the fact and its ID differ from 2.0.0's.
    let old = paged(
        &["page_token"],
        json!({"nextPageToken": {"type": "string"}}),
    );
    let before = json!({
        "controls": [{"name": "page_token", "schema": {"type": "string"}}],
        "responses": [{"status": "200", "continuation": ["nextPageToken"]}]
    });
    for (properties, continuation, id) in [
        (
            json!({"next_token": {"type": "string"}}),
            "next_token",
            "vc2_5bb27b8d7b34ba3ec84f7eb51f6916e7a94e7ac12c91bf6b6e06880e27cbd16b",
        ),
        (
            json!({"meta": {"type": "object", "properties": {
                "nextPageToken": {"type": "string"}}}}),
            "meta/nextPageToken",
            "vc2_bddb40e8e778fb9a042fbd9ec408e9d3b1f00aca304bed495e50286654e2d9d4",
        ),
    ] {
        let fact = pagination_fact(&old, &paged(&["page_token"], properties));
        assert_ne!(
            fact["id"],
            "vc2_ae366567f999c18ace88f1723cf06edcd8a0a283b8c44da1aa8d1932d6b5973d"
        );
        assert_eq!(fact["id"], id);
        assert_eq!(
            fact["details"],
            json!({
                "before": before,
                "after": {
                    "controls": [{"name": "page_token", "schema": {"type": "string"}}],
                    "responses": [{"status": "200", "continuation": [continuation]}]
                },
                "confidence": "inferred",
                "evidence_rule": "list+request_controls+continuation"
            })
        );
    }
}

// ── #137: uniqueness across the whole document ────────────────────────────

#[test]
fn version_key_shared_with_an_exact_match_pairs_nothing() {
    // Old {v1, v2}, new {v2, v3}: v2 matches exactly, but `GET /v*/items` is
    // not unique in either document, so v1 → v3 is not a move.
    let old = merge(&[
        paging_old("/v1/items", "listV1"),
        paging_old("/v2/items", "listV2"),
    ]);
    let new = merge(&[
        paging_old("/v2/items", "listV2"),
        paging_old("/v3/items", "listV3"),
    ]);
    let legacy = diff::diff_with(&parse(&old), &parse(&new), EndpointMatching::Moves);
    assert_eq!(legacy.endpoints_moved(), 0);
    assert_eq!(legacy.endpoints_removed(), 1);
    assert_eq!(legacy.endpoints_added(), 1);
    let changes = neutral(&old, &new, EndpointMatching::Moves);
    assert!(!kinds(&changes).contains(&ChangeKind::OperationMoved));
}

#[test]
fn operation_id_duplicated_anywhere_in_a_document_pairs_nothing() {
    // `x` is held by `/a` (an exact match) and `/old` in the old document; the
    // leftover `/old` must not pair with the new `/new` by that operationId.
    let old = merge(&[paging_old("/a", "x"), paging_old("/old", "x")]);
    let new = merge(&[paging_old("/a", "y"), paging_old("/new", "x")]);
    let legacy = diff::diff_with(&parse(&old), &parse(&new), EndpointMatching::Moves);
    assert_eq!(legacy.endpoints_moved(), 0);
    // And the mirror image on the new side.
    let legacy = diff::diff_with(&parse(&new), &parse(&old), EndpointMatching::Moves);
    assert_eq!(legacy.endpoints_moved(), 0);
}

// ── #137: subjects resolve in the surface they name ───────────────────────

#[test]
fn moved_operation_facts_resolve_in_their_own_surface() {
    // Removed: query `q`, response property `a`, response `404`, enum value
    // `B`. Added: query `limit`, response property `b`. Changed: `mode` type.
    let side = |path: &str, params: Value, properties: Value, extra: Value| {
        let mut responses = json!({"200": {"description": "ok", "content": {"application/json": {
            "schema": {"type": "object", "properties": properties}}}}});
        for (status, response) in extra.as_object().unwrap() {
            responses[status] = response.clone();
        }
        json!({
            "openapi": "3.0.3",
            "info": {"title": "Items", "version": "1"},
            "paths": {path: {"get": {"operationId": "listItems", "parameters": params,
                "responses": responses}}}
        })
    };
    let old = side(
        "/v1/items",
        json!([
            {"name": "q", "in": "query", "schema": {"type": "string"}},
            {"name": "mode", "in": "query", "schema": {"type": "string", "enum": ["A", "B"]}}
        ]),
        json!({"a": {"type": "string"}, "c": {"type": "string"}}),
        json!({"404": {"description": "missing"}}),
    );
    let new = side(
        "/v2/items",
        json!([
            {"name": "limit", "in": "query", "schema": {"type": "integer"}},
            {"name": "mode", "in": "query", "schema": {"type": "integer", "enum": ["A"]}}
        ]),
        json!({"b": {"type": "string"}, "c": {"type": "string"}}),
        json!({}),
    );
    let changes = neutral(&old, &new, EndpointMatching::Moves);
    let old_surface = contract::openapi::adapt(&parse(&old));
    let new_surface = contract::openapi::adapt(&parse(&new));
    let in_surface =
        |surface: &contract::Contract, id: &str| surface.entities.iter().any(|e| e.id == id);
    let path_of = |change: &contract::Change| match &change.native {
        NativeLocation::Openapi { path, .. } => path.clone(),
        other => panic!("{other:?}"),
    };
    let mut removals = 0;
    for change in &changes {
        let removal = matches!(
            change.kind,
            ChangeKind::ArgumentRemoved
                | ChangeKind::ResultRemoved
                | ChangeKind::FieldRemoved
                | ChangeKind::EnumValueRemoved
        );
        if removal {
            removals += 1;
            assert!(in_surface(&old_surface, &change.subject), "{change:?}");
            assert!(!in_surface(&new_surface, &change.subject), "{change:?}");
            assert_eq!(path_of(change).as_deref(), Some("/v1/items"), "{change:?}");
        } else {
            assert!(in_surface(&new_surface, &change.subject), "{change:?}");
            assert_eq!(path_of(change).as_deref(), Some("/v2/items"), "{change:?}");
        }
    }
    let kinds = kinds(&changes);
    for kind in [
        ChangeKind::OperationMoved,
        ChangeKind::ArgumentRemoved,
        ChangeKind::ArgumentAdded,
        ChangeKind::ResultRemoved,
        ChangeKind::FieldRemoved,
        ChangeKind::FieldAdded,
        ChangeKind::EnumValueRemoved,
        ChangeKind::ArgumentTypeChanged,
    ] {
        assert!(kinds.contains(&kind), "{kind:?} missing from {kinds:?}");
    }
    assert!(removals >= 4);
    // The removal facts carry exactly the exact-mode subjects and IDs that an
    // unmoved comparison of the same change produces at the old path.
    let unmoved_new = side(
        "/v1/items",
        new["paths"]["/v2/items"]["get"]["parameters"].clone(),
        json!({"b": {"type": "string"}, "c": {"type": "string"}}),
        json!({}),
    );
    let exact = neutral(&old, &unmoved_new, EndpointMatching::Exact);
    let removed_ids = |changes: &[contract::Change]| -> Vec<String> {
        changes
            .iter()
            .filter(|c| {
                matches!(
                    c.kind,
                    ChangeKind::ArgumentRemoved
                        | ChangeKind::ResultRemoved
                        | ChangeKind::FieldRemoved
                        | ChangeKind::EnumValueRemoved
                )
            })
            .map(|c| c.id.clone())
            .collect()
    };
    assert_eq!(removed_ids(&changes), removed_ids(&exact));
}
