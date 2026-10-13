//! JSON v2 envelope around neutral surfaces and facts.
use super::{Change, Contract, Severity};
use crate::diff::{
    Deltas,
    json::{JsonDeltas, JsonGenerator},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One side of a neutral comparison, including its full usable surface.
#[derive(Debug, Serialize, Deserialize)]
pub struct Side {
    /// Raw input SHA-256.
    pub file_sha256: String,
    /// Adapted contract.
    pub contract: Contract,
}
/// Neutral summary (counts are facts, including independent type facts).
#[derive(Debug, Serialize, Deserialize)]
pub struct Summary {
    /// Added operations.
    pub operations_added: usize,
    /// Removed operations.
    pub operations_removed: usize,
    /// Surviving changed operations (including argument/result facts).
    pub operations_changed: usize,
    /// Breaking facts.
    pub breaking: usize,
    /// Non-breaking facts.
    pub non_breaking: usize,
    /// Review facts.
    pub review: usize,
}
/// Machine-readable neutral contract v2.
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonDiff {
    /// Always 2.
    pub schema_version: u8,
    /// Generator identity.
    pub generator: JsonGenerator,
    /// Old source and surface.
    pub old: Side,
    /// New source and surface.
    pub new: Side,
    /// Neutral counts.
    pub summary: Summary,
    /// Adapter-classified facts.
    pub changes: Vec<Change>,
    /// Optional existing OpenAPI report deltas.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deltas: Option<JsonDeltas>,
}
impl JsonDiff {
    /// Whether the adapter classified any fact as breaking.
    pub fn has_breaking(&self) -> bool {
        self.summary.breaking > 0
    }
}
/// Build an OpenAPI neutral v2 comparison; input hashes come from exact parsed bytes.
///
/// Propagates duplicate entity IDs from the surface comparison instead of panicking.
pub fn openapi(
    old: &crate::ApiDocumentation,
    new: &crate::ApiDocumentation,
    deltas: Option<&Deltas>,
    old_sha: &str,
    new_sha: &str,
) -> anyhow::Result<JsonDiff> {
    let legacy = crate::diff::json::to_json(&crate::diff::diff(old, new), deltas, old_sha, new_sha);
    Ok(to_json(
        super::openapi::adapt(old),
        super::openapi::adapt(new),
        super::openapi::diff(old, new)?,
        old_sha,
        new_sha,
        legacy.deltas,
    ))
}

/// Build a v2 envelope for any adapter-created surfaces and classified facts.
/// Operation counts follow subject ancestry; named-type facts remain independent.
pub fn to_json(
    old: Contract,
    new: Contract,
    changes: Vec<Change>,
    old_sha: &str,
    new_sha: &str,
    deltas: Option<JsonDeltas>,
) -> JsonDiff {
    let entities: std::collections::HashMap<_, _> = old
        .entities
        .iter()
        .chain(&new.entities)
        .map(|e| (e.id.as_str(), e))
        .collect();
    let surviving: BTreeSet<_> = new
        .entities
        .iter()
        .filter(|e| {
            e.kind == super::EntityKind::Operation && old.entities.iter().any(|o| o.id == e.id)
        })
        .map(|e| e.id.as_str())
        .collect();
    let operations_changed = changes
        .iter()
        .filter_map(|change| {
            if matches!(
                change.kind,
                super::ChangeKind::OperationAdded | super::ChangeKind::OperationRemoved
            ) {
                return None;
            }
            let mut id = change.subject.as_str();
            let mut visited = BTreeSet::new();
            while visited.insert(id) {
                let entity = entities.get(id)?;
                if entity.kind == super::EntityKind::Operation {
                    return surviving.contains(id).then_some(id);
                }
                id = entity.parent.as_deref()?;
            }
            None
        })
        .collect::<BTreeSet<_>>()
        .len();
    let summary = Summary {
        operations_added: changes
            .iter()
            .filter(|c| c.kind == super::ChangeKind::OperationAdded)
            .count(),
        operations_removed: changes
            .iter()
            .filter(|c| c.kind == super::ChangeKind::OperationRemoved)
            .count(),
        operations_changed,
        breaking: changes
            .iter()
            .filter(|c| c.severity == Severity::Breaking)
            .count(),
        non_breaking: changes
            .iter()
            .filter(|c| c.severity == Severity::NonBreaking)
            .count(),
        review: changes
            .iter()
            .filter(|c| c.severity == Severity::Review)
            .count(),
    };
    JsonDiff {
        schema_version: 2,
        generator: JsonGenerator {
            name: "vimanam".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        old: Side {
            file_sha256: old_sha.into(),
            contract: old,
        },
        new: Side {
            file_sha256: new_sha.into(),
            contract: new,
        },
        summary,
        changes,
        deltas,
    }
}
