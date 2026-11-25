//! Differential oracle: replays `formal/oracle/*.json`, written by the Lean
//! model's `lake exe oracle` (`formal/OracleExport/`), through the real
//! `diff::severity`, `locate`, `under_not` and `diff_values`, and asserts the
//! outputs the Lean definitions computed. Nothing here restates the rule
//! table; a mismatch names the function, the case index and the input. See
//! `formal/README.md`, "Differential oracle".

use std::path::PathBuf;

use serde_json::{Value, json};

use super::test_support::change;
use super::{ChangeKind, MoveRule, Severity, ValueChange, ValueChangeKind, severity};
use super::{diff_values, locate, under_not};
use crate::diff::value::Location;
use crate::report::EndpointRef;

fn oracle_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("formal/oracle")
}

fn load(name: &str) -> Value {
    let path = oracle_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Breaking => "breaking",
        Severity::NonBreaking => "non_breaking",
        Severity::Review => "review",
    }
}

fn location_name(location: Location) -> &'static str {
    match location {
        Location::Type => "type",
        Location::RequiredElement => "required_element",
        Location::EnumElement => "enum_element",
        Location::Enum => "enum",
        Location::Property => "property",
        Location::AdditionalProperties => "additional_properties",
        Location::Nullable => "nullable",
        Location::Other => "other",
    }
}

/// The model's `Val`: `other` is any value the table cannot tell apart.
fn value(name: &Value) -> Value {
    match name.as_str().expect("value name") {
        "true" => json!(true),
        "false" => json!(false),
        "null" => Value::Null,
        "other" => json!("x"),
        other => panic!("unknown value {other}"),
    }
}

fn kind(input: &Value) -> ValueChangeKind {
    let parts = input.as_array().expect("kind array");
    match parts[0].as_str().expect("kind tag") {
        "added" => ValueChangeKind::Added(value(&parts[1])),
        "removed" => ValueChangeKind::Removed(value(&parts[1])),
        "changed" => ValueChangeKind::Changed {
            old: value(&parts[1]),
            new: value(&parts[2]),
        },
        other => panic!("unknown kind {other}"),
    }
}

fn kind_tag(kind: &ValueChangeKind) -> &'static str {
    match kind {
        ValueChangeKind::Added(_) => "added",
        ValueChangeKind::Removed(_) => "removed",
        ValueChangeKind::Changed { .. } => "changed",
    }
}

/// The model's pointer (its tokens) as the Rust's JSON pointer. The oracle's
/// instantiation is that no token needs RFC 6901 escaping.
fn pointer(input: &Value) -> String {
    let tokens: Vec<&str> = input
        .as_array()
        .expect("pointer array")
        .iter()
        .map(|token| token.as_str().expect("pointer token"))
        .collect();
    for token in &tokens {
        assert!(
            !token.contains('~') && !token.contains('/'),
            "the oracle's instantiation does not hold: token {token:?} needs escaping"
        );
    }
    tokens.iter().map(|token| format!("/{token}")).collect()
}

fn structural(input: &Value) -> ChangeKind {
    let parts = input.as_array().expect("change array");
    let flag = |index: usize| parts[index].as_bool().expect("flag");
    let present = |index: usize| flag(index).then(|| "op".to_string());
    let name = || "p".to_string();
    let location = || "query".to_string();
    match parts[0].as_str().expect("change tag") {
        "endpoint_added" => ChangeKind::EndpointAdded,
        "endpoint_removed" => ChangeKind::EndpointRemoved {
            was_deprecated: flag(1),
        },
        "endpoint_moved" => ChangeKind::EndpointMoved {
            from: EndpointRef {
                method: "GET".into(),
                path: "/old".into(),
            },
            matched_by: MoveRule::OperationId,
            old_operation_id: Some("op".into()),
            new_operation_id: Some("op".into()),
        },
        "parameter_added" => ChangeKind::ParameterAdded {
            name: name(),
            location: location(),
            required: flag(1),
        },
        "parameter_removed" => ChangeKind::ParameterRemoved {
            name: name(),
            location: location(),
        },
        "parameter_required_changed" => ChangeKind::ParameterRequiredChanged {
            name: name(),
            location: location(),
            now_required: flag(1),
        },
        "parameter_location_changed" => ChangeKind::ParameterLocationChanged {
            name: name(),
            old_location: "query".into(),
            new_location: "header".into(),
        },
        "response_added" => ChangeKind::ResponseAdded {
            status: "200".into(),
        },
        "response_removed" => ChangeKind::ResponseRemoved {
            status: "200".into(),
        },
        "operation_id_changed" => ChangeKind::OperationIdChanged {
            old: present(1),
            new: present(2),
        },
        "deprecated_changed" => ChangeKind::DeprecatedChanged { now: flag(1) },
        other => panic!("unknown change {other}"),
    }
}

fn schema_change(rust: &str, pointer: String, kind: ValueChangeKind) -> ChangeKind {
    let change = ValueChange { pointer, kind };
    match rust {
        "diff::severity(RequestSchemaChanged)" => ChangeKind::RequestSchemaChanged { change },
        "diff::severity(ParameterSchemaChanged)" => ChangeKind::ParameterSchemaChanged {
            name: "p".into(),
            location: "query".into(),
            change,
        },
        "diff::severity(ResponseSchemaChanged)" => ChangeKind::ResponseSchemaChanged {
            status: "200".into(),
            change,
        },
        other => panic!("no replay for {other}"),
    }
}

fn enum_schema(members: &Value) -> Value {
    if members.is_null() {
        json!({})
    } else {
        json!({ "enum": members })
    }
}

/// Replays one function's cases; returns the mismatches.
fn replay(rust: &str, cases: &[Value]) -> Vec<String> {
    let mut failures = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let input = &case["input"];
        let expected = &case["output"];
        let actual = match rust {
            "diff::value::locate, diff::value::under_not" => {
                let pointer = pointer(input);
                json!([location_name(locate(&pointer)), under_not(&pointer)])
            }
            "diff::severity(structural ChangeKind)" => {
                json!(severity_name(severity(&change(structural(input)))))
            }
            "diff::value::diff_values on an enum keyword" => {
                let mut out = Vec::new();
                diff_values(
                    &enum_schema(&input[0]),
                    &enum_schema(&input[1]),
                    &mut String::new(),
                    &mut out,
                );
                Value::Array(
                    out.into_iter()
                        .map(|value_change| {
                            let tag = kind_tag(&value_change.kind);
                            let request = severity(&change(ChangeKind::RequestSchemaChanged {
                                change: value_change.clone(),
                            }));
                            let response = severity(&change(ChangeKind::ResponseSchemaChanged {
                                status: "200".into(),
                                change: value_change.clone(),
                            }));
                            json!([
                                value_change.pointer,
                                [tag, "other"],
                                severity_name(request),
                                severity_name(response)
                            ])
                        })
                        .collect(),
                )
            }
            _ => {
                let kind = schema_change(rust, pointer(&input[0]), kind(&input[1]));
                json!(severity_name(severity(&change(kind))))
            }
        };
        if &actual != expected {
            failures.push(format!(
                "{rust} case {index}: input {input}, Lean {expected}, Rust {actual}"
            ));
        }
    }
    failures
}

#[test]
fn oracle() {
    let replayed = ["Severity.json"];
    let mut present: Vec<String> = std::fs::read_dir(oracle_dir())
        .expect("formal/oracle")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.ends_with(".json"))
        .collect();
    present.sort();
    assert_eq!(
        present, replayed,
        "every formal/oracle/*.json file needs a replay here"
    );

    let mut failures = Vec::new();
    let mut total = 0;
    for file in replayed {
        let vectors = load(file);
        for function in vectors["functions"].as_array().expect("functions") {
            let rust = function["rust"].as_str().expect("rust name");
            let cases = function["cases"].as_array().expect("cases");
            assert!(!cases.is_empty(), "{file}: {rust} has no cases");
            total += cases.len();
            failures.extend(
                replay(rust, cases)
                    .into_iter()
                    .map(|f| format!("{file}: {f}")),
            );
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {total} oracle cases disagree with the Lean model:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
