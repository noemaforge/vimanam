//! Renders request/response schemas as nested field tables at `--detail full
//! --include-schemas`, resolving `$ref`s (with cycle detection).

use std::collections::HashSet;
use std::io::Write;

use anyhow::Result;

use crate::models::{ApiDocumentation, Response, Schema};
use crate::utils::decode_json_pointer_token;

#[derive(Debug)]
struct SchemaRow {
    field: String,
    type_name: String,
    required: String,
    description: String,
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
