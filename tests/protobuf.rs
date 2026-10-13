use serde_json::{Value, json};
use std::{fs, path::Path};
use vimanam::contract::{
    Change, ChangeKind, Severity,
    protobuf::{self, Document},
};
fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/protobuf")
        .join(name)
}
fn source(body: &str) -> Document {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("api.proto");
    fs::write(&path, format!("syntax = \"proto3\"; package demo; {body}")).unwrap();
    Document::read(&path, &[]).unwrap()
}
fn compare(a: &str, b: &str) -> Vec<Change> {
    protobuf::diff(&source(a), &source(b)).unwrap()
}
fn has_rule(changes: &[Change], rule: &str, impact: &str) -> bool {
    changes.iter().any(|c| {
        c.native_kind == rule
            && c.severity == Severity::Breaking
            && c.impacts.iter().any(|i| i == impact)
    })
}
#[test]
fn public_health_versions_and_descriptor_parity() {
    let a = Document::read(&fixture("health-v1.0.0.proto"), &[]).unwrap();
    let b = Document::read(&fixture("health-v1.20.0.proto"), &[]).unwrap();
    let changes = protobuf::diff(&a, &b).unwrap();
    let expected: Value =
        serde_json::from_slice(&fs::read(fixture("health-expected.json")).unwrap()).unwrap();
    let actual=Value::Array(changes.iter().map(|c|json!({"kind":c.kind,"native_kind":c.native_kind,"severity":c.severity,"name":match &c.native {vimanam::contract::NativeLocation::Protobuf{full_name,..}=>full_name,_=>panic!()}})).collect());
    assert_eq!(actual, expected);
    let ad = Document::read(&fixture("health-v1.0.0.pb"), &[]).unwrap();
    let bd = Document::read(&fixture("health-v1.20.0.pb"), &[]).unwrap();
    assert_eq!(changes, protobuf::diff(&ad, &bd).unwrap());
    assert_eq!(protobuf::adapt(&a), protobuf::adapt(&ad));
}
#[test]
fn incompatible_field_type_is_wire_break() {
    assert!(has_rule(
        &compare(
            "message M { int32 id = 1; }",
            "message M { string id = 1; }"
        ),
        "FIELD_SAME_TYPE",
        "wire"
    ));
}

#[test]
fn required_field_addition_breaks_only_when_parent_message_survives() {
    let (old, new) = proto2_pair(
        "syntax=\"proto2\"; message Existing {}",
        "syntax=\"proto2\"; message Existing {} message Added { required int32 id=1; }",
    );
    for (old, new) in [
        (&old, &new),
        (
            &Document::decode(&old.descriptor_bytes).unwrap(),
            &Document::decode(&new.descriptor_bytes).unwrap(),
        ),
    ] {
        let changes = protobuf::diff(old, new).unwrap();
        assert!(changes.iter().any(|c| {
            c.kind == ChangeKind::FieldAdded
                && c.native_kind == "PROTOBUF_SURFACE_CHANGED"
                && c.severity == Severity::NonBreaking
                && c.impacts.is_empty()
        }));
    }

    let (old, new) = proto2_pair(
        "syntax=\"proto2\"; message Existing {}",
        "syntax=\"proto2\"; message Existing { required int32 id=1; }",
    );
    for (old, new) in [
        (&old, &new),
        (
            &Document::decode(&old.descriptor_bytes).unwrap(),
            &Document::decode(&new.descriptor_bytes).unwrap(),
        ),
    ] {
        assert!(has_rule(
            &protobuf::diff(old, new).unwrap(),
            "PROTOBUF_SURFACE_CHANGED",
            "wire"
        ));
    }
}
#[test]
fn unreserved_field_removal_is_wire_break() {
    assert!(has_rule(
        &compare("message M { int32 id = 1; }", "message M {}"),
        "FIELD_NO_DELETE_UNLESS_NUMBER_RESERVED",
        "wire"
    ));
}
#[test]
fn reserved_field_removal_only_breaks_source() {
    let c = compare("message M { int32 id = 1; }", "message M { reserved 1; }");
    assert!(has_rule(&c, "FIELD_NO_DELETE", "source"));
    assert!(!c.iter().any(|c| c.impacts.iter().any(|i| i == "wire")));
}
#[test]
fn enum_number_reassignment_is_wire_break() {
    assert!(has_rule(
        &compare("enum E { ZERO=0; A=1; B=2; }", "enum E { ZERO=0; A=2; }"),
        "ENUM_VALUE_NUMBER_REUSED",
        "wire"
    ));
}
#[test]
fn enum_unreserved_removal_is_wire_break() {
    assert!(has_rule(
        &compare("enum E { ZERO=0; A=1; }", "enum E { ZERO=0; }"),
        "ENUM_VALUE_NO_DELETE_UNLESS_NUMBER_RESERVED",
        "wire"
    ));
}
#[test]
fn enum_reserved_removal_only_breaks_source() {
    let c = compare("enum E { ZERO=0; A=1; }", "enum E { reserved 1; ZERO=0; }");
    assert!(has_rule(&c, "ENUM_VALUE_NO_DELETE", "source"));
    assert!(!c.iter().any(|c| c.impacts.iter().any(|i| i == "wire")));
}
#[test]
fn oneof_membership_is_wire_break() {
    assert!(has_rule(
        &compare(
            "message M { string id=1; }",
            "message M { oneof value { string id=1; } }"
        ),
        "FIELD_SAME_ONEOF",
        "wire"
    ));
}
#[test]
fn field_rename_is_source_not_wire_break() {
    let c = compare(
        "message M { string old_name=1; }",
        "message M { string new_name=1; }",
    );
    assert!(has_rule(&c, "FIELD_SAME_NAME", "source"));
    assert!(!c.iter().any(|c| c.impacts.iter().any(|i| i == "wire")));
}
#[test]
fn message_and_enum_renames_break_source() {
    let c = compare(
        "message M {} enum E { ZERO=0; }",
        "message NewM {} enum NewE { ZERO=0; }",
    );
    assert!(has_rule(&c, "MESSAGE_NO_DELETE", "source"));
    assert!(has_rule(&c, "ENUM_NO_DELETE", "source"));
}
#[test]
fn service_and_rpc_renames_break_source() {
    let c = compare(
        "message M {} service S { rpc Old(M) returns (M); }",
        "message M {} service S { rpc New(M) returns (M); }",
    );
    assert!(has_rule(&c, "RPC_NO_DELETE", "source"));
    let c = compare(
        "message M {} service S { rpc R(M) returns (M); }",
        "message M {} service NewS { rpc R(M) returns (M); }",
    );
    assert!(has_rule(&c, "SERVICE_NO_DELETE", "source"));
}
#[test]
fn streaming_changes_break_source() {
    let c = compare(
        "message M {} service S { rpc R(M) returns (M); }",
        "message M {} service S { rpc R(stream M) returns (stream M); }",
    );
    assert!(has_rule(&c, "RPC_SAME_CLIENT_STREAMING", "source"));
    assert!(has_rule(&c, "RPC_SAME_SERVER_STREAMING", "source"));
}
#[test]
fn json_name_changes_break_source() {
    assert!(has_rule(
        &compare(
            "message M { string id=1; }",
            "message M { string id=1 [json_name=\"identifier\"]; }"
        ),
        "FIELD_SAME_JSON_NAME",
        "source"
    ));
}
#[test]
fn package_changes_break_source() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("api.proto");
    fs::write(
        &p,
        "syntax=\"proto3\"; package old; option java_package=\"before\"; message M {}",
    )
    .unwrap();
    let a = Document::read(&p, &[]).unwrap();
    fs::write(
        &p,
        "syntax=\"proto3\"; package new; option java_package=\"after\"; message M {}",
    )
    .unwrap();
    let b = Document::read(&p, &[]).unwrap();
    let changes = protobuf::diff(&a, &b).unwrap();
    let new_contract = protobuf::adapt(&b);
    assert!(has_rule(&changes, "FILE_SAME_PACKAGE", "source"));
    assert_eq!(
        changes
            .iter()
            .filter(|c| c.native_kind == "FILE_OPTIONS_CHANGED")
            .count(),
        1
    );
    for change in changes {
        assert!(
            protobuf::adapt(&a)
                .entities
                .iter()
                .any(|e| e.id == change.subject)
                || new_contract.entities.iter().any(|e| e.id == change.subject)
        );
    }
}

#[test]
fn file_options_deduplicate_within_file_but_not_across_files() {
    fn make_tree(root: &Path, version: &str) -> Document {
        let dir = root.join(version);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("main.proto"),
            "syntax=\"proto3\"; import \"a.proto\"; import \"b.proto\"; message Main {}",
        )
        .unwrap();
        let option = if version == "old" { "before" } else { "after" };
        fs::write(
            dir.join("a.proto"),
            format!("syntax=\"proto3\"; package a; option java_package=\"{option}\"; message A1 {{}} message A2 {{}}"),
        )
        .unwrap();
        fs::write(
            dir.join("b.proto"),
            format!("syntax=\"proto3\"; package b; option java_package=\"{option}\"; message B1 {{}} message B2 {{}}"),
        )
        .unwrap();
        Document::read(&dir.join("main.proto"), &[dir]).unwrap()
    }

    let root = tempfile::tempdir().unwrap();
    let old = make_tree(root.path(), "old");
    let new = make_tree(root.path(), "new");
    let source_changes = protobuf::diff(&old, &new).unwrap();
    assert_eq!(
        source_changes
            .iter()
            .filter(|c| c.native_kind == "FILE_OPTIONS_CHANGED")
            .count(),
        2
    );
    assert!(source_changes.iter().all(|c| {
        protobuf::adapt(&old)
            .entities
            .iter()
            .any(|e| e.id == c.subject)
            && protobuf::adapt(&new)
                .entities
                .iter()
                .any(|e| e.id == c.subject)
    }));

    let old_descriptor = Document::decode(&old.descriptor_bytes).unwrap();
    let new_descriptor = Document::decode(&new.descriptor_bytes).unwrap();
    assert_eq!(
        source_changes,
        protobuf::diff(&old_descriptor, &new_descriptor).unwrap()
    );
}

#[test]
fn file_options_are_compared_without_message_enum_or_service_symbols() {
    let root = tempfile::tempdir().unwrap();
    for kind in ["empty", "extension"] {
        let old_dir = root.path().join("old");
        let new_dir = root.path().join("new");
        fs::create_dir_all(&old_dir).unwrap();
        fs::create_dir_all(&new_dir).unwrap();
        let (filename, suffix) = if kind == "empty" {
            ("empty.proto", "")
        } else {
            (
                "extension.proto",
                "import \"google/protobuf/descriptor.proto\"; extend google.protobuf.FileOptions { optional string custom_file = 50001; }",
            )
        };
        for (dir, value) in [(&old_dir, "before"), (&new_dir, "after")] {
            fs::write(
                dir.join(filename),
                format!(
                    "syntax=\"proto2\"; package {kind}; option java_package=\"{value}\"; {suffix}"
                ),
            )
            .unwrap();
        }
        let old = Document::read(&old_dir.join(filename), &[]).unwrap();
        let new = Document::read(&new_dir.join(filename), &[]).unwrap();
        let changes = protobuf::diff(&old, &new).unwrap();
        assert_eq!(
            changes
                .iter()
                .filter(|c| c.native_kind == "FILE_OPTIONS_CHANGED")
                .count(),
            1,
            "{kind}"
        );
        for change in &changes {
            assert!(
                protobuf::adapt(&old)
                    .entities
                    .iter()
                    .any(|e| e.id == change.subject)
            );
            assert!(
                protobuf::adapt(&new)
                    .entities
                    .iter()
                    .any(|e| e.id == change.subject)
            );
        }
        assert_eq!(
            changes,
            protobuf::diff(
                &Document::decode(&old.descriptor_bytes).unwrap(),
                &Document::decode(&new.descriptor_bytes).unwrap()
            )
            .unwrap(),
            "{kind}"
        );
    }
}

fn proto2_pair(old: &str, new: &str) -> (Document, Document) {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("old.proto");
    let b = t.path().join("new.proto");
    fs::write(&a, old).unwrap();
    fs::write(&b, new).unwrap();
    (
        Document::read(&a, &[]).unwrap(),
        Document::read(&b, &[]).unwrap(),
    )
}

#[test]
fn required_field_removal_is_wire_break_even_when_reserved() {
    let (a, b) = proto2_pair(
        "syntax=\"proto2\"; message M { required int32 id=1; }",
        "syntax=\"proto2\"; message M { reserved 1; }",
    );
    for (old, new) in [
        (&a, &b),
        (
            &Document::decode(&a.descriptor_bytes).unwrap(),
            &Document::decode(&b.descriptor_bytes).unwrap(),
        ),
    ] {
        assert!(has_rule(
            &protobuf::diff(old, new).unwrap(),
            "FIELD_REQUIRED_NO_DELETE",
            "wire"
        ));
    }
}

#[test]
fn group_encoding_changes_are_wire_breaks() {
    let (a, b) = proto2_pair(
        "syntax=\"proto2\"; message M { optional group Item=1 { optional int32 x=2; } }",
        "syntax=\"proto2\"; message M { optional Item item=1; message Item { optional int32 x=2; } }",
    );
    for (old, new) in [
        (&a, &b),
        (
            &Document::decode(&a.descriptor_bytes).unwrap(),
            &Document::decode(&b.descriptor_bytes).unwrap(),
        ),
    ] {
        assert!(has_rule(
            &protobuf::diff(old, new).unwrap(),
            "FIELD_SAME_GROUP_ENCODING",
            "wire"
        ));
    }
}

#[test]
fn enum_reordering_updates_implicit_defaults_but_explicit_defaults_compare_by_value() {
    let prefix = "syntax=\"proto2\"; enum E { A=1; B=2; } message M { optional E e=1";
    let (old, new) = proto2_pair(
        &format!("{prefix}; }}"),
        "syntax=\"proto2\"; enum E { B=2; A=1; } message M { optional E e=1; }",
    );
    let changes = protobuf::diff(&old, &new).unwrap();
    assert!(changes.iter().any(|c| c.kind == ChangeKind::DefaultChanged));
    let (old, new) = proto2_pair(
        "syntax=\"proto2\"; enum E { A=1; B=2; } message M { optional E e=1 [default=A]; }",
        "syntax=\"proto2\"; enum E { B=2; A=1; } message M { optional E e=1 [default=A]; }",
    );
    assert!(
        !protobuf::diff(&old, &new)
            .unwrap()
            .iter()
            .any(|c| c.kind == ChangeKind::DefaultChanged)
    );
}

#[test]
fn non_finite_float_defaults_remain_typed_and_distinct() {
    let (old, new) = proto2_pair(
        "syntax=\"proto2\"; message M { optional double value=1 [default=nan]; }",
        "syntax=\"proto2\"; message M { optional double value=1 [default=inf]; }",
    );
    let field = |d: &Document| {
        protobuf::adapt(d)
            .entities
            .into_iter()
            .find(|e| e.name == "M.value")
            .unwrap()
    };
    assert_eq!(field(&old).attributes["default"]["kind"], "f64");
    assert_eq!(field(&old).attributes["default"]["value"], "NaN");
    assert_eq!(field(&new).attributes["default"]["value"], "Infinity");
    assert!(
        protobuf::diff(&old, &new)
            .unwrap()
            .iter()
            .any(|c| c.kind == ChangeKind::DefaultChanged)
    );
}
#[test]
fn pagination_inference_is_review_only() {
    let c = compare(
        "message In {} message Out {} service S {rpc List(In) returns (Out);}",
        "message In {int32 page_size=1; string page_token=2;} message Out {string next_page_token=1;} service S {rpc List(In) returns (Out);}",
    );
    assert!(
        c.iter()
            .any(|c| c.kind == ChangeKind::PaginationChanged && c.severity == Severity::Review)
    );
}
#[test]
fn deprecation_is_neutral_fact() {
    let c = compare(
        "message M {string id=1;}",
        "message M {string id=1 [deprecated=true];}",
    );
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].kind, ChangeKind::DeprecatedChanged);
}
#[test]
fn additions_are_nonbreaking() {
    let c = compare(
        "message M {} enum E {ZERO=0;}",
        "message M {string id=1;} message Added {} enum E {ZERO=0; ONE=1;}",
    );
    assert!(c.iter().all(|c| c.severity == Severity::NonBreaking));
}
#[test]
fn reserved_field_number_reuse_is_wire_break() {
    assert!(has_rule(
        &compare("message M {reserved 1;}", "message M {string id=1;}"),
        "RESERVED_IDENTITY_REUSED",
        "wire"
    ));
}
#[test]
fn reserved_enum_number_reuse_is_wire_break() {
    assert!(has_rule(
        &compare("enum E {reserved 1; ZERO=0;}", "enum E {ZERO=0; ONE=1;}"),
        "RESERVED_IDENTITY_REUSED",
        "wire"
    ));
}
#[test]
fn enum_alias_addition_is_not_wire_reuse() {
    let c = compare(
        "enum E {ZERO=0; ONE=1;}",
        "enum E {option allow_alias=true; ZERO=0; ONE=1; ALSO_ONE=1;}",
    );
    assert!(c.iter().all(|c| c.severity != Severity::Breaking));
}
#[test]
fn order_and_comments_do_not_change_surface_or_ids() {
    let a =
        source("message M {string a=1; int32 b=2;} message N {} service S {rpc R(M) returns (N);}");
    let b = source(
        "// description\nservice S {rpc R(M) returns (N);} message N {} message M {int32 b=2; string a=1;}",
    );
    assert_eq!(protobuf::adapt(&a), protobuf::adapt(&b));
    assert!(protobuf::diff(&a, &b).unwrap().is_empty());
    let c =
        source("message N {} message M {int32 b=2; bool a=1;} service S {rpc R(M) returns (N);}");
    let d =
        source("service S {rpc R(M) returns (N);} message M {bool a=1; int32 b=2;} message N {}");
    assert_eq!(
        protobuf::diff(&a, &c).unwrap(),
        protobuf::diff(&b, &d).unwrap()
    );
}
#[test]
fn proto2_and_nested_types_preserve_native_facts() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("api.proto");
    fs::write(&p,"syntax=\"proto2\"; package demo; message M {message Nested {required int32 id=1;} optional Nested nested=1;}").unwrap();
    let a = Document::read(&p, &[]).unwrap();
    let surface = protobuf::adapt(&a);
    assert!(surface.entities.iter().any(|e| e.name == "demo.M.Nested"));
    assert!(
        surface
            .entities
            .iter()
            .any(|e| e.name == "demo.M.Nested.id" && e.attributes["cardinality"] == "required")
    );
}
#[test]
fn unknown_options_and_original_bytes_are_retained() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("api.proto");
    fs::write(&p,"syntax=\"proto2\"; package demo; import \"google/protobuf/descriptor.proto\"; extend google.protobuf.FieldOptions {optional string meaning=50001;} message M {optional string id=1 [(meaning)=\"identifier\"];} ").unwrap();
    let a = Document::read(&p, &[]).unwrap();
    assert!(protobuf::adapt(&a).entities.iter().any(
        |e| e.name == "demo.M.id" && e.attributes["options"].to_string().contains("identifier")
    ));
    assert_eq!(
        a.descriptor_bytes,
        Document::decode(&a.descriptor_bytes)
            .unwrap()
            .descriptor_bytes
    );
}
#[test]
fn unsupported_editions_and_unknown_syntax_return_errors_without_source_info() {
    use prost::Message;
    for syntax in ["editions", "not-a-syntax", ""] {
        for source_code_info in [None, Some(prost_types::SourceCodeInfo::default())] {
            let set = prost_types::FileDescriptorSet {
                file: vec![prost_types::FileDescriptorProto {
                    name: Some("api.proto".into()),
                    syntax: Some(syntax.into()),
                    source_code_info,
                    ..Default::default()
                }],
            };
            let error = Document::decode(&set.encode_to_vec()).unwrap_err();
            assert!(error.to_string().contains("Unsupported protobuf syntax"));
        }
    }
}
#[test]
fn maximum_enum_reserved_number_does_not_overflow() {
    let a = source("enum E {reserved 2147483647; ZERO=0;}");
    let b = source("enum E {ZERO=0; MAX=2147483647;}");
    assert!(has_rule(
        &protobuf::diff(&a, &b).unwrap(),
        "RESERVED_IDENTITY_REUSED",
        "wire"
    ));
}
#[test]
fn enum_alias_reorder_is_stable_and_removal_breaks_source_only() {
    let a = source("enum E {option allow_alias=true; ZERO=0; ONE=1; ALSO_ONE=1;}");
    let b = source("enum E {option allow_alias=true; ZERO=0; ALSO_ONE=1; ONE=1;}");
    assert_eq!(protobuf::adapt(&a), protobuf::adapt(&b));
    let c = source("enum E {ZERO=0; ONE=1;}");
    let changes = protobuf::diff(&a, &c).unwrap();
    assert!(has_rule(&changes, "ENUM_VALUE_NO_DELETE", "source"));
    assert!(
        !changes
            .iter()
            .any(|c| c.impacts.iter().any(|i| i == "wire"))
    );
}
#[test]
fn recursive_messages_maps_options_and_imports_match_descriptors() {
    let a = source(
        "message M {map<string,M> children=1; M next=2;} enum E {ZERO=0;} service S {rpc R(M) returns(M);}",
    );
    let b = Document::decode(&a.descriptor_bytes).unwrap();
    assert_eq!(protobuf::adapt(&a), protobuf::adapt(&b));
    assert!(protobuf::diff(&a, &b).unwrap().is_empty());
    let c = protobuf::adapt(&a);
    assert!(c.entities.iter().any(|e| e.name == "demo.M.ChildrenEntry"));
}
#[test]
fn extension_fields_have_numbered_native_identity() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("api.proto");
    fs::write(&p,"syntax=\"proto2\";package demo; message M {extensions 100 to max;} extend M {optional string metadata=100;}").unwrap();
    let d = Document::read(&p, &[]).unwrap();
    let c = protobuf::adapt(&d);
    let e = c
        .entities
        .iter()
        .find(|e| e.name == "demo.metadata")
        .unwrap();
    assert_eq!(e.attributes["extension"], true);
    assert!(matches!(
        e.native,
        vimanam::contract::NativeLocation::Protobuf {
            number: Some(100),
            ..
        }
    ));
    assert_eq!(
        c,
        protobuf::adapt(&Document::decode(&d.descriptor_bytes).unwrap())
    );
}
#[test]
fn malformed_descriptor_fields_and_duplicate_symbols_return_errors() {
    use prost::Message;
    let file = prost_types::FileDescriptorProto {
        name: Some("api.proto".into()),
        syntax: Some("proto3".into()),
        message_type: vec![prost_types::DescriptorProto {
            name: Some("M".into()),
            field: vec![prost_types::FieldDescriptorProto {
                name: Some("id".into()),
                number: Some(-1),
                r#type: Some(prost_types::field_descriptor_proto::Type::String as i32),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(
        Document::decode(&prost_types::FileDescriptorSet { file: vec![file] }.encode_to_vec())
            .is_err()
    );
    let file = prost_types::FileDescriptorProto {
        name: Some("api.proto".into()),
        syntax: Some("proto3".into()),
        message_type: vec![
            prost_types::DescriptorProto {
                name: Some("M".into()),
                ..Default::default()
            },
            prost_types::DescriptorProto {
                name: Some("M".into()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert!(
        Document::decode(&prost_types::FileDescriptorSet { file: vec![file] }.encode_to_vec())
            .is_err()
    );
}
