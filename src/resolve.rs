//! `$ref` handling for `diff` (#152, #181).
//!
//! `diff` compares schemas with references as edges:
//! [`PairResolver::localize`] rewrites an old/new pair so that positions where
//! both sides reference a component become edges (compared once per pair by
//! `diff::components`) and a reference met by an inline object is expanded in
//! place, one hop at a time. Fully inlining every reference (the semantics
//! before #181, still available to tests as [`DocResolver::resolve`]) reported
//! a change under a cross-linked cycle once per reference path, which on
//! Stripe meant millions of rows.
//!
//! - each component `Schema` is converted to a `Value` once per document;
//! - [`PairResolver::values_equal`] proves, without resolving anything, that
//!   two schemas resolve to identical values, so unchanged operations skip
//!   comparison entirely, and [`PairResolver::is_stable`] does the same for a
//!   component paired with itself;
//! - [`DocResolver::resolve_shallow`] resolves only the top few JSON levels for
//!   readers (pagination inference) that never look deeper.
//!
//! Expansions are deliberately not memoized: the result is an owned tree, so a
//! cache hit still clones the whole subtree, and on OpenAI's spec that made the
//! diff slower and ~40% larger in memory.

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
    pub(crate) schemas: &'a IndexMap<String, Schema>,
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
    #[cfg(test)]
    pub(crate) fn resolve(&self, schema: &Schema) -> Value {
        self.resolve_value(&schema_json(schema))
    }

    /// [`Self::resolve`] over an already serialized schema.
    #[cfg(test)]
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
    #[cfg(test)]
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

// ── refs as edges (#181) ────────────────────────────────────────────────────

/// A `$ref` on the old side and one on the new side at the same position,
/// each to a component its own document declares: `diff` compares each pair
/// of target names once, instead of inlining both targets at every position
/// that reaches them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RefEdge {
    /// Old and new component names: the pair's identity.
    pub(crate) names: (String, String),
    /// The top-level key of the walked schema under which the reference
    /// sits, or `None` when the walked schema itself is the reference (an
    /// alias). A mask on the way into that schema hides edges under a masked
    /// key.
    pub(crate) at: Option<String>,
    /// The overlay: keys other than annotations that sit beside `$ref` on
    /// either side, sorted. Resolution lays them over the target, so they are
    /// compared at the reference with their effective values, and a change
    /// in the target under one of them is invisible through this edge.
    /// Empty almost always.
    pub(crate) mask: Vec<String>,
    /// Whether the reference sits at or beneath a `not` keyword of the
    /// walked schema. `not` reverses every compatibility direction, so a
    /// change reached through it is Review, as one written under it is.
    pub(crate) negated: bool,
}

/// Keys whose values canonicalisation drops from a schema object; refs under
/// them are left as written.
const ANNOTATIONS: [&str; 5] = ["description", "title", "example", "examples", "deprecated"];

/// The schema-grammar role of a node while [`PairResolver::localize`] walks
/// it: the grammar `diff_values` and the severity table read (`value::Node`),
/// plus annotations.
///
/// Only a schema position treats a `$ref` object as a reference (an edge, or
/// a one-sided expansion). Outside the grammar (`default`, `const`, `enum`
/// and `required` members, `if`/`then`/`else`, `prefixItems`, tuple `items`,
/// `$defs`, `dependentSchemas`, unknown keywords, ...) a reference is not a
/// schema use the table can rate (#186): each side is inlined there as
/// [`DocResolver::resolve`] would, and compared as plain JSON at the use (or
/// the component) that holds it, where every change is Review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// A schema object (`Node::Schema`).
    Schema,
    /// A `properties`-like map or an `allOf`-like list: children are schemas
    /// (`Node::PropertyMap`, `Node::SchemaList`); the container itself is
    /// never a reference.
    Container,
    /// Outside the grammar (`Node::Opaque`, `Node::Set`): inlined.
    Value,
    /// An annotation of a schema object: left as written (canonicalisation
    /// drops it).
    Annotation,
}

impl Role {
    fn child(self, key: &str) -> Role {
        match self {
            Role::Schema => match key {
                "properties" | "patternProperties" | "allOf" | "oneOf" | "anyOf" => Role::Container,
                "items" | "additionalProperties" | "not" => Role::Schema,
                _ if key.starts_with("x-") || ANNOTATIONS.contains(&key) => Role::Annotation,
                _ => Role::Value,
            },
            Role::Container => Role::Schema,
            Role::Value | Role::Annotation => self,
        }
    }
}

/// The component a `{"$ref": ...}` object names, when the reference has one of
/// the two component forms.
fn component_ref(map: &Map<String, Value>) -> Option<String> {
    match map.get("$ref") {
        Some(Value::String(reference)) => reference_name(reference),
        _ => None,
    }
}

/// The keys beside `$ref` that change the merged schema (anything but
/// annotations): [`DocResolver::resolve`] lays them over the target.
fn overlay_keys(map: &Map<String, Value>) -> impl Iterator<Item = &String> {
    map.keys().filter(|key| {
        *key != "$ref" && !key.starts_with("x-") && !ANNOTATIONS.contains(&key.as_str())
    })
}

/// The mutable state of one [`PairResolver::localize`] walk.
struct LocalWalk<'w> {
    /// Component names expanded in place on the current chain, per side
    /// (names, so differently spelled references to one component are one).
    old_stack: Vec<String>,
    new_stack: Vec<String>,
    /// JSON depth below the walked schema, and the top-level key the walk is
    /// under (see [`RefEdge::at`]).
    depth: usize,
    top: Option<String>,
    /// Whether the walk is at or beneath a schema `not` keyword.
    negated: bool,
    edges: &'w mut Vec<RefEdge>,
}

impl DocResolver<'_> {
    /// The serialized component `name` (declared: edges only ever name
    /// declared components), or `Null`.
    pub(crate) fn component_value(&self, name: &str) -> Value {
        self.component(name)
            .map(|value| value.as_ref().clone())
            .unwrap_or(Value::Null)
    }

    /// The value resolution gives `key` at the top of component `name`: its
    /// own key, else, while the component is a `$ref` to another declared
    /// component (whose siblings win, as in [`Self::resolve`]), that
    /// component's, hop by hop. `None` when no hop has it, or at a cycle,
    /// an undeclared target or [`MAX_RESOLVE_DEPTH`] hops, where resolution
    /// leaves the reference literal.
    fn resolved_key(&self, name: &str, key: &str) -> Option<Value> {
        let mut seen = vec![name.to_string()];
        let mut current = self.component(name)?;
        loop {
            if let Some(value) = current.get(key) {
                return Some(value.clone());
            }
            let next = component_ref(current.as_object()?)?;
            if !self.declares(&next) || seen.contains(&next) || seen.len() > MAX_RESOLVE_DEPTH {
                return None;
            }
            seen.push(next.clone());
            current = self.component(&next)?;
        }
    }

    /// Whether this document declares component `name`.
    fn declares(&self, name: &str) -> bool {
        self.schemas.contains_key(name)
    }

    /// The `$ref` object `map` with its target laid under it, exactly as
    /// [`Self::resolve`] expands one hop (siblings win), or `None` when it is
    /// unresolvable, its component already on `stack`, or past
    /// [`MAX_RESOLVE_DEPTH`]. On `Some`, the component name is pushed onto
    /// `stack` for the caller to pop.
    fn expand_once(&self, map: &Map<String, Value>, stack: &mut Vec<String>) -> Option<Value> {
        let Some(Value::String(reference)) = map.get("$ref") else {
            return None;
        };
        let name = reference_name(reference)?;
        if stack.len() >= MAX_RESOLVE_DEPTH || stack.contains(&name) {
            return None;
        }
        let target = self.target(reference)?;
        stack.push(name);
        Some(match target.as_ref() {
            target @ Value::Object(_) if map.len() == 1 => target.clone(),
            target => {
                let mut merged = match target {
                    Value::Object(target_map) => target_map.clone(),
                    _ => Map::new(),
                };
                for (key, sibling) in map {
                    if key != "$ref" {
                        merged.insert(key.clone(), sibling.clone());
                    }
                }
                Value::Object(merged)
            }
        })
    }
}

impl DocResolver<'_> {
    /// `value` with every component reference replaced by its target (siblings
    /// merged over it), recursively, as [`Self::resolve`] expands them; a
    /// component already on `stack` (by name), or past [`MAX_RESOLVE_DEPTH`]
    /// of them, stays the object as written. What [`PairResolver::localize`]
    /// compares outside the schema grammar (#186).
    fn inline(&self, value: &Value, stack: &mut Vec<String>) -> Value {
        match value {
            Value::Object(map) => {
                if component_ref(map).is_some_and(|name| self.declares(&name)) {
                    match self.expand_once(map, stack) {
                        Some(expanded) => {
                            let out = self.inline(&expanded, stack);
                            stack.pop();
                            out
                        }
                        None => value.clone(),
                    }
                } else {
                    map.iter()
                        .map(|(key, child)| (key.clone(), self.inline(child, stack)))
                        .collect::<Map<String, Value>>()
                        .into()
                }
            }
            Value::Array(items) => items
                .iter()
                .map(|item| self.inline(item, stack))
                .collect::<Vec<Value>>()
                .into(),
            other => other.clone(),
        }
    }
}

impl PairResolver<'_> {
    /// Whether component `name` provably resolves identically in both
    /// documents (it is declared, unchanged, and reaches only such
    /// components). A `(name, name)` edge to it can produce no change.
    pub(crate) fn is_stable(&self, name: &str) -> bool {
        !self.unstable.contains(name)
    }

    /// Rewrites two serialized schemas (`old` from the old document, `new`
    /// from the new one) for a comparison that treats references as edges.
    ///
    /// Walking both in parallel, wherever both sides hold a component `$ref`
    /// whose target its own document declares, the pair of target names is
    /// appended to `edges` and `$ref` is dropped from both sides; the caller
    /// compares the targets once per pair. Siblings other than annotations
    /// override the target in resolution, so they form an overlay: each such
    /// key (from either side) is compared here with its effective value (the
    /// sibling, else the target's own) and recorded as the edge's mask (see
    /// [`RefEdge`]); annotation siblings are compared as written. Where only
    /// one side holds a component `$ref` and the other is a plain object,
    /// that reference is expanded in place (as [`DocResolver::resolve`]
    /// would) and the walk continues into it, so a schema moving between
    /// inline and referenced form is compared field by field; the inline side
    /// shrinks at every step, so this terminates. Only schema positions of the
    /// grammar the severity table reads hold references; outside it (see
    /// [`Role`]) both sides are inlined in full and no edge is recorded (#186).
    /// Everything else, including subtrees present on one side only and
    /// references under annotation keywords, is kept as written. Key order is
    /// preserved, and a value without component refs is returned unchanged.
    pub(crate) fn localize(
        &self,
        old: &Value,
        new: &Value,
        edges: &mut Vec<RefEdge>,
    ) -> (Value, Value) {
        let mut walk = LocalWalk {
            old_stack: Vec::new(),
            new_stack: Vec::new(),
            depth: 0,
            top: None,
            negated: false,
            edges,
        };
        self.local(old, new, Role::Schema, &mut walk)
    }

    fn local(&self, old: &Value, new: &Value, role: Role, walk: &mut LocalWalk) -> (Value, Value) {
        match role {
            Role::Annotation => return (old.clone(), new.clone()),
            Role::Value => {
                // Equal values over unchanged components inline identically.
                if self.values_equal(old, new) {
                    return (old.clone(), new.clone());
                }
                return (
                    self.old.inline(old, &mut Vec::new()),
                    self.new.inline(new, &mut Vec::new()),
                );
            }
            Role::Schema | Role::Container => {}
        }
        match (old, new) {
            (Value::Object(before), Value::Object(after)) if role == Role::Container => {
                self.local_maps(before, after, role, walk)
            }
            (Value::Object(before), Value::Object(after)) => {
                // Only a reference whose own document declares its target is
                // a component reference; an undeclared one stays as written,
                // exactly as resolution leaves it.
                let old_ref = component_ref(before).filter(|name| self.old.declares(name));
                let new_ref = component_ref(after).filter(|name| self.new.declares(name));
                match (old_ref, new_ref) {
                    (Some(a), Some(b)) => {
                        // The overlay: keys beside `$ref` that override the
                        // target are compared here with their effective
                        // values (the sibling's, else the target's), and the
                        // edge masks them; annotation siblings stay as they
                        // are; `$ref` itself is the edge.
                        let mut mask: Vec<String> = overlay_keys(before)
                            .chain(overlay_keys(after))
                            .cloned()
                            .collect();
                        mask.sort();
                        mask.dedup();
                        let effective =
                            |map: &Map<String, Value>, doc: &DocResolver, target: &str| {
                                let mut out: Map<String, Value> = map
                                    .iter()
                                    .filter(|(key, _)| *key != "$ref")
                                    .map(|(key, value)| (key.clone(), value.clone()))
                                    .collect();
                                for key in &mask {
                                    if !out.contains_key(key)
                                        && let Some(value) = doc.resolved_key(target, key)
                                    {
                                        out.insert(key.clone(), value);
                                    }
                                }
                                out
                            };
                        let old_effective = effective(before, &self.old, &a);
                        let new_effective = effective(after, &self.new, &b);
                        let at = (walk.depth > 0).then(|| walk.top.clone()).flatten();
                        walk.edges.push(RefEdge {
                            names: (a, b),
                            at,
                            mask,
                            negated: walk.negated,
                        });
                        self.local_maps(&old_effective, &new_effective, role, walk)
                    }
                    (Some(_), None) => match self.old.expand_once(before, &mut walk.old_stack) {
                        Some(expanded) => {
                            let out = self.local(&expanded, new, role, walk);
                            walk.old_stack.pop();
                            out
                        }
                        None => self.local_maps(before, after, role, walk),
                    },
                    (None, Some(_)) => match self.new.expand_once(after, &mut walk.new_stack) {
                        Some(expanded) => {
                            let out = self.local(old, &expanded, role, walk);
                            walk.new_stack.pop();
                            out
                        }
                        None => self.local_maps(before, after, role, walk),
                    },
                    _ => self.local_maps(before, after, role, walk),
                }
            }
            (Value::Array(before), Value::Array(after)) => {
                let common = before.len().min(after.len());
                let child = role.child("");
                let (mut left, mut right) = (Vec::new(), Vec::new());
                walk.depth += 1;
                for index in 0..common {
                    let (a, b) = self.local(&before[index], &after[index], child, walk);
                    left.push(a);
                    right.push(b);
                }
                walk.depth -= 1;
                left.extend(before[common..].iter().cloned());
                right.extend(after[common..].iter().cloned());
                (Value::Array(left), Value::Array(right))
            }
            _ => (old.clone(), new.clone()),
        }
    }

    fn local_maps(
        &self,
        before: &Map<String, Value>,
        after: &Map<String, Value>,
        role: Role,
        walk: &mut LocalWalk,
    ) -> (Value, Value) {
        let mut left = Map::new();
        let mut shared: HashMap<&str, Value> = HashMap::new();
        let outer_top = walk.top.clone();
        for (key, old_child) in before {
            match after.get(key) {
                Some(new_child) => {
                    if walk.depth == 0 {
                        walk.top = Some(key.clone());
                    }
                    // The same grammar `under_not` walks: a `not` keyword of
                    // a schema object (never a property named `not`).
                    let outer_negated = walk.negated;
                    walk.negated |= role == Role::Schema && key == "not";
                    walk.depth += 1;
                    let (a, b) = self.local(old_child, new_child, role.child(key), walk);
                    walk.depth -= 1;
                    walk.negated = outer_negated;
                    left.insert(key.clone(), a);
                    shared.insert(key.as_str(), b);
                }
                None => {
                    left.insert(key.clone(), old_child.clone());
                }
            }
        }
        walk.top = outer_top;
        let right = after
            .iter()
            .map(|(key, new_child)| {
                let value = shared
                    .remove(key.as_str())
                    .unwrap_or_else(|| new_child.clone());
                (key.clone(), value)
            })
            .collect();
        (Value::Object(left), Value::Object(right))
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

    // ── localize (#181) ──────────────────────────────────────────────────

    fn localized(
        pair: &PairResolver,
        old: Value,
        new: Value,
    ) -> (Value, Value, Vec<(String, String)>) {
        let mut edges = Vec::new();
        let (old, new) = pair.localize(&old, &new, &mut edges);
        (old, new, edges.into_iter().map(|edge| edge.names).collect())
    }

    #[test]
    fn localize_leaves_ref_free_values_untouched() {
        let (old, new) = (graph("string"), graph("integer"));
        let pair = PairResolver::new(&old, &new);
        let before = json!({"type": "object", "properties": {"z": {"type": "string"}, "a": {"enum": [2, 1]}}});
        let after = json!({"properties": {"a": {"enum": [1]}, "y": {}}, "type": "array"});
        let (left, right, edges) = localized(&pair, before.clone(), after.clone());
        assert!(edges.is_empty());
        // Byte for byte, key order included.
        assert_eq!(left.to_string(), before.to_string());
        assert_eq!(right.to_string(), after.to_string());
    }

    #[test]
    fn localize_turns_ref_pairs_into_edges_and_keeps_siblings() {
        let (old, new) = (graph("string"), graph("integer"));
        let pair = PairResolver::new(&old, &new);
        let mut sibling = reference("B");
        sibling["description"] = json!("annotations do not override the target");
        let (left, right, edges) = localized(
            &pair,
            json!({"properties": {"a": reference("A"), "b": sibling, "o": reference("Other")}}),
            json!({"properties": {"a": reference("A"), "b": reference("B"), "o": reference("Node")}}),
        );
        assert_eq!(
            edges,
            vec![
                ("A".to_string(), "A".to_string()),
                ("B".to_string(), "B".to_string()),
                ("Other".to_string(), "Node".to_string()),
            ]
        );
        assert_eq!(
            left,
            json!({"properties": {"a": {}, "b": {"description": "annotations do not override the target"}, "o": {}}})
        );
        assert_eq!(right, json!({"properties": {"a": {}, "b": {}, "o": {}}}));
        // A changed component is never expanded at the use site.
        assert!(!pair.is_stable("A") && !pair.is_stable("C") && pair.is_stable("Other"));
    }

    #[test]
    fn localize_expands_one_sided_refs_against_inline_schemas() {
        let (old, new) = (graph("string"), graph("string"));
        let pair = PairResolver::new(&old, &new);
        // Old references A; new inlines A's shape with a changed `b`: A is
        // expanded in place, and its own `b` reference meets the inline
        // object, so B is expanded too, down to the C edge.
        let (left, right, edges) = localized(
            &pair,
            reference("A"),
            json!({"type": "object", "properties": {"b": {"type": "array", "items": reference("C"), "minItems": 1}}}),
        );
        assert_eq!(edges, vec![("C".to_string(), "C".to_string())]);
        assert_eq!(
            left,
            json!({"type": "object", "properties": {"b": {"type": "array", "items": {}}}})
        );
        assert_eq!(
            right,
            json!({"type": "object", "properties": {"b": {"type": "array", "items": {}, "minItems": 1}}})
        );
        // A reference with nothing to compare against stays as written.
        let (left, right, edges) = localized(&pair, reference("A"), json!(true));
        assert!(edges.is_empty());
        assert_eq!((left, right), (reference("A"), json!(true)));
    }

    #[test]
    fn localize_overlays_siblings_that_override_the_target() {
        let (old, new) = (graph("string"), graph("integer"));
        let pair = PairResolver::new(&old, &new);
        // `C` changed type, but this use pins `type` itself: the overlay
        // compares the effective `type` here (equal) and masks it on the edge.
        let mut pinned = reference("C");
        pinned["type"] = json!("string");
        let mut edges = Vec::new();
        let (left, right) = pair.localize(&pinned, &pinned, &mut edges);
        assert_eq!(
            (left, right),
            (json!({"type": "string"}), json!({"type": "string"}))
        );
        assert_eq!(
            edges,
            vec![super::RefEdge {
                names: ("C".into(), "C".into()),
                at: None,
                mask: vec!["type".into()],
                negated: false,
            }]
        );
        // A sibling on one side only compares against the other side's
        // target value of that key (here: absent).
        let mut nullable = reference("A");
        nullable["nullable"] = json!(true);
        let wrapped = |value: Value| json!({"properties": {"a": value}});
        let mut edges = Vec::new();
        let (left, right) = pair.localize(&wrapped(reference("A")), &wrapped(nullable), &mut edges);
        assert_eq!(left, json!({"properties": {"a": {}}}));
        assert_eq!(right, json!({"properties": {"a": {"nullable": true}}}));
        assert_eq!(edges[0].at.as_deref(), Some("properties"));
        assert_eq!(edges[0].mask, vec!["nullable".to_string()]);
    }

    #[test]
    fn localize_stops_on_cycles_and_keeps_dangling_refs_literal() {
        let docs = schemas(&[("Loop", reference("Loop"))]);
        let pair = PairResolver::new(&docs, &docs);
        // Expanding Loop yields Loop again: kept literal once on the stack.
        let (left, right, edges) = localized(&pair, reference("Loop"), json!({"type": "object"}));
        assert!(edges.is_empty());
        assert_eq!(left, reference("Loop"));
        assert_eq!(right, json!({"type": "object"}));
        // Neither target exists: compared as written, so a retarget shows.
        let (left, right, edges) = localized(&pair, reference("Gone"), reference("Missing"));
        assert!(edges.is_empty());
        assert_eq!((left, right), (reference("Gone"), reference("Missing")));
    }

    #[test]
    fn localize_leaves_refs_under_annotations_alone() {
        let (old, new) = (graph("string"), graph("integer"));
        let pair = PairResolver::new(&old, &new);
        let value =
            json!({"type": "object", "example": reference("A"), "x-sample": reference("A")});
        let (left, right, edges) = localized(&pair, value.clone(), value.clone());
        assert!(edges.is_empty());
        assert_eq!((left, right), (value.clone(), value));
    }

    /// #186: outside the schema grammar a reference is not an edge; each side
    /// is inlined in full (cycles cut by name), as resolution would.
    #[test]
    fn localize_inlines_refs_outside_the_schema_grammar() {
        let (old, new) = (graph("string"), graph("integer"));
        let pair = PairResolver::new(&old, &new);
        let value = json!({
            "type": "object",
            "properties": {"a": reference("A")},
            "default": reference("C"),
            "if": reference("B"),
            "prefixItems": [reference("C")],
            "$defs": {"d": {"not": reference("C")}},
            "enum": [reference("C")],
            "const": reference("Node"),
        });
        let (left, right, edges) = localized(&pair, value.clone(), value);
        // Only the `properties` use is an edge.
        assert_eq!(edges, [("A".to_string(), "A".to_string())]);
        for (side, ty) in [(&left, "string"), (&right, "integer")] {
            assert_eq!(side["properties"]["a"], json!({}));
            assert_eq!(side["default"], json!({"type": ty}));
            assert_eq!(side["if"], json!({"type": "array", "items": {"type": ty}}));
            assert_eq!(side["prefixItems"], json!([{"type": ty}]));
            assert_eq!(side["$defs"]["d"]["not"], json!({"type": ty}));
            assert_eq!(side["enum"], json!([{"type": ty}]));
            // `Node` is unchanged on both sides: equal, so left as written.
            assert_eq!(side["const"], reference("Node"));
        }
        // Equal values over unchanged components are left as written.
        let same = json!({"default": reference("Other")});
        let (left, right, edges) = localized(&pair, same.clone(), same.clone());
        assert!(edges.is_empty());
        assert_eq!((left, right), (same.clone(), same));
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
