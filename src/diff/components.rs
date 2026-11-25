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

use super::{
    Change, ChangeKind, ComponentRef, Direction, SchemaUse, ValueChange, ValueChangeKind,
    value_changes,
};
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
            .filter(|edge| seen.insert(edge.clone()))
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
        let intern = |edge: RefEdge, pairs: &mut IndexSet<(String, String)>| Target {
            to: pairs.insert_full(edge.names).0,
            at: edge.at,
            mask: edge.mask,
        };
        let use_targets: Targets = self
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
        let mut pair_targets: Targets = Vec::new();
        let mut pair_changes: Vec<Vec<ValueChange>> = Vec::new();
        let mut next = 0;
        while next < pairs.len() {
            let (old, new) = pairs.get_index(next).expect("interned");
            let (changes, edges) = self.local_changes(
                &self.resolver.old.component_value(old),
                &self.resolver.new.component_value(new),
            );
            // A reference retargeted to a differently named component is a
            // change even when the two shapes match: generated clients and
            // typed consumers depend on the name. Review either way.
            let mut changes = changes;
            if old != new {
                changes.insert(
                    0,
                    ValueChange {
                        pointer: "/$ref".to_string(),
                        kind: ValueChangeKind::Changed {
                            old: Value::String(old.clone()),
                            new: Value::String(new.clone()),
                        },
                    },
                );
            }
            pair_changes.push(changes);
            pair_targets.push(
                edges
                    .into_iter()
                    .map(|edge| intern(edge, &mut pairs))
                    .collect(),
            );
            next += 1;
        }

        let reach = Reach::new(&pairs, &use_targets, &pair_targets);

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
            for change in &pair_changes[index] {
                let reaching = reach.uses_of(index, &change.pointer);
                for direction in [Direction::Request, Direction::Response] {
                    let uses: Vec<SchemaUse> = reaching
                        .iter()
                        .map(|&use_index| &self.uses[use_index])
                        .filter(|use_| use_.location.direction() == direction)
                        .cloned()
                        .collect();
                    if uses.is_empty() {
                        continue;
                    }
                    out.push(Change {
                        endpoint: uses[0].endpoint.clone(),
                        kind: ChangeKind::ComponentSchemaChanged {
                            component: component.clone(),
                            direction,
                            change: change.clone(),
                            uses,
                        },
                    });
                }
            }
        }
        out
    }
}

/// One edge as the reachability search sees it (see [`RefEdge`]).
struct Target {
    to: usize,
    at: Option<String>,
    mask: Vec<String>,
}

type Targets = Vec<Vec<Target>>;

/// The first decoded token of a pointer into a component, or `None` at the
/// root.
fn top_key(pointer: &str) -> Option<String> {
    pointer
        .split('/')
        .nth(1)
        .map(crate::utils::decode_json_pointer_token)
}

/// Which uses reach which pairs.
///
/// Without overlays (the normal case) a pair is reached by every use whose
/// edges lead to it, found by a reverse search per pair. With an overlay
/// (a `$ref` whose siblings override its target), the keys it masks hide
/// the target's changes under them through that edge, and the target's own
/// edges under them; a forward search per use tracks the mask each pair is
/// entered with. The states are (pair, mask), so the search is polynomial.
enum Reach {
    Reverse {
        pair_sources: Vec<Vec<usize>>,
        use_sources: Vec<Vec<usize>>,
    },
    /// Per pair, each reaching use with every mask it arrives with.
    Forward(Vec<Vec<(usize, Vec<String>)>>),
}

impl Reach {
    fn new(
        pairs: &IndexSet<(String, String)>,
        use_targets: &Targets,
        pair_targets: &Targets,
    ) -> Self {
        let masked = use_targets
            .iter()
            .chain(pair_targets)
            .flatten()
            .any(|target| !target.mask.is_empty());
        if !masked {
            let mut pair_sources = vec![Vec::new(); pairs.len()];
            for (from, targets) in pair_targets.iter().enumerate() {
                for target in targets {
                    pair_sources[target.to].push(from);
                }
            }
            let mut use_sources = vec![Vec::new(); pairs.len()];
            for (use_index, targets) in use_targets.iter().enumerate() {
                for target in targets {
                    use_sources[target.to].push(use_index);
                }
            }
            return Reach::Reverse {
                pair_sources,
                use_sources,
            };
        }
        let mut reached: Vec<Vec<(usize, Vec<String>)>> = vec![Vec::new(); pairs.len()];
        for (use_index, targets) in use_targets.iter().enumerate() {
            let mut seen: HashSet<(usize, Vec<String>)> = HashSet::new();
            let mut stack: Vec<(usize, Vec<String>)> = targets
                .iter()
                .map(|target| (target.to, target.mask.clone()))
                .collect();
            while let Some((pair, mask)) = stack.pop() {
                if !seen.insert((pair, mask.clone())) {
                    continue;
                }
                for target in &pair_targets[pair] {
                    let next = match &target.at {
                        // Under a masked key the overlay replaced it.
                        Some(key) if mask.contains(key) => continue,
                        Some(_) => target.mask.clone(),
                        // An alias passes the overlay on to its target.
                        None => {
                            let mut next = mask.clone();
                            next.extend(target.mask.iter().cloned());
                            next.sort();
                            next.dedup();
                            next
                        }
                    };
                    stack.push((target.to, next));
                }
                reached[pair].push((use_index, mask));
            }
        }
        Reach::Forward(reached)
    }

    /// Every use (index into the collector's `uses`, ascending) that sees a
    /// change at `pointer` in pair `target`.
    fn uses_of(&self, target: usize, pointer: &str) -> Vec<usize> {
        match self {
            Reach::Reverse {
                pair_sources,
                use_sources,
            } => reaching_uses(target, pair_sources, use_sources),
            Reach::Forward(reached) => {
                let key = top_key(pointer);
                let mut uses: Vec<usize> = reached[target]
                    .iter()
                    .filter(|(_, mask)| key.as_ref().is_none_or(|key| !mask.contains(key)))
                    .map(|(use_index, _)| *use_index)
                    .collect();
                uses.sort_unstable();
                uses.dedup();
                uses
            }
        }
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
