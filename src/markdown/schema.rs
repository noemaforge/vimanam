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
use std::io::Write;

use anyhow::Result;
use indexmap::IndexMap;

use crate::models::{ApiDocumentation, Response, Schema};
use crate::utils::{clean_for_id, decode_json_pointer_token};

#[derive(Debug)]
struct SchemaRow {
    field: String,
    type_name: String,
    required: String,
    description: String,
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
}

impl<'a> SchemaContext<'a> {
    pub(super) fn new(doc: &'a ApiDocumentation, inline: bool) -> Self {
        Self {
            doc,
            inline,
            anchors: IndexMap::new(),
            used_anchors: HashSet::new(),
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
pub(super) fn response_schema(response: &Response) -> Option<&Schema> {
    if let Some(schema) = &response.schema {
        return Some(schema);
    }

    response
        .content
        .as_ref()
        .and_then(|content| content.values().find_map(|media| media.schema.as_ref()))
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
    let mut rows = Vec::new();
    let mut ref_stack = Vec::new();
    collect_schema_rows(schema, root_label, None, &mut rows, &mut ref_stack, 0, ctx);
    write_rows(writer, &rows)
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
        writeln!(writer, "### {} {{#{}}}", name, anchor)?;

        let mut rows = Vec::new();
        let mut ref_stack = Vec::new();
        match resolve_schema_reference(&reference, doc) {
            Some(resolved) => {
                collect_schema_rows(resolved, &name, None, &mut rows, &mut ref_stack, 0, ctx)
            }
            None => rows.push(SchemaRow {
                field: name.clone(),
                type_name: "unknown".to_string(),
                required: "-".to_string(),
                description: format!("Unresolved schema reference: {reference}"),
            }),
        }

        write_rows(writer, &rows)?;
        writeln!(writer)?;
    }

    Ok(())
}

fn write_rows<W: Write>(writer: &mut W, rows: &[SchemaRow]) -> Result<()> {
    if rows.is_empty() {
        writeln!(writer, "*No schema fields available*")?;
        return Ok(());
    }

    writeln!(writer, "| Field | Type | Required | Description |")?;
    writeln!(writer, "|------|------|---------:|-------------|")?;
    for row in rows {
        writeln!(
            writer,
            "| `{}` | {} | {} | {} |",
            row.field,
            escape_table_cell(&row.type_name),
            row.required,
            escape_table_cell(&row.description)
        )?;
    }

    Ok(())
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

    if depth >= MAX_DEPTH {
        rows.push(SchemaRow {
            field: field.to_string(),
            type_name: "truncated".to_string(),
            required: required_to_string(required).to_string(),
            description: "Maximum schema depth reached; nested expansion stopped".to_string(),
        });
        return;
    }

    if let Some(reference) = &schema.reference {
        let doc = ctx.doc;

        if ctx.inline {
            // Fully self-contained mode: expand inline, guarding against cycles
            // on the current expansion chain.
            if ref_stack.contains(reference) {
                rows.push(SchemaRow {
                    field: field.to_string(),
                    type_name: format!("ref {}", short_schema_reference(reference)),
                    required: required_to_string(required).to_string(),
                    description: "Cycle detected while expanding schema reference".to_string(),
                });
                return;
            }

            if let Some(resolved) = resolve_schema_reference(reference, doc) {
                ref_stack.push(reference.clone());
                collect_schema_rows(resolved, field, required, rows, ref_stack, depth + 1, ctx);
                ref_stack.pop();
                return;
            }
        } else if let Some(resolved) = resolve_schema_reference(reference, doc) {
            // Linked mode: emit one row pointing at the shared definition and
            // register it for rendering. Self- and mutual references resolve to
            // a link, so there is no cycle to guard against.
            let name = short_schema_reference(reference);
            let description = resolved
                .description
                .clone()
                .unwrap_or_else(|| "-".to_string());
            let anchor = ctx.register(reference);
            rows.push(SchemaRow {
                field: field.to_string(),
                type_name: format!("[{name}](#{anchor})"),
                required: required_to_string(required).to_string(),
                description,
            });
            return;
        }

        // Unresolved reference (either mode).
        rows.push(SchemaRow {
            field: field.to_string(),
            type_name: format!("ref {}", short_schema_reference(reference)),
            required: required_to_string(required).to_string(),
            description: format!("Unresolved schema reference: {reference}"),
        });
        return;
    }

    let description = schema.description.as_deref().unwrap_or("-");
    rows.push(SchemaRow {
        field: field.to_string(),
        type_name: schema_type_label(schema),
        required: required_to_string(required).to_string(),
        description: description.to_string(),
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

    if let Some(all_of) = &schema.all_of {
        for (index, variant) in all_of.iter().enumerate() {
            let variant_field = format!("{}.allOf[{}]", field, index);
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

    if let Some(one_of) = &schema.one_of {
        for (index, variant) in one_of.iter().enumerate() {
            let variant_field = format!("{}.oneOf[{}]", field, index);
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

    if let Some(any_of) = &schema.any_of {
        for (index, variant) in any_of.iter().enumerate() {
            let variant_field = format!("{}.anyOf[{}]", field, index);
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

fn resolve_schema_reference<'a>(reference: &str, doc: &'a ApiDocumentation) -> Option<&'a Schema> {
    if let Some(name) = reference.strip_prefix("#/components/schemas/") {
        return doc.schemas.get(&decode_json_pointer_token(name));
    }

    if let Some(name) = reference.strip_prefix("#/definitions/") {
        return doc.schemas.get(&decode_json_pointer_token(name));
    }

    None
}

fn schema_type_label(schema: &Schema) -> String {
    let mut label = if let Some(schema_type) = &schema.schema_type {
        if schema_type == "array" {
            let item_label = schema
                .items
                .as_deref()
                .map(schema_type_hint)
                .unwrap_or_else(|| "unknown".to_string());
            format!("array<{}>", item_label)
        } else if let Some(format) = &schema.format {
            format!("{}({})", schema_type, format)
        } else {
            schema_type.clone()
        }
    } else if schema.properties.is_some() {
        "object".to_string()
    } else if schema.items.is_some() {
        let item_label = schema
            .items
            .as_deref()
            .map(schema_type_hint)
            .unwrap_or_else(|| "unknown".to_string());
        format!("array<{}>", item_label)
    } else if schema.all_of.as_ref().is_some_and(|v| !v.is_empty()) {
        "allOf".to_string()
    } else if schema.one_of.as_ref().is_some_and(|v| !v.is_empty()) {
        "oneOf".to_string()
    } else if schema.any_of.as_ref().is_some_and(|v| !v.is_empty()) {
        "anyOf".to_string()
    } else if let Some(enum_values) = &schema.enum_values {
        format!("enum[{}]", enum_values.len())
    } else {
        "unknown".to_string()
    };

    if schema.nullable.unwrap_or(false) {
        label.push_str(" | null");
    }

    label
}

fn schema_type_hint(schema: &Schema) -> String {
    if let Some(reference) = &schema.reference {
        return format!("ref {}", short_schema_reference(reference));
    }

    if let Some(schema_type) = &schema.schema_type {
        return schema_type.clone();
    }

    if schema.properties.is_some() {
        return "object".to_string();
    }

    if schema.items.is_some() {
        return "array".to_string();
    }

    "unknown".to_string()
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

fn short_schema_reference(reference: &str) -> String {
    reference
        .rsplit('/')
        .next()
        .map(decode_json_pointer_token)
        .unwrap_or_else(|| reference.to_string())
}

fn escape_table_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', "<br/>")
}
