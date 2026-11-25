//! Compares `$ref` targets as edges: every schema use (a parameter, a request
//! body, a response) is compared locally with [`PairResolver::localize`], which
//! records each pair of component names that both sides reference at the same
//! position. Each `(old component, new component)` pair is then compared once,
//! the same way, and its changes are reported once per usage direction with
//! every use that reaches it (directly or through other changed or
//! retargeted components) listed beside them.

use std::collections::HashSet;

use indexmap::{IndexMap, IndexSet};
use serde_json::Value;

use super::{Change, ChangeKind, ComponentRef, Direction, SchemaUse, ValueChange, value_changes};
use crate::resolve::{PairResolver, RefEdge};
use crate::utils::canonicalize_schema_value;

/// Collects the local comparisons of one diff and, at the end, the changes of
/// the component pairs they reach.
pub(super) struct Components<'r, 'a> {
    resolver: &'r PairResolver<'a>,
    /// Uses that reference at least one pair worth comparing, in report order.
    uses: Vec<SchemaUse>,
    /// The pairs each entry of `uses` references directly.
    use_edges: Vec<Vec<RefEdge>>,
}

impl<'r, 'a> Components<'r, 'a> {
    pub(super) fn new(resolver: &'r PairResolver<'a>) -> Self {
        Components {
            resolver,
            uses: Vec::new(),
            use_edges: Vec::new(),
        }
    }

    pub(super) fn resolver(&self) -> &'r PairResolver<'a> {
        self.resolver
    }

    /// Drops edges that provably compare equal: a component paired with
    /// itself whose whole reference closure is unchanged.
    fn relevant(&self, edges: Vec<RefEdge>) -> Vec<RefEdge> {
        let mut seen = HashSet::new();
        edges
            .into_iter()
            .filter(
                |RefEdge {
                     names: (old, new), ..
                 }| old != new || !self.resolver.is_stable(old),
            )
            .filter(|edge| seen.insert(edge.names.clone()))
            .collect()
    }

    /// The local changes between two serialized schemas (`Null` when absent)
    /// of one use, with references compared as edges; records the edges.
    pub(super) fn compare(
        &mut self,
        use_: SchemaUse,
        old: &Value,
        new: &Value,
    ) -> Vec<ValueChange> {
        let (changes, edges) = self.local_changes(old, new);
        if !edges.is_empty() {
            self.uses.push(use_);
            self.use_edges.push(edges);
        }
        changes
    }

    fn local_changes(&self, old: &Value, new: &Value) -> (Vec<ValueChange>, Vec<RefEdge>) {
        let mut edges = Vec::new();
        let (mut old, mut new) = self.resolver.localize(old, new, &mut edges);
        canonicalize_schema_value(&mut old);
        canonicalize_schema_value(&mut new);
        (value_changes(&old, &new), self.relevant(edges))
    }

    /// Compares every component pair the recorded uses reach, once each, and
    /// returns their changes: per pair (in old-document then new-document
    /// component order), per change, one record per usage direction that
    /// reaches the pair.
    pub(super) fn finish(self) -> Vec<Change> {
        let mut pairs: IndexSet<(String, String)> = IndexSet::new();
        let intern =
            |edge: RefEdge, pairs: &mut IndexSet<(String, String)>| pairs.insert_full(edge.names).0;
        let use_targets: Vec<Vec<usize>> = self
            .use_edges
            .iter()
            .map(|edges| {
                edges
                    .iter()
                    .map(|edge| intern(edge.clone(), &mut pairs))
                    .collect()
            })
            .collect();

        // Breadth-first over pairs: each is compared exactly once, so the
        // work is linear in the size of the changed components no matter how
        // many reference paths reach them.
        let mut pair_targets: Vec<Vec<usize>> = Vec::new();
        let mut pair_changes: Vec<Vec<ValueChange>> = Vec::new();
        let mut next = 0;
        while next < pairs.len() {
            let (old, new) = pairs.get_index(next).expect("interned");
            let (changes, edges) = self.local_changes(
                &self.resolver.old.component_value(old),
                &self.resolver.new.component_value(new),
            );
            pair_changes.push(changes);
            pair_targets.push(
                edges
                    .into_iter()
                    .map(|edge| intern(edge, &mut pairs))
                    .collect(),
            );
            next += 1;
        }

        // Reverse edges, to find every use that reaches a changed pair.
        let mut pair_sources: Vec<Vec<usize>> = vec![Vec::new(); pairs.len()];
        for (from, targets) in pair_targets.iter().enumerate() {
            for &to in targets {
                pair_sources[to].push(from);
            }
        }
        let mut use_sources: Vec<Vec<usize>> = vec![Vec::new(); pairs.len()];
        for (use_index, targets) in use_targets.iter().enumerate() {
            for &to in targets {
                use_sources[to].push(use_index);
            }
        }

        let position = |schemas: &IndexMap<String, crate::models::Schema>, name: &str| {
            schemas.get_index_of(name).unwrap_or(usize::MAX)
        };
        let mut changed: Vec<usize> = (0..pairs.len())
            .filter(|&index| !pair_changes[index].is_empty())
            .collect();
        changed.sort_by_cached_key(|&index| {
            let (old, new) = pairs.get_index(index).expect("interned");
            (
                position(self.resolver.old.schemas, old),
                position(self.resolver.new.schemas, new),
                old.clone(),
                new.clone(),
            )
        });

        let mut out = Vec::new();
        for index in changed {
            let (old, new) = pairs.get_index(index).expect("interned");
            let component = ComponentRef {
                old: old.clone(),
                new: new.clone(),
            };
            let reaching = reaching_uses(index, &pair_sources, &use_sources);
            let by_direction: Vec<(Direction, Vec<SchemaUse>)> =
                [Direction::Request, Direction::Response]
                    .into_iter()
                    .map(|direction| {
                        let uses = reaching
                            .iter()
                            .map(|&use_index| &self.uses[use_index])
                            .filter(|use_| use_.location.direction() == direction)
                            .cloned()
                            .collect::<Vec<_>>();
                        (direction, uses)
                    })
                    .filter(|(_, uses)| !uses.is_empty())
                    .collect();
            for change in &pair_changes[index] {
                for (direction, uses) in &by_direction {
                    out.push(Change {
                        endpoint: uses[0].endpoint.clone(),
                        kind: ChangeKind::ComponentSchemaChanged {
                            component: component.clone(),
                            direction: *direction,
                            change: change.clone(),
                            uses: uses.clone(),
                        },
                    });
                }
            }
        }
        out
    }
}

/// Every use (index into the collector's `uses`, ascending) whose reference
/// closure contains pair `target`.
fn reaching_uses(
    target: usize,
    pair_sources: &[Vec<usize>],
    use_sources: &[Vec<usize>],
) -> Vec<usize> {
    let mut visited = vec![false; pair_sources.len()];
    let mut stack = vec![target];
    visited[target] = true;
    let mut uses = Vec::new();
    while let Some(pair) = stack.pop() {
        uses.extend_from_slice(&use_sources[pair]);
        for &source in &pair_sources[pair] {
            if !visited[source] {
                visited[source] = true;
                stack.push(source);
            }
        }
    }
    uses.sort_unstable();
    uses.dedup();
    uses
}
