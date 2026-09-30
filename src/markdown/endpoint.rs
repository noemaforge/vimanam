//! Renders a single endpoint section, branching on the configured detail level.

use std::io::Write;

use anyhow::Result;

use crate::models::{DetailLevel, DocConfig, Endpoint};
use crate::utils::extract_content_type;

use super::examples::write_examples;
use super::schema::{
    SchemaContext, first_success_response, request_body_schema, response_schema, write_schema_table,
};

/// Writes a single endpoint section; the amount of detail depends on `config.detail_level`.
///
/// `anchor` is the heading's explicit id (the caller computes it so the table of
/// contents and the body agree); when `None`, the endpoint is written as a bold
/// label without a heading anchor. `ctx` carries the document-level schema
/// memoization shared across every endpoint.
pub(super) fn write_endpoint<W: Write>(
    writer: &mut W,
    endpoint: &Endpoint,
    config: &DocConfig,
    anchor: Option<&str>,
    ctx: &mut SchemaContext,
) -> Result<()> {
    let title = get_short_title(endpoint);
    ctx.set_current_operation(Some(format!("{} {}", endpoint.method, endpoint.path)));

    match anchor {
        Some(anchor) => writeln!(writer, "### {} {{#{}}}", title, anchor)?,
        None => writeln!(writer, "**{}**", title)?,
    }

    // Operation line (method + path)
    writeln!(
        writer,
        "**Operation:** {} {}",
        endpoint.method, endpoint.path
    )?;

    // Description/summary only if it exists
    if let Some(description) = &endpoint.description {
        writeln!(writer, "**Description:** {}", description)?;
    } else if let Some(summary) = &endpoint.summary {
        writeln!(writer, "**Description:** {}", summary)?;
    }

    if endpoint.deprecated {
        writeln!(writer, "\n> **Deprecated**: This endpoint is deprecated.")?;
    }

    // Write operation ID if available
    if let Some(operation_id) = &endpoint.operation_id {
        writeln!(writer, "**Operation ID:** `{}`", operation_id)?;
    }

    // Only include detailed information if detail level is not basic
    if config.detail_level != DetailLevel::Basic {
        // Write parameters based on detail level
        if !endpoint.parameters.is_empty() {
            writeln!(writer, "\n#### Parameters")?;

            // More detailed parameter listing
            writeln!(writer, "| Name | In | Required | Description |")?;
            writeln!(writer, "|------|----|---------:|-------------|")?;

            for param in &endpoint.parameters {
                // Skip non-required parameters when --required-only is set,
                // treating an unspecified `required` as not required.
                if config.required_only && !param.required.unwrap_or(false) {
                    continue;
                }

                let required_str = if param.required.unwrap_or(false) {
                    "Yes"
                } else {
                    "No"
                };

                let desc = param.description.as_deref().unwrap_or("-");
                writeln!(
                    writer,
                    "| `{}` | {} | {} | {} |",
                    param.name, param.parameter_in, required_str, desc
                )?;
            }
        }

        // Write responses based on detail level
        writeln!(writer, "\n#### Responses")?;
        writeln!(writer, "| Code | Type | Description |")?;
        writeln!(writer, "|------|------|-------------|")?;

        for (code, response) in &endpoint.responses {
            let desc = response.description.as_deref().unwrap_or("-");
            let content_type = extract_content_type(response).unwrap_or_default();
            writeln!(writer, "| {} | {} | {} |", code, content_type, desc)?;
        }

        // Add schemas if configured. Roots match `rendered_endpoint_schemas`
        // (request body + first 2xx) so pre-discovery and rendering stay aligned.
        if config.include_schemas && config.detail_level == DetailLevel::Full {
            writeln!(writer, "\n#### Request Schema")?;
            if let Some(schema) = request_body_schema(endpoint) {
                write_schema_table(writer, schema, "request", ctx)?;
            } else {
                writeln!(writer, "*No request schema available*")?;
            }

            writeln!(writer, "\n#### Response Schema")?;
            match first_success_response(endpoint) {
                Some((_, response)) => {
                    if let Some(schema) = response_schema(response) {
                        write_schema_table(writer, schema, "response", ctx)?;
                    } else {
                        writeln!(writer, "*No response schema available*")?;
                    }
                }
                None => {
                    writeln!(writer, "*No success response schema available*")?;
                }
            }
        }

        // Add examples if configured
        if config.include_examples && config.detail_level == DetailLevel::Full {
            write_examples(writer, endpoint, ctx.doc())?;
        }
    }

    writeln!(writer)?; // End with a blank line
    ctx.set_current_operation(None);
    Ok(())
}

/// Returns a short endpoint title: operation ID, else a name derived from the
/// summary, else `METHOD /path`.
///
/// #75: restructured from a nested `if let` / `else if let` chain into a
/// `match` with if-let guards to reduce nesting depth while preserving
/// identical behaviour.
pub(super) fn get_short_title(endpoint: &Endpoint) -> String {
    match (&endpoint.operation_id, &endpoint.summary) {
        // Prefer the explicit operation ID.
        (Some(op_id), _) => op_id.clone(),

        // Summary with an apparent camelCase / PascalCase first word — use
        // just that word as the title.
        (None, Some(summary))
            if let Some(first_word) = summary.split_whitespace().next()
                && first_word.chars().any(|c| c.is_uppercase()) =>
        {
            first_word.to_string()
        }

        // Summary present but no good first word — use the whole thing.
        (None, Some(summary)) => summary.clone(),

        // Fallback: method + path.
        (None, None) => format!("{} {}", endpoint.method, endpoint.path),
    }
}
