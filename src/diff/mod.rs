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
//! (`resolve_schema_value`) and the result canonicalised
//! (`canonicalize_schema_value`) before a generic JSON differ
//! (`diff_values`) reports field-level changes as JSON pointers. That is what
//! makes a change behind a shared component visible: an operation whose object
//! is byte-identical in both specs still reports "response schema changed" when
//! the `Widget` it points at gained a required property.
//!
//! Only a response's first media type is compared (the same one the renderer
//! documents, see `response_schema`), and only the first request-body media
//! type. `allOf`/`oneOf`/`anyOf` lists are compared index-wise. With [`diff`]
//! a path-template rename (`/pets/{id}` → `/pets/{petId}`) appears as a
//! removal plus an addition; [`diff_with`] and [`EndpointMatching::Moves`]
//! pair it when the operationId is unique across both specs.
//!
//! # Moved operations
//!
//! [`diff`] matches by exact `(METHOD, path)` only. [`diff_with`] with
//! [`EndpointMatching::Moves`] then pairs the leftover removed and added
//! operations of the same method, first by `operationId`, then by path
//! equality after replacing whole `v<digits>` segments (`/api/v1/items` ↔
//! `/api/v2/items`; `/v1beta` is not a version segment). A key pairs only when
//! it is unique across the whole old and the whole new document, so pairings
//! are one-to-one; ambiguity keeps the removal and addition. A pair yields [`ChangeKind::EndpointMoved`]
//! followed by the ordinary changes inside the operation, all attributed to
//! the new endpoint.
//!
//! # Determinism
//!
//! Changes are emitted in old-spec order (old endpoints first, then endpoints
//! only the new spec has, each in its spec's order; a moved operation sits at
//! its old position) and the canonical schema
//! values have sorted keys, so the same pair of specs always produces
//! byte-identical output.

pub mod json;
mod render;
mod value;

pub use render::write_diff;
pub(crate) use value::{Location, diff_values, locate};
pub use value::{ValueChange, ValueChangeKind};

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use indexmap::IndexSet;
use serde_json::Value;

use crate::markdown::{estimate_tokens, generate_markdown, response_schema};
use crate::models::{ApiDocumentation, DocConfig, Endpoint, Parameter};
use crate::report;
pub use crate::report::EndpointRef;
use crate::utils::{canonicalize_schema_value, resolve_schema_value};

use render::required_member_pointer;

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
    /// The HTTP operation to which this change is attributed.
    pub endpoint: EndpointRef,
    /// The concrete difference or operation applied at this location.
    pub kind: ChangeKind,
}

/// The kind of contract difference. Future versions may add variants.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ChangeKind {
    /// An operation exists only in the new spec.
    EndpointAdded,
    /// An operation from the old spec was removed.
    EndpointRemoved {
        /// Whether the removed endpoint was already deprecated.
        was_deprecated: bool,
    },
    /// An operation moved to a new path (reported only by [`diff_with`] with
    /// [`EndpointMatching::Moves`]). The change's `endpoint` is the new one;
    /// callers of `from` break.
    EndpointMoved {
        /// The old-spec method and path.
        from: EndpointRef,
        /// The rule that paired the two operations.
        matched_by: MoveRule,
        /// The old operationId.
        old_operation_id: Option<String>,
        /// The new operationId.
        new_operation_id: Option<String>,
    },
    /// A parameter was added to a surviving operation.
    ParameterAdded {
        /// The declared name.
        name: String,
        /// Parameter location such as `path`, `query`, `header`, or `body`.
        location: String,
        /// Whether clients must supply this parameter.
        required: bool,
    },
    /// A parameter was removed from a surviving operation.
    ParameterRemoved {
        /// The declared name.
        name: String,
        /// Parameter location such as `path`, `query`, `header`, or `body`.
        location: String,
    },
    /// A parameter changed between optional and required.
    ParameterRequiredChanged {
        /// The declared name.
        name: String,
        /// Parameter location such as `path`, `query`, `header`, or `body`.
        location: String,
        /// Whether the parameter is required in the new spec.
        now_required: bool,
    },
    /// A parameter that disappeared from one location and reappeared, under the
    /// same name, in another (detected only when the name is unambiguous on
    /// both sides).
    ParameterLocationChanged {
        /// The declared name.
        name: String,
        /// Parameter location in the old spec.
        old_location: String,
        /// Parameter location in the new spec.
        new_location: String,
    },
    /// A parameter schema has a field-level change.
    ParameterSchemaChanged {
        /// The declared name.
        name: String,
        /// Parameter location such as `path`, `query`, `header`, or `body`.
        location: String,
        /// The field-level change in the resolved, canonicalized schema.
        change: ValueChange,
    },
    /// A response status was added.
    ResponseAdded {
        /// Response status code exactly as declared, including `default`.
        status: String,
    },
    /// A response status was removed.
    ResponseRemoved {
        /// Response status code exactly as declared, including `default`.
        status: String,
    },
    /// An operationId changed, appeared, or disappeared.
    OperationIdChanged {
        /// The value or count in the old spec.
        old: Option<String>,
        /// The value or count in the new spec.
        new: Option<String>,
    },
    /// An operation changed its deprecation flag.
    DeprecatedChanged {
        /// Whether the endpoint is deprecated in the new spec.
        now: bool,
    },
    /// A field-level change in the resolved request body schema.
    RequestSchemaChanged {
        /// The field-level change in the resolved, canonicalized schema.
        change: ValueChange,
    },
    /// A field-level change in the resolved schema of one response.
    ResponseSchemaChanged {
        /// Response status code exactly as declared, including `default`.
        status: String,
        /// The field-level change in the resolved, canonicalized schema.
        change: ValueChange,
    },
}

/// How [`diff_with`] pairs operations across the two specs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum EndpointMatching {
    /// Exact `(METHOD, path)` only; what [`diff`] uses.
    #[default]
    Exact,
    /// Exact first, then unique moves of the leftovers (see the module docs).
    Moves,
}

/// Why two operations at different paths were treated as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MoveRule {
    /// The same `operationId`, unique in each whole document.
    OperationId,
    /// Paths equal once whole `v<digits>` segments are ignored.
    VersionSegment,
}

impl MoveRule {
    /// Stable snake_case name used in JSON details.
    pub fn as_str(self) -> &'static str {
        match self {
            MoveRule::OperationId => "operation_id",
            MoveRule::VersionSegment => "version_segment",
        }
    }
}

/// The outcome of comparing two specs.
#[derive(Debug, Clone, PartialEq)]
pub struct SpecDiff {
    /// Title declared by the old spec.
    pub old_title: String,
    /// API version declared by the old spec.
    pub old_version: String,
    /// Title declared by the new spec.
    pub new_title: String,
    /// API version declared by the new spec.
    pub new_version: String,
    /// Every change, in report order (see the module docs).
    pub changes: Vec<Change>,
}

impl SpecDiff {
    /// Count operations present only in the new spec.
    pub fn endpoints_added(&self) -> usize {
        self.changes
            .iter()
            .filter(|change| matches!(change.kind, ChangeKind::EndpointAdded))
            .count()
    }

    /// Count operations present only in the old spec.
    pub fn endpoints_removed(&self) -> usize {
        self.changes
            .iter()
            .filter(|change| matches!(change.kind, ChangeKind::EndpointRemoved { .. }))
            .count()
    }

    /// Count operations paired across a path move.
    pub fn endpoints_moved(&self) -> usize {
        self.changes
            .iter()
            .filter(|change| matches!(change.kind, ChangeKind::EndpointMoved { .. }))
            .count()
    }

    /// Endpoints present in both specs (including moved ones) with at least
    /// one change.
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

    /// Count changes with the requested severity.
    pub fn count(&self, level: Severity) -> usize {
        self.changes
            .iter()
            .filter(|change| severity(change) == level)
            .count()
    }

    /// Whether any change has breaking severity.
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
/// | Endpoint moved (callers of the old path break) | Breaking |
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
/// the same property is not reported at all (see `value_changes`).
pub fn severity(change: &Change) -> Severity {
    match &change.kind {
        ChangeKind::EndpointAdded => Severity::NonBreaking,
        ChangeKind::EndpointRemoved { .. } | ChangeKind::EndpointMoved { .. } => Severity::Breaking,
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

/// Compares two specs by exact `(METHOD, path)` (see the module docs for
/// identity and ordering rules).
pub fn diff(old: &ApiDocumentation, new: &ApiDocumentation) -> SpecDiff {
    diff_with(old, new, EndpointMatching::Exact)
}

/// Compares two specs with the given operation matching.
pub fn diff_with(
    old: &ApiDocumentation,
    new: &ApiDocumentation,
    matching: EndpointMatching,
) -> SpecDiff {
    let mut changes = Vec::new();

    // Lookup tables only; iteration always follows the endpoint vectors so the
    // report order never depends on hashing.
    let new_by_key: HashMap<(&str, &str), &Endpoint> = new
        .endpoints
        .iter()
        .map(|endpoint| ((endpoint.method.as_str(), endpoint.path.as_str()), endpoint))
        .collect();
    let moves = match matching {
        EndpointMatching::Exact => Vec::new(),
        EndpointMatching::Moves => endpoint_moves(old, new),
    };
    let mut moved_from: Vec<Option<&EndpointMove>> = vec![None; old.endpoints.len()];
    let mut moved_to = vec![false; new.endpoints.len()];
    for m in &moves {
        moved_from[m.old] = Some(m);
        moved_to[m.new] = true;
    }

    for (index, old_endpoint) in old.endpoints.iter().enumerate() {
        let key = (old_endpoint.method.as_str(), old_endpoint.path.as_str());
        if let Some(new_endpoint) = new_by_key.get(&key) {
            diff_endpoint(old_endpoint, new_endpoint, old, new, &mut changes);
        } else if let Some(pair) = moved_from[index] {
            let new_endpoint = &new.endpoints[pair.new];
            changes.push(Change {
                endpoint: EndpointRef::from(new_endpoint),
                kind: ChangeKind::EndpointMoved {
                    from: EndpointRef::from(old_endpoint),
                    matched_by: pair.rule,
                    old_operation_id: old_endpoint.operation_id.clone(),
                    new_operation_id: new_endpoint.operation_id.clone(),
                },
            });
            diff_endpoint(old_endpoint, new_endpoint, old, new, &mut changes);
        } else {
            changes.push(Change {
                endpoint: EndpointRef::from(old_endpoint),
                kind: ChangeKind::EndpointRemoved {
                    was_deprecated: old_endpoint.deprecated,
                },
            });
        }
    }

    let old_keys: HashSet<(&str, &str)> = old
        .endpoints
        .iter()
        .map(|endpoint| (endpoint.method.as_str(), endpoint.path.as_str()))
        .collect();
    for (index, new_endpoint) in new.endpoints.iter().enumerate() {
        let key = (new_endpoint.method.as_str(), new_endpoint.path.as_str());
        if !old_keys.contains(&key) && !moved_to[index] {
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

/// One operation paired across a path move: indexes into the old and new
/// endpoint vectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EndpointMove {
    pub(crate) old: usize,
    pub(crate) new: usize,
    pub(crate) rule: MoveRule,
}

/// Pairs operations left over by exact `(METHOD, path)` matching: first by a
/// shared `operationId`, then by version-normalised path. A key pairs only
/// when it is unique across the **whole** document on each side (not merely
/// among the leftovers), both holders are leftovers and their methods agree,
/// so ambiguity keeps removal plus addition. Sorted by old index, so the
/// result is deterministic. Linear in the endpoint count.
pub(crate) fn endpoint_moves(old: &ApiDocumentation, new: &ApiDocumentation) -> Vec<EndpointMove> {
    fn keys(doc: &ApiDocumentation) -> HashSet<(&str, &str)> {
        doc.endpoints
            .iter()
            .map(|e| (e.method.as_str(), e.path.as_str()))
            .collect()
    }
    let (old_keys, new_keys) = (keys(old), keys(new));
    let mut removed: Vec<bool> = old
        .endpoints
        .iter()
        .map(|e| !new_keys.contains(&(e.method.as_str(), e.path.as_str())))
        .collect();
    let mut added: Vec<bool> = new
        .endpoints
        .iter()
        .map(|e| !old_keys.contains(&(e.method.as_str(), e.path.as_str())))
        .collect();
    if !removed.contains(&true) || !added.contains(&true) {
        return Vec::new();
    }

    let mut moves = Vec::new();
    type MoveKey = fn(&Endpoint) -> Option<String>;
    let rules: [(MoveRule, MoveKey); 2] = [
        (MoveRule::OperationId, |e| e.operation_id.clone()),
        (MoveRule::VersionSegment, |e| {
            version_normalized_path(&e.path).map(|path| format!("{} {path}", e.method))
        }),
    ];
    for (rule, key) in rules {
        // key -> (holders in the whole document, first holder's index)
        let index = |doc: &ApiDocumentation| {
            let mut map: HashMap<String, (usize, usize)> = HashMap::new();
            for (i, e) in doc.endpoints.iter().enumerate() {
                if let Some(k) = key(e) {
                    map.entry(k).or_insert((0, i)).0 += 1;
                }
            }
            map
        };
        let (old_index, new_index) = (index(old), index(new));
        for (o, before) in old.endpoints.iter().enumerate() {
            if !removed[o] {
                continue;
            }
            let Some(k) = key(before) else { continue };
            if old_index.get(&k).map(|&(count, _)| count) != Some(1) {
                continue;
            }
            let Some(&(1, n)) = new_index.get(&k) else {
                continue;
            };
            if !added[n] || new.endpoints[n].method != before.method {
                continue;
            }
            moves.push(EndpointMove {
                old: o,
                new: n,
                rule,
            });
            removed[o] = false;
            added[n] = false;
        }
    }
    moves.sort_by_key(|m| m.old);
    moves
}

/// The path with every whole `v<digits>` segment replaced by `v*`, or `None`
/// when it has no such segment (so unversioned paths never pair this way).
fn version_normalized_path(path: &str) -> Option<String> {
    let is_version = |segment: &str| {
        segment
            .strip_prefix(['v', 'V'])
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
    };
    let mut found = false;
    let normalized: Vec<&str> = path
        .split('/')
        .map(|segment| {
            if is_version(segment) {
                found = true;
                "v*"
            } else {
                segment
            }
        })
        .collect();
    found.then(|| normalized.join("/"))
}

fn diff_endpoint(
    old_endpoint: &Endpoint,
    new_endpoint: &Endpoint,
    old_doc: &ApiDocumentation,
    new_doc: &ApiDocumentation,
    changes: &mut Vec<Change>,
) {
    // Attributed to the new endpoint: identical to the old one for an exact
    // match, and the destination for a moved operation.
    let endpoint = EndpointRef::from(new_endpoint);

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

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    fn endpoint() -> EndpointRef {
        EndpointRef {
            method: "GET".into(),
            path: "/widgets".into(),
        }
    }

    pub(crate) fn change(kind: ChangeKind) -> Change {
        Change {
            endpoint: endpoint(),
            kind,
        }
    }

    pub(crate) fn added(pointer: &str, value: Value) -> ValueChange {
        ValueChange {
            pointer: pointer.into(),
            kind: ValueChangeKind::Added(value),
        }
    }

    pub(crate) fn removed(pointer: &str, value: Value) -> ValueChange {
        ValueChange {
            pointer: pointer.into(),
            kind: ValueChangeKind::Removed(value),
        }
    }

    pub(crate) fn changed(pointer: &str, old: Value, new: Value) -> ValueChange {
        ValueChange {
            pointer: pointer.into(),
            kind: ValueChangeKind::Changed { old, new },
        }
    }

    pub(crate) fn request(change: ValueChange) -> Severity {
        severity(&self::change(ChangeKind::RequestSchemaChanged { change }))
    }

    pub(crate) fn response(change: ValueChange) -> Severity {
        severity(&self::change(ChangeKind::ResponseSchemaChanged {
            status: "200".into(),
            change,
        }))
    }

    pub(crate) fn parameter(change: ValueChange) -> Severity {
        severity(&self::change(ChangeKind::ParameterSchemaChanged {
            name: "limit".into(),
            location: "query".into(),
            change,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::test_support::{
        added, change, changed, parameter, removed, request, response,
    };
    use serde_json::json;

    // ── diff_values ──────────────────────────────────────────────────────

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
    fn severity_endpoint_moved_is_breaking() {
        assert_eq!(
            severity(&change(ChangeKind::EndpointMoved {
                from: EndpointRef {
                    method: "GET".into(),
                    path: "/v1/widgets".into(),
                },
                matched_by: MoveRule::VersionSegment,
                old_operation_id: None,
                new_operation_id: None,
            })),
            Severity::Breaking
        );
    }

    #[test]
    fn version_segments_are_whole_v_digit_segments() {
        assert_eq!(
            version_normalized_path("/api/v1/items").as_deref(),
            Some("/api/v*/items")
        );
        assert_eq!(
            version_normalized_path("/V12/items/{id}").as_deref(),
            Some("/v*/items/{id}")
        );
        for unversioned in [
            "/items",
            "/v1beta/items",
            "/v/items",
            "/items/v1x",
            "/av1/x",
        ] {
            assert_eq!(version_normalized_path(unversioned), None, "{unversioned}");
        }
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
}
