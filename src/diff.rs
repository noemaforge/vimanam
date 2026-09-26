//! `vimanam diff <OLD> <NEW>`: compares two versions of a spec and reports
//! what changed, classified as breaking, non-breaking or needing review.
//!
//! [`diff`] derives a data-only [`SpecDiff`] from two intermediate
//! representations; [`severity`] classifies each [`Change`]; [`write_diff`]
//! renders the result as Markdown. Keeping the three apart lets the rule table
//! be unit-tested row by row and lets another front end reuse the data.
//!
//! # What is compared
//!
//! Endpoints are matched by `(METHOD, path)`, parameters by `(name, in)` and
//! responses by status code. Request and response bodies are compared as
//! **resolved** schemas: every `$ref` is inlined
//! ([`resolve_schema_value`]) and the result canonicalised
//! ([`canonicalize_schema_value`]) before a generic JSON differ
//! ([`diff_values`]) reports field-level changes as JSON pointers. That is what
//! makes a change behind a shared component visible: an operation whose object
//! is byte-identical in both specs still reports "response schema changed" when
//! the `Widget` it points at gained a required property.
//!
//! Only a response's first media type is compared (the same one the renderer
//! documents, see [`response_schema`]), and only the first request-body media
//! type. `allOf`/`oneOf`/`anyOf` lists are compared index-wise. A path-template
//! rename (`/pets/{id}` → `/pets/{petId}`) appears as a removal plus an
//! addition.
//!
//! # Determinism
//!
//! Changes are emitted in old-spec order (old endpoints first, then endpoints
//! only the new spec has, each in its spec's order) and the canonical schema
//! values have sorted keys, so the same pair of specs always produces
//! byte-identical output.

use std::collections::{HashMap, HashSet};
use std::io::Write;

use anyhow::{Context, Result};
use indexmap::IndexSet;
use serde_json::Value;

use crate::markdown::{estimate_tokens, generate_markdown, response_schema};
use crate::models::{ApiDocumentation, DocConfig, Endpoint, Parameter};
use crate::report::{self, EndpointRef};
use crate::utils::{canonicalize_schema_value, decode_json_pointer_token, resolve_schema_value};

// ── generic value differ ────────────────────────────────────────────────────

/// One difference between two canonical JSON values.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueChange {
    /// JSON pointer (RFC 6901) from the schema root, e.g. `/properties/pricing`
    /// or `/items/type`. Elements of the order-insensitive `required` and
    /// `enum` sets are addressed as `/required/<name>` and `/enum/<value>`.
    /// The empty pointer is the root itself.
    pub pointer: String,
    pub kind: ValueChangeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ValueChangeKind {
    Added(Value),
    Removed(Value),
    Changed { old: Value, new: Value },
}

// ── schema grammar ──────────────────────────────────────────────────────────

/// What a JSON pointer into a schema addresses, as far as the severity rules
/// care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Location {
    /// A `type` keyword.
    Type,
    /// A member of a `required` array.
    RequiredElement,
    /// A member of an `enum` array.
    EnumElement,
    /// A named property (the schema under `properties/<name>`).
    Property,
    /// An `additionalProperties` keyword.
    AdditionalProperties,
    /// A `nullable` keyword.
    Nullable,
    /// Anything else (`format`, `minimum`, an `allOf` variant, ...).
    Other,
}

/// The grammatical role of one node in a schema tree. Both the differ
/// ([`diff_values`]) and the pointer classifier ([`locate`]) walk the tree with
/// this state, so keyword semantics (set comparison of `required`, per-member
/// reporting of `properties`, "`type` is a type") apply only where the JSON
/// Schema grammar puts a keyword — never to a *property* that happens to be
/// called `properties`, `required`, `enum` or `type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Node {
    /// A schema object: its keys are keywords.
    Schema,
    /// A `properties`/`patternProperties` map: its keys are field names, its
    /// values schemas.
    PropertyMap,
    /// An `allOf`/`oneOf`/`anyOf` list: its elements are schemas.
    SchemaList,
    /// A `required`/`enum` array: compared as a set of opaque members.
    Set(Location),
    /// Anything else (`default`, `example`, a set member, an unknown keyword's
    /// value): compared generically, with no keyword semantics.
    Opaque,
}

impl Node {
    /// The role of the child reached from `self` through `segment` (an object
    /// key or array index).
    fn child(self, segment: &str) -> Node {
        match self {
            Node::Schema => match segment {
                "properties" | "patternProperties" => Node::PropertyMap,
                "allOf" | "oneOf" | "anyOf" => Node::SchemaList,
                "items" | "additionalProperties" | "not" => Node::Schema,
                "required" => Node::Set(Location::RequiredElement),
                "enum" => Node::Set(Location::EnumElement),
                _ => Node::Opaque,
            },
            Node::PropertyMap | Node::SchemaList => Node::Schema,
            Node::Set(_) | Node::Opaque => Node::Opaque,
        }
    }

    /// What the child reached from `self` through `segment` *is*, for the
    /// severity rules.
    fn locate(self, segment: &str) -> Location {
        match self {
            Node::Schema => match segment {
                "type" => Location::Type,
                "nullable" => Location::Nullable,
                "additionalProperties" => Location::AdditionalProperties,
                _ => Location::Other,
            },
            Node::PropertyMap => Location::Property,
            Node::Set(element) => element,
            Node::SchemaList | Node::Opaque => Location::Other,
        }
    }
}

/// Classifies `pointer` by walking it with the schema grammar in mind: under
/// `properties` the next segment is a field name (so a field called `type` is a
/// property, not a type keyword), under `allOf` it is an index, and so on.
pub(crate) fn locate(pointer: &str) -> Location {
    let mut node = Node::Schema;
    let mut location = Location::Other;
    for segment in pointer.split('/').skip(1) {
        let segment = decode_json_pointer_token(segment);
        location = node.locate(&segment);
        node = node.child(&segment);
    }
    location
}

/// The value to compare `present` against when the key for `child` exists on
/// only one side of a schema object: an empty set for `required`/`enum` and an
/// empty map for `properties`, so that members are reported one by one
/// (`/required/x`, `/properties/x`) instead of as a single opaque change at
/// `/required` or `/properties`. `None` for every other node, which is then
/// reported whole.
fn empty_counterpart(child: Node, present: &Value) -> Option<Value> {
    match child {
        Node::Set(_) if present.is_array() => Some(Value::Array(Vec::new())),
        Node::PropertyMap if present.is_object() => Some(Value::Object(serde_json::Map::new())),
        _ => None,
    }
}

/// Encodes a JSON Pointer reference token (RFC 6901): `~` → `~0`, `/` → `~1`.
fn encode_json_pointer_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// The pointer segment used for a member of a `required`/`enum` set: the bare
/// string for string members, compact JSON otherwise.
fn set_element_segment(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Appends `/segment` to `path`, returning the length to truncate back to.
fn push_segment(path: &mut String, segment: &str) -> usize {
    let mark = path.len();
    path.push('/');
    path.push_str(&encode_json_pointer_token(segment));
    mark
}

/// Reports every difference between `old` and `new` into `out`, addressing each
/// by JSON pointer under `path` (pass an empty `String` for the root). Both
/// values are taken to be schemas.
///
/// Objects are compared key-wise: the old object's keys in order, then any key
/// only the new object has. Where the schema grammar says a keyword sits, the
/// `required` and `enum` arrays are compared as sets (a missing array counts as
/// empty), reporting each member added or removed at `/required/<member>`;
/// likewise a `properties` map present on one side only reports each property
/// (`/properties/<name>`) rather than the map as a whole. A *property* named
/// `required` or `properties` gets none of that treatment (see [`Node`]).
/// Every other array is compared index-wise, so a longer list reports `Added`
/// at the extra indices (`/allOf/2`) and a shorter one `Removed`. Scalars that
/// differ are `Changed`.
pub fn diff_values(old: &Value, new: &Value, path: &mut String, out: &mut Vec<ValueChange>) {
    diff_nodes(old, new, Node::Schema, path, out);
}

fn diff_nodes(old: &Value, new: &Value, node: Node, path: &mut String, out: &mut Vec<ValueChange>) {
    match (old, new) {
        (Value::Object(old_map), Value::Object(new_map)) => {
            for (key, old_child) in old_map {
                let child = node.child(key);
                let mark = push_segment(path, key);
                match (new_map.get(key), empty_counterpart(child, old_child)) {
                    (Some(new_child), _) => diff_nodes(old_child, new_child, child, path, out),
                    (None, Some(empty)) => diff_nodes(old_child, &empty, child, path, out),
                    (None, None) => out.push(ValueChange {
                        pointer: path.clone(),
                        kind: ValueChangeKind::Removed(old_child.clone()),
                    }),
                }
                path.truncate(mark);
            }
            for (key, new_child) in new_map {
                if old_map.contains_key(key) {
                    continue;
                }
                let child = node.child(key);
                let mark = push_segment(path, key);
                match empty_counterpart(child, new_child) {
                    Some(empty) => diff_nodes(&empty, new_child, child, path, out),
                    None => out.push(ValueChange {
                        pointer: path.clone(),
                        kind: ValueChangeKind::Added(new_child.clone()),
                    }),
                }
                path.truncate(mark);
            }
        }
        (Value::Array(_), Value::Array(_)) if matches!(node, Node::Set(_)) => {
            diff_sets(old, new, path, out)
        }
        (Value::Array(old_items), Value::Array(new_items)) => {
            let common = old_items.len().min(new_items.len());
            for index in 0..common {
                let segment = index.to_string();
                let child = node.child(&segment);
                let mark = push_segment(path, &segment);
                diff_nodes(&old_items[index], &new_items[index], child, path, out);
                path.truncate(mark);
            }
            for (index, item) in old_items.iter().enumerate().skip(common) {
                let mark = push_segment(path, &index.to_string());
                out.push(ValueChange {
                    pointer: path.clone(),
                    kind: ValueChangeKind::Removed(item.clone()),
                });
                path.truncate(mark);
            }
            for (index, item) in new_items.iter().enumerate().skip(common) {
                let mark = push_segment(path, &index.to_string());
                out.push(ValueChange {
                    pointer: path.clone(),
                    kind: ValueChangeKind::Added(item.clone()),
                });
                path.truncate(mark);
            }
        }
        _ if old == new => {}
        _ => out.push(ValueChange {
            pointer: path.clone(),
            kind: ValueChangeKind::Changed {
                old: old.clone(),
                new: new.clone(),
            },
        }),
    }
}

/// Set difference for `required`/`enum`: members only in `old` are `Removed`,
/// members only in `new` are `Added`, each addressed as `<path>/<member>`.
fn diff_sets(old: &Value, new: &Value, path: &mut String, out: &mut Vec<ValueChange>) {
    let old_items = old.as_array().map(Vec::as_slice).unwrap_or_default();
    let new_items = new.as_array().map(Vec::as_slice).unwrap_or_default();

    for item in old_items.iter().filter(|item| !new_items.contains(item)) {
        let mark = push_segment(path, &set_element_segment(item));
        out.push(ValueChange {
            pointer: path.clone(),
            kind: ValueChangeKind::Removed(item.clone()),
        });
        path.truncate(mark);
    }
    for item in new_items.iter().filter(|item| !old_items.contains(item)) {
        let mark = push_segment(path, &set_element_segment(item));
        out.push(ValueChange {
            pointer: path.clone(),
            kind: ValueChangeKind::Added(item.clone()),
        });
        path.truncate(mark);
    }
}

// ── changes ─────────────────────────────────────────────────────────────────

/// How a change affects existing clients of the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Existing clients can break: something they rely on went away or became
    /// stricter.
    Breaking,
    /// Purely additive or relaxing; existing clients keep working.
    NonBreaking,
    /// The shape changed in a way whose impact depends on usage (a `format`, a
    /// bound, a composition variant); a human should look. Never trips
    /// `--fail-on-breaking`.
    Review,
}

/// One difference between the two specs, attributed to an endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub endpoint: EndpointRef,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChangeKind {
    EndpointAdded,
    EndpointRemoved {
        was_deprecated: bool,
    },
    ParameterAdded {
        name: String,
        location: String,
        required: bool,
    },
    ParameterRemoved {
        name: String,
        location: String,
    },
    ParameterRequiredChanged {
        name: String,
        location: String,
        now_required: bool,
    },
    /// A parameter that disappeared from one location and reappeared, under the
    /// same name, in another (detected only when the name is unambiguous on
    /// both sides).
    ParameterLocationChanged {
        name: String,
        old_location: String,
        new_location: String,
    },
    ParameterSchemaChanged {
        name: String,
        location: String,
        change: ValueChange,
    },
    ResponseAdded {
        status: String,
    },
    ResponseRemoved {
        status: String,
    },
    OperationIdChanged {
        old: Option<String>,
        new: Option<String>,
    },
    DeprecatedChanged {
        now: bool,
    },
    /// A field-level change in the resolved request body schema.
    RequestSchemaChanged {
        change: ValueChange,
    },
    /// A field-level change in the resolved schema of one response.
    ResponseSchemaChanged {
        status: String,
        change: ValueChange,
    },
}

/// The outcome of comparing two specs.
#[derive(Debug, Clone, PartialEq)]
pub struct SpecDiff {
    pub old_title: String,
    pub old_version: String,
    pub new_title: String,
    pub new_version: String,
    /// Every change, in report order (see the module docs).
    pub changes: Vec<Change>,
}

impl SpecDiff {
    pub fn endpoints_added(&self) -> usize {
        self.changes
            .iter()
            .filter(|change| matches!(change.kind, ChangeKind::EndpointAdded))
            .count()
    }

    pub fn endpoints_removed(&self) -> usize {
        self.changes
            .iter()
            .filter(|change| matches!(change.kind, ChangeKind::EndpointRemoved { .. }))
            .count()
    }

    /// Endpoints present in both specs with at least one change.
    pub fn endpoints_changed(&self) -> usize {
        self.changes
            .iter()
            .filter(|change| {
                !matches!(
                    change.kind,
                    ChangeKind::EndpointAdded | ChangeKind::EndpointRemoved { .. }
                )
            })
            .map(|change| &change.endpoint)
            .collect::<IndexSet<_>>()
            .len()
    }

    pub fn count(&self, level: Severity) -> usize {
        self.changes
            .iter()
            .filter(|change| severity(change) == level)
            .count()
    }

    pub fn has_breaking(&self) -> bool {
        self.changes
            .iter()
            .any(|change| severity(change) == Severity::Breaking)
    }
}

/// Classifies a change. The rule table:
///
/// | Change | Severity |
/// |---|---|
/// | Endpoint removed | Breaking |
/// | Endpoint added | Non-breaking |
/// | Parameter removed | Breaking |
/// | Parameter added, required | Breaking |
/// | Parameter added, optional | Non-breaking |
/// | Parameter newly required | Breaking |
/// | Parameter made optional | Non-breaking |
/// | Parameter location changed | Breaking |
/// | Response code removed | Breaking |
/// | Response code added | Non-breaking |
/// | `operationId` changed (Some→Some, Some→None) | Breaking |
/// | `operationId` added (None→Some) | Non-breaking |
/// | `deprecated` flag changed either way | Non-breaking |
///
/// Parameter and request schemas (what the client sends):
///
/// | Change | Severity |
/// |---|---|
/// | member added to `required` | Breaking |
/// | member removed from `required` | Non-breaking |
/// | `type` changed at any pointer | Breaking |
/// | property removed | Breaking |
/// | property added | Non-breaking |
/// | `enum` member removed | Breaking |
/// | `enum` member added | Non-breaking |
/// | `additionalProperties` became `false` | Breaking |
/// | `nullable` became `true` | Non-breaking |
/// | `nullable` became `false` (clients sending `null` now fail) | Breaking |
/// | anything else (`format`, bounds, `pattern`, `items` shape, `allOf`/`oneOf`/`anyOf` variants, other `additionalProperties`) | Review |
///
/// Response schemas (what the client receives):
///
/// | Change | Severity |
/// |---|---|
/// | schema removed entirely (root became absent) | Breaking |
/// | property removed | Breaking |
/// | property added | Non-breaking |
/// | `type` changed at any pointer | Breaking |
/// | `enum` member removed | Breaking |
/// | `enum` member added (strictly typed clients reject unknown values) | Review |
/// | member removed from `required` (consumers relied on it) | Breaking |
/// | member added to `required` | Non-breaking |
/// | `nullable` became `true` | Breaking |
/// | `nullable` became `false` | Non-breaking |
/// | anything else | Review |
///
/// When a property is removed, the companion "removed from `required`" row for
/// the same property is not reported at all (see [`value_changes`]).
pub fn severity(change: &Change) -> Severity {
    match &change.kind {
        ChangeKind::EndpointAdded => Severity::NonBreaking,
        ChangeKind::EndpointRemoved { .. } => Severity::Breaking,
        ChangeKind::ParameterAdded { required, .. } => {
            if *required {
                Severity::Breaking
            } else {
                Severity::NonBreaking
            }
        }
        ChangeKind::ParameterRemoved { .. } => Severity::Breaking,
        ChangeKind::ParameterRequiredChanged { now_required, .. } => {
            if *now_required {
                Severity::Breaking
            } else {
                Severity::NonBreaking
            }
        }
        ChangeKind::ParameterLocationChanged { .. } => Severity::Breaking,
        ChangeKind::ParameterSchemaChanged { change, .. }
        | ChangeKind::RequestSchemaChanged { change } => request_schema_severity(change),
        ChangeKind::ResponseAdded { .. } => Severity::NonBreaking,
        ChangeKind::ResponseRemoved { .. } => Severity::Breaking,
        ChangeKind::OperationIdChanged {
            old: None,
            new: Some(_),
        } => Severity::NonBreaking,
        ChangeKind::OperationIdChanged { .. } => Severity::Breaking,
        ChangeKind::DeprecatedChanged { .. } => Severity::NonBreaking,
        ChangeKind::ResponseSchemaChanged { change, .. } => response_schema_severity(change),
    }
}

/// True when the change leaves a boolean keyword equal to `flag` (added or
/// changed to it, or removed when its old value was the opposite).
fn became(kind: &ValueChangeKind, flag: bool) -> bool {
    match kind {
        ValueChangeKind::Added(Value::Bool(now))
        | ValueChangeKind::Changed {
            new: Value::Bool(now),
            ..
        } => *now == flag,
        ValueChangeKind::Removed(Value::Bool(was)) => *was != flag,
        _ => false,
    }
}

fn request_schema_severity(change: &ValueChange) -> Severity {
    use ValueChangeKind::{Added, Removed};

    match (locate(&change.pointer), &change.kind) {
        (Location::RequiredElement, Added(_)) => Severity::Breaking,
        (Location::RequiredElement, Removed(_)) => Severity::NonBreaking,
        (Location::Type, _) => Severity::Breaking,
        (Location::Property, Removed(_)) => Severity::Breaking,
        (Location::Property, Added(_)) => Severity::NonBreaking,
        (Location::EnumElement, Removed(_)) => Severity::Breaking,
        (Location::EnumElement, Added(_)) => Severity::NonBreaking,
        (Location::AdditionalProperties, kind) if became(kind, false) => Severity::Breaking,
        (Location::Nullable, kind) if became(kind, true) => Severity::NonBreaking,
        (Location::Nullable, kind) if became(kind, false) => Severity::Breaking,
        _ => Severity::Review,
    }
}

fn response_schema_severity(change: &ValueChange) -> Severity {
    use ValueChangeKind::{Added, Changed, Removed};

    if change.pointer.is_empty()
        && let Changed {
            new: Value::Null, ..
        } = &change.kind
    {
        // The response used to document a body and no longer does.
        return Severity::Breaking;
    }

    match (locate(&change.pointer), &change.kind) {
        (Location::Property, Removed(_)) => Severity::Breaking,
        (Location::Property, Added(_)) => Severity::NonBreaking,
        (Location::Type, _) => Severity::Breaking,
        (Location::EnumElement, Removed(_)) => Severity::Breaking,
        (Location::EnumElement, Added(_)) => Severity::Review,
        (Location::RequiredElement, Removed(_)) => Severity::Breaking,
        (Location::RequiredElement, Added(_)) => Severity::NonBreaking,
        (Location::Nullable, kind) if became(kind, true) => Severity::Breaking,
        (Location::Nullable, kind) if became(kind, false) => Severity::NonBreaking,
        _ => Severity::Review,
    }
}

// ── structural diff ─────────────────────────────────────────────────────────

/// Compares two specs (see the module docs for identity and ordering rules).
pub fn diff(old: &ApiDocumentation, new: &ApiDocumentation) -> SpecDiff {
    let mut changes = Vec::new();

    // Lookup tables only; iteration always follows the endpoint vectors so the
    // report order never depends on hashing.
    let new_by_key: HashMap<(&str, &str), &Endpoint> = new
        .endpoints
        .iter()
        .map(|endpoint| ((endpoint.method.as_str(), endpoint.path.as_str()), endpoint))
        .collect();
    let old_keys: HashSet<(&str, &str)> = old
        .endpoints
        .iter()
        .map(|endpoint| (endpoint.method.as_str(), endpoint.path.as_str()))
        .collect();

    for old_endpoint in &old.endpoints {
        let key = (old_endpoint.method.as_str(), old_endpoint.path.as_str());
        match new_by_key.get(&key) {
            Some(new_endpoint) => diff_endpoint(old_endpoint, new_endpoint, old, new, &mut changes),
            None => changes.push(Change {
                endpoint: EndpointRef::from(old_endpoint),
                kind: ChangeKind::EndpointRemoved {
                    was_deprecated: old_endpoint.deprecated,
                },
            }),
        }
    }

    for new_endpoint in &new.endpoints {
        let key = (new_endpoint.method.as_str(), new_endpoint.path.as_str());
        if !old_keys.contains(&key) {
            changes.push(Change {
                endpoint: EndpointRef::from(new_endpoint),
                kind: ChangeKind::EndpointAdded,
            });
        }
    }

    SpecDiff {
        old_title: old.title.clone(),
        old_version: old.version.clone(),
        new_title: new.title.clone(),
        new_version: new.version.clone(),
        changes,
    }
}

fn diff_endpoint(
    old_endpoint: &Endpoint,
    new_endpoint: &Endpoint,
    old_doc: &ApiDocumentation,
    new_doc: &ApiDocumentation,
    changes: &mut Vec<Change>,
) {
    let endpoint = EndpointRef::from(old_endpoint);

    if old_endpoint.operation_id != new_endpoint.operation_id {
        changes.push(Change {
            endpoint: endpoint.clone(),
            kind: ChangeKind::OperationIdChanged {
                old: old_endpoint.operation_id.clone(),
                new: new_endpoint.operation_id.clone(),
            },
        });
    }

    if old_endpoint.deprecated != new_endpoint.deprecated {
        changes.push(Change {
            endpoint: endpoint.clone(),
            kind: ChangeKind::DeprecatedChanged {
                now: new_endpoint.deprecated,
            },
        });
    }

    diff_parameters(
        &endpoint,
        old_endpoint,
        new_endpoint,
        old_doc,
        new_doc,
        changes,
    );
    diff_request_body(
        &endpoint,
        old_endpoint,
        new_endpoint,
        old_doc,
        new_doc,
        changes,
    );
    diff_responses(
        &endpoint,
        old_endpoint,
        new_endpoint,
        old_doc,
        new_doc,
        changes,
    );
}

fn is_body(parameter: &Parameter) -> bool {
    parameter.parameter_in == "body"
}

/// Compares the non-body parameters by `(name, in)`. A parameter that vanished
/// from one location and appeared under the same name in another is reported
/// as a location change rather than a removal plus an addition, provided the
/// name is unambiguous on both sides.
fn diff_parameters(
    endpoint: &EndpointRef,
    old_endpoint: &Endpoint,
    new_endpoint: &Endpoint,
    old_doc: &ApiDocumentation,
    new_doc: &ApiDocumentation,
    changes: &mut Vec<Change>,
) {
    let old_params: Vec<&Parameter> = old_endpoint
        .parameters
        .iter()
        .filter(|parameter| !is_body(parameter))
        .collect();
    let new_params: Vec<&Parameter> = new_endpoint
        .parameters
        .iter()
        .filter(|parameter| !is_body(parameter))
        .collect();

    let mut removed: Vec<&Parameter> = Vec::new();
    for old_param in &old_params {
        match find_parameter(&new_params, old_param) {
            Some(new_param) => {
                diff_parameter_pair(endpoint, old_param, new_param, old_doc, new_doc, changes)
            }
            None => removed.push(old_param),
        }
    }
    let added: Vec<&Parameter> = new_params
        .iter()
        .copied()
        .filter(|new_param| find_parameter(&old_params, new_param).is_none())
        .collect();

    let mut paired: Vec<usize> = Vec::new();
    for old_param in &removed {
        let same_name_removed = removed
            .iter()
            .filter(|candidate| candidate.name == old_param.name)
            .count();
        let candidates: Vec<usize> = added
            .iter()
            .enumerate()
            .filter(|(index, candidate)| {
                candidate.name == old_param.name && !paired.contains(index)
            })
            .map(|(index, _)| index)
            .collect();

        if same_name_removed == 1 && candidates.len() == 1 {
            let index = candidates[0];
            let new_param = added[index];
            paired.push(index);
            changes.push(Change {
                endpoint: endpoint.clone(),
                kind: ChangeKind::ParameterLocationChanged {
                    name: old_param.name.clone(),
                    old_location: old_param.parameter_in.clone(),
                    new_location: new_param.parameter_in.clone(),
                },
            });
            diff_parameter_pair(endpoint, old_param, new_param, old_doc, new_doc, changes);
        } else {
            changes.push(Change {
                endpoint: endpoint.clone(),
                kind: ChangeKind::ParameterRemoved {
                    name: old_param.name.clone(),
                    location: old_param.parameter_in.clone(),
                },
            });
        }
    }

    for (index, new_param) in added.iter().enumerate() {
        if paired.contains(&index) {
            continue;
        }
        changes.push(Change {
            endpoint: endpoint.clone(),
            kind: ChangeKind::ParameterAdded {
                name: new_param.name.clone(),
                location: new_param.parameter_in.clone(),
                required: new_param.required.unwrap_or(false),
            },
        });
    }
}

/// The parameter in `haystack` with the same `(name, in)` identity as `needle`.
fn find_parameter<'a>(haystack: &[&'a Parameter], needle: &Parameter) -> Option<&'a Parameter> {
    haystack.iter().copied().find(|candidate| {
        candidate.name == needle.name && candidate.parameter_in == needle.parameter_in
    })
}

/// Compares two versions of the same parameter: its `required` flag and its
/// (resolved, canonical) type schema.
fn diff_parameter_pair(
    endpoint: &EndpointRef,
    old_param: &Parameter,
    new_param: &Parameter,
    old_doc: &ApiDocumentation,
    new_doc: &ApiDocumentation,
    changes: &mut Vec<Change>,
) {
    let old_required = old_param.required.unwrap_or(false);
    let new_required = new_param.required.unwrap_or(false);
    if old_required != new_required {
        changes.push(Change {
            endpoint: endpoint.clone(),
            kind: ChangeKind::ParameterRequiredChanged {
                name: new_param.name.clone(),
                location: new_param.parameter_in.clone(),
                now_required: new_required,
            },
        });
    }

    let old_schema = parameter_schema_value(old_param, old_doc);
    let new_schema = parameter_schema_value(new_param, new_doc);
    for change in value_changes(&old_schema, &new_schema) {
        changes.push(Change {
            endpoint: endpoint.clone(),
            kind: ChangeKind::ParameterSchemaChanged {
                name: new_param.name.clone(),
                location: new_param.parameter_in.clone(),
                change,
            },
        });
    }
}

/// The canonical type of a parameter: its resolved `schema` (OpenAPI 3), or
/// for a Swagger 2 non-body parameter the inline `type`/`format`/`items`/`enum`
/// keywords, which the model keeps in `extensions`. `Null` when it has neither.
fn parameter_schema_value(parameter: &Parameter, doc: &ApiDocumentation) -> Value {
    let mut value = match &parameter.schema {
        Some(schema) => resolve_schema_value(schema, doc),
        None => {
            let mut inline = serde_json::Map::new();
            for key in ["type", "format", "items", "enum"] {
                if let Some(keyword) = parameter.extensions.get(key) {
                    inline.insert(key.to_string(), keyword.clone());
                }
            }
            if inline.is_empty() {
                Value::Null
            } else {
                Value::Object(inline)
            }
        }
    };
    canonicalize_schema_value(&mut value);
    value
}

/// The canonical resolved form of an optional schema; `Null` when absent.
fn optional_schema_value(schema: Option<&crate::models::Schema>, doc: &ApiDocumentation) -> Value {
    match schema {
        Some(schema) => {
            let mut value = resolve_schema_value(schema, doc);
            canonicalize_schema_value(&mut value);
            value
        }
        None => Value::Null,
    }
}

/// All differences between two canonical schemas, minus the redundant
/// "removed from `required`" row that accompanies every removed property: once
/// `/properties/x` is gone, `/required/x` going with it is not a second change.
fn value_changes(old: &Value, new: &Value) -> Vec<ValueChange> {
    let mut out = Vec::new();
    diff_values(old, new, &mut String::new(), &mut out);

    let removed_properties: HashSet<String> = out
        .iter()
        .filter(|change| {
            matches!(change.kind, ValueChangeKind::Removed(_))
                && locate(&change.pointer) == Location::Property
        })
        .map(|change| change.pointer.clone())
        .collect();
    if removed_properties.is_empty() {
        return out;
    }

    out.retain(|change| {
        !(matches!(change.kind, ValueChangeKind::Removed(_))
            && locate(&change.pointer) == Location::RequiredElement
            && removed_properties.contains(&required_member_pointer(&change.pointer)))
    });
    out
}

/// Compares the request bodies: the parser represents a body as a synthetic
/// `in: body` parameter (one per media type; the first is compared).
fn diff_request_body(
    endpoint: &EndpointRef,
    old_endpoint: &Endpoint,
    new_endpoint: &Endpoint,
    old_doc: &ApiDocumentation,
    new_doc: &ApiDocumentation,
    changes: &mut Vec<Change>,
) {
    let old_body = old_endpoint.parameters.iter().find(|p| is_body(p));
    let new_body = new_endpoint.parameters.iter().find(|p| is_body(p));

    match (old_body, new_body) {
        (None, None) => {}
        (Some(old_body), None) => changes.push(Change {
            endpoint: endpoint.clone(),
            kind: ChangeKind::ParameterRemoved {
                name: old_body.name.clone(),
                location: old_body.parameter_in.clone(),
            },
        }),
        (None, Some(new_body)) => changes.push(Change {
            endpoint: endpoint.clone(),
            kind: ChangeKind::ParameterAdded {
                name: new_body.name.clone(),
                location: new_body.parameter_in.clone(),
                required: new_body.required.unwrap_or(false),
            },
        }),
        (Some(old_body), Some(new_body)) => {
            let old_required = old_body.required.unwrap_or(false);
            let new_required = new_body.required.unwrap_or(false);
            if old_required != new_required {
                changes.push(Change {
                    endpoint: endpoint.clone(),
                    kind: ChangeKind::ParameterRequiredChanged {
                        name: new_body.name.clone(),
                        location: new_body.parameter_in.clone(),
                        now_required: new_required,
                    },
                });
            }

            let old_schema = optional_schema_value(old_body.schema.as_ref(), old_doc);
            let new_schema = optional_schema_value(new_body.schema.as_ref(), new_doc);
            for change in value_changes(&old_schema, &new_schema) {
                changes.push(Change {
                    endpoint: endpoint.clone(),
                    kind: ChangeKind::RequestSchemaChanged { change },
                });
            }
        }
    }
}

/// Compares responses by status code, then the resolved schema of each status
/// present in both.
fn diff_responses(
    endpoint: &EndpointRef,
    old_endpoint: &Endpoint,
    new_endpoint: &Endpoint,
    old_doc: &ApiDocumentation,
    new_doc: &ApiDocumentation,
    changes: &mut Vec<Change>,
) {
    for (status, old_response) in &old_endpoint.responses {
        let Some(new_response) = new_endpoint.responses.get(status) else {
            changes.push(Change {
                endpoint: endpoint.clone(),
                kind: ChangeKind::ResponseRemoved {
                    status: status.clone(),
                },
            });
            continue;
        };

        let old_schema = optional_schema_value(response_schema(old_response), old_doc);
        let new_schema = optional_schema_value(response_schema(new_response), new_doc);
        for change in value_changes(&old_schema, &new_schema) {
            changes.push(Change {
                endpoint: endpoint.clone(),
                kind: ChangeKind::ResponseSchemaChanged {
                    status: status.clone(),
                    change,
                },
            });
        }
    }

    for status in new_endpoint.responses.keys() {
        if !old_endpoint.responses.contains_key(status) {
            changes.push(Change {
                endpoint: endpoint.clone(),
                kind: ChangeKind::ResponseAdded {
                    status: status.clone(),
                },
            });
        }
    }
}

// ── --report deltas ─────────────────────────────────────────────────────────

/// Whole-document hygiene counts and token estimates for both specs
/// (`--report`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deltas {
    /// `(check label, old count, new count)` in hygiene-report order.
    pub hygiene: Vec<(&'static str, usize, usize)>,
    /// Estimated tokens of the old spec at `--detail full --include-schemas`.
    pub tokens_old: usize,
    /// Estimated tokens of the new spec at `--detail full --include-schemas`.
    pub tokens_new: usize,
}

/// Runs the hygiene checks and a full-detail render over both specs with an
/// unfiltered configuration ([`DocConfig::unfiltered`]).
pub fn compute_deltas(old: &ApiDocumentation, new: &ApiDocumentation) -> Result<Deltas> {
    let config = DocConfig::unfiltered();

    let old_report = report::analyze(old, &config);
    let new_report = report::analyze(new, &config);
    let hygiene = old_report
        .counts()
        .into_iter()
        .zip(new_report.counts())
        .map(|((label, old_count), (_, new_count))| (label, old_count, new_count))
        .collect();

    Ok(Deltas {
        hygiene,
        tokens_old: estimate_document(old, &config).context("Failed to render the old spec")?,
        tokens_new: estimate_document(new, &config).context("Failed to render the new spec")?,
    })
}

fn estimate_document(doc: &ApiDocumentation, config: &DocConfig) -> Result<usize> {
    let mut buffer = Vec::new();
    generate_markdown(&mut buffer, doc, config)?;
    Ok(estimate_tokens(&buffer))
}

// ── rendering ───────────────────────────────────────────────────────────────

/// Writes the diff as Markdown: a title, a one-line summary, one table per
/// severity that has entries (breaking, non-breaking, needs review), and the
/// `## Deltas` section when `deltas` is given.
pub fn write_diff<W: Write>(
    writer: &mut W,
    diff: &SpecDiff,
    deltas: Option<&Deltas>,
) -> Result<()> {
    let title = if diff.old_title == diff.new_title {
        diff.old_title.clone()
    } else {
        format!("{} → {}", diff.old_title, diff.new_title)
    };
    writeln!(
        writer,
        "# API Diff: {title} {} → {}",
        diff.old_version, diff.new_version
    )?;
    writeln!(writer)?;

    if diff.changes.is_empty() {
        writeln!(writer, "**Summary:** No changes.")?;
    } else {
        writeln!(
            writer,
            "**Summary:** {} added, {} removed, {} changed; {} breaking, {} non-breaking, {} to review",
            pluralize(diff.endpoints_added(), "endpoint"),
            diff.endpoints_removed(),
            diff.endpoints_changed(),
            diff.count(Severity::Breaking),
            diff.count(Severity::NonBreaking),
            diff.count(Severity::Review),
        )?;

        let sections = [
            (Severity::Breaking, "Breaking changes"),
            (Severity::NonBreaking, "Non-breaking changes"),
            (Severity::Review, "Needs review"),
        ];
        for (level, heading) in sections {
            let rows: Vec<&Change> = diff
                .changes
                .iter()
                .filter(|change| severity(change) == level)
                .collect();
            if rows.is_empty() {
                continue;
            }

            writeln!(writer)?;
            writeln!(writer, "## {heading} ({})", rows.len())?;
            writeln!(writer)?;
            writeln!(writer, "| Change | Endpoint | Detail |")?;
            writeln!(writer, "|--------|----------|--------|")?;
            for change in rows {
                writeln!(
                    writer,
                    "| {} | `{}` | {} |",
                    change_label(&change.kind),
                    escape_cell(&change.endpoint.to_string()),
                    escape_cell(&change_detail(&change.kind)),
                )?;
            }
        }
    }

    if let Some(deltas) = deltas {
        writeln!(writer)?;
        writeln!(writer, "## Deltas")?;
        writeln!(writer)?;
        writeln!(writer, "| Check | Old | New | Δ |")?;
        writeln!(writer, "|-------|----:|----:|--:|")?;
        for (label, old_count, new_count) in &deltas.hygiene {
            writeln!(
                writer,
                "| {label} | {old_count} | {new_count} | {} |",
                signed_delta(*old_count, *new_count)
            )?;
        }
        writeln!(writer)?;
        writeln!(
            writer,
            "Token estimate (--detail full --include-schemas): {} → {} ({})",
            deltas.tokens_old,
            deltas.tokens_new,
            signed_delta(deltas.tokens_old, deltas.tokens_new)
        )?;
    }

    Ok(())
}

/// `+3`, `-1` or `0`.
fn signed_delta(old: usize, new: usize) -> String {
    let delta = new as i64 - old as i64;
    if delta > 0 {
        format!("+{delta}")
    } else {
        delta.to_string()
    }
}

/// `1 endpoint`, `2 endpoints`.
fn pluralize(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// Escapes the characters that would break a Markdown table cell.
fn escape_cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', "<br/>")
}

/// The `Change` column.
fn change_label(kind: &ChangeKind) -> &'static str {
    match kind {
        ChangeKind::EndpointAdded => "Endpoint added",
        ChangeKind::EndpointRemoved { .. } => "Endpoint removed",
        ChangeKind::ParameterAdded { .. } => "Parameter added",
        ChangeKind::ParameterRemoved { .. } => "Parameter removed",
        ChangeKind::ParameterRequiredChanged {
            now_required: true, ..
        } => "Parameter newly required",
        ChangeKind::ParameterRequiredChanged {
            now_required: false,
            ..
        } => "Parameter made optional",
        ChangeKind::ParameterLocationChanged { .. } => "Parameter location changed",
        ChangeKind::ParameterSchemaChanged { .. } => "Parameter schema changed",
        ChangeKind::ResponseAdded { .. } => "Response added",
        ChangeKind::ResponseRemoved { .. } => "Response removed",
        ChangeKind::OperationIdChanged { .. } => "operationId changed",
        ChangeKind::DeprecatedChanged { now: true } => "Marked deprecated",
        ChangeKind::DeprecatedChanged { now: false } => "Deprecation removed",
        ChangeKind::RequestSchemaChanged { .. } => "Request schema changed",
        ChangeKind::ResponseSchemaChanged { .. } => "Response schema changed",
    }
}

/// The `Detail` column.
fn change_detail(kind: &ChangeKind) -> String {
    match kind {
        ChangeKind::EndpointAdded => "-".to_string(),
        ChangeKind::EndpointRemoved { was_deprecated } => if *was_deprecated {
            "was deprecated"
        } else {
            "-"
        }
        .to_string(),
        ChangeKind::ParameterAdded {
            name,
            location,
            required,
        } => format!(
            "`{name}` ({location}), {}",
            if *required { "required" } else { "optional" }
        ),
        ChangeKind::ParameterRemoved { name, location }
        | ChangeKind::ParameterRequiredChanged { name, location, .. } => {
            format!("`{name}` ({location})")
        }
        ChangeKind::ParameterLocationChanged {
            name,
            old_location,
            new_location,
        } => format!("`{name}` {old_location} → {new_location}"),
        ChangeKind::ParameterSchemaChanged {
            name,
            location,
            change,
        } => format!("`{name}` ({location}) {}", describe_value_change(change)),
        ChangeKind::ResponseAdded { status } | ChangeKind::ResponseRemoved { status } => {
            status.clone()
        }
        ChangeKind::OperationIdChanged { old, new } => {
            format!("{} → {}", operation_id(old), operation_id(new))
        }
        ChangeKind::DeprecatedChanged { .. } => "-".to_string(),
        ChangeKind::RequestSchemaChanged { change } => describe_value_change(change),
        ChangeKind::ResponseSchemaChanged { status, change } => {
            format!("{status} {}", describe_value_change(change))
        }
    }
}

fn operation_id(id: &Option<String>) -> String {
    match id {
        Some(id) => format!("`{id}`"),
        None => "(none)".to_string(),
    }
}

/// A short rendering of a JSON value for the detail column: bare strings,
/// compact JSON for everything else, truncated when long.
fn brief(value: &Value) -> String {
    const MAX_CHARS: usize = 60;
    let text = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    if text.chars().count() > MAX_CHARS {
        let cut: String = text.chars().take(MAX_CHARS).collect();
        format!("{cut}…")
    } else {
        text
    }
}

fn display_pointer(pointer: &str) -> &str {
    if pointer.is_empty() { "/" } else { pointer }
}

/// Human wording for one value change, e.g.
/// `` `/properties/pricing` added to `required` `` or
/// `` `/type` changed `string` → `integer` ``.
fn describe_value_change(change: &ValueChange) -> String {
    use ValueChangeKind::{Added, Changed, Removed};

    let pointer = &change.pointer;
    match (locate(pointer), &change.kind) {
        (Location::RequiredElement, Added(_)) => {
            format!("`{}` added to `required`", required_member_pointer(pointer))
        }
        (Location::RequiredElement, Removed(_)) => {
            format!(
                "`{}` removed from `required`",
                required_member_pointer(pointer)
            )
        }
        (Location::EnumElement, Added(value)) => {
            format!(
                "`{}` value `{}` added",
                parent_pointer(pointer),
                brief(value)
            )
        }
        (Location::EnumElement, Removed(value)) => {
            format!(
                "`{}` value `{}` removed",
                parent_pointer(pointer),
                brief(value)
            )
        }
        (_, Added(value)) if !value.is_object() && !value.is_array() => {
            format!("`{}` added (`{}`)", display_pointer(pointer), brief(value))
        }
        (_, Added(_)) => format!("`{}` added", display_pointer(pointer)),
        (_, Removed(_)) => format!("`{}` removed", display_pointer(pointer)),
        (_, Changed { old, .. }) if pointer.is_empty() && old.is_null() => {
            "schema added".to_string()
        }
        (_, Changed { new, .. }) if pointer.is_empty() && new.is_null() => {
            "schema removed".to_string()
        }
        (_, Changed { old, new }) => format!(
            "`{}` changed `{}` → `{}`",
            display_pointer(pointer),
            brief(old),
            brief(new)
        ),
    }
}

/// `/x/required/name` → `/x/properties/name`: names the property a `required`
/// membership change is about.
fn required_member_pointer(pointer: &str) -> String {
    match pointer.rsplit_once('/') {
        Some((set, member)) => {
            let base = set.strip_suffix("/required").unwrap_or(set);
            format!("{base}/properties/{member}")
        }
        None => pointer.to_string(),
    }
}

fn parent_pointer(pointer: &str) -> &str {
    pointer
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or(pointer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn endpoint() -> EndpointRef {
        EndpointRef {
            method: "GET".into(),
            path: "/widgets".into(),
        }
    }

    fn change(kind: ChangeKind) -> Change {
        Change {
            endpoint: endpoint(),
            kind,
        }
    }

    fn added(pointer: &str, value: Value) -> ValueChange {
        ValueChange {
            pointer: pointer.into(),
            kind: ValueChangeKind::Added(value),
        }
    }

    fn removed(pointer: &str, value: Value) -> ValueChange {
        ValueChange {
            pointer: pointer.into(),
            kind: ValueChangeKind::Removed(value),
        }
    }

    fn changed(pointer: &str, old: Value, new: Value) -> ValueChange {
        ValueChange {
            pointer: pointer.into(),
            kind: ValueChangeKind::Changed { old, new },
        }
    }

    fn request(change: ValueChange) -> Severity {
        severity(&self::change(ChangeKind::RequestSchemaChanged { change }))
    }

    fn response(change: ValueChange) -> Severity {
        severity(&self::change(ChangeKind::ResponseSchemaChanged {
            status: "200".into(),
            change,
        }))
    }

    fn parameter(change: ValueChange) -> Severity {
        severity(&self::change(ChangeKind::ParameterSchemaChanged {
            name: "limit".into(),
            location: "query".into(),
            change,
        }))
    }

    // ── diff_values ──────────────────────────────────────────────────────

    #[test]
    fn diff_values_reports_object_keys_added_removed_changed() {
        let changes = value_changes(
            &json!({"a": 1, "b": {"c": "x"}, "d": true}),
            &json!({"a": 1, "b": {"c": "y"}, "e": null}),
        );
        assert_eq!(
            changes,
            vec![
                changed("/b/c", json!("x"), json!("y")),
                removed("/d", json!(true)),
                added("/e", Value::Null),
            ]
        );
    }

    #[test]
    fn diff_values_compares_generic_arrays_by_index() {
        let changes = value_changes(
            &json!({"allOf": [{"type": "object"}, {"type": "string"}]}),
            &json!({"allOf": [{"type": "object"}, {"type": "integer"}, {"format": "x"}]}),
        );
        assert_eq!(
            changes,
            vec![
                changed("/allOf/1/type", json!("string"), json!("integer")),
                added("/allOf/2", json!({"format": "x"})),
            ]
        );

        let shorter = value_changes(&json!({"oneOf": [1, 2]}), &json!({"oneOf": [1]}));
        assert_eq!(shorter, vec![removed("/oneOf/1", json!(2))]);
    }

    #[test]
    fn diff_values_compares_required_as_a_set() {
        let changes = value_changes(
            &json!({"required": ["a", "b"]}),
            &json!({"required": ["b", "c"]}),
        );
        assert_eq!(
            changes,
            vec![
                removed("/required/a", json!("a")),
                added("/required/c", json!("c")),
            ]
        );
    }

    #[test]
    fn diff_values_treats_missing_required_as_empty_set() {
        assert_eq!(
            value_changes(
                &json!({"type": "object"}),
                &json!({"type": "object", "required": ["a"]})
            ),
            vec![added("/required/a", json!("a"))]
        );
        assert_eq!(
            value_changes(&json!({"required": ["a"]}), &json!({})),
            vec![removed("/required/a", json!("a"))]
        );
    }

    #[test]
    fn diff_values_treats_missing_properties_as_empty_map() {
        assert_eq!(
            value_changes(
                &json!({"type": "object"}),
                &json!({"type": "object", "properties": {"name": {"type": "string"}}})
            ),
            vec![added("/properties/name", json!({"type": "string"}))]
        );
        assert_eq!(
            value_changes(
                &json!({"properties": {"a": {}, "b": {}}}),
                &json!({"type": "object"})
            ),
            vec![
                removed("/properties/a", json!({})),
                removed("/properties/b", json!({})),
                added("/type", json!("object")),
            ]
        );
    }

    #[test]
    fn diff_values_compares_enum_as_a_set_with_non_string_members() {
        let changes = value_changes(
            &json!({"properties": {"n": {"enum": [1, 2]}}}),
            &json!({"properties": {"n": {"enum": [2, 3]}}}),
        );
        assert_eq!(
            changes,
            vec![
                removed("/properties/n/enum/1", json!(1)),
                added("/properties/n/enum/3", json!(3)),
            ]
        );
    }

    #[test]
    fn diff_values_escapes_pointer_tokens() {
        let changes = value_changes(
            &json!({"properties": {"a/b": {"type": "string"}}}),
            &json!({"properties": {}}),
        );
        assert_eq!(
            changes,
            vec![removed("/properties/a~1b", json!({"type": "string"}))]
        );
    }

    #[test]
    fn diff_values_reports_nothing_for_equal_values() {
        let value =
            json!({"type": "object", "required": ["a"], "properties": {"a": {"type": "x"}}});
        assert!(value_changes(&value, &value).is_empty());
    }

    #[test]
    fn diff_values_treats_property_named_properties_as_a_field() {
        // A GeoJSON-style `properties` field is a property, not the keyword:
        // adding it is one property added, not a descent into its schema.
        let old = json!({"type": "object", "properties": {"id": {"type": "string"}}});
        let new = json!({
            "type": "object",
            "properties": {
                "id": {"type": "string"},
                "properties": {"type": "object", "additionalProperties": true}
            }
        });
        let changes = value_changes(&old, &new);
        assert_eq!(
            changes,
            vec![added(
                "/properties/properties",
                json!({"type": "object", "additionalProperties": true})
            )]
        );
        assert_eq!(response(changes[0].clone()), Severity::NonBreaking);
        assert_eq!(request(changes[0].clone()), Severity::NonBreaking);

        // Removing it is likewise one row, and removing a field called
        // `required` or `enum` does not turn into set arithmetic.
        let old = json!({"properties": {
            "properties": {"type": "object"},
            "required": {"type": "array", "items": {"type": "string"}},
            "enum": {"type": "array"}
        }});
        let new = json!({"properties": {}});
        assert_eq!(
            value_changes(&old, &new),
            vec![
                removed("/properties/properties", json!({"type": "object"})),
                removed(
                    "/properties/required",
                    json!({"type": "array", "items": {"type": "string"}})
                ),
                removed("/properties/enum", json!({"type": "array"})),
            ]
        );
    }

    #[test]
    fn diff_values_treats_property_named_required_as_a_field() {
        let old = json!({"type": "object", "properties": {"required": {"type": "boolean"}}});
        let new = json!({"type": "object", "properties": {"required": {"type": "string"}}});
        let changes = value_changes(&old, &new);
        assert_eq!(
            changes,
            vec![changed(
                "/properties/required/type",
                json!("boolean"),
                json!("string")
            )]
        );
        assert_eq!(request(changes[0].clone()), Severity::Breaking);
        assert_eq!(response(changes[0].clone()), Severity::Breaking);
    }

    #[test]
    fn diff_values_keyword_semantics_resume_inside_a_field_named_like_a_keyword() {
        // Under `/properties/required` we are back in a schema, so *its*
        // `required` array is a set again.
        let changes = value_changes(
            &json!({"properties": {"required": {"type": "object", "required": ["a"]}}}),
            &json!({"properties": {"required": {"type": "object", "required": ["a", "b"]}}}),
        );
        assert_eq!(
            changes,
            vec![added("/properties/required/required/b", json!("b"))]
        );
        assert_eq!(locate(&changes[0].pointer), Location::RequiredElement);
    }

    #[test]
    fn diff_values_does_not_apply_set_semantics_outside_schema_positions() {
        // `default` is opaque: an array under it is compared by index even when
        // it is keyed `required` or `enum` further down.
        let changes = value_changes(
            &json!({"default": {"required": ["a", "b"]}}),
            &json!({"default": {"required": ["b", "a"]}}),
        );
        assert_eq!(
            changes,
            vec![
                changed("/default/required/0", json!("a"), json!("b")),
                changed("/default/required/1", json!("b"), json!("a")),
            ]
        );
    }

    #[test]
    fn value_changes_suppresses_required_removal_of_a_removed_property() {
        let old = json!({
            "type": "object",
            "required": ["a", "b"],
            "properties": {"a": {"type": "string"}, "b": {"type": "string"}}
        });
        let new = json!({
            "type": "object",
            "required": ["b"],
            "properties": {"b": {"type": "string"}}
        });
        assert_eq!(
            value_changes(&old, &new),
            vec![removed("/properties/a", json!({"type": "string"}))]
        );

        // Nested schemas get the same treatment, scoped to their own level.
        let old = json!({"items": {
            "required": ["id"],
            "properties": {"id": {"type": "string"}}
        }});
        let new = json!({"items": {"properties": {}}});
        assert_eq!(
            value_changes(&old, &new),
            vec![removed("/items/properties/id", json!({"type": "string"}))]
        );
    }

    #[test]
    fn value_changes_keeps_required_removal_when_the_property_stays() {
        let old = json!({"required": ["a"], "properties": {"a": {"type": "string"}}});
        let new = json!({"properties": {"a": {"type": "string"}}});
        assert_eq!(
            value_changes(&old, &new),
            vec![removed("/required/a", json!("a"))]
        );
    }

    // ── locate ───────────────────────────────────────────────────────────

    #[test]
    fn locate_distinguishes_keywords_from_property_names() {
        assert_eq!(locate("/type"), Location::Type);
        assert_eq!(locate("/properties/type"), Location::Property);
        assert_eq!(locate("/properties/type/type"), Location::Type);
        assert_eq!(locate("/items/type"), Location::Type);
        assert_eq!(locate("/allOf/0/type"), Location::Type);
        assert_eq!(locate("/allOf/0"), Location::Other);
        assert_eq!(locate("/required/x"), Location::RequiredElement);
        assert_eq!(locate("/properties/required"), Location::Property);
        assert_eq!(locate("/properties/s/enum/a"), Location::EnumElement);
        assert_eq!(
            locate("/additionalProperties"),
            Location::AdditionalProperties
        );
        assert_eq!(locate("/additionalProperties/type"), Location::Type);
        assert_eq!(locate("/nullable"), Location::Nullable);
        assert_eq!(locate("/format"), Location::Other);
        assert_eq!(locate("/default/type"), Location::Other);
        assert_eq!(locate(""), Location::Other);
    }

    // ── severity: structural rows ────────────────────────────────────────

    #[test]
    fn severity_endpoint_rows() {
        assert_eq!(
            severity(&change(ChangeKind::EndpointAdded)),
            Severity::NonBreaking
        );
        assert_eq!(
            severity(&change(ChangeKind::EndpointRemoved {
                was_deprecated: true
            })),
            Severity::Breaking
        );
        assert_eq!(
            severity(&change(ChangeKind::EndpointRemoved {
                was_deprecated: false
            })),
            Severity::Breaking
        );
    }

    #[test]
    fn severity_parameter_rows() {
        let name = || "limit".to_string();
        let location = || "query".to_string();
        assert_eq!(
            severity(&change(ChangeKind::ParameterAdded {
                name: name(),
                location: location(),
                required: false
            })),
            Severity::NonBreaking
        );
        assert_eq!(
            severity(&change(ChangeKind::ParameterAdded {
                name: name(),
                location: location(),
                required: true
            })),
            Severity::Breaking
        );
        assert_eq!(
            severity(&change(ChangeKind::ParameterRemoved {
                name: name(),
                location: location()
            })),
            Severity::Breaking
        );
        assert_eq!(
            severity(&change(ChangeKind::ParameterRequiredChanged {
                name: name(),
                location: location(),
                now_required: true
            })),
            Severity::Breaking
        );
        assert_eq!(
            severity(&change(ChangeKind::ParameterRequiredChanged {
                name: name(),
                location: location(),
                now_required: false
            })),
            Severity::NonBreaking
        );
        assert_eq!(
            severity(&change(ChangeKind::ParameterLocationChanged {
                name: name(),
                old_location: location(),
                new_location: "header".into()
            })),
            Severity::Breaking
        );
    }

    #[test]
    fn severity_response_code_rows() {
        assert_eq!(
            severity(&change(ChangeKind::ResponseAdded {
                status: "404".into()
            })),
            Severity::NonBreaking
        );
        assert_eq!(
            severity(&change(ChangeKind::ResponseRemoved {
                status: "404".into()
            })),
            Severity::Breaking
        );
    }

    #[test]
    fn severity_operation_id_rows() {
        let id = |s: &str| Some(s.to_string());
        assert_eq!(
            severity(&change(ChangeKind::OperationIdChanged {
                old: id("a"),
                new: id("b")
            })),
            Severity::Breaking
        );
        assert_eq!(
            severity(&change(ChangeKind::OperationIdChanged {
                old: id("a"),
                new: None
            })),
            Severity::Breaking
        );
        assert_eq!(
            severity(&change(ChangeKind::OperationIdChanged {
                old: None,
                new: id("b")
            })),
            Severity::NonBreaking
        );
    }

    #[test]
    fn severity_deprecated_rows() {
        assert_eq!(
            severity(&change(ChangeKind::DeprecatedChanged { now: true })),
            Severity::NonBreaking
        );
        assert_eq!(
            severity(&change(ChangeKind::DeprecatedChanged { now: false })),
            Severity::NonBreaking
        );
    }

    // ── severity: parameter schema rows ──────────────────────────────────

    #[test]
    fn severity_parameter_schema_rows() {
        assert_eq!(
            parameter(changed("/type", json!("string"), json!("integer"))),
            Severity::Breaking
        );
        assert_eq!(
            parameter(removed("/enum/a", json!("a"))),
            Severity::Breaking
        );
        assert_eq!(
            parameter(added("/enum/z", json!("z"))),
            Severity::NonBreaking
        );
        assert_eq!(
            parameter(changed("/format", json!("int32"), json!("int64"))),
            Severity::Review
        );
    }

    // ── severity: request schema rows ────────────────────────────────────

    #[test]
    fn severity_request_schema_breaking_rows() {
        assert_eq!(
            request(added("/required/x", json!("x"))),
            Severity::Breaking
        );
        assert_eq!(
            request(changed("/type", json!("string"), json!("integer"))),
            Severity::Breaking
        );
        assert_eq!(
            request(changed(
                "/properties/a/items/type",
                json!("string"),
                json!("number")
            )),
            Severity::Breaking
        );
        assert_eq!(request(added("/type", json!("string"))), Severity::Breaking);
        assert_eq!(
            request(removed("/properties/x", json!({"type": "string"}))),
            Severity::Breaking
        );
        assert_eq!(
            request(removed("/properties/a/properties/b", json!({}))),
            Severity::Breaking
        );
        assert_eq!(request(removed("/enum/a", json!("a"))), Severity::Breaking);
        assert_eq!(
            request(changed("/additionalProperties", json!(true), json!(false))),
            Severity::Breaking
        );
        assert_eq!(
            request(added("/additionalProperties", json!(false))),
            Severity::Breaking
        );
        // Clients that were sending `null` now get rejected.
        assert_eq!(
            request(changed("/nullable", json!(true), json!(false))),
            Severity::Breaking
        );
        assert_eq!(
            request(removed("/nullable", json!(true))),
            Severity::Breaking
        );
        assert_eq!(
            request(added("/properties/a/nullable", json!(false))),
            Severity::Breaking
        );
    }

    #[test]
    fn severity_request_schema_non_breaking_rows() {
        assert_eq!(
            request(added("/properties/x", json!({"type": "string"}))),
            Severity::NonBreaking
        );
        assert_eq!(
            request(removed("/required/x", json!("x"))),
            Severity::NonBreaking
        );
        assert_eq!(request(added("/enum/z", json!("z"))), Severity::NonBreaking);
        assert_eq!(
            request(added("/nullable", json!(true))),
            Severity::NonBreaking
        );
        assert_eq!(
            request(changed("/properties/a/nullable", json!(false), json!(true))),
            Severity::NonBreaking
        );
    }

    #[test]
    fn severity_request_schema_review_rows() {
        assert_eq!(
            request(changed("/format", json!("float"), json!("double"))),
            Severity::Review
        );
        assert_eq!(request(added("/minimum", json!(0))), Severity::Review);
        assert_eq!(request(removed("/maximum", json!(10))), Severity::Review);
        assert_eq!(
            request(changed("/pattern", json!("a"), json!("b"))),
            Severity::Review
        );
        assert_eq!(
            request(changed("/items/format", json!("a"), json!("b"))),
            Severity::Review
        );
        assert_eq!(request(added("/allOf/1", json!({}))), Severity::Review);
        assert_eq!(request(removed("/oneOf/0", json!({}))), Severity::Review);
        assert_eq!(
            request(changed("/additionalProperties", json!(false), json!(true))),
            Severity::Review
        );
        assert_eq!(
            request(added("/additionalProperties", json!({"type": "string"}))),
            Severity::Review
        );
        assert_eq!(
            request(changed("", Value::Null, json!({"type": "object"}))),
            Severity::Review
        );
    }

    // ── severity: response schema rows ───────────────────────────────────

    #[test]
    fn severity_response_schema_breaking_rows() {
        assert_eq!(
            response(removed("/properties/x", json!({}))),
            Severity::Breaking
        );
        assert_eq!(
            response(changed("/type", json!("object"), json!("array"))),
            Severity::Breaking
        );
        assert_eq!(
            response(changed(
                "/properties/a/type",
                json!("string"),
                json!("integer")
            )),
            Severity::Breaking
        );
        assert_eq!(response(removed("/enum/a", json!("a"))), Severity::Breaking);
        assert_eq!(
            response(removed("/required/x", json!("x"))),
            Severity::Breaking
        );
        assert_eq!(
            response(added("/nullable", json!(true))),
            Severity::Breaking
        );
        assert_eq!(
            response(changed("/properties/a/nullable", json!(false), json!(true))),
            Severity::Breaking
        );
        // The response schema went away entirely.
        assert_eq!(
            response(changed("", json!({"type": "object"}), Value::Null)),
            Severity::Breaking
        );
    }

    #[test]
    fn severity_response_schema_non_breaking_rows() {
        assert_eq!(
            response(added("/properties/pricing", json!({"type": "object"}))),
            Severity::NonBreaking
        );
        assert_eq!(
            response(added("/required/x", json!("x"))),
            Severity::NonBreaking
        );
        assert_eq!(
            response(changed("/nullable", json!(true), json!(false))),
            Severity::NonBreaking
        );
        assert_eq!(
            response(removed("/nullable", json!(true))),
            Severity::NonBreaking
        );
    }

    #[test]
    fn severity_response_schema_review_rows() {
        assert_eq!(
            response(changed("/format", json!("float"), json!("double"))),
            Severity::Review
        );
        // Strictly typed clients reject values they do not know.
        assert_eq!(response(added("/enum/z", json!("z"))), Severity::Review);
        assert_eq!(
            response(added("/properties/status/enum/archived", json!("archived"))),
            Severity::Review
        );
        // A response that gained a body: nothing existing clients parse broke.
        assert_eq!(
            response(changed("", Value::Null, json!({"type": "object"}))),
            Severity::Review
        );
        assert_eq!(
            response(changed("/additionalProperties", json!(true), json!(false))),
            Severity::Review
        );
        assert_eq!(response(added("/allOf/1", json!({}))), Severity::Review);
        assert_eq!(response(added("/minimum", json!(1))), Severity::Review);
    }

    // ── rendering helpers ────────────────────────────────────────────────

    #[test]
    fn describe_value_change_wording() {
        assert_eq!(
            describe_value_change(&added("/required/pricing", json!("pricing"))),
            "`/properties/pricing` added to `required`"
        );
        assert_eq!(
            describe_value_change(&removed("/items/required/id", json!("id"))),
            "`/items/properties/id` removed from `required`"
        );
        assert_eq!(
            describe_value_change(&removed(
                "/properties/status/enum/archived",
                json!("archived")
            )),
            "`/properties/status/enum` value `archived` removed"
        );
        assert_eq!(
            describe_value_change(&changed("/type", json!("string"), json!("integer"))),
            "`/type` changed `string` → `integer`"
        );
        assert_eq!(
            describe_value_change(&added("/properties/pricing", json!({"type": "object"}))),
            "`/properties/pricing` added"
        );
        assert_eq!(
            describe_value_change(&added("/minimum", json!(0))),
            "`/minimum` added (`0`)"
        );
        assert_eq!(
            describe_value_change(&changed("", Value::Null, json!({}))),
            "schema added"
        );
    }

    #[test]
    fn signed_delta_formats_sign() {
        assert_eq!(signed_delta(3, 5), "+2");
        assert_eq!(signed_delta(5, 3), "-2");
        assert_eq!(signed_delta(4, 4), "0");
    }

    #[test]
    fn write_diff_with_no_changes_says_so() {
        let diff = SpecDiff {
            old_title: "API".into(),
            old_version: "1".into(),
            new_title: "API".into(),
            new_version: "2".into(),
            changes: Vec::new(),
        };
        let mut buffer = Vec::new();
        write_diff(&mut buffer, &diff, None).unwrap();
        assert_eq!(
            String::from_utf8(buffer).unwrap(),
            "# API Diff: API 1 → 2\n\n**Summary:** No changes.\n"
        );
    }

    #[test]
    fn write_diff_shows_both_titles_when_they_differ() {
        let diff = SpecDiff {
            old_title: "Old".into(),
            old_version: "1".into(),
            new_title: "New".into(),
            new_version: "2".into(),
            changes: Vec::new(),
        };
        let mut buffer = Vec::new();
        write_diff(&mut buffer, &diff, None).unwrap();
        assert!(
            String::from_utf8(buffer)
                .unwrap()
                .starts_with("# API Diff: Old → New 1 → 2\n")
        );
    }
}
