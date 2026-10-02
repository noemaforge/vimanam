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
    omissions: Vec<String>,
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

    pub(super) fn take_omissions(&mut self) -> Vec<String> {
        std::mem::take(&mut self.omissions)
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

    fn retrieval(&self, reference: Option<&str>) -> String {
        let mut command = format!("vimanam {}", super::split::quote_shell(&self.source_path));

        if let Some(selector) = &self.active_selector {
            command.push_str(&format!(
                " --schema-field {}",
                super::split::quote_shell(selector)
            ));
            command.push_str(" --no-report");
            return command;
        }

        if let Some(name) = reference.map(short_schema_reference) {
            command.push_str(&format!(" --schema {}", super::split::quote_shell(&name)));
            command.push_str(" --no-report");
            return command;
        }

        if let Some(name) = &self.current_schema {
            command.push_str(&format!(" --schema {}", super::split::quote_shell(name)));
            command.push_str(" --no-report");
            return command;
        }

        // Inline operation body: reconstruct from the active endpoint and filters.
        self.append_scope_flags(&mut command);
        command.push_str(" --detail full --include-schemas --no-report");
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

/// Print omission notices collected during a render that was actually written.
pub(super) fn emit_omissions(omissions: &[String]) {
    for notice in omissions {
        eprintln!("{notice}");
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
        .find(|(code, _)| code.starts_with('2'))
        .map(|(code, response)| (code.as_str(), response))
}

/// Schemas [`super::endpoint::write_endpoint`] actually expands under
/// `--detail full --include-schemas`: request body (if any) and the first 2xx
/// response schema (if any). Shared by single-file and split/Skill pre-discovery
/// so cutoff linking never invents pages for schemas the tree does not render.
pub(super) fn rendered_endpoint_schemas(endpoint: &Endpoint) -> Vec<&Schema> {
    let mut schemas = Vec::new();
    if let Some(schema) = request_body_schema(endpoint) {
        schemas.push(schema);
    }
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
    collect_schema_rows(
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
    super::estimate_tokens(&buffer)
}

/// Estimated tokens of one inline expansion of `reference` at a use site (the
/// field rows the expansion renders in place of a single link row), measured
/// with the schema rendered fully inline under the same configuration, cycle
/// guards included. Used by the `--costs` amplification model.
pub(crate) fn inline_expansion_tokens(
    doc: &ApiDocumentation,
    config: &DocConfig,
    reference: &str,
) -> usize {
    let Some(resolved) = resolve_schema_reference(reference, doc) else {
        return 0;
    };
    let mut ctx = SchemaContext::configured(doc, config, false);
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
    super::estimate_tokens(&buffer)
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
        let command = ctx.retrieval(schema.reference.as_deref());
        description.push_str(&format!(
            "; Omitted nested expansion at schema depth {depth}. Retrieve full detail: {command}"
        ));
        ctx.omissions.push(format!(
            "vimanam: omitted nested schema expansion at depth {depth} for {field}; retrieve full detail: {command}"
        ));
    }
    rows.push(SchemaRow {
        field: field.to_string(),
        type_name: ctx.cutoff_type_name(schema, depth),
        required: required_to_string(required).to_string(),
        description,
    });
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

fn collect_schema_rows(
    schema: &Schema,
    field: &str,
    required: Option<bool>,
    rows: &mut Vec<SchemaRow>,
    ref_stack: &mut Vec<String>,
    depth: usize,
    ctx: &mut SchemaContext,
) {
    const MAX_DEPTH: usize = 24;

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
            rows.push(SchemaRow {
                field: field.to_string(),
                type_name: format!("ref {}", short_schema_reference(reference)),
                required: required_to_string(required).to_string(),
                description: "Cycle detected while expanding schema reference".to_string(),
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
                collect_schema_rows(resolved, field, required, rows, ref_stack, depth + 1, ctx);
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
            let description = if ctx.selected {
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
            &format!("{field}.*"),
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
                let variant_field = format!("{field}.{label}[{source_index}]");
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
}
