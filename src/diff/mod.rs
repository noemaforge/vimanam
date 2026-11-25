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
//! responses by status code. Parameter, request and response schemas are
//! canonicalised (`canonicalize_schema_value`) and compared by a generic JSON
//! differ (`diff_values`) that reports field-level changes as JSON pointers.
//!
//! `$ref` targets are compared as **edges** (#181, the `components` module):
//! where both sides of a schema reference a component at the same position,
//! that `(old component, new component)` pair is compared once for the whole
//! diff instead of being inlined at every position that reaches it, and a
//! change inside it is one [`ChangeKind::ComponentSchemaChanged`] per usage
//! direction, listing every parameter, body and response that reaches the
//! pair. So an operation whose object is byte-identical in both specs is still
//! listed when the `Widget` it points at gained a required property, and a
//! change under a densely cross-linked graph is reported once rather than once
//! per reference path. A reference met by an inline object on the other side
//! is expanded in place, so moving a schema between inline and referenced form
//! is an ordinary change of that operation; a schema without references is
//! compared exactly as before. Keys beside a `$ref` (other than annotations)
//! overlay its target: they are compared at the reference, and a target
//! change under one is not listed for that use. A reference retargeted to a
//! differently named component is always a Review `/$ref` change.
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
//! its old position), then the shared-component changes in old-spec component
//! order (then new-only components), and the canonical schema
//! values have sorted keys, so the same pair of specs always produces
//! byte-identical output.

mod components;
pub mod json;
mod render;
mod value;

pub use render::write_diff;
pub(crate) use value::{Location, diff_values, locate, under_not};
pub use value::{ValueChange, ValueChangeKind};

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use indexmap::IndexSet;
use serde_json::Value;

use crate::markdown::{estimate_tokens, generate_markdown, response_schema};
use crate::models::{ApiDocumentation, DocConfig, Endpoint, Parameter};
use crate::report;
pub use crate::report::EndpointRef;
use crate::resolve::{PairResolver, schema_json};
use components::Components;

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
    /// A field-level change inside a shared component (a `$ref` target),
    /// reported once per usage direction instead of once per operation or
    /// reference path (#181). The change's `endpoint` is the first entry of
    /// `uses`; identity is the component, the pointer and the direction.
    ComponentSchemaChanged {
        /// The old and new component compared (the same name unless a
        /// reference was retargeted).
        component: ComponentRef,
        /// Whether `uses` send (request) or receive (response) the
        /// component; it selects the severity rule table.
        direction: Direction,
        /// The field-level change; its pointer is relative to the component.
        change: ValueChange,
        /// Whether `uses` reach the component through a `not` keyword. `not`
        /// reverses every compatibility direction, so such a record is
        /// Review; uses that reach it plainly are a separate record.
        negated: bool,
        /// Every parameter, request body or response of this direction that
        /// reaches the component, directly or through other components, in
        /// report order.
        uses: Vec<SchemaUse>,
    },
}

/// The pair of components a shared-component change was found in: each
/// `(old, new)` pair of `$ref` targets is compared once.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ComponentRef {
    /// Component name in the old spec (`#/components/schemas/<old>` or
    /// `#/definitions/<old>`, decoded).
    pub old: String,
    /// Component name in the new spec.
    pub new: String,
}

impl ComponentRef {
    /// Whether a reference was retargeted to a differently named component.
    pub fn renamed(&self) -> bool {
        self.old != self.new
    }
}

/// Whether a schema use is sent by the client or returned to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Parameters and request bodies: the request rule table.
    Request,
    /// Responses: the response rule table.
    Response,
}

impl Direction {
    /// Stable lowercase name used in JSON details.
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Request => "request",
            Direction::Response => "response",
        }
    }
}

/// One operation schema that reaches a shared component.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SchemaUse {
    /// The operation, attributed like its other changes (the new endpoint of
    /// a moved operation).
    pub endpoint: EndpointRef,
    /// Which schema of the operation.
    pub location: UseLocation,
}

/// Where in an operation a schema sits.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UseLocation {
    /// A non-body parameter.
    Parameter {
        /// The declared name.
        name: String,
        /// Parameter location such as `path`, `query` or `header`.
        location: String,
    },
    /// The (first) request body, under its synthetic body-parameter name.
    RequestBody {
        /// The body parameter's name.
        name: String,
    },
    /// The first media type schema of one response.
    Response {
        /// Response status code exactly as declared, including `default`.
        status: String,
    },
}

impl UseLocation {
    /// Request for parameters and bodies, response for responses.
    pub fn direction(&self) -> Direction {
        match self {
            UseLocation::Parameter { .. } | UseLocation::RequestBody { .. } => Direction::Request,
            UseLocation::Response { .. } => Direction::Response,
        }
    }
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
    /// one change, counting every use of a changed shared component.
    pub fn endpoints_changed(&self) -> usize {
        self.changes
            .iter()
            .filter(|change| {
                !matches!(
                    change.kind,
                    ChangeKind::EndpointAdded | ChangeKind::EndpointRemoved { .. }
                )
            })
            .flat_map(|change| match &change.kind {
                ChangeKind::ComponentSchemaChanged { uses, .. } => {
                    uses.iter().map(|use_| &use_.endpoint).collect()
                }
                _ => vec![&change.endpoint],
            })
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
/// | `enum` keyword added where none existed (narrows accepted values) | Breaking |
/// | `enum` keyword removed entirely (accepts any value) | Non-breaking |
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
/// | `enum` member removed (the server returns a subset of what clients already handle) | Non-breaking |
/// | `enum` member added (strictly typed clients reject unknown values) | Review |
/// | `enum` keyword added where none existed (narrows what the server returns) | Non-breaking |
/// | `enum` keyword removed entirely (the server may return any value) | Review |
/// | member removed from `required` (consumers relied on it) | Breaking |
/// | member added to `required` | Non-breaking |
/// | `nullable` became `true` | Breaking |
/// | `nullable` became `false` | Non-breaking |
/// | anything else | Review |
///
/// Any change at or beneath a `not` keyword (in either table) is Review: `not`
/// reverses every direction above, so narrowing the excluded schema widens
/// what is accepted or returned, and vice versa.
///
/// When a property is removed, the companion "removed from `required`" row for
/// the same property is not reported at all (see `value_changes`).
///
/// Shared components (`$ref` targets, #181) are compared once per `(old, new)`
/// component pair, and a change inside one is reported once per **usage
/// direction** that reaches it ([`ChangeKind::ComponentSchemaChanged`]), with
/// the reaching uses listed:
///
/// | Component reached from | Records | Severity |
/// |---|---|---|
/// | parameters or request bodies only | one, `request` | the request table |
/// | responses only | one, `response` | the response table |
/// | both | two, `request` then `response` | each its own table |
///
/// A use that reaches the component through a `not` keyword (at the use or
/// in any component on the way) gets its own record, which is Review, like
/// any change under `not`; uses reaching it plainly keep their verdict.
///
/// So a narrowed `enum` on a component shared by a request and a response is
/// a breaking request record plus a non-breaking response record, and
/// `--fail-on-breaking` fires whenever any usage is breaking.
///
/// | Component change | Severity |
/// |---|---|
/// | reference retargeted to a differently named component (`/$ref`, old and new names), even when the two have the same shape | Review |
///
/// The retarget is one record per direction for the pair, followed by the
/// pair's structural differences, if any. A `$ref` with siblings other than
/// annotations overlays them: those keys are compared at the reference with
/// their effective values, and a change in the target under one of them is
/// not listed for a use that reaches the target only through that overlay. The `not` and
/// whole-`enum` rows apply to component pointers exactly as above. A
/// component declared on one side only is compared against the literal
/// `{"$ref": ...}` the other side leaves unresolved, as before.
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
        ChangeKind::ComponentSchemaChanged { negated: true, .. } => Severity::Review,
        ChangeKind::ComponentSchemaChanged {
            direction, change, ..
        } => match direction {
            Direction::Request => request_schema_severity(change),
            Direction::Response => response_schema_severity(change),
        },
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

    if under_not(&change.pointer) {
        return Severity::Review;
    }

    match (locate(&change.pointer), &change.kind) {
        (Location::RequiredElement, Added(_)) => Severity::Breaking,
        (Location::RequiredElement, Removed(_)) => Severity::NonBreaking,
        (Location::Type, _) => Severity::Breaking,
        (Location::Property, Removed(_)) => Severity::Breaking,
        (Location::Property, Added(_)) => Severity::NonBreaking,
        (Location::EnumElement, Removed(_)) => Severity::Breaking,
        (Location::EnumElement, Added(_)) => Severity::NonBreaking,
        (Location::Enum, Added(_)) => Severity::Breaking,
        (Location::Enum, Removed(_)) => Severity::NonBreaking,
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

    if under_not(&change.pointer) {
        return Severity::Review;
    }

    match (locate(&change.pointer), &change.kind) {
        (Location::Property, Removed(_)) => Severity::Breaking,
        (Location::Property, Added(_)) => Severity::NonBreaking,
        (Location::Type, _) => Severity::Breaking,
        // Narrowing what the server returns cannot break a client (#169). A
        // component shared with a request still reports its request-side
        // removal as breaking.
        (Location::EnumElement, Removed(_)) => Severity::NonBreaking,
        (Location::EnumElement, Added(_)) => Severity::Review,
        // Dropping the whole restriction widens the response like adding a
        // member does; adding one narrows it.
        (Location::Enum, Removed(_)) => Severity::Review,
        (Location::Enum, Added(_)) => Severity::NonBreaking,
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
    diff_with_resolver(
        old,
        new,
        matching,
        &PairResolver::new(&old.schemas, &new.schemas),
    )
}

/// [`diff_with`] with a caller-owned resolution cache for the same two specs,
/// so later passes over them (the v2 contract facts) reuse its work.
pub(crate) fn diff_with_resolver(
    old: &ApiDocumentation,
    new: &ApiDocumentation,
    matching: EndpointMatching,
    resolver: &PairResolver,
) -> SpecDiff {
    let mut changes = Vec::new();
    let mut components = Components::new(resolver);

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
            diff_endpoint(old_endpoint, new_endpoint, &mut components, &mut changes);
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
            diff_endpoint(old_endpoint, new_endpoint, &mut components, &mut changes);
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
    changes.extend(components.finish());

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
    components: &mut Components,
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

    diff_parameters(&endpoint, old_endpoint, new_endpoint, components, changes);
    diff_request_body(&endpoint, old_endpoint, new_endpoint, components, changes);
    diff_responses(&endpoint, old_endpoint, new_endpoint, components, changes);
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
    components: &mut Components,
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
                diff_parameter_pair(endpoint, old_param, new_param, components, changes)
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
            diff_parameter_pair(endpoint, old_param, new_param, components, changes);
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
    components: &mut Components,
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

    let (old_schema, new_schema) = match (&old_param.schema, &new_param.schema) {
        (Some(old_schema), Some(new_schema)) => {
            let (old_schema, new_schema) = (schema_json(old_schema), schema_json(new_schema));
            if components.resolver().values_equal(&old_schema, &new_schema) {
                return;
            }
            (old_schema, new_schema)
        }
        _ => (
            parameter_schema_json(old_param),
            parameter_schema_json(new_param),
        ),
    };
    let use_ = SchemaUse {
        endpoint: endpoint.clone(),
        location: UseLocation::Parameter {
            name: new_param.name.clone(),
            location: new_param.parameter_in.clone(),
        },
    };
    for change in components.compare(use_, &old_schema, &new_schema) {
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

/// The serialized type of a parameter: its `schema` (OpenAPI 3), or for a
/// Swagger 2 non-body parameter the inline `type`/`format`/`items`/`enum`
/// keywords, which the model keeps in `extensions`. `Null` when it has
/// neither. References are compared as edges by [`Components::compare`].
fn parameter_schema_json(parameter: &Parameter) -> Value {
    match &parameter.schema {
        Some(schema) => schema_json(schema),
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
    }
}

/// The local changes between two optional schemas of one use (`Null` when
/// absent); empty without comparing anything when they provably resolve to
/// the same value.
fn schema_changes(
    use_: SchemaUse,
    old: Option<&crate::models::Schema>,
    new: Option<&crate::models::Schema>,
    components: &mut Components,
) -> Vec<ValueChange> {
    let (old, new) = (
        old.map_or(Value::Null, schema_json),
        new.map_or(Value::Null, schema_json),
    );
    if !old.is_null() && !new.is_null() && components.resolver().values_equal(&old, &new) {
        return Vec::new();
    }
    components.compare(use_, &old, &new)
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
    components: &mut Components,
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

            let use_ = SchemaUse {
                endpoint: endpoint.clone(),
                location: UseLocation::RequestBody {
                    name: new_body.name.clone(),
                },
            };
            for change in schema_changes(
                use_,
                old_body.schema.as_ref(),
                new_body.schema.as_ref(),
                components,
            ) {
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
    components: &mut Components,
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

        let old_schema = response_schema(old_response);
        let new_schema = response_schema(new_response);
        let use_ = SchemaUse {
            endpoint: endpoint.clone(),
            location: UseLocation::Response {
                status: status.clone(),
            },
        };
        for change in schema_changes(use_, old_schema, new_schema, components) {
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

    /// The severity of `change` inside a shared component reached in
    /// `direction`.
    pub(crate) fn component(direction: Direction, change: ValueChange) -> Severity {
        severity(&self::change(ChangeKind::ComponentSchemaChanged {
            component: ComponentRef {
                old: "Widget".into(),
                new: "Widget".into(),
            },
            direction,
            change,
            negated: false,
            uses: vec![SchemaUse {
                endpoint: endpoint(),
                location: UseLocation::Response {
                    status: "200".into(),
                },
            }],
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
        added, change, changed, component, parameter, removed, request, response,
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
        // An `enum` restriction appeared: values accepted before now fail.
        assert_eq!(
            request(added("/properties/s/enum", json!(["a"]))),
            Severity::Breaking
        );
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
        // The `enum` restriction went away: every value is accepted.
        assert_eq!(
            request(removed("/properties/s/enum", json!(["a", "b"]))),
            Severity::NonBreaking
        );
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
        // `not` reverses direction: dropping an excluded enum forbids more.
        assert_eq!(
            request(removed("/properties/s/not/enum", json!(["a"]))),
            Severity::Review
        );
        assert_eq!(
            request(added("/not", json!({"type": "string"}))),
            Severity::Review
        );
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
        // A new `enum` restriction narrows what the server returns.
        assert_eq!(
            response(added("/properties/s/enum", json!(["a"]))),
            Severity::NonBreaking
        );
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
        // #169: the server stops returning a value; every value clients still
        // receive was valid before.
        assert_eq!(
            response(removed("/enum/a", json!("a"))),
            Severity::NonBreaking
        );
        assert_eq!(
            response(removed("/properties/status/enum/0", json!("retired"))),
            Severity::NonBreaking
        );
    }

    #[test]
    fn severity_response_schema_review_rows() {
        // `not` reverses direction: a smaller excluded set widens the output.
        assert_eq!(
            response(removed("/not/enum/a", json!("a"))),
            Severity::Review
        );
        assert_eq!(
            response(removed("/properties/s/not/properties/x", json!({}))),
            Severity::Review
        );
        // Dropping the restriction widens the response like a new member.
        assert_eq!(
            response(removed("/properties/s/enum", json!(["a", "b"]))),
            Severity::Review
        );
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

    // ── severity: shared component rows (#181) ───────────────────────────

    #[test]
    fn severity_component_rows_follow_the_usage_direction() {
        use Direction::{Request, Response};
        let rows = [
            // (change, request verdict, response verdict)
            (
                removed("/properties/s/enum/a", json!("a")),
                Severity::Breaking,
                Severity::NonBreaking,
            ),
            (
                added("/properties/s/enum/z", json!("z")),
                Severity::NonBreaking,
                Severity::Review,
            ),
            // Whole `enum` keyword, by direction.
            (
                removed("/properties/s/enum", json!(["a"])),
                Severity::NonBreaking,
                Severity::Review,
            ),
            (
                added("/properties/s/enum", json!(["a"])),
                Severity::Breaking,
                Severity::NonBreaking,
            ),
            // `not` reverses direction: review either way.
            (
                removed("/properties/s/not/enum/a", json!("a")),
                Severity::Review,
                Severity::Review,
            ),
            (
                added("/properties/a/nullable", json!(true)),
                Severity::NonBreaking,
                Severity::Breaking,
            ),
            (
                added("/required/x", json!("x")),
                Severity::Breaking,
                Severity::NonBreaking,
            ),
            (
                removed("/properties/x", json!({})),
                Severity::Breaking,
                Severity::Breaking,
            ),
            (
                changed("/format", json!("int32"), json!("int64")),
                Severity::Review,
                Severity::Review,
            ),
            // A schema that went away entirely.
            (
                changed("", json!({"type": "object"}), Value::Null),
                Severity::Review,
                Severity::Breaking,
            ),
        ];
        for (value, on_request, on_response) in rows {
            assert_eq!(component(Request, value.clone()), on_request, "{value:?}");
            assert_eq!(component(Response, value.clone()), on_response, "{value:?}");
            // Exactly the tables an operation's own schema uses.
            assert_eq!(component(Request, value.clone()), request(value.clone()));
            assert_eq!(component(Response, value.clone()), response(value));
        }
    }

    fn component_doc(schemas: Value, paths: Value) -> ApiDocumentation {
        let spec = json!({
            "openapi": "3.0.3",
            "info": {"title": "T", "version": "1"},
            "paths": paths,
            "components": {"schemas": schemas}
        });
        crate::parse_openapi_bytes(&serde_json::to_vec(&spec).unwrap(), "json", None).unwrap()
    }

    fn body(schema: Value) -> Value {
        json!({"content": {"application/json": {"schema": schema}}})
    }

    fn ok(schema: Value) -> Value {
        json!({"200": {"description": "ok", "content": {"application/json": {"schema": schema}}}})
    }

    #[test]
    fn a_component_shared_by_request_and_response_reports_each_direction_once() {
        let r = |name: &str| json!({"$ref": format!("#/components/schemas/{name}")});
        let paths = json!({
            "/a": {"post": {"requestBody": body(r("Wrap")), "responses": ok(r("Status"))}},
            "/b": {"get": {"responses": ok(json!({"type": "array", "items": r("Wrap")}))}},
            "/c": {"get": {"parameters": [{"name": "s", "in": "query", "schema": r("Status")}],
                           "responses": ok(json!({"type": "string"}))}}
        });
        let schemas = |values: Value| {
            json!({
                "Wrap": {"type": "object", "properties": {"status": r("Status"), "self": r("Wrap")}},
                "Status": {"type": "string", "enum": values}
            })
        };
        let old = component_doc(schemas(json!(["a", "b"])), paths.clone());
        let new = component_doc(schemas(json!(["a"])), paths);
        let result = diff(&old, &new);
        let rows: Vec<(Direction, Severity, usize)> = result
            .changes
            .iter()
            .map(|change| match &change.kind {
                ChangeKind::ComponentSchemaChanged {
                    component,
                    direction,
                    change: value,
                    uses,
                    ..
                } => {
                    assert_eq!(component.old, "Status");
                    assert!(!component.renamed());
                    assert_eq!(value.pointer, "/enum/b");
                    assert_eq!(change.endpoint, uses[0].endpoint);
                    (*direction, severity(change), uses.len())
                }
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        // Request uses: POST /a body (via Wrap) and GET /c's parameter;
        // response uses: POST /a 200 and GET /b 200 (via Wrap, a cycle).
        assert_eq!(
            rows,
            vec![
                (Direction::Request, Severity::Breaking, 2),
                (Direction::Response, Severity::NonBreaking, 2),
            ]
        );
        assert!(result.has_breaking());
        assert_eq!(result.endpoints_changed(), 3);
    }

    #[test]
    fn a_retargeted_reference_compares_the_two_components() {
        let r = |name: &str| json!({"$ref": format!("#/components/schemas/{name}")});
        let doc = |schemas: &Value, target: &str| {
            component_doc(
                schemas.clone(),
                json!({"/p": {"get": {"responses": ok(r(target))}}}),
            )
        };
        let schemas = json!({
            "Pet": {"type": "object", "properties": {"name": {"type": "string"}}},
            "Dog": {"type": "object", "required": ["name"], "properties": {"name": {"type": "string"}}}
        });
        // The retarget itself (review: typed clients depend on the name),
        // then the structural difference between the two components.
        assert_eq!(
            rows(&diff(&doc(&schemas, "Pet"), &doc(&schemas, "Dog"))),
            vec![
                (
                    "/p".into(),
                    "component",
                    "Pet->Dog/$ref".into(),
                    Severity::Review
                ),
                (
                    "/p".into(),
                    "component",
                    "Pet->Dog/required/name".into(),
                    Severity::NonBreaking
                ),
            ]
        );

        // Two names for the same shape: only the retarget.
        let same = json!({
            "Pet": {"type": "object"},
            "Dog": {"type": "object", "description": "a dog"}
        });
        let result = diff(&doc(&same, "Pet"), &doc(&same, "Dog"));
        assert_eq!(
            rows(&result),
            vec![(
                "/p".into(),
                "component",
                "Pet->Dog/$ref".into(),
                Severity::Review
            )]
        );
        let ChangeKind::ComponentSchemaChanged { change, .. } = &result.changes[0].kind else {
            panic!("{result:#?}");
        };
        assert_eq!(
            change.kind,
            ValueChangeKind::Changed {
                old: json!("Pet"),
                new: json!("Dog")
            }
        );
    }

    #[test]
    fn inline_and_inline_to_ref_changes_stay_at_the_use_site() {
        let paths = |schema: Value| json!({"/p": {"get": {"responses": ok(schema)}}});
        let schemas = json!({"Pet": {"type": "object", "properties": {"name": {"type": "string"}, "age": {"type": "integer"}}}});
        let old = component_doc(
            schemas.clone(),
            paths(json!({"type": "object", "properties": {"name": {"type": "string"}}})),
        );
        let new = component_doc(schemas, paths(json!({"$ref": "#/components/schemas/Pet"})));
        let result = diff(&old, &new);
        assert_eq!(
            result.changes,
            vec![Change {
                endpoint: EndpointRef {
                    method: "GET".into(),
                    path: "/p".into()
                },
                kind: ChangeKind::ResponseSchemaChanged {
                    status: "200".into(),
                    change: added("/properties/age", json!({"type": "integer"})),
                },
            }]
        );
    }

    /// `(endpoint path, kind label, pointer, severity)` of every change.
    fn rows(result: &SpecDiff) -> Vec<(String, &'static str, String, Severity)> {
        result
            .changes
            .iter()
            .map(|change| {
                let (label, pointer) = match &change.kind {
                    ChangeKind::ResponseSchemaChanged { change: value, .. } => {
                        ("response", value.pointer.clone())
                    }
                    ChangeKind::ComponentSchemaChanged {
                        component,
                        change: value,
                        ..
                    } => (
                        "component",
                        format!("{}->{}{}", component.old, component.new, value.pointer),
                    ),
                    other => panic!("unexpected {other:?}"),
                };
                (
                    change.endpoint.path.clone(),
                    label,
                    pointer,
                    severity(change),
                )
            })
            .collect()
    }

    #[test]
    fn a_component_declared_on_one_side_is_compared_at_the_use_site() {
        // Resolution leaves an undeclared target as the literal
        // `{"$ref": ...}`; that comparison stays at the use, as before 2.3.
        let r = json!({"$ref": "#/components/schemas/Pet"});
        let old = component_doc(
            json!({}),
            json!({"/p": {"get": {"responses": ok(r.clone())}}}),
        );
        let new = component_doc(
            json!({"Pet": {"type": "object"}}),
            json!({"/p": {"get": {"responses": ok(r)}}}),
        );
        assert_eq!(
            rows(&diff(&old, &new)),
            vec![
                ("/p".into(), "response", "/$ref".into(), Severity::Review),
                ("/p".into(), "response", "/type".into(), Severity::Breaking),
            ]
        );
    }

    #[test]
    fn a_removed_alias_is_not_a_retarget_of_its_target() {
        // #182 review: `Alias` (a bare `$ref` to the unchanged `Base`) is
        // deleted while `GET /a` still references it.
        let r = |name: &str| json!({"$ref": format!("#/components/schemas/{name}")});
        let base = json!({"type": "object", "properties": {"a": {"type": "string"}}});
        let paths = json!({"/a": {"get": {"responses": ok(r("Alias"))}}});
        let old = component_doc(
            json!({"Alias": r("Base"), "Base": base.clone()}),
            paths.clone(),
        );
        let new = component_doc(json!({"Base": base}), paths);
        let result = diff(&old, &new);
        assert_eq!(
            rows(&result),
            vec![
                (
                    "/a".into(),
                    "response",
                    "/properties/a".into(),
                    Severity::Breaking
                ),
                ("/a".into(), "response", "/type".into(), Severity::Breaking),
                ("/a".into(), "response", "/$ref".into(), Severity::Review),
            ]
        );

        // The same along a chain: L0 -> L1 -> L2 -> Leaf, and L1 is deleted.
        let chain = |with_l1: bool| {
            let mut schemas = json!({
                "L0": r("L1"), "L1": r("L2"), "L2": r("Leaf"), "Leaf": {"type": "string"}
            });
            if !with_l1 {
                schemas.as_object_mut().unwrap().remove("L1");
            }
            component_doc(schemas, json!({"/c": {"get": {"responses": ok(r("L0"))}}}))
        };
        // L0 itself is declared on both sides and its reference now dangles:
        // a real change inside L0, never a retarget along the chain.
        let result = diff(&chain(true), &chain(false));
        assert_eq!(
            rows(&result),
            vec![
                (
                    "/c".into(),
                    "component",
                    "L0->L0/type".into(),
                    Severity::Breaking
                ),
                (
                    "/c".into(),
                    "component",
                    "L0->L0/$ref".into(),
                    Severity::Review
                ),
            ]
        );
    }

    #[test]
    fn a_recursive_component_behind_an_overriding_sibling_is_reported_once() {
        // #182 review: the use overlays `nullable` on X, and X references
        // itself. With refs as edges the cycle is an ordinary graph cycle:
        // X's change is reported once, at X.
        let x = |v: &str| {
            json!({"type": "object", "properties": {
                "self": {"$ref": "#/components/schemas/X"}, "v": {"type": v}
            }})
        };
        let paths = json!({"/x": {"get": {"responses": ok(
            json!({"$ref": "#/components/schemas/X", "nullable": true})
        )}}});
        let old = component_doc(json!({"X": x("string")}), paths.clone());
        let new = component_doc(json!({"X": x("integer")}), paths);
        assert_eq!(
            rows(&diff(&old, &new)),
            vec![(
                "/x".into(),
                "component",
                "X->X/properties/v/type".into(),
                Severity::Breaking
            )]
        );
    }

    #[test]
    fn a_sibling_that_pins_a_keyword_masks_that_change_at_its_use() {
        let r = |name: &str| json!({"$ref": format!("#/components/schemas/{name}")});
        let paths = json!({"/p": {"get": {"responses": ok(
            json!({"$ref": "#/components/schemas/Leaf", "type": "string"})
        )}}, "/q": {"get": {"responses": ok(r("Leaf"))}}});
        let old = component_doc(json!({"Leaf": {"type": "string"}}), paths.clone());
        let new = component_doc(json!({"Leaf": {"type": "integer"}}), paths);
        let result = diff(&old, &new);
        assert_eq!(result.changes.len(), 1, "{result:#?}");
        let ChangeKind::ComponentSchemaChanged { uses, .. } = &result.changes[0].kind else {
            panic!("{result:#?}");
        };
        // Only the unpinned use reaches the change.
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].endpoint.path, "/q");
    }

    #[test]
    fn an_indirect_cycle_behind_an_overriding_sibling_is_reported_once() {
        // #182 rounds 2-3: X -> Y -> X behind an overlay, the self-reference
        // spelled either way, and a self-reference that itself overlays a key.
        let x = |s: &str, self_ref: &str| {
            json!({
                "X": {"type": "object", "properties": {"y": {"$ref": "#/components/schemas/Y"}, "s": {"type": s}}},
                "Y": {"type": "object", "properties": {"x": {"$ref": self_ref, "type": "object"}}}
            })
        };
        let paths = json!({"/a": {"get": {"responses": ok(
            json!({"$ref": "#/components/schemas/X", "nullable": true})
        )}}});
        let expected = vec![(
            "/a".into(),
            "component",
            "X->X/properties/s/type".into(),
            Severity::Breaking,
        )];
        for self_ref in ["#/components/schemas/X", "#/definitions/X"] {
            let old = component_doc(x("string", self_ref), paths.clone());
            let new = component_doc(x("integer", self_ref), paths.clone());
            assert_eq!(rows(&diff(&old, &new)), expected, "{self_ref}");
        }
        // #182 round 3: a direct self-reference with an overlay, reached
        // through a plain reference.
        let page = |x: &str| {
            json!({"Page": {"type": "object", "properties": {
                "self": {"$ref": "#/components/schemas/Page", "type": "object"},
                "x": {"type": x}
            }}})
        };
        let plain =
            json!({"/a": {"get": {"responses": ok(json!({"$ref": "#/components/schemas/Page"}))}}});
        let old = component_doc(page("object"), plain.clone());
        let new = component_doc(page("boolean"), plain);
        assert_eq!(
            rows(&diff(&old, &new)),
            vec![(
                "/a".into(),
                "component",
                "Page->Page/properties/x/type".into(),
                Severity::Breaking
            )]
        );
    }

    #[test]
    fn a_retarget_behind_an_overlay_compares_the_two_components() {
        // #182 rounds 2-3: X and Y reference themselves; the use moves from an
        // overlaid X to an overlaid Y.
        let schemas = |y_type: &str| {
            json!({
                "X": {"type": "object", "properties": {"p": {"$ref": "#/components/schemas/X"}, "s": {"type": "string"}}},
                "Y": {"type": "object", "properties": {"p": {"$ref": "#/components/schemas/Y"}, "s": {"type": y_type}}}
            })
        };
        let path = |name: &str| {
            json!({"/a": {"get": {"responses": ok(
                json!({"$ref": format!("#/components/schemas/{name}"), "nullable": true})
            )}}})
        };
        let old = component_doc(schemas("string"), path("X"));
        // Same shape: only the retarget, once, although the pair is reached
        // again through its own self-reference.
        let new = component_doc(schemas("string"), path("Y"));
        assert_eq!(
            rows(&diff(&old, &new)),
            vec![(
                "/a".into(),
                "component",
                "X->Y/$ref".into(),
                Severity::Review
            )]
        );
        let new = component_doc(schemas("integer"), path("Y"));
        assert_eq!(
            rows(&diff(&old, &new)),
            vec![
                (
                    "/a".into(),
                    "component",
                    "X->Y/$ref".into(),
                    Severity::Review
                ),
                (
                    "/a".into(),
                    "component",
                    "X->Y/properties/s/type".into(),
                    Severity::Breaking
                ),
            ]
        );
    }

    #[test]
    fn an_overlay_masks_the_targets_change_under_the_overridden_key() {
        // The use pins `properties` wholesale: X's change under it (and the
        // reference to Z it holds) is invisible there, but not at a plain use.
        let r = |name: &str| json!({"$ref": format!("#/components/schemas/{name}")});
        let x = |t: &str| json!({"type": "object", "properties": {"z": r("Z"), "t": {"type": t}}});
        let paths = json!({
            "/pinned": {"get": {"responses": ok(json!({"$ref": "#/components/schemas/X", "properties": {}}))}},
            "/plain": {"get": {"responses": ok(r("X"))}}
        });
        let z = |t: &str| json!({"type": t});
        let old = component_doc(json!({"X": x("string"), "Z": z("string")}), paths.clone());
        let new = component_doc(json!({"X": x("integer"), "Z": z("integer")}), paths);
        assert_eq!(
            rows(&diff(&old, &new)),
            vec![
                (
                    "/plain".into(),
                    "component",
                    "X->X/properties/t/type".into(),
                    Severity::Breaking
                ),
                (
                    "/plain".into(),
                    "component",
                    "Z->Z/type".into(),
                    Severity::Breaking
                ),
            ]
        );
    }

    #[test]
    fn an_overlay_reads_the_target_through_alias_chains() {
        // #182 round 4: a sibling on one side only is compared against the
        // other side's effective value, which resolution finds by following
        // the alias chain.
        let r = |name: &str| json!({"$ref": format!("#/components/schemas/{name}")});
        let with = |name: &str, ty: &str| json!({"$ref": format!("#/components/schemas/{name}"), "type": ty});
        let doc = |schemas: &Value, use_: Value| {
            component_doc(
                schemas.clone(),
                json!({"/x": {"post": {"requestBody": body(use_.clone()), "responses": ok(use_)}}}),
            )
        };
        let one_hop = json!({"A": r("B"), "B": {"type": "integer"}});
        // al1: the sibling restates what the alias resolves to.
        assert!(
            diff(&doc(&one_hop, with("A", "integer")), &doc(&one_hop, r("A")))
                .changes
                .is_empty()
        );
        // al2: the sibling appears on the new side only.
        let type_rows = |result: SpecDiff| -> Vec<String> {
            result
                .changes
                .iter()
                .map(|change| match &change.kind {
                    ChangeKind::RequestSchemaChanged { change: value }
                    | ChangeKind::ResponseSchemaChanged { change: value, .. } => {
                        format!("{} {:?}", value.pointer, value.kind)
                    }
                    other => panic!("unexpected {other:?}"),
                })
                .collect()
        };
        let changed =
            "/type Changed { old: String(\"integer\"), new: String(\"string\") }".to_string();
        assert_eq!(
            type_rows(diff(
                &doc(&one_hop, r("A")),
                &doc(&one_hop, with("A", "string"))
            )),
            vec![changed.clone(), changed.clone()]
        );
        // Two hops.
        let two_hops = json!({"A": r("M"), "M": r("B"), "B": {"type": "integer"}});
        assert!(
            diff(
                &doc(&two_hops, with("A", "integer")),
                &doc(&two_hops, r("A"))
            )
            .changes
            .is_empty()
        );
        assert_eq!(
            type_rows(diff(
                &doc(&two_hops, r("A")),
                &doc(&two_hops, with("A", "string"))
            )),
            vec![changed.clone(), changed]
        );
        // An alias cycle resolves to nothing: the sibling is all there is.
        let cycle = json!({"A": r("B"), "B": r("A")});
        assert_eq!(
            type_rows(diff(
                &doc(&cycle, r("A")),
                &doc(&cycle, with("A", "string"))
            )),
            vec![
                "/type Added(String(\"string\"))".to_string(),
                "/type Added(String(\"string\"))".to_string()
            ]
        );
    }

    /// `(negated, severity, pointer)` of every component record.
    fn negation_rows(result: &SpecDiff) -> Vec<(bool, Severity, String)> {
        result
            .changes
            .iter()
            .map(|change| match &change.kind {
                ChangeKind::ComponentSchemaChanged {
                    negated,
                    change: value,
                    ..
                } => (*negated, severity(change), value.pointer.clone()),
                other => panic!("unexpected {other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_component_reached_through_not_is_review() {
        // #182 reference-model F1: `not` sits above the hop, so the relative
        // component pointer cannot show it; the reach carries it instead.
        let r = |name: &str| json!({"$ref": format!("#/components/schemas/{name}")});
        let e = |values: Value| json!({"type": "string", "enum": values});
        let doc = |schemas: Value, body_schema: Value| {
            component_doc(
                schemas,
                json!({"/x": {"post": {"requestBody": body(body_schema), "responses": ok(json!({"type": "string"}))}}}),
            )
        };
        let narrowed = |body_schema: Value, extra: Value| {
            let mut old = json!({"E": e(json!(["a", "b"]))});
            let mut new = json!({"E": e(json!(["a"]))});
            for (name, schema) in extra.as_object().unwrap() {
                old[name] = schema.clone();
                new[name] = schema.clone();
            }
            diff(&doc(old, body_schema.clone()), &doc(new, body_schema))
        };
        let review = vec![(true, Severity::Review, "/enum/b".to_string())];

        // The F1 pair, and its inline twin for comparison.
        let result = narrowed(json!({"not": r("E")}), json!({}));
        assert_eq!(negation_rows(&result), review);
        assert!(!result.has_breaking());
        let inline = |values: Value| doc(json!({}), json!({"not": e(values)}));
        let inline_result = diff(&inline(json!(["a", "b"])), &inline(json!(["a"])));
        assert_eq!(inline_result.count(Severity::Review), 1);
        assert!(!inline_result.has_breaking());

        // The same component reached plainly and through `not` by one body:
        // a breaking plain record and a review negated one.
        let both = narrowed(
            json!({"type": "object", "properties": {"p": r("E"), "q": {"not": r("E")}}}),
            json!({}),
        );
        assert_eq!(
            negation_rows(&both),
            vec![
                (false, Severity::Breaking, "/enum/b".to_string()),
                (true, Severity::Review, "/enum/b".to_string()),
            ]
        );
        assert!(both.has_breaking());

        // `not` -> alias -> target.
        assert_eq!(
            negation_rows(&narrowed(json!({"not": r("A")}), json!({"A": r("E")}))),
            review
        );
        // `not` inside a component the body reaches through a `$ref`.
        assert_eq!(
            negation_rows(&narrowed(r("W"), json!({"W": {"not": r("E")}}))),
            review
        );
        // A property named `not` is not the keyword.
        assert_eq!(
            negation_rows(&narrowed(
                json!({"type": "object", "properties": {"not": r("E")}}),
                json!({})
            )),
            vec![(false, Severity::Breaking, "/enum/b".to_string())]
        );
    }
}
