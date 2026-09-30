//! JSON output for `vimanam diff` (#98): a machine-readable rendering of a
//! [`SpecDiff`] with a stable, content-derived ID per change.
//!
//! The output types here are **Serialize-only** and kept separate from the
//! domain types in [`crate::diff`]: the JSON contract must not drift when the
//! internals are refactored, so `Change`/`ChangeKind`/`ValueChange` stay
//! serde-free. Field declaration order on the structs sets the pretty-printed
//! key order and is deterministic.
//!
//! # Change identity
//!
//! Every record carries `id = "vc1_" + hex(SHA-256(canonical_bytes))` where
//! `canonical_bytes` is the canonical JSON (object keys sorted recursively in
//! byte order, compact separators) of `{ "v": 1, "endpoint": …, "kind": …,
//! "details": … }` — exactly the values emitted in the record, so the hash
//! input and the output can't disagree. Severity, display strings, timestamps,
//! file paths and `file_sha256` are deliberately left out: a future change to
//! the severity rule table must not change any ID. The `vc1_` prefix and the
//! `"v": 1` field both version the construction; any future change to it bumps
//! both. A change behind a shared `$ref` yields one record per affected
//! endpoint, each with its own ID, because the endpoint is part of the hash
//! input.

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

use crate::diff::{
    Change, ChangeKind, Deltas, Location, Severity, SpecDiff, ValueChange, ValueChangeKind, locate,
    severity,
};
use crate::utils::decode_json_pointer_token;

// ── output types ────────────────────────────────────────────────────────────

/// The complete JSON document printed by `vimanam diff --format json`.
#[derive(Debug, Serialize)]
pub struct JsonDiff {
    pub schema_version: u8,
    pub generator: JsonGenerator,
    pub old: JsonSide,
    pub new: JsonSide,
    pub summary: JsonSummary,
    pub changes: Vec<JsonChange>,
    /// Present only under `--report`; the key is omitted (never `null`)
    /// otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deltas: Option<JsonDeltas>,
}

#[derive(Debug, Serialize)]
pub struct JsonGenerator {
    pub name: &'static str,
    pub version: &'static str,
}

#[derive(Debug, Serialize)]
pub struct JsonSide {
    pub title: String,
    pub version: String,
    /// SHA-256 of the original input file bytes exactly as read from disk,
    /// before any parsing. Reformatting a spec changes this hash but not the
    /// change IDs.
    pub file_sha256: String,
}

#[derive(Debug, Serialize)]
pub struct JsonSummary {
    pub endpoints_added: usize,
    pub endpoints_removed: usize,
    pub endpoints_changed: usize,
    pub breaking: usize,
    pub non_breaking: usize,
    pub review: usize,
}

#[derive(Debug, Serialize)]
pub struct JsonChange {
    pub id: String,
    pub endpoint: JsonEndpoint,
    pub kind: JsonChangeKind,
    pub severity: JsonSeverity,
    pub details: JsonDetails,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonEndpoint {
    pub method: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonChangeKind {
    EndpointAdded,
    EndpointRemoved,
    ParameterAdded,
    ParameterRemoved,
    ParameterRequiredChanged,
    ParameterLocationChanged,
    ParameterSchemaChanged,
    ResponseAdded,
    ResponseRemoved,
    OperationIdChanged,
    DeprecatedChanged,
    RequestSchemaChanged,
    ResponseSchemaChanged,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonSeverity {
    Breaking,
    NonBreaking,
    Review,
}

/// The `details` object of a change record: an insertion-ordered map whose key
/// order follows the contract table (deterministic under serde_json's
/// `preserve_order`). Newtype so the record serialises it as a bare object.
#[derive(Debug, Clone, Serialize)]
pub struct JsonDetails(serde_json::Map<String, Value>);

#[derive(Debug, Serialize)]
pub struct JsonSchemaChange {
    /// RFC 6901 pointer exactly as the differ emits it. It addresses the
    /// *resolved, canonicalised* schema (`$ref`s inlined, annotation keywords
    /// stripped), not necessarily a location in the input file.
    pub pointer: String,
    /// What the pointer addresses, per the schema grammar (`locate`).
    pub target: JsonTarget,
    /// The decoded last pointer segment when the target is a named member;
    /// `null` otherwise.
    pub member: Option<String>,
    pub operation: JsonOperation,
    pub before: JsonPresence,
    pub after: JsonPresence,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonTarget {
    Type,
    RequiredMember,
    EnumMember,
    Property,
    AdditionalProperties,
    Nullable,
    Other,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonOperation {
    Added,
    Removed,
    Changed,
}

/// Either `{ "present": false }` or `{ "present": true, "value": <json> }`.
/// A present `value` may legitimately be JSON `null` (`default: null`,
/// `enum: [null]`) — that is distinct from absence.
#[derive(Debug, Serialize)]
pub struct JsonPresence {
    pub present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct JsonDeltas {
    pub hygiene: Vec<JsonHygieneRow>,
    pub tokens: JsonTokenEstimate,
}

#[derive(Debug, Serialize)]
pub struct JsonHygieneRow {
    pub check: String,
    pub old: usize,
    pub new: usize,
}

#[derive(Debug, Serialize)]
pub struct JsonTokenEstimate {
    pub old: usize,
    pub new: usize,
    pub estimate: &'static str,
    pub detail: &'static str,
}

// ── construction ────────────────────────────────────────────────────────────

/// Builds the JSON document for a [`SpecDiff`]. `old_sha`/`new_sha` are the
/// lowercase-hex SHA-256 of the original input file bytes; `deltas` is passed
/// only under `--report`.
pub fn to_json(diff: &SpecDiff, deltas: Option<&Deltas>, old_sha: &str, new_sha: &str) -> JsonDiff {
    JsonDiff {
        schema_version: 1,
        generator: JsonGenerator {
            name: "vimanam",
            version: env!("CARGO_PKG_VERSION"),
        },
        old: JsonSide {
            title: diff.old_title.clone(),
            version: diff.old_version.clone(),
            file_sha256: old_sha.to_string(),
        },
        new: JsonSide {
            title: diff.new_title.clone(),
            version: diff.new_version.clone(),
            file_sha256: new_sha.to_string(),
        },
        summary: JsonSummary {
            endpoints_added: diff.endpoints_added(),
            endpoints_removed: diff.endpoints_removed(),
            endpoints_changed: diff.endpoints_changed(),
            breaking: diff.count(Severity::Breaking),
            non_breaking: diff.count(Severity::NonBreaking),
            review: diff.count(Severity::Review),
        },
        changes: diff.changes.iter().map(json_change).collect(),
        deltas: deltas.map(|deltas| JsonDeltas {
            hygiene: deltas
                .hygiene
                .iter()
                .map(|(check, old, new)| JsonHygieneRow {
                    check: (*check).to_string(),
                    old: *old,
                    new: *new,
                })
                .collect(),
            tokens: JsonTokenEstimate {
                old: deltas.tokens_old,
                new: deltas.tokens_new,
                estimate: "chars/4",
                detail: "full+schemas",
            },
        }),
    }
}

/// The stable ID of a change record: `vc1_` + hex(SHA-256(canonical_json({
/// v, endpoint, kind, details }))). Derived from the serialised record
/// itself, so the hash input can never disagree with the emitted document.
pub fn change_id(change: &JsonChange) -> String {
    #[derive(Serialize)]
    struct IdInput<'a> {
        v: u8,
        endpoint: &'a JsonEndpoint,
        kind: &'a JsonChangeKind,
        details: &'a JsonDetails,
    }

    let input = IdInput {
        v: 1,
        endpoint: &change.endpoint,
        kind: &change.kind,
        details: &change.details,
    };
    let value = serde_json::to_value(&input).expect("ID input always serialises");
    format!("vc1_{}", sha256_hex(canonical_json(&value).as_bytes()))
}

fn json_change(change: &Change) -> JsonChange {
    let (kind, details) = json_kind_and_details(&change.kind);
    let mut record = JsonChange {
        id: String::new(),
        endpoint: JsonEndpoint {
            method: change.endpoint.method.clone(),
            path: change.endpoint.path.clone(),
        },
        kind,
        severity: severity(change).into(),
        details,
    };
    record.id = change_id(&record);
    record
}

/// Maps a [`ChangeKind`] to its contract `(kind, details)` pair. One arm per
/// variant, so adding a variant fails to compile here instead of silently
/// missing from the JSON contract.
fn json_kind_and_details(kind: &ChangeKind) -> (JsonChangeKind, JsonDetails) {
    use serde_json::json;

    /// Builds the insertion-ordered details map for a record.
    fn details(entries: Vec<(&'static str, Value)>) -> JsonDetails {
        JsonDetails(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        )
    }

    match kind {
        ChangeKind::EndpointAdded => (JsonChangeKind::EndpointAdded, details(Vec::new())),
        ChangeKind::EndpointRemoved { was_deprecated } => (
            JsonChangeKind::EndpointRemoved,
            details(vec![("was_deprecated", json!(was_deprecated))]),
        ),
        ChangeKind::ParameterAdded {
            name,
            location,
            required,
        } => (
            JsonChangeKind::ParameterAdded,
            details(vec![
                ("name", json!(name)),
                ("location", json!(location)),
                ("required", json!(required)),
            ]),
        ),
        ChangeKind::ParameterRemoved { name, location } => (
            JsonChangeKind::ParameterRemoved,
            details(vec![("name", json!(name)), ("location", json!(location))]),
        ),
        ChangeKind::ParameterRequiredChanged {
            name,
            location,
            now_required,
        } => (
            JsonChangeKind::ParameterRequiredChanged,
            details(vec![
                ("name", json!(name)),
                ("location", json!(location)),
                ("now_required", json!(now_required)),
            ]),
        ),
        ChangeKind::ParameterLocationChanged {
            name,
            old_location,
            new_location,
        } => (
            JsonChangeKind::ParameterLocationChanged,
            details(vec![
                ("name", json!(name)),
                ("old_location", json!(old_location)),
                ("new_location", json!(new_location)),
            ]),
        ),
        ChangeKind::ParameterSchemaChanged {
            name,
            location,
            change,
        } => (
            JsonChangeKind::ParameterSchemaChanged,
            details(vec![
                ("name", json!(name)),
                ("location", json!(location)),
                ("schema_change", json!(json_schema_change(change))),
            ]),
        ),
        ChangeKind::ResponseAdded { status } => (
            JsonChangeKind::ResponseAdded,
            details(vec![("status", json!(status))]),
        ),
        ChangeKind::ResponseRemoved { status } => (
            JsonChangeKind::ResponseRemoved,
            details(vec![("status", json!(status))]),
        ),
        // The IR really has Option<String> here, so `null` is a legitimate
        // value in the contract.
        ChangeKind::OperationIdChanged { old, new } => (
            JsonChangeKind::OperationIdChanged,
            details(vec![("old", json!(old)), ("new", json!(new))]),
        ),
        ChangeKind::DeprecatedChanged { now } => (
            JsonChangeKind::DeprecatedChanged,
            details(vec![("now", json!(now))]),
        ),
        ChangeKind::RequestSchemaChanged { change } => (
            JsonChangeKind::RequestSchemaChanged,
            details(vec![("schema_change", json!(json_schema_change(change)))]),
        ),
        ChangeKind::ResponseSchemaChanged { status, change } => (
            JsonChangeKind::ResponseSchemaChanged,
            details(vec![
                ("status", json!(status)),
                ("schema_change", json!(json_schema_change(change))),
            ]),
        ),
    }
}

impl From<Severity> for JsonSeverity {
    fn from(severity: Severity) -> Self {
        match severity {
            Severity::Breaking => JsonSeverity::Breaking,
            Severity::NonBreaking => JsonSeverity::NonBreaking,
            Severity::Review => JsonSeverity::Review,
        }
    }
}

// ── schema_change ───────────────────────────────────────────────────────────

fn json_schema_change(change: &ValueChange) -> JsonSchemaChange {
    let at_root = change.pointer.is_empty();
    let (target, member) = target_and_member(&change.pointer);

    JsonSchemaChange {
        pointer: change.pointer.clone(),
        target,
        member,
        operation: match &change.kind {
            ValueChangeKind::Added(_) => JsonOperation::Added,
            ValueChangeKind::Removed(_) => JsonOperation::Removed,
            ValueChangeKind::Changed { old, new } => {
                // The differ encodes "no schema at all" as `Value::Null` at
                // the root pointer, so a root null side means the schema
                // appeared or vanished rather than changed to/from null.
                if at_root && old.is_null() {
                    JsonOperation::Added
                } else if at_root && new.is_null() {
                    JsonOperation::Removed
                } else {
                    JsonOperation::Changed
                }
            }
        },
        before: presence(old_side(&change.kind), at_root),
        after: presence(new_side(&change.kind), at_root),
    }
}

fn old_side(kind: &ValueChangeKind) -> Option<&Value> {
    match kind {
        ValueChangeKind::Added(_) => None,
        ValueChangeKind::Removed(old) | ValueChangeKind::Changed { old, .. } => Some(old),
    }
}

fn new_side(kind: &ValueChangeKind) -> Option<&Value> {
    match kind {
        ValueChangeKind::Added(new) | ValueChangeKind::Changed { new, .. } => Some(new),
        ValueChangeKind::Removed(_) => None,
    }
}

fn presence(side: Option<&Value>, at_root: bool) -> JsonPresence {
    match side {
        // A side the `ValueChangeKind` does not carry is absent.
        None => JsonPresence {
            present: false,
            value: None,
        },
        // At the root pointer `Value::Null` means "no schema at all"
        // (`optional_schema_value`/`parameter_schema_value`); everywhere else
        // it is a real `null` value.
        Some(value) if at_root && value.is_null() => JsonPresence {
            present: false,
            value: None,
        },
        Some(value) => JsonPresence {
            present: true,
            value: Some(value.clone()),
        },
    }
}

/// Classifies a pointer with the schema grammar ([`locate`]) — never by
/// guessing from the last segment, so a property named `type` is a `property`,
/// not the `type` keyword.
fn target_and_member(pointer: &str) -> (JsonTarget, Option<String>) {
    let member = || -> Option<String> {
        pointer
            .rsplit('/')
            .next()
            .filter(|segment| !segment.is_empty())
            .map(decode_json_pointer_token)
    };

    match locate(pointer) {
        Location::Type => (JsonTarget::Type, None),
        Location::RequiredElement => (JsonTarget::RequiredMember, member()),
        Location::EnumElement => (JsonTarget::EnumMember, member()),
        Location::Property => (JsonTarget::Property, member()),
        Location::AdditionalProperties => (JsonTarget::AdditionalProperties, None),
        Location::Nullable => (JsonTarget::Nullable, None),
        Location::Other => (JsonTarget::Other, None),
    }
}

// ── hashing helpers ─────────────────────────────────────────────────────────

/// Lowercase-hex SHA-256. Used for both `file_sha256` and change IDs.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

/// Serialises `value` as canonical JSON: object keys sorted recursively in
/// byte order, compact separators, strings and numbers as `serde_json`
/// serialises them. The crate enables serde_json's `preserve_order`, so maps
/// keep insertion order unless sorted explicitly — which is what makes the
/// change-ID hash input reproducible. (Not to be confused with
/// `utils::canonicalize_schema_value`, which strips schema annotation
/// keywords; that is schema semantics, this is serialisation.)
pub fn canonical_json(value: &Value) -> String {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                keys.into_iter()
                    .map(|key| (key.clone(), canonical(&map[key])))
                    .collect::<serde_json::Map<String, Value>>()
                    .into()
            }
            Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
            other => other.clone(),
        }
    }

    canonical(value).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::EndpointRef;
    use serde_json::json;

    /// Builds a JSON record for `kind` on a fixed endpoint.
    fn record(kind: ChangeKind) -> JsonChange {
        json_change(&Change {
            endpoint: EndpointRef {
                method: "GET".to_string(),
                path: "/widgets".to_string(),
            },
            kind,
        })
    }

    /// `(kind string, details value)` of a record.
    fn contract(record: &JsonChange) -> (String, Value) {
        (
            serde_json::to_value(&record.kind)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string(),
            serde_json::to_value(&record.details).unwrap(),
        )
    }

    fn value_change(pointer: &str, kind: ValueChangeKind) -> ValueChange {
        ValueChange {
            pointer: pointer.to_string(),
            kind,
        }
    }

    // ── kind/details mapping: one test per ChangeKind variant ───────────────

    #[test]
    fn endpoint_added_maps_to_empty_details() {
        let (kind, details) = contract(&record(ChangeKind::EndpointAdded));
        assert_eq!(kind, "endpoint_added");
        assert_eq!(details, json!({}));
    }

    #[test]
    fn endpoint_removed_maps_was_deprecated() {
        let (kind, details) = contract(&record(ChangeKind::EndpointRemoved {
            was_deprecated: true,
        }));
        assert_eq!(kind, "endpoint_removed");
        assert_eq!(details, json!({ "was_deprecated": true }));
    }

    #[test]
    fn parameter_added_maps_name_location_required() {
        let (kind, details) = contract(&record(ChangeKind::ParameterAdded {
            name: "limit".into(),
            location: "query".into(),
            required: false,
        }));
        assert_eq!(kind, "parameter_added");
        assert_eq!(
            details,
            json!({ "name": "limit", "location": "query", "required": false })
        );
    }

    #[test]
    fn parameter_removed_maps_name_location() {
        let (kind, details) = contract(&record(ChangeKind::ParameterRemoved {
            name: "limit".into(),
            location: "query".into(),
        }));
        assert_eq!(kind, "parameter_removed");
        assert_eq!(details, json!({ "name": "limit", "location": "query" }));
    }

    #[test]
    fn parameter_required_changed_maps_now_required() {
        let (kind, details) = contract(&record(ChangeKind::ParameterRequiredChanged {
            name: "fields".into(),
            location: "query".into(),
            now_required: true,
        }));
        assert_eq!(kind, "parameter_required_changed");
        assert_eq!(
            details,
            json!({ "name": "fields", "location": "query", "now_required": true })
        );
    }

    #[test]
    fn parameter_location_changed_maps_locations() {
        let (kind, details) = contract(&record(ChangeKind::ParameterLocationChanged {
            name: "X-Trace".into(),
            old_location: "header".into(),
            new_location: "query".into(),
        }));
        assert_eq!(kind, "parameter_location_changed");
        assert_eq!(
            details,
            json!({ "name": "X-Trace", "old_location": "header", "new_location": "query" })
        );
    }

    #[test]
    fn parameter_schema_changed_maps_nested_schema_change() {
        let (kind, details) = contract(&record(ChangeKind::ParameterSchemaChanged {
            name: "limit".into(),
            location: "query".into(),
            change: value_change(
                "/type",
                ValueChangeKind::Changed {
                    old: json!("integer"),
                    new: json!("string"),
                },
            ),
        }));
        assert_eq!(kind, "parameter_schema_changed");
        assert_eq!(
            details,
            json!({
                "name": "limit",
                "location": "query",
                "schema_change": {
                    "pointer": "/type",
                    "target": "type",
                    "member": null,
                    "operation": "changed",
                    "before": { "present": true, "value": "integer" },
                    "after": { "present": true, "value": "string" }
                }
            })
        );
    }

    #[test]
    fn response_added_maps_status() {
        let (kind, details) = contract(&record(ChangeKind::ResponseAdded {
            status: "404".into(),
        }));
        assert_eq!(kind, "response_added");
        assert_eq!(details, json!({ "status": "404" }));
    }

    #[test]
    fn response_removed_maps_status() {
        let (kind, details) = contract(&record(ChangeKind::ResponseRemoved {
            status: "default".into(),
        }));
        assert_eq!(kind, "response_removed");
        assert_eq!(details, json!({ "status": "default" }));
    }

    #[test]
    fn operation_id_changed_maps_nullable_old_and_new() {
        let (kind, details) = contract(&record(ChangeKind::OperationIdChanged {
            old: Some("listWidgets".into()),
            new: None,
        }));
        assert_eq!(kind, "operation_id_changed");
        assert_eq!(details, json!({ "old": "listWidgets", "new": null }));
    }

    #[test]
    fn deprecated_changed_maps_now() {
        let (kind, details) = contract(&record(ChangeKind::DeprecatedChanged { now: true }));
        assert_eq!(kind, "deprecated_changed");
        assert_eq!(details, json!({ "now": true }));
    }

    #[test]
    fn request_schema_changed_maps_schema_change() {
        let (kind, details) = contract(&record(ChangeKind::RequestSchemaChanged {
            change: value_change(
                "/properties/weight/format",
                ValueChangeKind::Changed {
                    old: json!("float"),
                    new: json!("double"),
                },
            ),
        }));
        assert_eq!(kind, "request_schema_changed");
        assert_eq!(
            details,
            json!({
                "schema_change": {
                    "pointer": "/properties/weight/format",
                    "target": "other",
                    "member": null,
                    "operation": "changed",
                    "before": { "present": true, "value": "float" },
                    "after": { "present": true, "value": "double" }
                }
            })
        );
    }

    #[test]
    fn response_schema_changed_maps_status_and_schema_change() {
        let (kind, details) = contract(&record(ChangeKind::ResponseSchemaChanged {
            status: "200".into(),
            change: value_change(
                "/properties/pricing",
                ValueChangeKind::Added(json!({ "type": "number" })),
            ),
        }));
        assert_eq!(kind, "response_schema_changed");
        assert_eq!(
            details,
            json!({
                "status": "200",
                "schema_change": {
                    "pointer": "/properties/pricing",
                    "target": "property",
                    "member": "pricing",
                    "operation": "added",
                    "before": { "present": false },
                    "after": { "present": true, "value": { "type": "number" } }
                }
            })
        );
    }

    // ── presence encoding ───────────────────────────────────────────────────

    #[test]
    fn explicit_null_value_is_present_elsewhere_but_absent_at_root() {
        // `default: null` → `default: "x"`: the null before-value is real.
        let change = value_change(
            "/properties/token/default",
            ValueChangeKind::Changed {
                old: Value::Null,
                new: json!("none"),
            },
        );
        let rendered = json!(json_schema_change(&change));
        assert_eq!(
            rendered["before"],
            json!({ "present": true, "value": null })
        );
        assert_eq!(
            rendered["after"],
            json!({ "present": true, "value": "none" })
        );
        assert_eq!(rendered["operation"], json!("changed"));

        // At the root pointer the same Null means "no schema": absent.
        let appeared = value_change(
            "",
            ValueChangeKind::Changed {
                old: Value::Null,
                new: json!({ "type": "integer" }),
            },
        );
        let rendered = json!(json_schema_change(&appeared));
        assert_eq!(rendered["before"], json!({ "present": false }));
        assert_eq!(
            rendered["after"],
            json!({ "present": true, "value": { "type": "integer" } })
        );
        // …and the operation flips to `added`.
        assert_eq!(rendered["operation"], json!("added"));

        let vanished = value_change(
            "",
            ValueChangeKind::Changed {
                old: json!({ "type": "integer" }),
                new: Value::Null,
            },
        );
        let rendered = json!(json_schema_change(&vanished));
        assert_eq!(
            rendered["before"],
            json!({ "present": true, "value": { "type": "integer" } })
        );
        assert_eq!(rendered["after"], json!({ "present": false }));
        assert_eq!(rendered["operation"], json!("removed"));
    }

    #[test]
    fn added_and_removed_sides_encode_absence() {
        let added = value_change("/properties/pricing", ValueChangeKind::Added(json!(1)));
        let rendered = json!(json_schema_change(&added));
        assert_eq!(rendered["before"], json!({ "present": false }));
        assert_eq!(rendered["after"], json!({ "present": true, "value": 1 }));

        let removed = value_change("/properties/pricing", ValueChangeKind::Removed(json!(1)));
        let rendered = json!(json_schema_change(&removed));
        assert_eq!(rendered["before"], json!({ "present": true, "value": 1 }));
        assert_eq!(rendered["after"], json!({ "present": false }));
    }

    #[test]
    fn enum_member_gaining_null_keeps_operation() {
        // `enum: [null]` → `enum: [null, "custom"]`: the new member is an
        // addition; the *value* "custom" is a string, and the existing null
        // member is untouched (no record for it).
        let added = value_change("/enum/custom", ValueChangeKind::Added(json!("custom")));
        let rendered = json!(json_schema_change(&added));
        assert_eq!(rendered["target"], json!("enum_member"));
        assert_eq!(rendered["member"], json!("custom"));
        assert_eq!(rendered["operation"], json!("added"));
        assert_eq!(rendered["before"], json!({ "present": false }));

        // A *changed* null member value away from the root stays present.
        let changed = value_change(
            "/enum/0",
            ValueChangeKind::Changed {
                old: Value::Null,
                new: json!("none"),
            },
        );
        let rendered = json!(json_schema_change(&changed));
        assert_eq!(
            rendered["before"],
            json!({ "present": true, "value": null })
        );
    }

    // ── target/member classification ────────────────────────────────────────

    #[test]
    fn target_distinguishes_property_named_type_from_type_keyword() {
        // A property called "type" is a property …
        let change = value_change("/properties/type", ValueChangeKind::Added(json!({})));
        let rendered = json!(json_schema_change(&change));
        assert_eq!(rendered["target"], json!("property"));
        assert_eq!(rendered["member"], json!("type"));

        // … while the keyword is `type`.
        let change = value_change(
            "/type",
            ValueChangeKind::Changed {
                old: json!("string"),
                new: json!("integer"),
            },
        );
        let rendered = json!(json_schema_change(&change));
        assert_eq!(rendered["target"], json!("type"));
        assert_eq!(rendered["member"], Value::Null);
    }

    #[test]
    fn target_resolves_required_enum_and_other_keywords() {
        let change = value_change("/required/x", ValueChangeKind::Added(json!("x")));
        let rendered = json!(json_schema_change(&change));
        assert_eq!(rendered["target"], json!("required_member"));
        assert_eq!(rendered["member"], json!("x"));

        let change = value_change("/enum/z", ValueChangeKind::Added(json!("z")));
        let rendered = json!(json_schema_change(&change));
        assert_eq!(rendered["target"], json!("enum_member"));
        assert_eq!(rendered["member"], json!("z"));

        // A pointer token is decoded before it becomes `member`.
        let change = value_change("/properties/a~1b", ValueChangeKind::Added(json!({})));
        let rendered = json!(json_schema_change(&change));
        assert_eq!(rendered["target"], json!("property"));
        assert_eq!(rendered["member"], json!("a/b"));

        let change = value_change(
            "/additionalProperties",
            ValueChangeKind::Removed(json!(false)),
        );
        let rendered = json!(json_schema_change(&change));
        assert_eq!(rendered["target"], json!("additional_properties"));
        assert_eq!(rendered["member"], Value::Null);

        let change = value_change("/nullable", ValueChangeKind::Removed(json!(true)));
        let rendered = json!(json_schema_change(&change));
        assert_eq!(rendered["target"], json!("nullable"));

        let change = value_change(
            "/format",
            ValueChangeKind::Changed {
                old: json!("float"),
                new: json!("double"),
            },
        );
        let rendered = json!(json_schema_change(&change));
        assert_eq!(rendered["target"], json!("other"));
        assert_eq!(rendered["member"], Value::Null);
    }

    // ── canonical JSON and identity ─────────────────────────────────────────

    #[test]
    fn canonical_json_sorts_object_keys_recursively_but_keeps_array_order() {
        let value = json!({
            "b": 1,
            "a": { "d": [json!({ "z": 1, "y": 2 }), "x"], "c": 3 },
        });
        assert_eq!(
            canonical_json(&value),
            r#"{"a":{"c":3,"d":[{"y":2,"z":1},"x"]},"b":1}"#
        );
        // Scalars pass through serde_json's compact form.
        assert_eq!(canonical_json(&json!("x")), r#""x""#);
        assert_eq!(canonical_json(&Value::Null), "null");
        assert_eq!(canonical_json(&json!(true)), "true");
        assert_eq!(canonical_json(&json!(1.5)), "1.5");
    }

    #[test]
    fn change_id_is_sha256_of_canonical_identity() {
        // Pinned independently (sha256sum of the canonical bytes below).
        let change = record(ChangeKind::EndpointAdded);
        let canonical = r#"{"details":{},"endpoint":{"method":"GET","path":"/widgets"},"kind":"endpoint_added","v":1}"#;
        let expected = format!("vc1_{}", sha256_hex(canonical.as_bytes()));
        assert_eq!(change.id, expected);
    }

    #[test]
    fn change_id_ignores_severity() {
        // Severity is not part of the hash input: two records differing only
        // in severity must have the same ID, so a future change to the
        // severity rule table cannot rewrite history.
        let with_breaking = JsonChange {
            id: String::new(),
            endpoint: JsonEndpoint {
                method: "GET".to_string(),
                path: "/widgets".to_string(),
            },
            kind: JsonChangeKind::EndpointAdded,
            details: JsonDetails(serde_json::Map::new()),
            severity: JsonSeverity::Breaking,
        };
        let mut with_review = JsonChange {
            id: String::new(),
            endpoint: with_breaking.endpoint.clone(),
            kind: JsonChangeKind::EndpointAdded,
            details: JsonDetails(serde_json::Map::new()),
            severity: JsonSeverity::Review,
        };
        assert_eq!(change_id(&with_breaking), change_id(&with_review));

        // The endpoint, by contrast, IS hashed.
        with_review.endpoint.path = "/elsewhere".to_string();
        assert_ne!(change_id(&with_breaking), change_id(&with_review));

        // And the record constructor produces the same ID it would hash.
        let built = record(ChangeKind::EndpointAdded);
        assert_eq!(built.id, change_id(&built));
    }

    #[test]
    fn different_details_give_different_ids() {
        let a = record(ChangeKind::ResponseSchemaChanged {
            status: "200".into(),
            change: value_change(
                "/properties/price/type",
                ValueChangeKind::Changed {
                    old: json!("string"),
                    new: json!("integer"),
                },
            ),
        });
        let b = record(ChangeKind::ResponseSchemaChanged {
            status: "200".into(),
            change: value_change(
                "/properties/price/type",
                ValueChangeKind::Changed {
                    old: json!("string"),
                    new: json!("boolean"),
                },
            ),
        });
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn sha256_hex_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn to_json_summary_matches_specdiff_counts() {
        let diff = crate::diff::diff(
            &crate::parser::parse_openapi("tests/fixtures/diff_old_oas3.json").unwrap(),
            &crate::parser::parse_openapi("tests/fixtures/diff_new_oas3.json").unwrap(),
        );
        let document = to_json(
            &diff,
            None,
            "a".repeat(64).as_str(),
            "b".repeat(64).as_str(),
        );
        assert_eq!(document.schema_version, 1);
        assert_eq!(document.generator.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(document.summary.endpoints_added, diff.endpoints_added());
        assert_eq!(document.summary.endpoints_removed, diff.endpoints_removed());
        assert_eq!(document.summary.endpoints_changed, diff.endpoints_changed());
        assert_eq!(document.summary.breaking, diff.count(Severity::Breaking));
        assert_eq!(
            document.summary.non_breaking,
            diff.count(Severity::NonBreaking)
        );
        assert_eq!(document.summary.review, diff.count(Severity::Review));
        assert_eq!(document.changes.len(), diff.changes.len());
        // Order follows SpecDiff::changes, and IDs are unique in the document.
        let mut ids: Vec<&str> = document.changes.iter().map(|c| c.id.as_str()).collect();
        let expected_order: Vec<String> = diff
            .changes
            .iter()
            .map(|change| json_change(change).id)
            .collect();
        let actual_order: Vec<String> = document.changes.iter().map(|c| c.id.clone()).collect();
        assert_eq!(actual_order, expected_order);
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), document.changes.len(), "IDs must be unique");
        // deltas stays omitted without --report.
        assert!(document.deltas.is_none());
    }
}
