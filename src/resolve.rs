//! Cached `$ref` resolution for `diff` (#152).
//!
//! `diff` compares request, parameter and response schemas as fully resolved
//! JSON: every component `$ref` is inlined (see [`DocResolver::resolve`] for the
//! exact rules). Doing that naively re-serialized the target `Schema` on every
//! hop and resolved every schema of every matched operation, including
//! unchanged ones; on a densely cross-linked graph such as Stripe's a single
//! resolved schema can have millions of nodes. This module keeps the resolved
//! output byte-for-byte identical while avoiding the work it does not need:
//!
//! - each component `Schema` is converted to a `Value` once per document;
//! - [`PairResolver::values_equal`] proves, without resolving anything, that
//!   two schemas resolve to identical values, so unchanged operations skip
//!   resolution and comparison entirely ([`PairResolver::resolve_pair`]
//!   serializes each root once for both the proof and the resolution);
//! - [`DocResolver::resolve_shallow`] resolves only the top few JSON levels for
//!   readers (pagination inference) that never look deeper.
//!
//! Expansions are deliberately not memoized: the result is an owned tree, so a
//! cache hit still clones the whole subtree, and on OpenAI's spec that made the
//! diff slower and ~40% larger in memory. When a schema did change, the size
//! of its resolved form is the real bound: a change under a cross-linked cycle
//! is reported once per reference path, which on Stripe means millions of rows.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::models::Schema;
use crate::utils::decode_json_pointer_token;

/// Maximum number of `$ref` expansions on one chain in
/// [`DocResolver::resolve`]. Termination is already guaranteed by the cycle
/// guard (the expansion stack), so this is purely a size guard against a spec
/// whose acyclic reference chains are absurdly long; plain object and array
/// nesting does not count towards it. Deliberately far above the renderer's
/// nesting cap: the diff must see a change however deep it sits.
pub(crate) const MAX_RESOLVE_DEPTH: usize = 64;

/// The component name a schema `$ref` points at, for the two forms the parser
/// collects into `ApiDocumentation::schemas`: `#/components/schemas/Name` and
/// `#/definitions/Name`. `None` for any other reference shape, which never
/// resolves.
fn reference_name(reference: &str) -> Option<String> {
    reference
        .strip_prefix("#/components/schemas/")
        .or_else(|| reference.strip_prefix("#/definitions/"))
        .map(decode_json_pointer_token)
}

/// `schema` as JSON, the form every resolution and comparison starts from.
pub(crate) fn schema_json(schema: &Schema) -> Value {
    serde_json::to_value(schema).unwrap_or(Value::Null)
}

/// Whether a `$ref` object appears anywhere in `value`.
pub(crate) fn contains_ref(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.contains_key("$ref") || map.values().any(contains_ref),
        Value::Array(items) => items.iter().any(contains_ref),
        _ => false,
    }
}

/// Every `$ref` string anywhere in `value`, in document order.
fn collect_refs<'v>(value: &'v Value, out: &mut Vec<&'v str>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(reference)) = map.get("$ref") {
                out.push(reference);
            }
            for child in map.values() {
                collect_refs(child, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_refs(item, out);
            }
        }
        _ => {}
    }
}

/// Resolves schemas against one document's components, converting each
/// component to a `Value` at most once. Single-threaded (interior mutability
/// via `RefCell`).
pub(crate) struct DocResolver<'a> {
    schemas: &'a IndexMap<String, Schema>,
    /// Component name -> serialized `Schema` (`None`: not a component).
    components: RefCell<HashMap<String, Option<Rc<Value>>>>,
}

/// The mutable state of one resolution walk.
struct Walk {
    /// The references being expanded, outermost first.
    stack: Vec<String>,
    /// Objects at this many JSON levels below the root are left as written
    /// (`usize::MAX` for a full resolution).
    max_levels: usize,
}

impl<'a> DocResolver<'a> {
    pub(crate) fn new(schemas: &'a IndexMap<String, Schema>) -> Self {
        DocResolver {
            schemas,
            components: RefCell::new(HashMap::new()),
        }
    }

    /// The serialized component `name`, converted once per document.
    fn component(&self, name: &str) -> Option<Rc<Value>> {
        if let Some(cached) = self.components.borrow().get(name) {
            return cached.clone();
        }
        let value = self
            .schemas
            .get(name)
            .map(|schema| Rc::new(serde_json::to_value(schema).unwrap_or(Value::Null)));
        self.components
            .borrow_mut()
            .insert(name.to_string(), value.clone());
        value
    }

    fn target(&self, reference: &str) -> Option<Rc<Value>> {
        self.component(&reference_name(reference)?)
    }

    /// Materialises `schema` as a self-contained JSON value with every
    /// component `$ref` replaced by its target, recursively.
    ///
    /// This is the resolved-shape view that `diff` compares: two operations
    /// whose objects are byte-identical still differ here when a shared schema
    /// they reference changed. Resolution rules:
    ///
    /// - An object of the form `{"$ref": r, ...siblings}` is replaced by the
    ///   target of `r`, with any sibling keys merged over it (siblings win).
    /// - A reference already being expanded on the current chain is left as
    ///   the literal `{"$ref": r}` object, so self-referential schemas
    ///   terminate.
    /// - A reference that cannot be resolved is likewise left literal, so two
    ///   specs that are equally unresolved compare equal instead of diverging.
    /// - Expansion stops after [`MAX_RESOLVE_DEPTH`] `$ref` hops on one chain;
    ///   anything beyond is kept as written. Descending plain objects and
    ///   arrays does not consume the budget.
    pub(crate) fn resolve(&self, schema: &Schema) -> Value {
        self.resolve_value(&schema_json(schema))
    }

    /// [`Self::resolve`] over an already serialized schema.
    pub(crate) fn resolve_value(&self, value: &Value) -> Value {
        let mut walk = Walk {
            stack: Vec::new(),
            max_levels: usize::MAX,
        };
        self.expand(value, 0, 0, &mut walk)
    }

    /// [`Self::resolve`], except that objects `levels` or more JSON levels
    /// below the root are left exactly as written (unresolved). Every object
    /// above that is identical to the full resolution, so a reader that only
    /// inspects keys of objects at depth `< levels` sees the same data.
    pub(crate) fn resolve_shallow(&self, schema: &Schema, levels: usize) -> Value {
        let value = schema_json(schema);
        let mut walk = Walk {
            stack: Vec::new(),
            max_levels: levels,
        };
        self.expand(&value, 0, 0, &mut walk)
    }

    /// `depth` counts `$ref` expansions on the current chain; `level` counts
    /// JSON nesting (only consulted by shallow resolution).
    fn expand(&self, value: &Value, depth: usize, level: usize, walk: &mut Walk) -> Value {
        match value {
            Value::Object(map) => {
                if level >= walk.max_levels {
                    return value.clone();
                }
                if let Some(Value::String(reference)) = map.get("$ref") {
                    return self.expand_ref(reference, map, depth, level, walk);
                }
                map.iter()
                    .map(|(key, child)| (key.clone(), self.expand(child, depth, level + 1, walk)))
                    .collect::<Map<String, Value>>()
                    .into()
            }
            Value::Array(items) => items
                .iter()
                .map(|item| self.expand(item, depth, level + 1, walk))
                .collect::<Vec<Value>>()
                .into(),
            other => other.clone(),
        }
    }

    fn expand_ref(
        &self,
        reference: &str,
        map: &Map<String, Value>,
        depth: usize,
        level: usize,
        walk: &mut Walk,
    ) -> Value {
        if depth >= MAX_RESOLVE_DEPTH || walk.stack.iter().any(|on| on == reference) {
            // Cycle (or an implausibly long chain): keep the pointer as a
            // marker and stop here.
            return Value::Object(map.clone());
        }
        let Some(target) = self.target(reference) else {
            return Value::Object(map.clone());
        };

        walk.stack.push(reference.to_string());
        let resolved = match target.as_ref() {
            // A bare `{"$ref": r}` to an object resolves to the target itself,
            // expanded in place without copying it first.
            target @ Value::Object(_) if map.len() == 1 => {
                self.expand(target, depth + 1, level, walk)
            }
            target => {
                // Start from the target and lay any sibling keys over it.
                let mut merged = match target {
                    Value::Object(target_map) => target_map.clone(),
                    _ => Map::new(),
                };
                for (key, sibling) in map {
                    if key != "$ref" {
                        merged.insert(key.clone(), sibling.clone());
                    }
                }
                self.expand(&Value::Object(merged), depth + 1, level, walk)
            }
        };
        walk.stack.pop();
        resolved
    }
}

/// The resolvers of an old and a new document, plus which component names may
/// resolve differently between them.
pub(crate) struct PairResolver<'a> {
    pub(crate) old: DocResolver<'a>,
    pub(crate) new: DocResolver<'a>,
    /// Component names whose resolved value can differ between the two
    /// documents: present on one side only, serialized differently, or
    /// referencing (transitively) such a name.
    unstable: HashSet<String>,
}

impl<'a> PairResolver<'a> {
    pub(crate) fn new(
        old: &'a IndexMap<String, Schema>,
        new: &'a IndexMap<String, Schema>,
    ) -> Self {
        let old_resolver = DocResolver::new(old);
        let new_resolver = DocResolver::new(new);

        let mut unstable = HashSet::new();
        // Referenced name -> names whose (identical) value references it.
        let mut referrers: HashMap<String, Vec<&str>> = HashMap::new();
        let names = old
            .keys()
            .chain(new.keys().filter(|n| !old.contains_key(*n)));
        // Serialized once here to compare, then dropped: resolution later
        // converts (and caches) only the components it actually reaches, which
        // keeps peak memory near a single copy of the document.
        let serialize = |schema: &Schema| serde_json::to_value(schema).unwrap_or(Value::Null);
        for name in names {
            match (old.get(name).map(serialize), new.get(name).map(serialize)) {
                (Some(before), Some(after)) if before == after => {
                    let mut refs = Vec::new();
                    collect_refs(&before, &mut refs);
                    for reference in refs {
                        if let Some(target) = reference_name(reference) {
                            referrers.entry(target).or_default().push(name);
                        }
                    }
                }
                _ => {
                    unstable.insert(name.clone());
                }
            }
        }
        let mut queue: Vec<String> = unstable.iter().cloned().collect();
        while let Some(name) = queue.pop() {
            for &referrer in referrers.get(&name).into_iter().flatten() {
                if unstable.insert(referrer.to_string()) {
                    queue.push(referrer.to_string());
                }
            }
        }

        PairResolver {
            old: old_resolver,
            new: new_resolver,
            unstable,
        }
    }

    /// Whether the serialized schemas `before` (resolved against the old
    /// document) and `after` (against the new one) are guaranteed to resolve to identical values: they
    /// serialize identically and every reference they contain resolves to a
    /// component that is identical on both sides, transitively. Resolution is
    /// a function of the value and the referenced components alone, so the
    /// resolved values are then equal and any comparison of them is empty.
    /// `false` means only "not proven".
    pub(crate) fn values_equal(&self, before: &Value, after: &Value) -> bool {
        if before != after {
            return false;
        }
        let mut refs = Vec::new();
        collect_refs(before, &mut refs);
        refs.into_iter().all(|reference| {
            reference_name(reference).is_none_or(|name| !self.unstable.contains(&name))
        })
    }

    /// `old` resolved against the old document and `new` against the new
    /// one, or `None` when [`Self::values_equal`] proves they resolve
    /// identically. Each root is serialized once for both steps.
    pub(crate) fn resolve_pair(&self, old: &Schema, new: &Schema) -> Option<(Value, Value)> {
        let (before, after) = (schema_json(old), schema_json(new));
        if self.values_equal(&before, &after) {
            return None;
        }
        Some((
            self.old.resolve_value(&before),
            self.new.resolve_value(&after),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{DocResolver, MAX_RESOLVE_DEPTH, PairResolver};

    /// The proof as callers use it: `resolve_pair` declines to resolve.
    fn proven_equal(pair: &PairResolver, old: &Schema, new: &Schema) -> bool {
        pair.resolve_pair(old, new).is_none()
    }
    use crate::models::Schema;
    use indexmap::IndexMap;
    use serde_json::{Value, json};

    fn schemas(entries: &[(&str, Value)]) -> IndexMap<String, Schema> {
        entries
            .iter()
            .map(|(name, value)| {
                let schema = serde_json::from_value(value.clone()).expect("valid schema");
                ((*name).to_string(), schema)
            })
            .collect()
    }

    fn schema(value: Value) -> Schema {
        serde_json::from_value(value).expect("valid schema")
    }

    fn reference(name: &str) -> Value {
        json!({"$ref": format!("#/components/schemas/{name}")})
    }

    /// A -> B -> C, plus a self-referential Node and an unrelated Other.
    fn graph(c_type: &str) -> IndexMap<String, Schema> {
        schemas(&[
            (
                "A",
                json!({"type": "object", "properties": {"b": reference("B")}}),
            ),
            ("B", json!({"type": "array", "items": reference("C")})),
            ("C", json!({"type": c_type})),
            (
                "Node",
                json!({"type": "object", "properties": {"next": reference("Node")}}),
            ),
            ("Other", json!({"type": "string"})),
        ])
    }

    #[test]
    fn proof_proves_unchanged_closures_equal() {
        let (old, new) = (graph("string"), graph("string"));
        let pair = PairResolver::new(&old, &new);
        for root in [
            reference("A"),
            reference("Node"),
            json!({"type": "integer"}),
        ] {
            assert!(proven_equal(&pair, &schema(root.clone()), &schema(root)));
        }
        // A reference that resolves on neither side stays literal on both.
        let dangling = schema(reference("Missing"));
        assert!(proven_equal(&pair, &dangling, &dangling));
    }

    #[test]
    fn proof_rejects_a_transitively_changed_reference() {
        let (old, new) = (graph("string"), graph("integer"));
        let pair = PairResolver::new(&old, &new);
        assert!(!proven_equal(
            &pair,
            &schema(reference("A")),
            &schema(reference("A"))
        ));
        assert!(!proven_equal(
            &pair,
            &schema(reference("B")),
            &schema(reference("B"))
        ));
        // Components that cannot reach C are still provably equal.
        assert!(proven_equal(
            &pair,
            &schema(reference("Node")),
            &schema(reference("Node"))
        ));
        assert!(proven_equal(
            &pair,
            &schema(reference("Other")),
            &schema(reference("Other"))
        ));
        // And the proof only ever claims equality the resolver confirms.
        let resolved = |resolver: &DocResolver| resolver.resolve(&schema(reference("A")));
        assert_ne!(resolved(&pair.old), resolved(&pair.new));
    }

    #[test]
    fn proof_rejects_differing_roots_and_one_sided_components() {
        let old = graph("string");
        let mut new = graph("string");
        new.shift_remove("Other");
        let pair = PairResolver::new(&old, &new);
        assert!(!proven_equal(
            &pair,
            &schema(reference("Other")),
            &schema(reference("Other"))
        ));
        assert!(!proven_equal(
            &pair,
            &schema(json!({"type": "string"})),
            &schema(json!({"type": "integer"}))
        ));
    }

    #[test]
    fn proof_propagates_changes_around_cycles() {
        let cycle = |leaf: &str| {
            schemas(&[
                (
                    "P",
                    json!({"type": "object", "properties": {"q": reference("Q")}}),
                ),
                (
                    "Q",
                    json!({"type": "object", "properties": {"p": reference("P"), "leaf": reference("Leaf")}}),
                ),
                ("Leaf", json!({"type": leaf})),
            ])
        };
        let (old, new) = (cycle("string"), cycle("boolean"));
        let pair = PairResolver::new(&old, &new);
        assert!(!proven_equal(
            &pair,
            &schema(reference("P")),
            &schema(reference("P"))
        ));
        let unchanged = cycle("string");
        let pair = PairResolver::new(&old, &unchanged);
        assert!(proven_equal(
            &pair,
            &schema(reference("P")),
            &schema(reference("P"))
        ));
    }

    #[test]
    fn shallow_resolution_matches_full_resolution_above_the_cut() {
        let docs = graph("string");
        let resolver = DocResolver::new(&docs);
        let root = schema(json!({"type": "object", "properties": {"a": reference("A")}}));
        let full = resolver.resolve(&root);
        // Level 0 root, 1 `properties`, 2 the resolved A, 3 A's `properties`,
        // 4 the `b` property: objects from level 4 down stay as written.
        let shallow = resolver.resolve_shallow(&root, 4);
        assert_eq!(
            shallow["properties"]["a"]["type"],
            full["properties"]["a"]["type"]
        );
        assert_eq!(
            shallow["properties"]["a"]["properties"]["b"],
            reference("B")
        );
        assert_eq!(
            full["properties"]["a"]["properties"]["b"]["items"]["type"],
            "string"
        );
        assert_eq!(resolver.resolve_shallow(&root, usize::MAX), full);
    }

    #[test]
    fn bare_and_sibling_refs_resolve_as_merges() {
        let docs = graph("string");
        let resolver = DocResolver::new(&docs);
        let bare = resolver.resolve(&schema(reference("C")));
        assert_eq!(bare, json!({"type": "string"}));
        let mut with_sibling = reference("C");
        with_sibling["nullable"] = json!(true);
        let merged = resolver.resolve(&schema(with_sibling));
        assert_eq!(merged["type"], "string");
        assert_eq!(merged["nullable"], true);
    }

    #[test]
    fn depth_cap_still_applies_through_the_cache() {
        let entries: Vec<(String, Value)> = (0..=MAX_RESOLVE_DEPTH)
            .map(|index| (format!("L{index}"), reference(&format!("L{}", index + 1))))
            .collect();
        let borrowed: Vec<(&str, Value)> = entries
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        let docs = schemas(&borrowed);
        let resolver = DocResolver::new(&docs);
        let resolved = resolver.resolve(&schema(reference("L0")));
        assert_eq!(resolved, reference(&format!("L{MAX_RESOLVE_DEPTH}")));
    }
}
