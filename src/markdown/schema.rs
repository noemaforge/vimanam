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

use crate::models::{ApiDocumentation, Response, Schema};
use crate::utils::{clean_for_id, decode_json_pointer_token, resolve_schema_reference};

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
    /// Split pages link definitions to separate files, never local anchors.
    external: bool,
}

impl<'a> SchemaContext<'a> {
    pub(super) fn new(doc: &'a ApiDocumentation, inline: bool) -> Self {
        Self {
            doc,
            inline,
            anchors: IndexMap::new(),
            used_anchors: HashSet::new(),
            external: false,
        }
    }

    pub(super) fn external(doc: &'a ApiDocumentation) -> Self {
        let mut ctx = Self::new(doc, false);
        ctx.external = true;
        ctx
    }

    pub(super) fn references(&self) -> impl Iterator<Item = &String> {
        self.anchors.keys()
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
        // #74: escape_table_cell now returns impl Display — no intermediate String.
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

    // #75: flatten the four $ref cases into a single match with if-let guards,
    // reducing nesting depth by one level throughout.
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
                ref_stack.push(reference.clone());
                collect_schema_rows(resolved, field, required, rows, ref_stack, depth + 1, ctx);
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
            let description = resolved
                .description
                .clone()
                .unwrap_or_else(|| "-".to_string());
            let anchor = ctx.register(reference);
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
        // #74: schema_type_label returns impl Display; .to_string() materialises
        // it once into the SchemaRow, which already owns a String.
        type_name: schema_type_label(schema).to_string(),
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

pub(super) fn short_schema_reference(reference: &str) -> String {
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
