use serde_json::{Value, json};
use vimanam::contract::{self, ChangeKind, EntityKind, Format, NativeLocation, Severity};
fn parse(value: &Value) -> vimanam::ApiDocumentation {
    vimanam::parse_openapi_bytes(&serde_json::to_vec(value).unwrap(), "json", None).unwrap()
}
fn spec() -> Value {
    json!({"openapi":"3.0.3","info":{"title":"Test","version":"1"},"paths":{"/widgets":{"get":{"operationId":"list","parameters":[],"responses":{"200":{"description":"ok","content":{"application/json":{"schema":{"$ref":"#/components/schemas/Page"}}}}}}}},"components":{"schemas":{"Page":{"type":"object","properties":{"items":{"type":"array","items":{"$ref":"#/components/schemas/Widget"}}}},"Widget":{"type":"object","properties":{"a/b":{"type":"string"},"self":{"$ref":"#/components/schemas/Widget"}},"enum":[null,"x"]},"Unused":{"type":"object","properties":{"id":{"type":"string"}}}}}})
}
#[test]
fn surface_has_all_entities_recursive_edges_and_stable_identities() {
    let api = parse(&spec());
    let surface = contract::openapi::adapt(&api);
    assert_eq!(surface.format, Format::Openapi);
    for kind in [
        EntityKind::Service,
        EntityKind::Operation,
        EntityKind::Type,
        EntityKind::Field,
        EntityKind::Result,
        EntityKind::Value,
    ] {
        assert!(surface.entities.iter().any(|e| e.kind == kind));
    }
    let widget = surface
        .entities
        .iter()
        .find(|e| e.kind == EntityKind::Type && e.name == "Widget")
        .unwrap();
    assert!(widget.references.contains(&widget.id));
    let field = surface.entities.iter().find(|e| e.name == "a/b").unwrap();
    assert!(
        matches!(&field.native,NativeLocation::Openapi{pointer,..} if pointer=="/properties/a~1b")
    );
    let mut changed = spec();
    changed["info"]["version"] = json!("2");
    changed["components"]["schemas"]["Widget"]["description"] = json!("new docs");
    let other = contract::openapi::adapt(&parse(&changed));
    assert_eq!(
        surface.entities.iter().map(|e| &e.id).collect::<Vec<_>>(),
        other.entities.iter().map(|e| &e.id).collect::<Vec<_>>()
    );
    let encoded = serde_json::to_vec(&surface).unwrap();
    assert_eq!(surface, serde_json::from_slice(&encoded).unwrap());
}
#[test]
fn independent_types_fields_defaults_deprecation_and_enum_are_neutral_facts() {
    let old = spec();
    let mut new = old.clone();
    new["components"]["schemas"]["Unused"]["properties"]["id"] =
        json!({"type":"integer","default":null,"deprecated":true});
    new["components"]["schemas"]["Widget"]["enum"] = json!([null, "x", "y"]);
    let changes = contract::openapi::diff(&parse(&old), &parse(&new));
    for kind in [
        ChangeKind::FieldTypeChanged,
        ChangeKind::DefaultChanged,
        ChangeKind::DeprecatedChanged,
        ChangeKind::EnumValueAdded,
    ] {
        assert!(changes.iter().any(|c| c.kind == kind), "missing {kind:?}");
    }
    assert!(
        changes
            .iter()
            .filter(|c| matches!(&c.native, NativeLocation::Openapi { method: None, .. }))
            .all(|c| c.severity == Severity::Review)
    );
    assert!(
        changes
            .iter()
            .any(|c| c.details["after"] == json!({"present":true,"value":null}))
    );
    let mut annotations = old.clone();
    annotations["components"]["schemas"]["Unused"]["description"] = json!("docs");
    assert!(contract::openapi::diff(&parse(&old), &parse(&annotations)).is_empty());
}
#[test]
fn pagination_requires_joint_evidence_and_remains_review() {
    let old = spec();
    let mut new = old.clone();
    new["paths"]["/widgets"]["get"]["parameters"] =
        json!([{"name":"pageToken","in":"query","schema":{"type":"string"}}]);
    assert!(
        !contract::openapi::diff(&parse(&old), &parse(&new))
            .iter()
            .any(|c| c.kind == ChangeKind::PaginationChanged)
    );
    new["components"]["schemas"]["Page"]["properties"]["nextPageToken"] = json!({"type":"string"});
    let changes = contract::openapi::diff(&parse(&old), &parse(&new));
    let pagination = changes
        .iter()
        .find(|c| c.kind == ChangeKind::PaginationChanged)
        .unwrap();
    assert_eq!(pagination.severity, Severity::Review);
    let mut docs = new.clone();
    docs["paths"]["/widgets"]["get"]["parameters"][0]["schema"]["description"] = json!("docs");
    assert!(
        !contract::openapi::diff(&parse(&new), &parse(&docs))
            .iter()
            .any(|c| c.kind == ChangeKind::PaginationChanged)
    );
    let mut default = new.clone();
    default["paths"]["/widgets"]["get"]["parameters"][0]["schema"]["default"] = json!("first");
    assert!(
        contract::openapi::diff(&parse(&new), &parse(&default))
            .iter()
            .any(|c| c.kind == ChangeKind::PaginationChanged)
    );
}
#[test]
fn vc2_ids_are_reproducible_and_ignore_verdicts() {
    let old = spec();
    let mut new = old.clone();
    new["paths"]["/widgets"]["get"]["operationId"] = json!("renamed");
    let mut changes = contract::openapi::diff(&parse(&old), &parse(&new));
    let c = changes.first_mut().unwrap();
    assert!(c.id.starts_with("vc2_"));
    let id = c.id.clone();
    c.severity = Severity::Breaking;
    c.impacts = vec!["wire".into()];
    assert_eq!(id, contract::change_id(c));
    c.details["new"] = json!("different");
    assert_ne!(id, contract::change_id(c));
}
#[test]
fn generic_adapter_has_no_http_requirement_and_owns_directional_verdict() {
    struct Graphql;
    impl contract::Adapter for Graphql {
        fn attribute_kind(
            &self,
            _: &contract::Entity,
            pointer: &str,
            _: &vimanam::diff::ValueChangeKind,
        ) -> ChangeKind {
            assert_eq!(pointer, "/nullable");
            ChangeKind::NullabilityChanged
        }
        fn classify(
            &self,
            _: &contract::Entity,
            kind: ChangeKind,
            _: &Value,
        ) -> (Severity, Vec<String>) {
            assert_eq!(kind, ChangeKind::NullabilityChanged);
            (Severity::Breaking, vec!["source".into()])
        }
    }
    let entity = contract::Entity {
        id: contract::entity_id(Format::Graphql, EntityKind::Field, "Widget.name"),
        kind: EntityKind::Field,
        name: "name".into(),
        parent: None,
        services: vec![],
        native: NativeLocation::Graphql {
            coordinate: "Widget.name".into(),
        },
        attributes: json!({"nullable":false}),
        references: vec![],
    };
    let old = contract::Contract {
        format: Format::Graphql,
        title: "Test".into(),
        version: "1".into(),
        entities: vec![entity],
    };
    let mut new = old.clone();
    new.entities[0].attributes["nullable"] = json!(true);
    let changes = contract::compare(&old, &new, &Graphql).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].severity, Severity::Breaking);
    new.format = Format::Protobuf;
    assert!(contract::compare(&old, &new, &Graphql).is_err());
}

#[test]
fn escaped_named_refs_compositions_and_mixed_usage_keep_coherent_ids() {
    let mut value = spec();
    value["components"]["schemas"]["A/B~C"] = json!({"type":"object","properties":{"~field/":{"type":"string"}},"allOf":[{"properties":{"a":{"type":"string"}}}],"oneOf":[{"properties":{"b":{"type":"integer"}}}],"additionalProperties":{"properties":{"c":{"type":"boolean"}}}});
    value["components"]["schemas"]["Widget"]["properties"]["escaped"] =
        json!({"$ref":"#/components/schemas/A~1B~0C"});
    value["paths"]["/widgets"]["get"]["parameters"] =
        json!([{"name":"filter","in":"query","schema":{"$ref":"#/components/schemas/A~1B~0C"}}]);
    let old = parse(&value);
    let surface = contract::openapi::adapt(&old);
    let ty = surface
        .entities
        .iter()
        .find(|e| e.kind == EntityKind::Type && e.name == "A/B~C")
        .unwrap();
    assert!(
        surface
            .entities
            .iter()
            .filter(|e| e.references.contains(&ty.id))
            .count()
            >= 2
    );
    for name in ["~field/", "a", "b", "c"] {
        assert!(
            surface
                .entities
                .iter()
                .any(|e| e.kind == EntityKind::Field && e.name == name)
        );
    }
    assert!(surface.entities.iter().any(|e|matches!(&e.native,NativeLocation::Openapi{pointer,..} if pointer=="/properties/~0field~1")));
    let mut new = value.clone();
    new["components"]["schemas"]["A/B~C"]["properties"]["~field/"]["type"] = json!("integer");
    let facts = contract::openapi::diff(&old, &parse(&new));
    assert!(
        facts
            .iter()
            .any(|c| c.kind == ChangeKind::ArgumentTypeChanged && c.severity == Severity::Breaking)
    );
    assert!(
        facts
            .iter()
            .filter(|c| c.subject == ty.id)
            .all(|c| c.severity == Severity::Review)
    );
    let ids: std::collections::BTreeSet<_> = facts.iter().map(|c| &c.id).collect();
    assert_eq!(ids.len(), facts.len());
}

#[test]
fn duplicate_surface_id_is_rejected_instead_of_ambiguous_matching() {
    let surface = contract::openapi::adapt(&parse(&spec()));
    let mut duplicate = surface.clone();
    duplicate.entities.push(duplicate.entities[0].clone());
    assert!(
        contract::compare(&surface, &duplicate, &contract::openapi::OpenApiAdapter)
            .unwrap_err()
            .to_string()
            .contains("Duplicate contract entity ID")
    );
    assert!(contract::compare(&duplicate, &surface, &contract::openapi::OpenApiAdapter).is_err());
}

#[test]
fn defaults_preserve_arbitrary_object_keys_and_v2_roundtrips() {
    let old = spec();
    let mut new = old.clone();
    new["components"]["schemas"]["Unused"]["properties"]["id"]["default"] =
        json!({"description":"a","enum":["x","y"]});
    let first = parse(&new);
    new["components"]["schemas"]["Unused"]["properties"]["id"]["default"]["description"] =
        json!("b");
    assert!(
        contract::openapi::diff(&first, &parse(&new))
            .iter()
            .any(|c| c.details["pointer"] == "/properties/id/default/description")
    );
    let document = contract::json::openapi(&parse(&old), &parse(&new), None, "old", "new");
    let bytes = serde_json::to_vec(&document).unwrap();
    let decoded: contract::json::JsonDiff = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(bytes, serde_json::to_vec(&decoded).unwrap());
}

#[test]
fn generic_comparison_reports_reference_and_native_wire_changes() {
    struct Proto;
    impl contract::Adapter for Proto {
        fn structural_kind(&self, _: &contract::Entity, field: &str) -> Option<ChangeKind> {
            Some(if field == "native" {
                ChangeKind::WireIdentityChanged
            } else {
                ChangeKind::ConstraintChanged
            })
        }
        fn classify(
            &self,
            _: &contract::Entity,
            _: ChangeKind,
            _: &Value,
        ) -> (Severity, Vec<String>) {
            (Severity::Review, vec!["wire".into()])
        }
    }
    let id = contract::entity_id(Format::Protobuf, EntityKind::Field, "pkg.Message.id");
    let entity = contract::Entity {
        id,
        kind: EntityKind::Field,
        name: "id".into(),
        parent: None,
        services: vec![],
        native: NativeLocation::Protobuf {
            full_name: "pkg.Message.id".into(),
            number: Some(1),
        },
        attributes: json!({"type":"int32"}),
        references: vec!["ve2_type_a".into()],
    };
    let old = contract::Contract {
        format: Format::Protobuf,
        title: "T".into(),
        version: "1".into(),
        entities: vec![entity.clone()],
    };
    let mut new = old.clone();
    new.entities[0].native = NativeLocation::Protobuf {
        full_name: "pkg.Message.id".into(),
        number: Some(2),
    };
    new.entities[0].references = vec!["ve2_type_b".into()];
    let changes = contract::compare(&old, &new, &Proto).unwrap();
    assert_eq!(changes.len(), 2);
    assert!(
        changes
            .iter()
            .any(|c| c.kind == ChangeKind::WireIdentityChanged
                && c.details["structural"] == "native")
    );
    assert!(
        changes
            .iter()
            .any(|c| c.details["structural"] == "references"
                && c.details["before"] == json!(["ve2_type_a"]))
    );
}

#[test]
fn inline_deprecation_and_swagger_query_defaults_and_constraints_are_v2_facts() {
    let mut old = spec();
    old["paths"]["/widgets"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
        ["properties"] = json!({"legacy":{"type":"string","deprecated":false}});
    let mut new = old.clone();
    new["paths"]["/widgets"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
        ["properties"]["legacy"]["deprecated"] = json!(true);
    let deprecated = contract::openapi::diff(&parse(&old), &parse(&new));
    let deprecated: Vec<_> = deprecated
        .iter()
        .filter(|c| c.kind == ChangeKind::DeprecatedChanged)
        .collect();
    assert_eq!(
        deprecated.len(),
        1,
        "one inline schema change has one operation use fact"
    );
    assert_eq!(deprecated[0].severity, Severity::Review);
    assert!(
        matches!(&deprecated[0].native, NativeLocation::Openapi { status:Some(_), pointer, .. } if pointer == "/properties/legacy/deprecated")
    );

    let mut ref_old = spec();
    ref_old["paths"]["/widgets"]["get"]["responses"]["200"]["content"]["application/json"]["schema"] =
        json!({"$ref":"#/components/schemas/Shared"});
    ref_old["components"]["schemas"]["Shared"] =
        json!({"type":"object","properties":{"legacy":{"type":"string","deprecated":false}}});
    let mut ref_new = ref_old.clone();
    ref_new["components"]["schemas"]["Shared"]["properties"]["legacy"]["deprecated"] = json!(true);
    let ref_changes = contract::openapi::diff(&parse(&ref_old), &parse(&ref_new));
    let ref_deprecations: Vec<_> = ref_changes
        .iter()
        .filter(|c| c.kind == ChangeKind::DeprecatedChanged)
        .collect();
    assert_eq!(
        ref_deprecations.len(),
        2,
        "one independent named-type fact and one operation use fact"
    );
    assert!(ref_deprecations.iter().any(|c| matches!(
        &c.native,
        NativeLocation::Openapi {
            method: Some(_),
            status: Some(_),
            resolved: true,
            ..
        }
    )));
    assert!(ref_deprecations.iter().any(|c| matches!(&c.native, NativeLocation::Openapi { method:None, pointer, .. } if pointer.starts_with("/types/Shared"))));

    let swagger = |default: i64, maximum: i64| {
        json!({
            "swagger":"2.0","info":{"title":"T","version":"1"},"paths":{"/items":{"get":{
                "parameters":[{"name":"limit","in":"query","type":"integer","default":default,"maximum":maximum},{"name":"X-Limit","in":"header","type":"integer","default":default,"maximum":maximum}],
                "responses":{"200":{"description":"ok"}}
            }}},"definitions":{}
        })
    };
    let old = parse(&swagger(10, 100));
    let new = parse(&swagger(20, 50));
    let changes = contract::openapi::diff(&old, &new);
    assert!(
        changes
            .iter()
            .any(|c| c.kind == ChangeKind::DefaultChanged && c.details["after"]["value"] == 20)
    );
    assert!(
        changes
            .iter()
            .any(|c| c.kind == ChangeKind::ConstraintChanged
                && c.details["pointer"].as_str().unwrap().ends_with("/maximum"))
    );
    assert!(changes.iter().any(|c| c.kind == ChangeKind::DefaultChanged && matches!(&c.native, NativeLocation::Openapi { parameter_in:Some(p), .. } if p == "header")));
}

#[test]
fn schema_keyword_detection_and_not_annotations_follow_schema_grammar() {
    let old = spec();
    let mut new = old.clone();
    new["components"]["schemas"]["Unused"]["properties"]["deprecated"] = json!({"type":"string"});
    let changes = contract::openapi::diff(&parse(&old), &parse(&new));
    assert!(changes.iter().any(|c| c.kind == ChangeKind::FieldAdded));
    assert!(
        !changes
            .iter()
            .any(|c| c.kind == ChangeKind::DeprecatedChanged)
    );

    let mut before = old.clone();
    before["components"]["schemas"]["Unused"]["not"] =
        json!({"type":"string","description":"before"});
    let mut after = before.clone();
    after["components"]["schemas"]["Unused"]["not"]["description"] = json!("after");
    assert!(contract::openapi::diff(&parse(&before), &parse(&after)).is_empty());
}

#[test]
fn operation_service_membership_changes_but_membership_order_does_not() {
    let document = |move_tags: Value| {
        json!({
            "openapi":"3.0.3","info":{"title":"T","version":"1"},"paths":{
                "/move":{"get":{"tags":move_tags,"responses":{"200":{"description":"ok"}}}},
                "/alpha":{"get":{"tags":["Alpha"],"responses":{"200":{"description":"ok"}}}},
                "/beta":{"get":{"tags":["Beta"],"responses":{"200":{"description":"ok"}}}}
            }
        })
    };
    let old = parse(&document(json!(["Alpha"])));
    let new = parse(&document(json!(["Beta"])));
    let changes = contract::openapi::diff(&old, &new);
    let moved: Vec<_> = changes
        .iter()
        .filter(|c| c.details.get("structural") == Some(&json!("services")))
        .collect();
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].kind, ChangeKind::ConstraintChanged);
    assert_eq!(
        moved[0].details["before"],
        json!([contract::entity_id(
            Format::Openapi,
            EntityKind::Service,
            "/services/Alpha"
        )])
    );
    assert!(
        matches!(&moved[0].native, NativeLocation::Openapi { method:Some(_), path:Some(path), .. } if path == "/move")
    );

    let old = parse(&document(json!(["Alpha", "Beta"])));
    let reordered = parse(&document(json!(["Beta", "Alpha"])));
    assert!(
        !contract::openapi::diff(&old, &reordered)
            .iter()
            .any(|c| c.details.get("structural") == Some(&json!("services")))
    );
}

#[test]
fn nested_request_and_response_deprecations_emit_one_fact_per_use() {
    let mut old = spec();
    old["paths"]["/widgets"]["get"]["requestBody"] = json!({"content":{"application/json":{"schema":{"type":"object","properties":{"input":{"type":"object","properties":{"nested":{"type":"string","deprecated":false}}}}}}}});
    old["paths"]["/widgets"]["get"]["responses"]["200"]["content"]["application/json"]["schema"] = json!({"type":"object","properties":{"output":{"type":"object","properties":{"nested":{"type":"string","deprecated":false}}}}});
    let mut new = old.clone();
    new["paths"]["/widgets"]["get"]["requestBody"]["content"]["application/json"]["schema"]["properties"]
        ["input"]["properties"]["nested"]["deprecated"] = json!(true);
    new["paths"]["/widgets"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
        ["properties"]["output"]["properties"]["nested"]["deprecated"] = json!(true);
    let changes = contract::openapi::diff(&parse(&old), &parse(&new));
    let facts: Vec<_> = changes
        .iter()
        .filter(|c| c.kind == ChangeKind::DeprecatedChanged)
        .collect();
    assert_eq!(facts.len(), 2, "one request use and one response use");
    assert_eq!(facts.iter().filter(|c| matches!(&c.native, NativeLocation::Openapi { parameter_in:Some(p), .. } if p == "body")).count(), 1);
    assert_eq!(
        facts
            .iter()
            .filter(|c| matches!(
                &c.native,
                NativeLocation::Openapi {
                    status: Some(_),
                    ..
                }
            ))
            .count(),
        1
    );
    assert!(facts.iter().all(|c| {
        c.details["pointer"]
            .as_str()
            .unwrap()
            .ends_with("/nested/deprecated")
    }));
}
