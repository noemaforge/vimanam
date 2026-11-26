//! Differential oracle for the protobuf rules: replays
//! `formal/oracle/Protobuf.json`, written by the Lean model's `lake exe
//! oracle` (`formal/OracleExport/Protobuf.lean`), through the real
//! `ProtobufAdapter`, `native_kind`, `breaking_gate` and `protobuf::diff`,
//! and asserts the outputs the Lean definitions computed. Nothing here
//! restates the rules; a mismatch names the function, the case index and the
//! input. See `formal/README.md`, "Differential oracle".
//!
//! Each document pair also carries the specification's hazards
//! (`ProtobufCompat.lean`). A wire hazard must fail `--breaking wire` and a
//! source hazard `--breaking source`, except a case pinned as a known gap
//! (an issue number), which must still pass the gate: when the issue is
//! fixed the pin fails here, and the pin, the Lean finding and the model are
//! updated together.

use std::collections::BTreeSet;
use std::path::PathBuf;

use prost::Message as _;
use prost_types::{
    DescriptorProto, EnumDescriptorProto, EnumOptions, EnumValueDescriptorProto,
    FieldDescriptorProto, FieldOptions, FileDescriptorProto, FileDescriptorSet,
    OneofDescriptorProto, descriptor_proto::ReservedRange,
    enum_descriptor_proto::EnumReservedRange, field_descriptor_proto::Label,
    field_descriptor_proto::Type,
};
use serde_json::{Value, json};

use super::{BreakingScope, Document, ProtobufAdapter, breaking_gate, diff, native_kind};
use crate::contract::{Adapter, Change, ChangeKind, Entity, EntityKind, NativeLocation, Severity};

/// Declares `ALL_VARIANTS` (every neutral `ChangeKind` variant's serde name)
/// from one exhaustive match with no wildcard, so a new variant does not
/// compile until it is listed here, and the test then fails until the Lean
/// model's `ChangeKind` (`formal/VimanamProofs/Protobuf.lean`) has it and the
/// oracle rates it. A name listed twice is an unreachable pattern, which
/// `clippy -D warnings` rejects.
macro_rules! variants {
    ($($name:ident),* $(,)?) => {
        /// Every `ChangeKind` variant, so the test can require that the
        /// oracle classifies each one.
        const ALL_VARIANTS: &[ChangeKind] = &[$(ChangeKind::$name),*];

        #[allow(dead_code)]
        fn exhaustive(kind: ChangeKind) {
            match kind {
                $(ChangeKind::$name => {})*
            }
        }
    };
}

variants!(
    ServiceAdded,
    ServiceRemoved,
    OperationAdded,
    OperationRemoved,
    OperationRenamed,
    OperationMoved,
    TypeAdded,
    TypeRemoved,
    TypeKindChanged,
    ArgumentAdded,
    ArgumentRemoved,
    ArgumentRequiredChanged,
    ArgumentTypeChanged,
    FieldAdded,
    FieldRemoved,
    FieldTypeChanged,
    FieldRequiredChanged,
    NullabilityChanged,
    EnumValueAdded,
    EnumValueRemoved,
    DeprecatedChanged,
    DefaultChanged,
    PaginationChanged,
    WireIdentityChanged,
    ResultAdded,
    ResultRemoved,
    ConstraintChanged,
);

fn oracle_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("formal/oracle/Protobuf.json")
}

fn kind_name(kind: ChangeKind) -> String {
    serde_json::to_value(kind)
        .expect("kind")
        .as_str()
        .expect("kind name")
        .to_owned()
}

fn kind(name: &Value) -> ChangeKind {
    serde_json::from_value(name.clone()).expect("change kind")
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Breaking => "breaking",
        Severity::NonBreaking => "non_breaking",
        Severity::Review => "review",
    }
}

fn severity(name: &Value) -> Severity {
    match name.as_str().expect("severity") {
        "breaking" => Severity::Breaking,
        "non_breaking" => Severity::NonBreaking,
        "review" => Severity::Review,
        other => panic!("unknown severity {other}"),
    }
}

fn scope(name: &str) -> BreakingScope {
    match name {
        "wire" => BreakingScope::Wire,
        "source" => BreakingScope::Source,
        "all" => BreakingScope::All,
        other => panic!("unknown scope {other}"),
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("string array")
        .iter()
        .map(|s| s.as_str().expect("string").to_owned())
        .collect()
}

/// An entity whose attributes hold only what the adapter reads.
fn entity(attributes: Value) -> Entity {
    Entity {
        id: "ve2_subject".into(),
        kind: EntityKind::Field,
        name: "pkg.M.a".into(),
        parent: None,
        services: vec![],
        native: NativeLocation::Protobuf {
            full_name: "pkg.M.a".into(),
            number: Some(1),
        },
        attributes,
        references: vec![],
    }
}

/// A change with only what the function under test reads.
fn change(kind: ChangeKind, pointer: &str, severity: Severity, impacts: Vec<String>) -> Change {
    Change {
        id: String::new(),
        subject: "ve2_subject".into(),
        kind,
        severity,
        impacts,
        native: NativeLocation::Protobuf {
            full_name: "pkg.M.a".into(),
            number: Some(1),
        },
        native_kind: String::new(),
        details: json!({ "pointer": pointer }),
        uses: Vec::new(),
    }
}

fn label(name: &str) -> Label {
    match name {
        "optional" => Label::Optional,
        "required" => Label::Required,
        "repeated" => Label::Repeated,
        other => panic!("unknown label {other}"),
    }
}

fn scalar(name: &str) -> Type {
    match name {
        "int32" => Type::Int32,
        "string" => Type::String,
        other => panic!("unknown type {other}"),
    }
}

fn int(value: &Value) -> i64 {
    value.as_i64().expect("integer")
}

fn message(m: &Value) -> DescriptorProto {
    let fields = m["fields"].as_array().expect("fields");
    let has_oneof = fields.iter().any(|f| f[4].as_bool().expect("oneof"));
    DescriptorProto {
        name: Some("M".into()),
        field: fields
            .iter()
            .map(|f| {
                let name = f[1].as_str().expect("name");
                FieldDescriptorProto {
                    name: Some(name.into()),
                    number: Some(i32::try_from(int(&f[0])).expect("field number")),
                    label: Some(label(f[3].as_str().expect("label")) as i32),
                    r#type: Some(scalar(f[2].as_str().expect("type")) as i32),
                    json_name: Some(name.into()),
                    oneof_index: f[4].as_bool().expect("oneof").then_some(0),
                    options: f[5].as_bool().expect("deprecated").then(|| FieldOptions {
                        deprecated: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }
            })
            .collect(),
        oneof_decl: if has_oneof {
            vec![OneofDescriptorProto {
                name: Some("o".into()),
                options: None,
            }]
        } else {
            vec![]
        },
        reserved_range: m["reserved_ranges"]
            .as_array()
            .expect("ranges")
            .iter()
            .map(|r| ReservedRange {
                start: Some(i32::try_from(int(&r[0])).expect("start")),
                end: Some(i32::try_from(int(&r[1])).expect("end")),
            })
            .collect(),
        reserved_name: strings(&m["reserved_names"]),
        ..Default::default()
    }
}

fn enumeration(e: &Value) -> EnumDescriptorProto {
    let values: Vec<(String, i32)> = e["values"]
        .as_array()
        .expect("values")
        .iter()
        .map(|v| {
            (
                v[0].as_str().expect("label").to_owned(),
                i32::try_from(int(&v[1])).expect("enum number"),
            )
        })
        .collect();
    let aliased = values
        .iter()
        .any(|(l, n)| values.iter().any(|(m, k)| l != m && n == k));
    EnumDescriptorProto {
        name: Some("E".into()),
        value: values
            .iter()
            .map(|(name, number)| EnumValueDescriptorProto {
                name: Some(name.clone()),
                number: Some(*number),
                options: None,
            })
            .collect(),
        options: aliased.then(|| EnumOptions {
            allow_alias: Some(true),
            ..Default::default()
        }),
        reserved_range: e["reserved_ranges"]
            .as_array()
            .expect("ranges")
            .iter()
            .map(|r| EnumReservedRange {
                start: Some(i32::try_from(int(&r[0])).expect("start")),
                end: Some(i32::try_from(int(&r[1])).expect("end")),
            })
            .collect(),
        reserved_name: strings(&e["reserved_names"]),
    }
}

/// The fragment document: one proto2 file declaring `M` and `E` when given.
fn document(side: &Value) -> Document {
    let file = FileDescriptorProto {
        name: Some("a.proto".into()),
        package: Some("pkg".into()),
        syntax: Some("proto2".into()),
        message_type: if side["message"].is_null() {
            vec![]
        } else {
            vec![message(&side["message"])]
        },
        enum_type: if side["enum"].is_null() {
            vec![]
        } else {
            vec![enumeration(&side["enum"])]
        },
        ..Default::default()
    };
    let bytes = FileDescriptorSet { file: vec![file] }.encode_to_vec();
    Document::decode(&bytes).unwrap_or_else(|err| panic!("fragment document {side}: {err:#}"))
}

/// A fact as the model records it.
fn record(c: &Change) -> Value {
    let NativeLocation::Protobuf { full_name, number } = &c.native else {
        panic!("non-protobuf location {:?}", c.native);
    };
    let name = full_name.strip_prefix("pkg.").unwrap_or(full_name);
    let subject = match number {
        Some(n) => format!("{name}#{n}"),
        None => name.to_owned(),
    };
    json!([subject, c.native_kind, severity_name(c.severity), c.impacts])
}

/// Records as a set: distinct, in a fixed order.
fn record_set(records: impl IntoIterator<Item = Value>) -> Value {
    let set: BTreeSet<String> = records.into_iter().map(|r| r.to_string()).collect();
    set.iter()
        .map(|r| serde_json::from_str::<Value>(r).expect("record"))
        .collect()
}

/// Replays one function's cases; returns the mismatches.
fn replay(rust: &str, cases: &[Value], classified: &mut BTreeSet<String>) -> Vec<String> {
    let mut failures = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let input = &case["input"];
        let expected = &case["output"];
        let mut extra = Vec::new();
        let actual = match rust {
            "ProtobufAdapter::attribute_kind" => {
                let pointer = input.as_str().expect("pointer");
                let value = crate::diff::ValueChangeKind::Added(Value::Null);
                json!(kind_name(ProtobufAdapter.attribute_kind(
                    &entity(json!({})),
                    pointer,
                    &value
                )))
            }
            "ProtobufAdapter::structural_kind" => {
                let field = input.as_str().expect("field");
                match ProtobufAdapter.structural_kind(&entity(json!({})), field) {
                    Some(kind) => json!(kind_name(kind)),
                    None => Value::Null,
                }
            }
            "ProtobufAdapter::classify" => {
                let kind = kind(&input[0]);
                classified.insert(kind_name(kind));
                let attributes = if input[2].is_null() {
                    json!({})
                } else {
                    json!({ "cardinality": input[2] })
                };
                let (severity, impacts) = ProtobufAdapter.classify(
                    &entity(attributes),
                    kind,
                    &json!({ "pointer": input[1] }),
                );
                json!([severity_name(severity), impacts])
            }
            "protobuf::native_kind" => {
                let c = change(
                    kind(&input[0]),
                    input[1].as_str().expect("pointer"),
                    Severity::Review,
                    vec![],
                );
                let removed = entity(if input[2].is_null() {
                    json!({})
                } else {
                    json!({ "kind": input[2] })
                });
                // `diff` passes the old entity only for a removed type.
                let removed = (c.kind == ChangeKind::TypeRemoved).then_some(&removed);
                json!(native_kind(&c, removed))
            }
            "protobuf::breaking_gate" => {
                let c = change(
                    ChangeKind::ConstraintChanged,
                    "",
                    severity(&input[1]),
                    strings(&input[2]),
                );
                json!(breaking_gate(&c, scope(input[0].as_str().expect("scope"))))
            }
            "protobuf::diff, protobuf::breaking_gate" => {
                let changes = diff(&document(&input["old"]), &document(&input["new"]))
                    .expect("protobuf::diff");
                let fails = |name: &str| changes.iter().any(|c| breaking_gate(c, scope(name)));
                let gap = &expected["known_gap"];
                for dimension in ["wire", "source"] {
                    if !expected["hazard"][dimension].as_bool().expect("hazard") {
                        continue;
                    }
                    let pinned = dimension == "wire" && !gap.is_null();
                    if pinned && fails(dimension) {
                        extra.push(format!(
                            "the known gap #{gap} no longer holds: --breaking wire now fails; \
                             remove the pin and the Lean finding"
                        ));
                    } else if !pinned && !fails(dimension) {
                        extra.push(format!(
                            "a {dimension} hazard passes --breaking {dimension}"
                        ));
                    }
                }
                json!({
                    "facts": record_set(changes.iter().map(record)),
                    "fails": { "wire": fails("wire"), "source": fails("source"), "all": fails("all") },
                })
            }
            other => panic!("no replay for {other}"),
        };
        // The model's facts are a set; hazards and gaps are checked above.
        let expected_cmp = if rust == "protobuf::diff, protobuf::breaking_gate" {
            json!({
                "facts": record_set(expected["facts"].as_array().expect("facts").iter().cloned()),
                "fails": expected["fails"],
            })
        } else {
            expected.clone()
        };
        if actual != expected_cmp {
            failures.push(format!(
                "{rust} case {index}: input {input}, Lean {expected_cmp}, Rust {actual}"
            ));
        }
        for e in extra {
            failures.push(format!("{rust} case {index}: input {input}: {e}"));
        }
    }
    failures
}

#[test]
fn oracle() {
    let path = oracle_path();
    if !path.is_file() {
        // A packaged crate may leave formal/ out; the repository always has
        // it, so in CI a missing file is a failure, not a skip.
        let in_ci = std::env::var("CI")
            .is_ok_and(|value| value.eq_ignore_ascii_case("true") || value == "1");
        assert!(
            !in_ci,
            "{} does not exist; the formal oracle replay cannot be skipped in CI",
            path.display()
        );
        eprintln!(
            "skipping the protobuf formal oracle replay: {} does not exist",
            path.display()
        );
        return;
    }
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
    let vectors: Value =
        serde_json::from_str(&text).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    let mut failures = Vec::new();
    let mut total = 0;
    let mut classified = BTreeSet::new();
    for function in vectors["functions"].as_array().expect("functions") {
        let rust = function["rust"].as_str().expect("rust name");
        let cases = function["cases"].as_array().expect("cases");
        assert!(!cases.is_empty(), "{rust} has no cases");
        total += cases.len();
        failures.extend(replay(rust, cases, &mut classified));
    }
    let unclassified: Vec<String> = ALL_VARIANTS
        .iter()
        .map(|kind| kind_name(*kind))
        .filter(|name| !classified.contains(name))
        .collect();
    assert!(
        unclassified.is_empty(),
        "no oracle case classifies these ChangeKind variants: {unclassified:?}"
    );
    assert!(
        failures.is_empty(),
        "{} of {total} protobuf oracle cases disagree with the Lean model:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
