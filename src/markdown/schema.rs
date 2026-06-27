//! Renders request/response schemas as nested field tables at `--detail full
//! --include-schemas`, resolving `$ref`s (with cycle detection).

use std::collections::HashSet;
use std::io::Write;

use anyhow::Result;

use crate::models::{ApiDocumentation, Response, Schema};
use crate::utils::{clean_for_id, decode_json_pointer_token};

#[derive(Debug)]
struct SchemaRow {
    field: String,
    type_name: String,
    required: String,
    description: String,
}

/// Context for schema expansion with memoization.
///
/// This struct holds all the state needed for schema expansion, including:
/// - Track expanded schema references to avoid re-expansion
/// - Manage anchor generation for cross-references
/// - Store configuration (depth limit, cycle detection)
/// - Collect rows for the current table
pub struct SchemaContext<'a> {
    /// Document being rendered (for resolving $refs)
    doc: &'a ApiDocumentation,
    
    /// Set of already-expanded schema references
    expanded_refs: HashSet<String>,
    
    /// Deferred schemas to render at the end of the document
    deferred_schemas: Vec<(String, String, Schema)>, // (anchor, name, schema)
}

impl<'a> SchemaContext<'a> {
    /// Create a new SchemaContext for document-level memoization
    pub fn new(doc: &'a ApiDocumentation) -> Self {
        Self {
            doc,
            expanded_refs: HashSet::new(),
            deferred_schemas: Vec::new(),
        }
    }
    
    /// Check if a reference has already been expanded
    pub fn is_expanded(&self, reference: &str) -> bool {
        self.expanded_refs.contains(reference)
    }
    
    /// Mark a reference as expanded and add to deferred schemas
    pub fn mark_expanded(&mut self, reference: String, schema: &Schema) {
        if self.expanded_refs.contains(&reference) {
            return; // Already expanded
        }
        
        self.expanded_refs.insert(reference.clone());
        
        // Generate anchor and name for deferred rendering
        let name = reference.rsplit('/').next().unwrap_or(&reference);
        let clean_name = decode_json_pointer_token(name);
        let anchor = format!("schema-{}", clean_for_id(&clean_name));
        
        // Store the schema for later rendering
        self.deferred_schemas.push((anchor, clean_name, schema.clone()));
    }
    
    /// Generate a unique anchor for a schema reference
    pub fn anchor_for_ref(&self, reference: &str) -> String {
        let name = reference.rsplit('/').next().unwrap_or(reference);
        let clean_name = decode_json_pointer_token(name);
        format!("schema-{}", clean_for_id(&clean_name))
    }
    
    /// Generate a display name for a schema reference
    pub fn display_name_for_ref(&self, reference: &str) -> String {
        let name = reference.rsplit('/').next().unwrap_or(reference);
        decode_json_pointer_token(name)
    }
    
    /// Get the deferred schemas for rendering
    pub fn take_deferred_schemas(&mut self) -> Vec<(String, String, Schema)> {
        std::mem::take(&mut self.deferred_schemas)
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

/// Writes a Markdown table of the fields of `schema`, expanding nested objects,
/// arrays, and `$ref`s. `root_label` names the top-level row (e.g. `request`).
///
/// This is the original function without memoization, kept for backward compatibility.
pub(super) fn write_schema_table<W: Write>(
    writer: &mut W,
    schema: &Schema,
    doc: &ApiDocumentation,
    root_label: &str,
) -> Result<()> {
    let mut rows = Vec::new();
    let mut ref_stack = Vec::new();
    collect_schema_rows(schema, doc, root_label, None, &mut rows, &mut ref_stack, 0);

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

/// Writes a Markdown table of the fields of `schema` with memoization support.
/// Uses the provided context to track and avoid re-expanding shared schemas.
/// `root_label` names the top-level row (e.g. `request`).
pub(super) fn write_schema_table_with_context<W: Write>(
    writer: &mut W,
    schema: &Schema,
    doc: &ApiDocumentation,
    root_label: &str,
    context: &mut SchemaContext,
) -> Result<()> {
    let mut rows = Vec::new();
    let mut ref_stack = Vec::new();
    
    collect_schema_rows_with_memo(
        schema, doc, root_label, None, 
        &mut rows, &mut ref_stack, 0, context
    );

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

/// Renders all deferred schemas at the end of the document.
/// This should be called once at the end of the document to render all
/// the schema definitions that were referenced via links.
pub(super) fn render_deferred_schemas<W: Write>(
    writer: &mut W,
    context: &mut SchemaContext,
) -> Result<()> {
    let deferred_schemas = context.take_deferred_schemas();
    
    if deferred_schemas.is_empty() {
        return Ok(());
    }

    writeln!(writer, "\n---\n")?;
    writeln!(writer, "## Schema Definitions\n")?;

    for (anchor, name, schema) in deferred_schemas {
        writeln!(writer, "### {} {{#{}}}", name, anchor)?;
        writeln!(writer)?;
        
        // Render this schema's table
        let mut rows = Vec::new();
        let mut ref_stack = Vec::new();
        
        // Create a temporary context for this schema to avoid cycles
        let mut temp_context = SchemaContext::new(context.doc);
        temp_context.expanded_refs = context.expanded_refs.clone();
        
        collect_schema_rows_with_memo(
            &schema, context.doc, &name, None,
            &mut rows, &mut ref_stack, 0, &mut temp_context
        );
        
        if rows.is_empty() {
            writeln!(writer, "*No schema fields available*")?;
        } else {
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
        }
        writeln!(writer)?;
    }

    Ok(())
}

fn collect_schema_rows(
    schema: &Schema,
    doc: &ApiDocumentation,
    field: &str,
    required: Option<bool>,
    rows: &mut Vec<SchemaRow>,
    ref_stack: &mut Vec<String>,
    depth: usize,
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
            collect_schema_rows(resolved, doc, field, required, rows, ref_stack, depth + 1);
            ref_stack.pop();
            return;
        }

        rows.push(SchemaRow {
            field: field.to_string(),
            type_name: format!("ref {}", short_schema_reference(reference)),
            required: required_to_string(required).to_string(),
            description: format!("Unresolved schema reference: {}", reference),
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
                doc,
                &child_field,
                Some(required_fields.contains(name.as_str())),
                rows,
                ref_stack,
                depth + 1,
            );
        }
    }

    if let Some(items) = &schema.items {
        let item_field = format!("{}[]", field);
        collect_schema_rows(items, doc, &item_field, None, rows, ref_stack, depth + 1);
    }

    if let Some(all_of) = &schema.all_of {
        for (index, variant) in all_of.iter().enumerate() {
            let variant_field = format!("{}.allOf[{}]", field, index);
            collect_schema_rows(
                variant,
                doc,
                &variant_field,
                required,
                rows,
                ref_stack,
                depth + 1,
            );
        }
    }

    if let Some(one_of) = &schema.one_of {
        for (index, variant) in one_of.iter().enumerate() {
            let variant_field = format!("{}.oneOf[{}]", field, index);
            collect_schema_rows(
                variant,
                doc,
                &variant_field,
                required,
                rows,
                ref_stack,
                depth + 1,
            );
        }
    }

    if let Some(any_of) = &schema.any_of {
        for (index, variant) in any_of.iter().enumerate() {
            let variant_field = format!("{}.anyOf[{}]", field, index);
            collect_schema_rows(
                variant,
                doc,
                &variant_field,
                required,
                rows,
                ref_stack,
                depth + 1,
            );
        }
    }
}

fn collect_schema_rows_with_memo(
    schema: &Schema,
    doc: &ApiDocumentation,
    field: &str,
    required: Option<bool>,
    rows: &mut Vec<SchemaRow>,
    ref_stack: &mut Vec<String>,
    depth: usize,
    context: &mut SchemaContext,
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
        if ref_stack.contains(reference) {
            // Cycle detected - create a link to the schema if it's been expanded
            let anchor = context.anchor_for_ref(reference);
            let display_name = context.display_name_for_ref(reference);
            
            if context.is_expanded(reference) {
                rows.push(SchemaRow {
                    field: field.to_string(),
                    type_name: format!("[{}](#{})", display_name, anchor),
                    required: required_to_string(required).to_string(),
                    description: format!("See [{}](#{})", display_name, anchor),
                });
            } else {
                rows.push(SchemaRow {
                    field: field.to_string(),
                    type_name: format!("ref {}", display_name),
                    required: required_to_string(required).to_string(),
                    description: "Cycle detected while expanding schema reference".to_string(),
                });
            }
            return;
        }

        // Check if this schema has already been expanded
        if context.is_expanded(reference) {
            let anchor = context.anchor_for_ref(reference);
            let display_name = context.display_name_for_ref(reference);
            
            rows.push(SchemaRow {
                field: field.to_string(),
                type_name: format!("[{}](#{})", display_name, anchor),
                required: required_to_string(required).to_string(),
                description: format!("See [{}](#{})", display_name, anchor),
            });
            return;
        }

        if let Some(resolved) = resolve_schema_reference(reference, doc) {
            // Mark this reference as expanded before recursing
            context.mark_expanded(reference.clone(), resolved);
            
            ref_stack.push(reference.clone());
            collect_schema_rows_with_memo(
                resolved, doc, field, required, rows, ref_stack, depth + 1, context
            );
            ref_stack.pop();
            return;
        }

        rows.push(SchemaRow {
            field: field.to_string(),
            type_name: format!("ref {}", context.display_name_for_ref(reference)),
            required: required_to_string(required).to_string(),
            description: format!("Unresolved schema reference: {}", reference),
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
            collect_schema_rows_with_memo(
                child_schema,
                doc,
                &child_field,
                Some(required_fields.contains(name.as_str())),
                rows,
                ref_stack,
                depth + 1,
                context,
            );
        }
    }

    if let Some(items) = &schema.items {
        let item_field = format!("{}[]", field);
        collect_schema_rows_with_memo(items, doc, &item_field, None, rows, ref_stack, depth + 1, context);
    }

    if let Some(all_of) = &schema.all_of {
        for (index, variant) in all_of.iter().enumerate() {
            let variant_field = format!("{}.allOf[{}]", field, index);
            collect_schema_rows_with_memo(
                variant, doc, &variant_field, required, rows, ref_stack, depth + 1, context
            );
        }
    }

    if let Some(one_of) = &schema.one_of {
        for (index, variant) in one_of.iter().enumerate() {
            let variant_field = format!("{}.oneOf[{}]", field, index);
            collect_schema_rows_with_memo(
                variant, doc, &variant_field, required, rows, ref_stack, depth + 1, context
            );
        }
    }

    if let Some(any_of) = &schema.any_of {
        for (index, variant) in any_of.iter().enumerate() {
            let variant_field = format!("{}.anyOf[{}]", field, index);
            collect_schema_rows_with_memo(
                variant, doc, &variant_field, required, rows, ref_stack, depth + 1, context
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
