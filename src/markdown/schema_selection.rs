//! Standalone named-schema and JSON-pointer subtree reads. All validation is
//! performed before the caller creates an output file.
use std::io::Write;

use anyhow::{Context, Result, bail};
use indexmap::IndexMap;

use super::schema::{SchemaContext, render_schema_definitions, write_schema_table};
use crate::models::{AdditionalProperties, ApiDocumentation, DocConfig, Schema};
use crate::utils::resolve_schema_reference;

pub(crate) fn active(config: &DocConfig) -> bool {
    !config.schema_names.is_empty() || !config.schema_fields.is_empty()
}

pub(crate) fn validate(doc: &ApiDocumentation, config: &DocConfig) -> Result<()> {
    if config.schema_depth.is_some()
        && !active(config)
        && (!config.include_schemas || config.detail_level != crate::models::DetailLevel::Full)
    {
        bail!(
            "--schema-depth requires --detail full --include-schemas, or --schema/--schema-field"
        );
    }
    if config.schema_depth.is_some_and(|depth| depth > 24) {
        bail!("--schema-depth supports 0..=24 (the schema recursion safety limit)");
    }
    selections(doc, config)?;
    Ok(())
}

fn selections(doc: &ApiDocumentation, config: &DocConfig) -> Result<Vec<(String, Schema)>> {
    let mut result = Vec::new();
    for name in &config.schema_names {
        let schema = doc
            .schemas
            .get(name)
            .with_context(|| format!("Unknown schema '{name}'"))?;
        if !result.iter().any(|(old, _)| old == name) {
            result.push((name.clone(), schema.clone()));
        }
    }
    for selector in &config.schema_fields {
        let (name, pointer) = selector.split_once('#').with_context(|| {
            format!("Invalid --schema-field '{selector}': expected NAME#JSON_POINTER")
        })?;
        let schema = doc
            .schemas
            .get(name)
            .with_context(|| format!("Unknown schema '{name}' in --schema-field '{selector}'"))?;
        if !pointer.is_empty() && !pointer.starts_with('/') {
            bail!(
                "Invalid JSON pointer in --schema-field '{selector}': expected '/' or empty pointer"
            );
        }
        let tokens = if pointer.is_empty() {
            Vec::new()
        } else {
            pointer[1..]
                .split('/')
                .map(decode_token)
                .collect::<Result<Vec<_>>>()?
        };
        let selected = prune(schema, &tokens, doc, &mut Vec::new())
            .with_context(|| format!("Unmatched --schema-field '{selector}'"))?;
        if !result.iter().any(|(old, _)| old == selector) {
            result.push((selector.clone(), selected));
        }
    }
    Ok(result)
}

fn decode_token(token: &str) -> Result<String> {
    let mut result = String::new();
    let mut chars = token.chars();
    while let Some(ch) = chars.next() {
        if ch != '~' {
            result.push(ch);
            continue;
        }
        result.push(match chars.next() {
            Some('0') => '~',
            Some('1') => '/',
            _ => bail!("Invalid JSON pointer escape in '{token}': use ~0 and ~1"),
        });
    }
    Ok(result)
}

fn prune(
    schema: &Schema,
    tokens: &[String],
    doc: &ApiDocumentation,
    stack: &mut Vec<String>,
) -> Result<Schema> {
    if tokens.is_empty() {
        return Ok(schema.clone());
    }
    if let Some(reference) = &schema.reference {
        let visit = format!("{reference}:{}", tokens.len());
        if stack.contains(&visit) {
            bail!("Reference cycle encountered while resolving pointer at '{reference}'");
        }
        let resolved = resolve_schema_reference(reference, doc)
            .with_context(|| format!("Unresolved reference '{reference}'"))?;
        stack.push(visit);
        let mut selected = prune(resolved, tokens, doc, stack)?;
        stack.pop();
        if schema.description.is_some() {
            selected.description = schema.description.clone();
        }
        return Ok(selected);
    }
    let mut selected = schema.clone();
    selected.properties = None;
    selected.items = None;
    selected.all_of = None;
    selected.one_of = None;
    selected.any_of = None;
    selected.additional_properties = None;
    match tokens[0].as_str() {
        "properties" if tokens.len() >= 2 => {
            let name = &tokens[1];
            let child = schema
                .properties
                .as_ref()
                .and_then(|props| props.get(name))
                .with_context(|| format!("No schema property '{name}'"))?;
            let mut properties = IndexMap::new();
            properties.insert(name.clone(), prune(child, &tokens[2..], doc, stack)?);
            selected.properties = Some(properties);
            selected.required = schema.required.as_ref().map(|required| {
                required
                    .iter()
                    .filter(|field| *field == name)
                    .cloned()
                    .collect()
            });
        }
        "items" => {
            let child = schema.items.as_deref().context("Schema has no items")?;
            selected.items = Some(Box::new(prune(child, &tokens[1..], doc, stack)?));
        }
        "additionalProperties" => {
            let Some(AdditionalProperties::Schema(child)) = &schema.additional_properties else {
                bail!("Schema has no additionalProperties schema");
            };
            selected.additional_properties = Some(AdditionalProperties::Schema(Box::new(prune(
                child,
                &tokens[1..],
                doc,
                stack,
            )?)));
        }
        "allOf" | "oneOf" | "anyOf" if tokens.len() >= 2 => {
            let index: usize = tokens[1]
                .parse()
                .context("Composition selector requires an array index")?;
            if tokens[1] != index.to_string() {
                bail!("Composition array index must be canonical decimal");
            }
            let variants = match tokens[0].as_str() {
                "allOf" => &schema.all_of,
                "oneOf" => &schema.one_of,
                _ => &schema.any_of,
            };
            let child = variants
                .as_ref()
                .and_then(|v| v.get(index))
                .context("Composition index is out of range")?;
            // Keep only the selected variant, with its source index in an internal annotation.
            let mut child = prune(child, &tokens[2..], doc, stack)?;
            child
                .extensions
                .insert("x-vimanam-selected-index".into(), serde_json::json!(index));
            let selected_variants = Some(vec![child]);
            match tokens[0].as_str() {
                "allOf" => selected.all_of = selected_variants,
                "oneOf" => selected.one_of = selected_variants,
                _ => selected.any_of = selected_variants,
            }
        }
        _ => bail!(
            "Pointer must select a schema subtree through properties/NAME, items, additionalProperties or composition/INDEX"
        ),
    }
    Ok(selected)
}

pub(super) fn render<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<Vec<String>> {
    writeln!(
        writer,
        "# {} — Selected schemas\n",
        super::split::escape(&doc.title)
    )?;
    writeln!(
        writer,
        "> Explicit schema read. Field selectors omit unselected sibling subtrees and their references; named-schema selectors retain complete schemas. Absence here does not mean absence from the spec.\n"
    )?;
    if config.operation_selector.is_some() {
        writeln!(writer, "## Operation context\n")?;
        for endpoint in super::visible_endpoints(doc, config) {
            writeln!(writer, "- {} {}", endpoint.method, endpoint.path)?;
        }
        writeln!(
            writer,
            "\nOperation selectors provide context; named schemas remain independently readable.\n"
        )?;
    }
    let mut ctx = SchemaContext::configured(doc, config, false);
    for (label, selected) in selections(doc, config)? {
        let name = label.split('#').next().expect("schema name");
        ctx.set_current_schema(name);
        ctx.set_active_selector(if label.contains('#') {
            Some(label.clone())
        } else {
            None
        });
        // Explicit ancestor paths and their leaf metadata must survive even a
        // zero expansion allowance; depth limits govern expansion beyond them.
        let protected_depth = label
            .split_once('#')
            .map(|(_, pointer)| {
                let tokens: Vec<_> = pointer.trim_start_matches('/').split('/').collect();
                let mut edges = 0;
                let mut position = 0;
                while position < tokens.len() {
                    edges += 1;
                    position +=
                        if ["properties", "allOf", "oneOf", "anyOf"].contains(&tokens[position]) {
                            2
                        } else {
                            1
                        };
                }
                if pointer.is_empty() { 0 } else { edges }
            })
            .unwrap_or(0);
        ctx.protect_selection_path(protected_depth);
        writeln!(writer, "## {}\n", super::split::escape(&label))?;
        write_schema_table(writer, &selected, name, &mut ctx)?;
        let command = format!(
            "vimanam {} --schema {} --no-report",
            super::split::quote_shell(config.source_path.as_deref().unwrap_or("spec.json")),
            super::split::quote_shell(name)
        );
        writeln!(
            writer,
            "\nRetrieve the complete named schema without selection/depth limits:\n\n```sh\n{command}\n```\n"
        )?;
    }
    ctx.set_active_selector(None);
    render_schema_definitions(writer, &mut ctx)?;
    Ok(ctx.take_omissions())
}
