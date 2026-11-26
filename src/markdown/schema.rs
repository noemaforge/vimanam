//! Renders request/response schemas as nested field tables at `--detail full
//! --include-schemas`.
//!
//! By default each component schema reached through a `$ref` is expanded once
//! into a trailing "Schema Definitions" section and linked from every use site,
//! so a schema shared across many endpoints (or referenced many times within
//! one) is not re-inlined at each occurrence (issue #58). `--inline-schemas`
//! restores the fully self-contained behaviour, expanding every `$ref` inline at
//! each use site (with per-chain cycle detection).

use std::collections::HashSet;
use std::fmt;
use std::io::Write;

use anyhow::Result;
use indexmap::IndexMap;

use crate::models::{
    AdditionalProperties, ApiDocumentation, DetailLevel, DocConfig, Endpoint, OperationSelector,
    Response, Schema,
};
use crate::utils::{clean_for_id, decode_json_pointer_token, resolve_schema_reference};

/// Root-row label of an operation's request body table. Body roots are
/// synthetic: their child rows carry bare JSON property paths (`name`,
/// `category.id`) rather than a `request.` prefix an agent could mistake for a
/// real body field (#156). The parenthesised label cannot collide with a
/// property, argument or component name.
pub(super) const REQUEST_BODY_LABEL: &str = "(request body)";
/// Root-row label of an operation's success response table (see
/// [`REQUEST_BODY_LABEL`]).
pub(super) const RESPONSE_BODY_LABEL: &str = "(response body)";

fn is_body_label(label: &str) -> bool {
    label == REQUEST_BODY_LABEL || label == RESPONSE_BODY_LABEL
}

/// `--schema-depth` applied to a selected slice (standalone `--schema` /
/// `--schema-field`, or `--operation`/`--operation-id` with `--detail full
/// --include-schemas`) when none is given and its component graph is large
/// (#160). Measured on pinned Stripe/GitHub specs, depth 2 keeps a slice in the
/// low tens of KB where unbounded expansion reached 1.2-1.4 MB.
pub(crate) const AUTO_SCHEMA_DEPTH: usize = 2;
/// A selection whose roots reach more distinct component schemas than this
/// (transitively, through `$ref`) is bounded at [`AUTO_SCHEMA_DEPTH`] unless
/// `--schema-depth` is passed. Graphs split sharply in practice: the Stripe
/// and GitHub GraphQL slices behind #160 reach 830-920 components (1.2-1.4 MB
/// unbounded), while the largest operations of a 3 MB protobuf-generated spec
/// reach at most ~150 (under 120 KB). Graphs at or below the threshold render
/// exactly as before.
pub(crate) const AUTO_SCHEMA_DEPTH_THRESHOLD: usize = 200;

/// Distinct component schemas transitively reachable from `roots` through
/// `$ref`s (properties, items, additionalProperties and compositions), counted
/// up to `cap + 1` so a huge graph stops early. Deterministic: a set size.
pub(crate) fn reachable_component_count<'a>(
    doc: &'a ApiDocumentation,
    roots: impl IntoIterator<Item = &'a Schema>,
    cap: usize,
) -> usize {
    let mut seen: HashSet<&'a str> = HashSet::new();
    let mut stack: Vec<&'a Schema> = roots.into_iter().collect();
    while let Some(schema) = stack.pop() {
        if let Some(reference) = schema.reference.as_deref() {
            if seen.len() <= cap
                && seen.insert(reference)
                && let Some(resolved) = resolve_schema_reference(reference, doc)
            {
                stack.push(resolved);
            }
            continue;
        }
        if seen.len() > cap {
            break;
        }
        if let Some(properties) = &schema.properties {
            stack.extend(properties.values());
        }
        if let Some(items) = schema.items.as_deref() {
            stack.push(items);
        }
        if let Some(AdditionalProperties::Schema(child)) = &schema.additional_properties {
            stack.push(child);
        }
        for variants in [&schema.all_of, &schema.one_of, &schema.any_of]
            .into_iter()
            .flatten()
        {
            stack.extend(variants);
        }
    }
    seen.len().min(cap + 1)
}

/// Schema rows a render may expand under `--inline-schemas` before schema
/// expansion is bounded at [`AUTO_SCHEMA_DEPTH`] (#177). Inline expansion
/// re-expands every acyclic path through the component graph at each use
/// site, so a densely cross-linked graph (Stripe's) grows without practical
/// bound even with cycle detection. Real specs that finish stay far below this
/// (a 10 MB GitHub REST spec renders ~63 thousand rows, 5.6 MB); two million
/// rows is a few hundred MB of Markdown. Renders at or below the budget are
/// unchanged.
pub(crate) const INLINE_ROW_BUDGET: usize = 2_000_000;

/// Rows an `--inline-schemas` table renders for `roots` without
/// `--schema-depth`, counted up to `cap + 1` so an exponential expansion stops
/// early. Mirrors [`collect_schema_rows`] exactly in that mode: a resolvable
/// `$ref` adds no row of its own and expands its target one level deeper, a
/// reference already on the chain or an unresolvable one is one row, every
/// other schema is one row plus its properties, items and composition
/// variants (and `additionalProperties` when `selected`, as a schema
/// selection renders it), and depth [`SCHEMA_DEPTH_LIMIT`] is one row.
pub(crate) fn inline_row_count<'a>(
    doc: &'a ApiDocumentation,
    roots: impl IntoIterator<Item = &'a Schema>,
    selected: bool,
    cap: usize,
) -> usize {
    let mut counter = InlineRowCounter {
        doc,
        selected,
        cap,
        rows: 0,
        chain: Vec::new(),
        resolved: std::collections::HashMap::new(),
    };
    for root in roots {
        if counter.rows > cap {
            break;
        }
        counter.walk(root, 0);
    }
    counter.rows.min(cap + 1)
}

struct InlineRowCounter<'a> {
    doc: &'a ApiDocumentation,
    selected: bool,
    cap: usize,
    rows: usize,
    /// References being expanded on the current path (`ref_stack`).
    chain: Vec<&'a str>,
    /// Resolution cache: the walk revisits the same references many times.
    resolved: std::collections::HashMap<&'a str, Option<&'a Schema>>,
}

impl<'a> InlineRowCounter<'a> {
    fn walk(&mut self, schema: &'a Schema, depth: usize) {
        if self.rows > self.cap {
            return;
        }
        if depth >= SCHEMA_DEPTH_LIMIT {
            self.rows += 1;
            return;
        }
        if let Some(reference) = schema.reference.as_deref() {
            let doc = self.doc;
            let target = *self
                .resolved
                .entry(reference)
                .or_insert_with(|| resolve_schema_reference(reference, doc));
            match target {
                Some(resolved) if !self.chain.contains(&reference) => {
                    self.chain.push(reference);
                    self.walk(resolved, depth + 1);
                    self.chain.pop();
                }
                _ => self.rows += 1,
            }
            return;
        }
        self.rows += 1;
        if let Some(properties) = &schema.properties {
            for child in properties.values() {
                self.walk(child, depth + 1);
            }
        }
        if let Some(items) = schema.items.as_deref() {
            self.walk(items, depth + 1);
        }
        if self.selected
            && let Some(AdditionalProperties::Schema(child)) = &schema.additional_properties
        {
            self.walk(child, depth + 1);
        }
        for variants in [&schema.all_of, &schema.one_of, &schema.any_of]
            .into_iter()
            .flatten()
        {
            for variant in variants {
                self.walk(variant, depth + 1);
            }
        }
    }
}

/// One depth-limited row whose nested expansion was omitted.
#[derive(Debug, Clone)]
pub(super) struct Omission {
    depth: usize,
    field: String,
    command: String,
    /// `METHOD path` when the row belongs to an operation body, whose field
    /// paths are relative to that body and so only distinct per operation.
    operation: Option<String>,
}

/// Collapses omission notices into one stderr line per retrieval command, in
/// first-seen order (#161). A single field keeps the original one-line form;
/// several fields sharing a command report their distinct count (a field is
/// identified by its operation and path), depth range, the first few field
/// paths, and how many operations contributed when more than one did.
pub(super) fn summarize_omissions(omissions: Vec<Omission>) -> Vec<String> {
    const EXAMPLES: usize = 3;
    let mut groups: IndexMap<String, Vec<Omission>> = IndexMap::new();
    for omission in omissions {
        groups
            .entry(omission.command.clone())
            .or_default()
            .push(omission);
    }
    groups
        .into_iter()
        .map(|(command, group)| {
            let low = group.iter().map(|o| o.depth).min().unwrap_or(0);
            let high = group.iter().map(|o| o.depth).max().unwrap_or(0);
            let depth = if low == high {
                format!("depth {low}")
            } else {
                format!("depths {low}-{high}")
            };
            // Distinct fields (operation + path); the same row rendered on
            // several split pages or cut at two depths counts once.
            let mut seen = HashSet::new();
            let fields: Vec<&Omission> = group
                .iter()
                .filter(|o| seen.insert((o.operation.as_deref(), o.field.as_str())))
                .collect();
            let operations: HashSet<&str> =
                group.iter().filter_map(|o| o.operation.as_deref()).collect();
            if let [single] = fields.as_slice() {
                return format!(
                    "vimanam: omitted nested schema expansion at {depth} for {}; retrieve full detail: {command}",
                    single.field
                );
            }
            let mut names = HashSet::new();
            let distinct_names: Vec<&str> = fields
                .iter()
                .map(|o| o.field.as_str())
                .filter(|field| names.insert(*field))
                .collect();
            let mut examples = distinct_names
                .iter()
                .take(EXAMPLES)
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
            if distinct_names.len() > EXAMPLES {
                examples.push_str(", ...");
            }
            let across = if operations.len() > 1 {
                format!(" across {} operations", operations.len())
            } else {
                String::new()
            };
            format!(
                "vimanam: omitted nested schema expansion at {depth} for {} fields{across} ({examples}); retrieve full detail: {command}",
                fields.len()
            )
        })
        .collect()
}

#[derive(Debug)]
struct SchemaRow {
    field: String,
    type_name: String,
    required: String,
    description: String,
}

/// Observation of one component reference's use sites in a rendered document.
/// Pure observation: recording never changes output bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SchemaUse {
    /// Rendered rows that link (or, in `--inline-schemas` mode, expand) the
    /// referenced schema.
    pub count: usize,
    /// Smallest depth such a row occurred at. Without `--schema-depth` the
    /// Schema Definitions entry always renders at depth 0; with it, entries
    /// render one edge below the shallowest discovered use (see
    /// [`SchemaContext::discover`]).
    pub min_depth: usize,
}

/// Document-level state for schema rendering.
///
/// In the default (linked) mode, each component schema reached through a `$ref`
/// is rendered once in the trailing "Schema Definitions" section and linked from
/// every use site. The context records which references have been seen and the
/// stable anchor assigned to each, in first-encounter order.
pub(super) struct SchemaContext<'a> {
    doc: &'a ApiDocumentation,
    /// When true, expand every `$ref` inline instead of linking (the fully
    /// self-contained mode).
    inline: bool,
    /// Reference (e.g. `#/components/schemas/Pet`) -> anchor, in first-seen order.
    anchors: IndexMap<String, String>,
    /// Anchors already handed out, so colliding name slugs get a unique suffix.
    used_anchors: HashSet<String>,
    /// Split pages link definitions to separate files, never local anchors.
    external: bool,
    max_depth: Option<usize>,
    depths: IndexMap<String, usize>,
    /// Refs that will be emitted in this build (pre-discovered for split/Skill).
    /// Cutoff rows may link only to these or to refs already in `anchors`.
    emitted_depths: IndexMap<String, usize>,
    source_path: String,
    proto_paths: Vec<String>,
    current_schema: Option<String>,
    /// Active `--schema-field` selector for the subtree being rendered, if any.
    active_selector: Option<String>,
    /// Endpoint identity (`METHOD path`) while rendering an operation body.
    current_operation: Option<String>,
    schema_names: Vec<String>,
    schema_fields: Vec<String>,
    operation_selector: Option<OperationSelector>,
    service_filter: Option<Vec<String>>,
    path_filter: Option<String>,
    method_filter: Option<Vec<String>>,
    exclude_deprecated: bool,
    selected: bool,
    protected_depth: usize,
    /// Omission notices for the render that owns this context; callers print
    /// them only when that render's buffer is actually written.
    omissions: Vec<Omission>,
    /// Display label of the synthetic body root being rendered, if any, used
    /// for its own (empty-path) row in omission notices.
    body_label: Option<String>,
    /// Depth at which the root currently being rendered (an operation body,
    /// selection, definition, split page or inlined `$ref` target) starts, so
    /// retrieval hints can size `--schema-depth` relative to that root.
    root_depth: usize,
    /// Use-site observations per reference, in first-encounter order. Fed to
    /// the `--costs` analysis through the render result.
    uses: IndexMap<String, SchemaUse>,
}

impl<'a> SchemaContext<'a> {
    pub(super) fn new(doc: &'a ApiDocumentation, inline: bool) -> Self {
        Self {
            doc,
            inline,
            anchors: IndexMap::new(),
            used_anchors: HashSet::new(),
            external: false,
            max_depth: None,
            depths: IndexMap::new(),
            emitted_depths: IndexMap::new(),
            source_path: "spec.json".to_string(),
            proto_paths: Vec::new(),
            current_schema: None,
            active_selector: None,
            current_operation: None,
            schema_names: Vec::new(),
            schema_fields: Vec::new(),
            operation_selector: None,
            service_filter: None,
            path_filter: None,
            method_filter: None,
            exclude_deprecated: false,
            selected: false,
            protected_depth: 0,
            omissions: Vec::new(),
            body_label: None,
            root_depth: 0,
            uses: IndexMap::new(),
        }
    }

    pub(super) fn configured(
        doc: &'a ApiDocumentation,
        config: &DocConfig,
        external: bool,
    ) -> Self {
        let mut ctx = Self::new(doc, config.inline_schemas);
        ctx.external = external;
        ctx.max_depth = config.schema_depth;
        ctx.source_path = config
            .source_path
            .clone()
            .unwrap_or_else(|| "spec.json".into());
        ctx.proto_paths = config.proto_paths.clone();
        ctx.schema_names = config.schema_names.clone();
        ctx.schema_fields = config.schema_fields.clone();
        ctx.operation_selector = config.operation_selector.clone();
        ctx.service_filter = config.service_filter.clone();
        ctx.path_filter = config.path_filter.clone();
        ctx.method_filter = config.method_filter.clone();
        ctx.exclude_deprecated = config.exclude_deprecated;
        ctx.selected = !config.schema_names.is_empty() || !config.schema_fields.is_empty();
        ctx
    }

    pub(super) fn protect_selection_path(&mut self, depth: usize) {
        self.protected_depth = depth;
    }

    pub(super) fn set_current_schema(&mut self, name: &str) {
        self.current_schema = Some(name.into());
    }

    pub(super) fn set_active_selector(&mut self, selector: Option<String>) {
        self.active_selector = selector;
    }

    pub(super) fn set_current_operation(&mut self, operation: Option<String>) {
        self.current_operation = operation;
    }

    /// Pre-discover references from a root so split/Skill pages know which
    /// schemas will actually be emitted before cutoff rows are written.
    pub(super) fn discover_root(&mut self, schema: &Schema, depth: usize) {
        self.discover(schema, depth);
    }

    /// Seed the set of schemas that will be written elsewhere in this build.
    pub(super) fn set_emitted_depths(&mut self, depths: IndexMap<String, usize>) {
        self.emitted_depths = depths;
    }

    pub(super) fn discovered_depths(&self) -> IndexMap<String, usize> {
        self.depths.clone()
    }

    /// Raw omissions, for callers that merge several contexts before
    /// summarizing them with [`summarize_omissions`].
    pub(super) fn take_omissions(&mut self) -> Vec<Omission> {
        std::mem::take(&mut self.omissions)
    }

    /// This context's omissions as summarized stderr lines.
    pub(super) fn take_omission_lines(&mut self) -> Vec<String> {
        summarize_omissions(self.take_omissions())
    }

    /// Records one rendered use of `reference`: a linked row (default mode), an
    /// inline expansion (`--inline-schemas`), or a cutoff row that still links.
    /// Observation only; never affects output bytes.
    fn record_use(&mut self, reference: &str, depth: usize) {
        match self.uses.get_mut(reference) {
            Some(entry) => {
                entry.count += 1;
                entry.min_depth = entry.min_depth.min(depth);
            }
            None => {
                self.uses.insert(
                    reference.to_string(),
                    SchemaUse {
                        count: 1,
                        min_depth: depth,
                    },
                );
            }
        }
    }

    /// Use-site observations collected while rendering, in first-encounter order.
    pub(super) fn schema_uses(&self) -> &IndexMap<String, SchemaUse> {
        &self.uses
    }

    pub(super) fn references(&self) -> impl Iterator<Item = (&String, usize)> {
        self.anchors
            .keys()
            .map(|reference| (reference, self.depths.get(reference).copied().unwrap_or(0)))
    }

    // Relax the complete reachable reference graph before rendering definitions.
    // A shared schema may first occur through a deeper route; its smallest depth
    // is propagated transitively so rendering order cannot change the result.
    fn discover(&mut self, schema: &Schema, depth: usize) {
        if self
            .max_depth
            .is_some_and(|limit| depth >= limit.max(self.protected_depth))
        {
            return;
        }
        if let Some(reference) = &schema.reference {
            if let Some(resolved) = resolve_schema_reference(reference, self.doc) {
                let next = depth + 1;
                if self.depths.get(reference).is_none_or(|old| next < *old) {
                    self.depths.insert(reference.clone(), next);
                    self.register(reference);
                    self.discover(resolved, next);
                }
            }
            return;
        }
        if let Some(properties) = &schema.properties {
            for child in properties.values() {
                self.discover(child, depth + 1);
            }
        }
        if let Some(items) = &schema.items {
            self.discover(items, depth + 1);
        }
        if let Some(AdditionalProperties::Schema(child)) = &schema.additional_properties {
            self.discover(child, depth + 1);
        }
        for variants in [&schema.all_of, &schema.one_of, &schema.any_of]
            .into_iter()
            .flatten()
        {
            for child in variants {
                self.discover(child, depth + 1);
            }
        }
    }

    /// Recovery command for a row cut off at `depth`. Under a depth limit the
    /// command carries a bounded `--schema-depth` too, so following it can
    /// never re-trigger an unbounded expansion (#160): a `$ref` re-rooted with
    /// `--schema NAME` shows the next `limit` levels of that schema; a command
    /// that re-renders the same root goes `limit` levels past the cutoff.
    fn retrieval(&self, reference: Option<&str>, depth: usize) -> String {
        let mut command = format!("vimanam {}", super::split::quote_shell(&self.source_path));
        command.push_str(&super::split::proto_path_flags(
            &self.source_path,
            &self.proto_paths,
        ));
        let step = self.max_depth.map(|limit| limit.max(1));
        let relative = depth.saturating_sub(self.root_depth);
        let deeper = step
            .map(|step| {
                format!(
                    " --schema-depth {}",
                    (relative + step).min(SCHEMA_DEPTH_LIMIT)
                )
            })
            .unwrap_or_default();

        if let Some(selector) = &self.active_selector {
            command.push_str(&format!(
                " --schema-field {}{deeper}",
                super::split::quote_shell(selector)
            ));
            command.push_str(" --no-report");
            return command;
        }

        if let Some(name) = reference.map(short_schema_reference) {
            let rerooted = step
                .map(|step| format!(" --schema-depth {step}"))
                .unwrap_or_default();
            command.push_str(&format!(
                " --schema {}{rerooted}",
                super::split::quote_shell(&name)
            ));
            command.push_str(" --no-report");
            return command;
        }

        if let Some(name) = &self.current_schema {
            command.push_str(&format!(
                " --schema {}{deeper}",
                super::split::quote_shell(name)
            ));
            command.push_str(" --no-report");
            return command;
        }

        // Inline operation body: reconstruct from the active endpoint and filters.
        self.append_scope_flags(&mut command);
        command.push_str(&format!(
            " --detail full --include-schemas{deeper} --no-report"
        ));
        command
    }

    fn append_scope_flags(&self, command: &mut String) {
        if let Some(selector) = &self.operation_selector {
            for operation in &selector.operations {
                command.push_str(&format!(
                    " --operation {}",
                    super::split::quote_shell(&operation.to_string())
                ));
            }
            for id in &selector.operation_ids {
                command.push_str(&format!(
                    " --operation-id {}",
                    super::split::quote_shell(id)
                ));
            }
        } else if let Some(operation) = &self.current_operation {
            command.push_str(&format!(
                " --operation {}",
                super::split::quote_shell(operation)
            ));
        }
        if let Some(services) = &self.service_filter {
            command.push_str(&format!(
                " --service-filter {}",
                super::split::quote_shell(&services.join(","))
            ));
        }
        if let Some(methods) = &self.method_filter {
            command.push_str(&format!(
                " --method-filter {}",
                super::split::quote_shell(&methods.join(","))
            ));
        }
        if let Some(path) = &self.path_filter {
            command.push_str(&format!(
                " --path-filter {}",
                super::split::quote_shell(path)
            ));
        }
        if self.exclude_deprecated {
            command.push_str(" --exclude-deprecated");
        }
    }

    /// Link target for a cutoff `$ref` when a fuller artifact will be (or was)
    /// written. Does not invent links to schemas that are not emitted.
    fn cutoff_type_name(&mut self, schema: &Schema, depth: usize) -> String {
        let Some(reference) = schema.reference.as_deref() else {
            return schema_type_label(schema).to_string();
        };
        let linkable =
            self.anchors.contains_key(reference) || self.emitted_depths.contains_key(reference);
        if !linkable {
            return format!("ref {}", short_schema_reference(reference));
        }
        if let Some(&depth) = self.emitted_depths.get(reference) {
            self.depths
                .entry(reference.to_string())
                .and_modify(|old| *old = (*old).min(depth))
                .or_insert(depth);
        }
        let name = short_schema_reference(reference);
        let anchor = self.register(reference);
        self.record_use(reference, depth);
        if self.external {
            format!("[{}]({anchor})", super::split::escape(&name))
        } else {
            format!("[{name}](#{anchor})")
        }
    }

    /// The documentation being rendered, for callers that need it alongside the
    /// context (e.g. example resolution).
    pub(super) fn doc(&self) -> &'a ApiDocumentation {
        self.doc
    }

    /// Registers a component reference for deferred rendering (if not already
    /// seen) and returns its stable, collision-free anchor.
    fn register(&mut self, reference: &str) -> String {
        if let Some(anchor) = self.anchors.get(reference) {
            return anchor.clone();
        }

        if self.external {
            let target = format!("../schemas/{}", super::split::schema_filename(reference));
            self.anchors.insert(reference.to_string(), target.clone());
            return target;
        }

        let base = format!(
            "schema-{}",
            clean_for_id(&short_schema_reference(reference))
        );
        let mut anchor = base.clone();
        let mut suffix = 2;
        while self.used_anchors.contains(&anchor) {
            anchor = format!("{base}-{suffix}");
            suffix += 1;
        }

        self.used_anchors.insert(anchor.clone());
        self.anchors.insert(reference.to_string(), anchor.clone());
        anchor
    }
}

/// Returns the schema of a response, preferring the OpenAPI 2.0 `schema` field
/// and falling back to the first media type's schema (OpenAPI 3.0 `content`).
///
/// Crate-visible (re-exported from `markdown`) so `diff` compares the same
/// schema the renderer documents. Only the first media type is considered.
pub(crate) fn response_schema(response: &Response) -> Option<&Schema> {
    if let Some(schema) = &response.schema {
        return Some(schema);
    }

    response
        .content
        .as_ref()
        .and_then(|content| content.values().find_map(|media| media.schema.as_ref()))
}

/// Request-body schema an endpoint's full-detail schema section expands, if any.
///
/// The parser emits one synthetic `in: body` parameter per requestBody media
/// type (and Swagger 2 may carry multiple body parameters). Prefer the first
/// that has a schema — same rule as [`response_schema`] for content media types
/// — so a schema-less `application/octet-stream` ahead of `application/json`
/// does not suppress the JSON table.
pub(super) fn request_body_schema(endpoint: &Endpoint) -> Option<&Schema> {
    endpoint
        .parameters
        .iter()
        .filter(|parameter| parameter.parameter_in == "body")
        .find_map(|parameter| parameter.schema.as_ref())
}

/// First 2xx response in spec order — the only success body the renderer expands.
pub(super) fn first_success_response(endpoint: &Endpoint) -> Option<(&str, &Response)> {
    endpoint
        .responses
        .iter()
        .find(|(code, _)| {
            code.starts_with('2')
                || ((endpoint.method == "RPC"
                    || crate::contract::graphql::OPERATION_METHODS
                        .contains(&endpoint.method.as_str()))
                    && code.as_str() == "result")
        })
        .map(|(code, response)| (code.as_str(), response))
}

/// Labelled request roots [`super::endpoint::write_endpoint`] expands under
/// `--detail full --include-schemas`.
///
/// For a GraphQL root field these are its arguments whose type names a schema
/// type (input object, enum or custom scalar, possibly inside lists), each a
/// depth-0 root labelled by the argument name, so `--schema-depth` counts from
/// the argument type exactly as it counts from an OpenAPI request body (#170).
/// Built-in scalar arguments are already fully described by the Arguments
/// table. Every other endpoint has at most the request body, labelled
/// [`REQUEST_BODY_LABEL`]. The third element is the root row's Required cell: the
/// argument's non-null flag, or `None` (`-`) for a request body.
pub(super) fn request_schema_roots(endpoint: &Endpoint) -> Vec<(&str, &Schema, Option<bool>)> {
    if crate::contract::graphql::OPERATION_METHODS.contains(&endpoint.method.as_str()) {
        return endpoint
            .parameters
            .iter()
            .filter(|parameter| parameter.parameter_in == "argument")
            .filter_map(|parameter| {
                parameter
                    .schema
                    .as_ref()
                    .filter(|schema| names_schema_type(schema))
                    .map(|schema| (parameter.name.as_str(), schema, parameter.required))
            })
            .collect();
    }
    request_body_schema(endpoint)
        .map(|schema| (REQUEST_BODY_LABEL, schema, None))
        .into_iter()
        .collect()
}

/// Whether a GraphQL argument schema is (a list of) a `$ref` to a schema type.
fn names_schema_type(schema: &Schema) -> bool {
    schema.reference.is_some() || schema.items.as_deref().is_some_and(names_schema_type)
}

/// Schemas [`super::endpoint::write_endpoint`] actually expands under
/// `--detail full --include-schemas`: request roots (see
/// [`request_schema_roots`]) and the first 2xx response schema (if any).
/// Shared by single-file and split/Skill pre-discovery so cutoff linking never
/// invents pages for schemas the tree does not render.
pub(super) fn rendered_endpoint_schemas(endpoint: &Endpoint) -> Vec<&Schema> {
    let mut schemas: Vec<&Schema> = request_schema_roots(endpoint)
        .into_iter()
        .map(|(_, schema, _)| schema)
        .collect();
    if let Some((_, response)) = first_success_response(endpoint)
        && let Some(schema) = response_schema(response)
    {
        schemas.push(schema);
    }
    schemas
}

/// Pre-discover every schema this document will expand so cutoff rows can link
/// to definitions regardless of endpoint write order. No-op without
/// `--schema-depth`, no-op in `--inline-schemas` mode (no Schema Definitions
/// section to link to), and no-op unless the renderer will expand schemas
/// (`--detail full --include-schemas`), so lower-detail budget trials and
/// default output stay unchanged.
pub(super) fn prediscover_rendered_endpoints<'a>(
    ctx: &mut SchemaContext<'_>,
    config: &DocConfig,
    endpoints: impl IntoIterator<Item = &'a Endpoint>,
) {
    if ctx.inline
        || ctx.max_depth.is_none()
        || config.detail_level != DetailLevel::Full
        || !config.include_schemas
    {
        return;
    }
    for endpoint in endpoints {
        for schema in rendered_endpoint_schemas(endpoint) {
            ctx.discover_root(schema, 0);
        }
    }
}

/// Writes a Markdown field table for `schema`. `root_label` names the top-level
/// row (e.g. `request`). Component `$ref`s are linked through `ctx` (or inlined
/// under `--inline-schemas`).
pub(super) fn write_schema_table<W: Write>(
    writer: &mut W,
    schema: &Schema,
    root_label: &str,
    ctx: &mut SchemaContext,
) -> Result<()> {
    write_schema_table_at(writer, schema, root_label, 0, ctx)
}

/// Writes one Markdown field table holding several depth-0 roots, each
/// labelled by its own name (GraphQL arguments). Each root is discovered and
/// expanded exactly as [`write_schema_table`] would expand it alone.
pub(super) fn write_schema_roots_table<W: Write>(
    writer: &mut W,
    roots: &[(&str, &Schema, Option<bool>)],
    ctx: &mut SchemaContext,
) -> Result<()> {
    let mut rows = Vec::new();
    for (label, schema, required) in roots {
        let mut ref_stack = Vec::new();
        if ctx.max_depth.is_some() && !ctx.inline {
            ctx.discover(schema, 0);
        }
        collect_root_rows(schema, label, *required, &mut rows, &mut ref_stack, 0, ctx);
    }
    write_rows(writer, &rows, ctx.external)
}

/// Collects one root's rows. A synthetic body root ([`REQUEST_BODY_LABEL`],
/// [`RESPONSE_BODY_LABEL`]) is walked with an empty path so its descendants
/// render as bare JSON property paths, and only its own row shows the label.
fn collect_root_rows(
    schema: &Schema,
    label: &str,
    required: Option<bool>,
    rows: &mut Vec<SchemaRow>,
    ref_stack: &mut Vec<String>,
    depth: usize,
    ctx: &mut SchemaContext,
) {
    ctx.root_depth = depth;
    if !is_body_label(label) {
        collect_schema_rows(schema, label, required, rows, ref_stack, depth, ctx);
        return;
    }
    let start = rows.len();
    let prior = ctx.body_label.replace(label.to_string());
    collect_schema_rows(schema, "", required, rows, ref_stack, depth, ctx);
    ctx.body_label = prior;
    for row in &mut rows[start..] {
        if row.field.is_empty() {
            row.field = label.to_string();
        }
    }
}

pub(super) fn write_schema_table_at<W: Write>(
    writer: &mut W,
    schema: &Schema,
    root_label: &str,
    depth: usize,
    ctx: &mut SchemaContext,
) -> Result<()> {
    let mut rows = Vec::new();
    let mut ref_stack = Vec::new();
    if ctx.max_depth.is_some() && !ctx.inline {
        ctx.discover(schema, depth);
    }
    collect_root_rows(
        schema,
        root_label,
        None,
        &mut rows,
        &mut ref_stack,
        depth,
        ctx,
    );
    write_rows(writer, &rows, ctx.external)
}

/// Renders the trailing "Schema Definitions" section: every component schema
/// linked during the document body, each expanded once. Expanding a definition
/// may link further components, which are appended and rendered in turn. Writes
/// nothing in `--inline-schemas` mode or when no schema was linked.
pub(super) fn render_schema_definitions<W: Write>(
    writer: &mut W,
    ctx: &mut SchemaContext,
) -> Result<()> {
    if ctx.inline || ctx.anchors.is_empty() {
        return Ok(());
    }

    let doc = ctx.doc;
    writeln!(writer, "## Schema Definitions\n")?;

    // The map grows while we render (a definition can link new components), so
    // walk it by index until the tail stops moving. Insertion order keeps the
    // section deterministic.
    let mut index = 0;
    while index < ctx.anchors.len() {
        let (reference, anchor) = {
            let (reference, anchor) = ctx.anchors.get_index(index).expect("index in range");
            (reference.clone(), anchor.clone())
        };
        index += 1;

        let name = short_schema_reference(&reference);
        ctx.current_schema = Some(name.clone());
        ctx.protected_depth = 0;
        let definition_depth = ctx.depths.get(&reference).copied().unwrap_or(0);
        ctx.root_depth = definition_depth;
        writeln!(writer, "### {} {{#{}}}", name, anchor)?;

        let mut rows = Vec::new();
        let mut ref_stack = Vec::new();
        match resolve_schema_reference(&reference, doc) {
            Some(resolved) => collect_schema_rows(
                resolved,
                &name,
                None,
                &mut rows,
                &mut ref_stack,
                definition_depth,
                ctx,
            ),
            None => rows.push(SchemaRow {
                field: name.clone(),
                type_name: "unknown".to_string(),
                required: "-".to_string(),
                description: format!("Unresolved schema reference: {reference}"),
            }),
        }

        write_rows(writer, &rows, false)?;
        writeln!(writer)?;
    }

    Ok(())
}

fn write_rows<W: Write>(writer: &mut W, rows: &[SchemaRow], external: bool) -> Result<()> {
    if rows.is_empty() {
        writeln!(writer, "*No schema fields available*")?;
        return Ok(());
    }

    writeln!(writer, "| Field | Type | Required | Description |")?;
    writeln!(writer, "|------|------|---------:|-------------|")?;
    for row in rows {
        // Split navigation supports arbitrary component/property names. Use a
        // sufficiently long code delimiter and escape table pipes/newlines.
        // Preserve existing single-document bytes.
        let field = if external {
            let longest_run = row
                .field
                .split(|ch| ch != '`')
                .map(str::len)
                .max()
                .unwrap_or(0);
            let delimiter = "`".repeat(longest_run + 1);
            format!("{delimiter} {} {delimiter}", escape_table_cell(&row.field))
        } else {
            format!("`{}`", row.field)
        };
        writeln!(
            writer,
            "| {} | {} | {} | {} |",
            field,
            escape_table_cell(&row.type_name),
            row.required,
            escape_table_cell(&row.description)
        )?;
    }

    Ok(())
}

/// Estimated tokens of the "Schema Definitions" entry (heading plus field
/// table) the renderer emits for `reference`, measured by rendering that entry
/// alone with the same configuration. Used by the `--costs` analysis.
///
/// The real section may pick a differently suffixed anchor when schema names
/// collide; the size difference is a few characters.
#[cfg(feature = "cli")]
pub(crate) fn definition_section_tokens(
    doc: &ApiDocumentation,
    config: &DocConfig,
    reference: &str,
    definition_depth: usize,
) -> usize {
    let Some(resolved) = resolve_schema_reference(reference, doc) else {
        return 0;
    };
    let mut ctx = SchemaContext::configured(doc, config, false);
    // Definitions are a linked-mode construct; measuring the hypothetical
    // "read once" cost in inline mode expands links like the linked section
    // would, so force linked rendering regardless of the configured mode.
    ctx.inline = false;
    ctx.protected_depth = 0;
    let name = short_schema_reference(reference);
    ctx.current_schema = Some(name.clone());
    let anchor = ctx.register(reference);

    let mut rows = Vec::new();
    let mut ref_stack = Vec::new();
    collect_schema_rows(
        resolved,
        &name,
        None,
        &mut rows,
        &mut ref_stack,
        definition_depth,
        &mut ctx,
    );

    let mut buffer = Vec::new();
    let _ = writeln!(&mut buffer, "### {name} {{#{anchor}}}");
    let _ = write_rows(&mut buffer, &rows, false);
    // The real section separates each entry from the next with a blank line;
    // without it every DEF row undercounts by one character.
    let _ = writeln!(&mut buffer);
    super::estimate_tokens(&buffer)
}

/// Estimated tokens of one inline expansion of `reference` at a use site (the
/// field rows the expansion renders in place of a single link row), measured
/// with the schema rendered fully inline under the same configuration, cycle
/// guards included. Used by the `--costs` amplification model.
///
/// The measurement expands the schema once at the root, under the schema's own
/// name as the field label, while real expansions happen at each use site's
/// label and depth; the drift is a few characters, though a depth-bounded
/// configuration may truncate the real expansion differently.
///
/// Returns the tokens and whether the expansion was bounded: without
/// `--schema-depth`, an expansion that would render more than
/// [`INLINE_ROW_BUDGET`] rows (#177) is measured at [`AUTO_SCHEMA_DEPTH`]
/// instead, so the figure is a lower bound.
#[cfg(feature = "cli")]
pub(crate) fn inline_expansion_tokens(
    doc: &ApiDocumentation,
    config: &DocConfig,
    reference: &str,
) -> (usize, bool) {
    let Some(resolved) = resolve_schema_reference(reference, doc) else {
        return (0, false);
    };
    let mut ctx = SchemaContext::configured(doc, config, false);
    let bounded = ctx.max_depth.is_none()
        && inline_row_count(doc, [resolved], ctx.selected, INLINE_ROW_BUDGET) > INLINE_ROW_BUDGET;
    if bounded {
        ctx.max_depth = Some(AUTO_SCHEMA_DEPTH);
    }
    ctx.inline = true;
    ctx.protected_depth = 0;
    let name = short_schema_reference(reference);
    ctx.current_schema = Some(name.clone());

    let mut rows = Vec::new();
    let mut ref_stack = Vec::new();
    collect_schema_rows(
        resolved,
        &name,
        None,
        &mut rows,
        &mut ref_stack,
        0,
        &mut ctx,
    );

    let mut buffer = Vec::new();
    let _ = write_rows(&mut buffer, &rows, false);
    (super::estimate_tokens(&buffer), bounded)
}

fn push_cutoff_row(
    schema: &Schema,
    field: &str,
    required: Option<bool>,
    rows: &mut Vec<SchemaRow>,
    depth: usize,
    ctx: &mut SchemaContext,
) {
    let metadata = schema
        .reference
        .as_deref()
        .and_then(|reference| resolve_schema_reference(reference, ctx.doc))
        .unwrap_or(schema);
    let mut description = schema
        .description
        .clone()
        .or_else(|| metadata.description.clone())
        .unwrap_or_else(|| "-".into());
    // Depth-limited rows always keep enums; this branch is only reached when
    // max_depth or selected is set, so default output is unchanged.
    append_enum(&mut description, metadata, true);
    let expandable = schema.reference.is_some()
        || schema.properties.as_ref().is_some_and(|v| !v.is_empty())
        || schema.items.is_some()
        || matches!(
            schema.additional_properties,
            Some(AdditionalProperties::Schema(_))
        )
        || schema.all_of.is_some()
        || schema.one_of.is_some()
        || schema.any_of.is_some();
    if expandable {
        let command = ctx.retrieval(schema.reference.as_deref(), depth);
        description.push_str(&format!(
            "; Omitted nested expansion at schema depth {depth}. Retrieve full detail: {command}"
        ));
        let field = match (&ctx.body_label, field.is_empty()) {
            (Some(label), true) => label.clone(),
            _ => field.to_string(),
        };
        let operation = ctx.current_operation.clone();
        ctx.omissions.push(Omission {
            depth,
            field,
            command,
            operation,
        });
    }
    rows.push(SchemaRow {
        field: field.to_string(),
        type_name: ctx.cutoff_type_name(schema, depth),
        required: required_to_string(required).to_string(),
        description,
    });
}

/// Internal schema extension flag requesting that a `$ref` row prefer its own
/// field-level `description` over the referenced target's description (used by
/// non-OpenAPI adapters such as Protobuf without altering OpenAPI rendering or
/// breaking public struct literals).
pub(crate) const PREFER_REF_DESCRIPTION_EXT: &str = "x-vimanam-prefer-ref-description";

fn prefer_ref_description(schema: &Schema) -> bool {
    schema
        .extensions
        .get(PREFER_REF_DESCRIPTION_EXT)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn inlined_ref_description(schema: &Schema, resolved: &Schema) -> Option<String> {
    let field_desc = schema.description.as_deref()?;
    match resolved.description.as_deref() {
        Some(target) if !target.is_empty() && target != "Protocol Buffers message" => {
            Some(format!("{field_desc}\n\n{target}"))
        }
        _ => Some(field_desc.to_owned()),
    }
}

fn composition_source_index(variant: &Schema, index: usize, selected: bool) -> usize {
    if selected {
        variant
            .extensions
            .get("x-vimanam-selected-index")
            .and_then(serde_json::Value::as_u64)
            .map(|index| index as usize)
            .unwrap_or(index)
    } else {
        index
    }
}

/// Recursion safety limit, and the largest accepted `--schema-depth`.
const SCHEMA_DEPTH_LIMIT: usize = 24;

fn collect_schema_rows(
    schema: &Schema,
    field: &str,
    required: Option<bool>,
    rows: &mut Vec<SchemaRow>,
    ref_stack: &mut Vec<String>,
    depth: usize,
    ctx: &mut SchemaContext,
) {
    const MAX_DEPTH: usize = SCHEMA_DEPTH_LIMIT;

    if ctx
        .max_depth
        .is_some_and(|limit| depth >= limit.max(ctx.protected_depth))
        || (ctx.selected && depth >= MAX_DEPTH.max(ctx.protected_depth))
    {
        push_cutoff_row(schema, field, required, rows, depth, ctx);
        return;
    }

    if ctx.max_depth.is_none() && !ctx.selected && depth >= MAX_DEPTH {
        rows.push(SchemaRow {
            field: field.to_string(),
            type_name: "truncated".to_string(),
            required: required_to_string(required).to_string(),
            description: "Maximum schema depth reached; nested expansion stopped".to_string(),
        });
        return;
    }

    match &schema.reference {
        // Inline mode + cycle detected.
        Some(reference) if ctx.inline && ref_stack.contains(reference) => {
            let description = if prefer_ref_description(schema)
                && let Some(field_desc) = &schema.description
            {
                format!("{field_desc}; Cycle detected while expanding schema reference")
            } else {
                "Cycle detected while expanding schema reference".to_string()
            };
            rows.push(SchemaRow {
                field: field.to_string(),
                type_name: format!("ref {}", short_schema_reference(reference)),
                required: required_to_string(required).to_string(),
                description,
            });
            return;
        }

        // Inline mode + resolvable: expand in place, guarding against cycles.
        Some(reference) if ctx.inline => {
            let doc = ctx.doc;
            if let Some(resolved) = resolve_schema_reference(reference, doc) {
                ctx.record_use(reference, depth);
                ref_stack.push(reference.clone());
                let prior_schema = ctx
                    .current_schema
                    .replace(short_schema_reference(reference));
                // The inlined target's own row sits one edge below the ref.
                let prior_root = std::mem::replace(&mut ctx.root_depth, depth + 1);
                let mut inlined;
                let target = if prefer_ref_description(schema)
                    && let Some(combined) = inlined_ref_description(schema, resolved)
                {
                    inlined = resolved.clone();
                    inlined.description = Some(combined);
                    &inlined
                } else {
                    resolved
                };
                collect_schema_rows(target, field, required, rows, ref_stack, depth + 1, ctx);
                ctx.root_depth = prior_root;
                ctx.current_schema = prior_schema;
                ref_stack.pop();
            } else {
                // Inline + unresolvable.
                rows.push(SchemaRow {
                    field: field.to_string(),
                    type_name: format!("ref {}", short_schema_reference(reference)),
                    required: required_to_string(required).to_string(),
                    description: format!("Unresolved schema reference: {reference}"),
                });
            }
            return;
        }

        // Linked mode + resolvable: emit one row pointing at the shared definition.
        Some(reference) if let Some(resolved) = resolve_schema_reference(reference, ctx.doc) => {
            let name = short_schema_reference(reference);
            let description = if ctx.selected || prefer_ref_description(schema) {
                schema
                    .description
                    .clone()
                    .or_else(|| resolved.description.clone())
            } else {
                resolved.description.clone()
            }
            .unwrap_or_else(|| "-".to_string());
            if ctx.max_depth.is_none() {
                ctx.depths.entry(reference.clone()).or_insert(0);
            }
            let anchor = ctx.register(reference);
            ctx.record_use(reference, depth);
            rows.push(SchemaRow {
                field: field.to_string(),
                type_name: if ctx.external {
                    format!("[{}]({anchor})", super::split::escape(&name))
                } else {
                    format!("[{name}](#{anchor})")
                },
                required: required_to_string(required).to_string(),
                description,
            });
            return;
        }

        // Any reference that couldn't be resolved (either mode).
        Some(reference) => {
            rows.push(SchemaRow {
                field: field.to_string(),
                type_name: format!("ref {}", short_schema_reference(reference)),
                required: required_to_string(required).to_string(),
                description: format!("Unresolved schema reference: {reference}"),
            });
            return;
        }

        // No $ref — fall through to inline field rendering below.
        None => {}
    }

    let description = schema.description.as_deref().unwrap_or("-");
    rows.push(SchemaRow {
        field: field.to_string(),
        type_name: schema_type_label(schema).to_string(),
        required: required_to_string(required).to_string(),
        description: {
            let mut description = description.to_string();
            append_enum(&mut description, schema, ctx.selected);
            if ctx.selected
                && let Some(title) = &schema.title
            {
                description.push_str(&format!("; {title}"));
            }
            description
        },
    });

    if let Some(properties) = &schema.properties {
        let required_fields: HashSet<&str> = schema
            .required
            .as_ref()
            .map(|items| items.iter().map(String::as_str).collect())
            .unwrap_or_default();

        for (name, child_schema) in properties {
            let child_field = format_field(field, name);
            collect_schema_rows(
                child_schema,
                &child_field,
                Some(required_fields.contains(name.as_str())),
                rows,
                ref_stack,
                depth + 1,
                ctx,
            );
        }
    }

    if let Some(items) = &schema.items {
        let item_field = format!("{}[]", field);
        collect_schema_rows(items, &item_field, None, rows, ref_stack, depth + 1, ctx);
    }

    if (ctx.max_depth.is_some() || ctx.selected)
        && let Some(AdditionalProperties::Schema(child)) = &schema.additional_properties
    {
        collect_schema_rows(
            child,
            &format_field(field, "*"),
            None,
            rows,
            ref_stack,
            depth + 1,
            ctx,
        );
    }

    for (label, variants) in [
        ("allOf", &schema.all_of),
        ("oneOf", &schema.one_of),
        ("anyOf", &schema.any_of),
    ] {
        if let Some(variants) = variants {
            for (index, variant) in variants.iter().enumerate() {
                let source_index = composition_source_index(variant, index, ctx.selected);
                let variant_field = format_field(field, &format!("{label}[{source_index}]"));
                collect_schema_rows(
                    variant,
                    &variant_field,
                    required,
                    rows,
                    ref_stack,
                    depth + 1,
                    ctx,
                );
            }
        }
    }
}

fn append_enum(description: &mut String, schema: &Schema, enabled: bool) {
    if enabled && let Some(values) = &schema.enum_values {
        description.push_str("; Enum: ");
        description.push_str(
            &values
                .iter()
                .map(serde_json::Value::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
}

/// Returns a `Display` adapter for the schema type label, writing directly into
/// the formatter without an intermediate `String` allocation (#74).
///
/// Examples: `"string"`, `"integer(int64)"`, `"array<string>"`, `"object"`,
/// `"enum[3]"`, `"boolean | null"`.
fn schema_type_label(schema: &Schema) -> impl fmt::Display + '_ {
    fmt::from_fn(move |f| {
        // Core type token — each branch writes directly into the formatter.
        if let Some(schema_type) = &schema.schema_type {
            if schema_type == "array" {
                f.write_str("array<")?;
                if let Some(items) = schema.items.as_deref() {
                    write!(f, "{}", schema_type_hint(items))?;
                } else {
                    f.write_str("unknown")?;
                }
                f.write_str(">")?;
            } else if let Some(format) = &schema.format {
                write!(f, "{schema_type}({format})")?;
            } else {
                f.write_str(schema_type)?;
            }
        } else if schema.properties.is_some() {
            f.write_str("object")?;
        } else if let Some(items) = schema.items.as_deref() {
            write!(f, "array<{}>", schema_type_hint(items))?;
        } else if schema.all_of.as_ref().is_some_and(|v| !v.is_empty()) {
            f.write_str("allOf")?;
        } else if schema.one_of.as_ref().is_some_and(|v| !v.is_empty()) {
            f.write_str("oneOf")?;
        } else if schema.any_of.as_ref().is_some_and(|v| !v.is_empty()) {
            f.write_str("anyOf")?;
        } else if let Some(enum_values) = &schema.enum_values {
            write!(f, "enum[{}]", enum_values.len())?;
        } else {
            f.write_str("unknown")?;
        }

        // Nullable suffix.
        if schema.nullable.unwrap_or(false) {
            f.write_str(" | null")?;
        }

        Ok(())
    })
}

/// Returns a `Display` adapter for a compact one-word type hint used inside
/// `array<…>` labels, writing directly into the formatter (#74).
fn schema_type_hint(schema: &Schema) -> impl fmt::Display + '_ {
    fmt::from_fn(move |f| {
        if let Some(reference) = &schema.reference {
            write!(f, "ref {}", short_schema_reference(reference))
        } else if let Some(schema_type) = &schema.schema_type {
            f.write_str(schema_type)
        } else if schema.properties.is_some() {
            f.write_str("object")
        } else if schema.items.is_some() {
            f.write_str("array")
        } else {
            f.write_str("unknown")
        }
    })
}

fn format_field(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        return child.to_string();
    }

    format!("{}.{}", parent, child)
}

fn required_to_string(required: Option<bool>) -> &'static str {
    match required {
        Some(true) => "Yes",
        Some(false) => "No",
        None => "-",
    }
}

pub(crate) fn short_schema_reference(reference: &str) -> String {
    reference
        .rsplit('/')
        .next()
        .map(decode_json_pointer_token)
        .unwrap_or_else(|| reference.to_string())
}

/// Returns a `Display` adapter that escapes Markdown table-cell special
/// characters, writing directly into the formatter without an intermediate
/// `String` allocation (#74).
///
/// Writes contiguous clean slices in bulk and only breaks out for the rare `|`
/// and `\n` characters, keeping the common (no-special-chars) path fast.
fn escape_table_cell(value: &str) -> impl fmt::Display + '_ {
    fmt::from_fn(move |f| {
        let mut start = 0;
        for (i, c) in value.char_indices() {
            let replacement = match c {
                '|' => "\\|",
                '\n' => "<br/>",
                _ => continue,
            };
            f.write_str(&value[start..i])?;
            f.write_str(replacement)?;
            start = i + c.len_utf8();
        }
        f.write_str(&value[start..])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Schema;

    // ── helpers ──────────────────────────────────────────────────────────────

    /// Convenience: materialise any `Display` as a `String`.
    fn display(d: impl std::fmt::Display) -> String {
        d.to_string()
    }

    fn schema_with_type(t: &str) -> Schema {
        Schema {
            schema_type: Some(t.to_string()),
            ..Schema::default()
        }
    }

    fn schema_with_type_and_format(t: &str, fmt: &str) -> Schema {
        Schema {
            schema_type: Some(t.to_string()),
            format: Some(fmt.to_string()),
            ..Schema::default()
        }
    }

    // ── escape_table_cell (#74) ───────────────────────────────────────────

    #[test]
    fn escape_table_cell_plain_passthrough() {
        assert_eq!(display(escape_table_cell("hello world")), "hello world");
    }

    #[test]
    fn escape_table_cell_escapes_pipe() {
        assert_eq!(display(escape_table_cell("a|b")), r"a\|b");
    }

    #[test]
    fn escape_table_cell_replaces_newline_with_br() {
        assert_eq!(display(escape_table_cell("a\nb")), "a<br/>b");
    }

    #[test]
    fn escape_table_cell_multiple_specials() {
        assert_eq!(display(escape_table_cell("x|y\nz")), r"x\|y<br/>z");
    }

    #[test]
    fn escape_table_cell_empty_string() {
        assert_eq!(display(escape_table_cell("")), "");
    }

    // ── schema_type_hint (#74) ────────────────────────────────────────────

    #[test]
    fn schema_type_hint_with_reference() {
        let schema = Schema {
            reference: Some("#/components/schemas/Pet".to_string()),
            ..Schema::default()
        };
        assert_eq!(display(schema_type_hint(&schema)), "ref Pet");
    }

    #[test]
    fn schema_type_hint_with_type() {
        assert_eq!(
            display(schema_type_hint(&schema_with_type("integer"))),
            "integer"
        );
    }

    #[test]
    fn schema_type_hint_properties_returns_object() {
        let schema = Schema {
            properties: Some(indexmap::IndexMap::new()),
            ..Schema::default()
        };
        assert_eq!(display(schema_type_hint(&schema)), "object");
    }

    #[test]
    fn schema_type_hint_items_returns_array() {
        let schema = Schema {
            items: Some(Box::new(schema_with_type("string"))),
            ..Schema::default()
        };
        assert_eq!(display(schema_type_hint(&schema)), "array");
    }

    #[test]
    fn schema_type_hint_unknown_fallback() {
        assert_eq!(display(schema_type_hint(&Schema::default())), "unknown");
    }

    // ── schema_type_label (#74) ───────────────────────────────────────────

    #[test]
    fn schema_type_label_plain_string() {
        assert_eq!(
            display(schema_type_label(&schema_with_type("string"))),
            "string"
        );
    }

    #[test]
    fn schema_type_label_type_with_format() {
        assert_eq!(
            display(schema_type_label(&schema_with_type_and_format(
                "integer", "int64"
            ))),
            "integer(int64)"
        );
    }

    #[test]
    fn schema_type_label_array_with_item_type() {
        let schema = Schema {
            schema_type: Some("array".to_string()),
            items: Some(Box::new(schema_with_type("string"))),
            ..Schema::default()
        };
        assert_eq!(display(schema_type_label(&schema)), "array<string>");
    }

    #[test]
    fn schema_type_label_array_without_items_is_unknown() {
        let schema = Schema {
            schema_type: Some("array".to_string()),
            ..Schema::default()
        };
        assert_eq!(display(schema_type_label(&schema)), "array<unknown>");
    }

    #[test]
    fn schema_type_label_object_from_properties() {
        let schema = Schema {
            properties: Some(indexmap::IndexMap::new()),
            ..Schema::default()
        };
        assert_eq!(display(schema_type_label(&schema)), "object");
    }

    #[test]
    fn schema_type_label_inferred_array_from_items() {
        let schema = Schema {
            items: Some(Box::new(schema_with_type("integer"))),
            ..Schema::default()
        };
        assert_eq!(display(schema_type_label(&schema)), "array<integer>");
    }

    #[test]
    fn schema_type_label_all_of() {
        let schema = Schema {
            all_of: Some(vec![schema_with_type("object")]),
            ..Schema::default()
        };
        assert_eq!(display(schema_type_label(&schema)), "allOf");
    }

    #[test]
    fn schema_type_label_one_of() {
        let schema = Schema {
            one_of: Some(vec![schema_with_type("string")]),
            ..Schema::default()
        };
        assert_eq!(display(schema_type_label(&schema)), "oneOf");
    }

    #[test]
    fn schema_type_label_any_of() {
        let schema = Schema {
            any_of: Some(vec![schema_with_type("string")]),
            ..Schema::default()
        };
        assert_eq!(display(schema_type_label(&schema)), "anyOf");
    }

    #[test]
    fn schema_type_label_enum() {
        let schema = Schema {
            enum_values: Some(vec![
                serde_json::json!("a"),
                serde_json::json!("b"),
                serde_json::json!("c"),
            ]),
            ..Schema::default()
        };
        assert_eq!(display(schema_type_label(&schema)), "enum[3]");
    }

    #[test]
    fn schema_type_label_unknown_fallback() {
        assert_eq!(display(schema_type_label(&Schema::default())), "unknown");
    }

    #[test]
    fn schema_type_label_nullable_appends_suffix() {
        let schema = Schema {
            schema_type: Some("string".to_string()),
            nullable: Some(true),
            ..Schema::default()
        };
        assert_eq!(display(schema_type_label(&schema)), "string | null");
    }

    #[test]
    fn schema_type_label_non_nullable_no_suffix() {
        let schema = Schema {
            schema_type: Some("boolean".to_string()),
            nullable: Some(false),
            ..Schema::default()
        };
        assert_eq!(display(schema_type_label(&schema)), "boolean");
    }

    #[test]
    fn schema_type_label_array_with_ref_item() {
        let schema = Schema {
            schema_type: Some("array".to_string()),
            items: Some(Box::new(Schema {
                reference: Some("#/components/schemas/Pet".to_string()),
                ..Schema::default()
            })),
            ..Schema::default()
        };
        assert_eq!(display(schema_type_label(&schema)), "array<ref Pet>");
    }

    // ── omission summaries (#161) ────────────────────────────────────────────

    fn omission(depth: usize, field: &str, command: &str) -> Omission {
        Omission {
            depth,
            field: field.into(),
            command: command.into(),
            operation: None,
        }
    }

    fn op_omission(operation: &str, field: &str, command: &str) -> Omission {
        Omission {
            operation: Some(operation.into()),
            ..omission(2, field, command)
        }
    }

    #[test]
    fn field_cut_at_two_depths_counts_once() {
        assert_eq!(
            summarize_omissions(vec![omission(2, "a", "c"), omission(3, "a", "c")]),
            [
                "vimanam: omitted nested schema expansion at depths 2-3 for a; retrieve full detail: c"
            ]
        );
        assert_eq!(
            summarize_omissions(vec![
                omission(2, "a", "c"),
                omission(3, "a", "c"),
                omission(2, "b", "c"),
            ]),
            [
                "vimanam: omitted nested schema expansion at depths 2-3 for 2 fields (a, b); retrieve full detail: c"
            ]
        );
    }

    #[test]
    fn same_path_in_different_operations_counts_per_operation() {
        assert_eq!(
            summarize_omissions(vec![
                op_omission("POST /a", "node", "c"),
                op_omission("POST /b", "node", "c"),
                op_omission("POST /b", "other", "c"),
            ]),
            [
                "vimanam: omitted nested schema expansion at depth 2 for 3 fields across 2 operations (node, other); retrieve full detail: c"
            ]
        );
    }

    #[test]
    fn single_omission_keeps_one_line_form() {
        assert_eq!(
            summarize_omissions(vec![omission(2, "a.b", "cmd")]),
            [
                "vimanam: omitted nested schema expansion at depth 2 for a.b; retrieve full detail: cmd"
            ]
        );
    }

    #[test]
    fn omissions_group_by_command_in_first_seen_order() {
        let lines = summarize_omissions(vec![
            omission(2, "a", "one"),
            omission(3, "b", "two"),
            omission(2, "c", "one"),
            omission(4, "d", "one"),
            omission(2, "a", "one"),
            omission(2, "e", "one"),
        ]);
        assert_eq!(
            lines,
            [
                "vimanam: omitted nested schema expansion at depths 2-4 for 4 fields (a, c, d, ...); retrieve full detail: one",
                "vimanam: omitted nested schema expansion at depth 3 for b; retrieve full detail: two",
            ]
        );
    }

    #[test]
    fn reachable_component_count_stops_past_cap() {
        let mut doc = ApiDocumentation {
            title: String::new(),
            version: String::new(),
            description: None,
            services: Vec::new(),
            endpoints: Vec::new(),
            servers: Vec::new(),
            security_schemes: IndexMap::new(),
            schemas: IndexMap::new(),
            unparsed_schemas: IndexMap::new(),
            examples: IndexMap::new(),
        };
        for i in 0..10 {
            doc.schemas.insert(
                format!("N{i}"),
                Schema {
                    reference: Some(format!("#/components/schemas/N{}", (i + 1) % 10)),
                    ..Schema::default()
                },
            );
        }
        let root = Schema {
            reference: Some("#/components/schemas/N0".into()),
            ..Schema::default()
        };
        assert_eq!(reachable_component_count(&doc, [&root], 100), 10);
        assert_eq!(reachable_component_count(&doc, [&root], 4), 5);
    }

    /// Mutually recursive components `C0..Cn`, each with `k` references to the
    /// next components plus a scalar, an array of refs and a composition.
    fn cyclic_doc(n: usize, k: usize) -> ApiDocumentation {
        let mut schemas = serde_json::Map::new();
        for i in 0..n {
            let mut properties = serde_json::Map::new();
            properties.insert("id".into(), serde_json::json!({"type": "string"}));
            for j in 1..=k {
                properties.insert(
                    format!("r{j}"),
                    serde_json::json!({"$ref": format!("#/components/schemas/C{}", (i + j) % n)}),
                );
            }
            properties.insert(
                "list".into(),
                serde_json::json!({"type": "array", "items": {"$ref": format!("#/components/schemas/C{}", (i + 1) % n)}}),
            );
            properties.insert(
                "either".into(),
                serde_json::json!({"oneOf": [{"$ref": "#/components/schemas/Missing"}, {"type": "integer"}]}),
            );
            properties.insert(
                "extra".into(),
                serde_json::json!({"type": "object", "additionalProperties": {"$ref": format!("#/components/schemas/C{}", (i + 2) % n)}}),
            );
            schemas.insert(
                format!("C{i}"),
                serde_json::json!({"type": "object", "properties": properties}),
            );
        }
        let spec = serde_json::json!({
            "openapi": "3.0.0",
            "info": {"title": "Cyclic", "version": "1"},
            "paths": {},
            "components": {"schemas": schemas},
        });
        crate::parser::parse_openapi_bytes(spec.to_string().as_bytes(), "json", None)
            .expect("cyclic spec parses")
    }

    fn rendered_rows(doc: &ApiDocumentation, schema: &Schema, selected: bool) -> usize {
        let mut ctx = SchemaContext::new(doc, true);
        ctx.selected = selected;
        let mut rows = Vec::new();
        collect_schema_rows(
            schema,
            "root",
            None,
            &mut rows,
            &mut Vec::new(),
            0,
            &mut ctx,
        );
        rows.len()
    }

    #[test]
    fn inline_row_count_matches_the_inline_render() {
        for (n, k) in [(1, 1), (3, 2), (5, 2), (4, 3)] {
            let doc = cyclic_doc(n, k);
            for schema in doc.schemas.values() {
                for selected in [false, true] {
                    let rows = rendered_rows(&doc, schema, selected);
                    assert_eq!(
                        inline_row_count(&doc, [schema], selected, usize::MAX - 1),
                        rows,
                        "n={n} k={k} selected={selected}"
                    );
                }
            }
        }
    }

    #[test]
    fn inline_row_count_stops_past_cap() {
        let doc = cyclic_doc(40, 4);
        let root = &doc.schemas["C0"];
        assert_eq!(inline_row_count(&doc, [root], false, 1_000), 1_001);
        assert_eq!(
            inline_row_count(&doc, [root, root], false, INLINE_ROW_BUDGET),
            INLINE_ROW_BUDGET + 1
        );
    }
}
