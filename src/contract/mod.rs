//! Format-neutral contract surfaces and adapter-classified change records.
//!
//! OpenAPI is the first adapter. GraphQL and protobuf native locations are
//! representable without making HTTP concepts part of neutral identity.

pub mod json;
pub mod openapi;
use crate::diff::json::{canonical_json, sha256_hex};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Source contract language (not its document version).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// OpenAPI or Swagger.
    Openapi,
    /// GraphQL SDL or introspection.
    Graphql,
    /// Protocol Buffers or descriptors.
    Protobuf,
}

/// A native coordinate. Pointers in OpenAPI address normalized schemas when
/// `resolved` is true; they must not be interpreted as input-file pointers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "snake_case")]
pub enum NativeLocation {
    /// OpenAPI HTTP or reusable-schema coordinate.
    Openapi {
        /// HTTP method, when operation-scoped.
        method: Option<String>,
        /// Exact template without basePath.
        path: Option<String>,
        /// Parameter location, when applicable.
        parameter_in: Option<String>,
        /// Response status, when applicable.
        status: Option<String>,
        /// Normalized schema or surface coordinate.
        pointer: String,
        /// Whether refs were expanded.
        resolved: bool,
    },
    /// GraphQL schema coordinate, including Type.field(arg:).
    Graphql {
        /// Schema coordinate.
        coordinate: String,
    },
    /// Protobuf symbol and optional wire identity.
    Protobuf {
        /// Fully qualified symbol.
        full_name: String,
        /// Field or enum number, if applicable.
        number: Option<i32>,
    },
}

/// Role of a contract entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    /// Service or GraphQL root.
    Service,
    /// HTTP operation, root field or RPC.
    Operation,
    /// Named type.
    Type,
    /// Object/input/message field.
    Field,
    /// Operation/field input argument.
    Argument,
    /// Result variant (HTTP status, RPC output).
    Result,
    /// Enum value.
    Value,
}

/// A node in the neutral contract surface. Parent links form a forest;
/// references represent shared types and recursive edges without expansion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    /// Stable identity, independent of annotations and document order.
    pub id: String,
    /// Entity category.
    pub kind: EntityKind,
    /// Native display name.
    pub name: String,
    /// Containing entity, absent for roots.
    pub parent: Option<String>,
    /// Service memberships; operations may have several.
    pub services: Vec<String>,
    /// Source coordinate.
    pub native: NativeLocation,
    /// Contract-language-specific attributes, including schema constraints.
    pub attributes: Value,
    /// Referenced named type IDs (including cyclic references).
    pub references: Vec<String>,
}

/// Complete neutral surface for a single source document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contract {
    /// Source language.
    pub format: Format,
    /// Declared API title.
    pub title: String,
    /// Declared API version.
    pub version: String,
    /// Nodes in deterministic native order.
    pub entities: Vec<Entity>,
}

/// Format-neutral change vocabulary. Native detail covers refinements that do
/// not fit these categories without inventing cross-format equivalences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// A service appeared.
    ServiceAdded,
    /// A service disappeared.
    ServiceRemoved,
    /// An operation appeared.
    OperationAdded,
    /// An operation disappeared.
    OperationRemoved,
    /// An operation's declared name changed.
    OperationRenamed,
    /// A type appeared.
    TypeAdded,
    /// A type disappeared.
    TypeRemoved,
    /// A type category changed.
    TypeKindChanged,
    /// An argument appeared.
    ArgumentAdded,
    /// An argument disappeared.
    ArgumentRemoved,
    /// Requiredness changed.
    ArgumentRequiredChanged,
    /// An argument type changed.
    ArgumentTypeChanged,
    /// A field appeared.
    FieldAdded,
    /// A field disappeared.
    FieldRemoved,
    /// A field type changed.
    FieldTypeChanged,
    /// Requiredness changed.
    FieldRequiredChanged,
    /// Nullability changed.
    NullabilityChanged,
    /// An enum value appeared.
    EnumValueAdded,
    /// An enum value disappeared.
    EnumValueRemoved,
    /// Deprecation changed.
    DeprecatedChanged,
    /// A default changed.
    DefaultChanged,
    /// Pagination shape/controls changed.
    PaginationChanged,
    /// Wire or transport identity changed.
    WireIdentityChanged,
    /// A result variant appeared.
    ResultAdded,
    /// A result variant disappeared.
    ResultRemoved,
    /// Another constraint changed.
    ConstraintChanged,
}

/// Adapter verdict, deliberately independent of neutral kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Existing clients can break.
    Breaking,
    /// Additive or relaxing.
    NonBreaking,
    /// Usage-dependent or inferred.
    Review,
}

/// One neutral fact with its adapter verdict and native explanation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    /// Content-derived vc2 identifier.
    pub id: String,
    /// Stable ID of the changed entity or containing schema root.
    pub subject: String,
    /// Neutral fact.
    pub kind: ChangeKind,
    /// Adapter verdict.
    pub severity: Severity,
    /// Compatibility dimensions (e.g. source, wire, wire_json).
    pub impacts: Vec<String>,
    /// Native location, retaining format-specific attribution.
    pub native: NativeLocation,
    /// Adapter's native change kind.
    pub native_kind: String,
    /// Lossless adapter-specific change details; absence is explicit.
    pub details: Value,
}

impl Change {
    /// Recompute the stable ID. Severity, impacts and native display details
    /// are excluded; subject, neutral kind and fact details are hashed.
    pub fn refresh_id(&mut self) {
        self.id = change_id(self);
    }
}

/// Stable entity ID: SHA-256 of canonical {v:2, format, kind, coordinate}.
pub fn entity_id(format: Format, kind: EntityKind, coordinate: &str) -> String {
    format!(
        "ve2_{}",
        sha256_hex(
            canonical_json(&json!({"v":2,"format":format,"kind":kind,"coordinate":coordinate}))
                .as_bytes()
        )
    )
}

/// Stable neutral change ID: SHA-256 of canonical {v:2, subject, kind, details}.
pub fn change_id(change: &Change) -> String {
    format!(
        "vc2_{}",
        sha256_hex(
            canonical_json(
                &json!({"v":2,"subject":change.subject,"kind":change.kind,"details":change.details})
            )
            .as_bytes()
        )
    )
}

/// An adapter owns compatibility classification. Neutral kinds alone cannot
/// decide directional nullability, source compatibility or wire compatibility.
pub trait Adapter {
    /// Normalize attributes before comparison; default compares all values.
    fn normalize_attributes(&self, subject: &Entity) -> Value {
        subject.attributes.clone()
    }

    /// Refine a native location for an attribute pointer.
    fn change_location(&self, subject: &Entity, _pointer: &str) -> NativeLocation {
        subject.native.clone()
    }

    /// Map an adapter-specific attribute difference to a neutral kind.
    /// Default preserves an opaque constraint fact for custom adapters.
    fn attribute_kind(
        &self,
        _subject: &Entity,
        _pointer: &str,
        _change: &crate::diff::ValueChangeKind,
    ) -> ChangeKind {
        ChangeKind::ConstraintChanged
    }

    /// Map a structural edge or native identity difference to a neutral fact.
    /// Returning `None` ignores an adapter-specific representation detail.
    fn structural_kind(&self, _subject: &Entity, _field: &str) -> Option<ChangeKind> {
        Some(ChangeKind::ConstraintChanged)
    }

    /// Classify a fact about this subject (including input/output context).
    fn classify(
        &self,
        subject: &Entity,
        kind: ChangeKind,
        details: &Value,
    ) -> (Severity, Vec<String>);
}

/// Neutral comparison for adapter-created surfaces. Adapters can add composite
/// facts (pagination, renames) after this basic identity/attribute comparison.
/// Changed attributes retain explicit presence and are classified by the adapter.
pub fn compare(
    old: &Contract,
    new: &Contract,
    adapter: &impl Adapter,
) -> anyhow::Result<Vec<Change>> {
    anyhow::ensure!(
        old.format == new.format,
        "Cannot compare different contract formats"
    );
    let index = |contract: &Contract| -> anyhow::Result<std::collections::HashMap<String, usize>> {
        let mut map = std::collections::HashMap::with_capacity(contract.entities.len());
        for (i, entity) in contract.entities.iter().enumerate() {
            anyhow::ensure!(
                map.insert(entity.id.clone(), i).is_none(),
                "Duplicate contract entity ID: {}",
                entity.id
            );
        }
        Ok(map)
    };
    let old_index = index(old)?;
    let new_index = index(new)?;
    let mut changes = Vec::new();
    for entity in &old.entities {
        if let Some(&position) = new_index.get(&entity.id) {
            let next = &new.entities[position];
            let mut values = Vec::new();
            crate::diff::diff_values(
                &adapter.normalize_attributes(entity),
                &adapter.normalize_attributes(next),
                &mut String::new(),
                &mut values,
            );
            for value in values {
                use crate::diff::ValueChangeKind as V;
                let (before, after) = match &value.kind {
                    V::Added(v) => (json!({"present":false}), json!({"present":true,"value":v})),
                    V::Removed(v) => (json!({"present":true,"value":v}), json!({"present":false})),
                    V::Changed { old, new } => (
                        json!({"present":true,"value":old}),
                        json!({"present":true,"value":new}),
                    ),
                };
                let kind = adapter.attribute_kind(entity, &value.pointer, &value.kind);
                let details = json!({"pointer":value.pointer,"before":before,"after":after});
                push(&mut changes, entity, kind, details, adapter);
            }
            for (field, before, after) in structural_values(entity, next) {
                if before != after
                    && let Some(kind) = adapter.structural_kind(entity, field)
                {
                    push(
                        &mut changes,
                        entity,
                        kind,
                        json!({"structural":field,"before":before,"after":after}),
                        adapter,
                    );
                }
            }
        } else {
            push(
                &mut changes,
                entity,
                removed(entity.kind),
                json!({}),
                adapter,
            );
        }
    }
    for entity in &new.entities {
        if !old_index.contains_key(&entity.id) {
            push(&mut changes, entity, added(entity.kind), json!({}), adapter);
        }
    }
    Ok(changes)
}

fn structural_values(old: &Entity, new: &Entity) -> Vec<(&'static str, Value, Value)> {
    let mut old_services = old.services.clone();
    let mut new_services = new.services.clone();
    old_services.sort();
    new_services.sort();
    let mut old_references = old.references.clone();
    let mut new_references = new.references.clone();
    old_references.sort();
    new_references.sort();
    vec![
        ("name", json!(old.name), json!(new.name)),
        ("parent", json!(old.parent), json!(new.parent)),
        ("services", json!(old_services), json!(new_services)),
        ("references", json!(old_references), json!(new_references)),
        ("native", json!(old.native), json!(new.native)),
    ]
}

fn push(
    changes: &mut Vec<Change>,
    entity: &Entity,
    kind: ChangeKind,
    details: Value,
    adapter: &impl Adapter,
) {
    let (severity, impacts) = adapter.classify(entity, kind, &details);
    let native = adapter.change_location(
        entity,
        details.get("pointer").and_then(Value::as_str).unwrap_or(""),
    );
    let mut change = Change {
        id: String::new(),
        subject: entity.id.clone(),
        kind,
        severity,
        impacts,
        native,
        native_kind: "surface_changed".into(),
        details,
    };
    change.refresh_id();
    changes.push(change);
}
fn added(kind: EntityKind) -> ChangeKind {
    match kind {
        EntityKind::Service => ChangeKind::ServiceAdded,
        EntityKind::Operation => ChangeKind::OperationAdded,
        EntityKind::Type => ChangeKind::TypeAdded,
        EntityKind::Field => ChangeKind::FieldAdded,
        EntityKind::Argument => ChangeKind::ArgumentAdded,
        EntityKind::Result => ChangeKind::ResultAdded,
        EntityKind::Value => ChangeKind::EnumValueAdded,
    }
}
fn removed(kind: EntityKind) -> ChangeKind {
    match kind {
        EntityKind::Service => ChangeKind::ServiceRemoved,
        EntityKind::Operation => ChangeKind::OperationRemoved,
        EntityKind::Type => ChangeKind::TypeRemoved,
        EntityKind::Field => ChangeKind::FieldRemoved,
        EntityKind::Argument => ChangeKind::ArgumentRemoved,
        EntityKind::Result => ChangeKind::ResultRemoved,
        EntityKind::Value => ChangeKind::EnumValueRemoved,
    }
}

// Strip descriptive annotations only at schema objects, not named property keys.
pub(super) fn semantic_attributes(value: &Value) -> Value {
    const SCHEMA_MAPS: &[&str] = &["properties", "patternProperties", "definitions", "$defs"];
    const SCHEMA_CHILDREN: &[&str] = &[
        "schema",
        "items",
        "not",
        "additionalProperties",
        "if",
        "then",
        "else",
    ];
    const SCHEMA_ARRAYS: &[&str] = &["allOf", "oneOf", "anyOf"];
    let Some(map) = value.as_object() else {
        return value.clone();
    };
    let mut output = serde_json::Map::new();
    for (key, child) in map {
        if ["description", "title", "example", "examples"].contains(&key.as_str())
            || key.starts_with("x-")
        {
            continue;
        }
        let normalized = if key == "required" || key == "enum" {
            if let Some(items) = child.as_array() {
                let mut items = items.clone();
                items.sort_by_key(crate::diff::json::canonical_json);
                Value::Array(items)
            } else {
                child.clone()
            }
        } else if SCHEMA_MAPS.contains(&key.as_str()) {
            child
                .as_object()
                .map(|members| {
                    Value::Object(
                        members
                            .iter()
                            .map(|(name, schema)| (name.clone(), semantic_attributes(schema)))
                            .collect(),
                    )
                })
                .unwrap_or_else(|| child.clone())
        } else if SCHEMA_CHILDREN.contains(&key.as_str()) {
            semantic_attributes(child)
        } else if SCHEMA_ARRAYS.contains(&key.as_str()) {
            child
                .as_array()
                .map(|items| Value::Array(items.iter().map(semantic_attributes).collect()))
                .unwrap_or_else(|| child.clone())
        } else {
            child.clone()
        };
        output.insert(key.clone(), normalized);
    }
    Value::Object(output)
}
