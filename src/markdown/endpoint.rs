//! Renders a single endpoint section, branching on the configured detail level.

use std::io::Write;

use anyhow::Result;

use crate::models::{ApiDocumentation, DetailLevel, DocConfig, Endpoint};
use crate::utils::{clean_for_id, extract_content_type};

use super::examples::write_examples;
use super::schema::{response_schema, write_schema_table};

/// Writes a single endpoint section; the amount of detail depends on `config.detail_level`.
pub(super) fn write_endpoint<W: Write>(
    writer: &mut W,
    endpoint: &Endpoint,
    doc: &ApiDocumentation,
    config: &DocConfig,
    include_heading: bool,
) -> Result<()> {
    let title = get_short_title(endpoint);

    if include_heading {
        let anchor = clean_for_id(&title);
        writeln!(writer, "### {} {{#{}}}", title, anchor)?;
    } else {
        writeln!(writer, "**{}**", title)?;
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
                // Skip non-required parameters if required_only is enabled
                if let Some(required) = param.required {
                    if !required && config.required_only {
                        continue;
                    }
                }

                let required_str = if let Some(req) = param.required {
                    if req {
                        "Yes"
                    } else {
                        "No"
                    }
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

        // Add schemas if configured
        if config.include_schemas && config.detail_level == DetailLevel::Full {
            writeln!(writer, "\n#### Request Schema")?;

            // Find a body parameter with schema
            let body_param = endpoint
                .parameters
                .iter()
                .find(|p| p.parameter_in == "body" && p.schema.is_some());

            if let Some(param) = body_param {
                if let Some(schema) = &param.schema {
                    write_schema_table(writer, schema, doc, "request")?;
                } else {
                    writeln!(writer, "*No request schema available*")?;
                }
            } else {
                writeln!(writer, "*No request schema available*")?;
            }

            writeln!(writer, "\n#### Response Schema")?;
            if let Some((_, response)) = endpoint
                .responses
                .iter()
                .find(|(code, _)| code.starts_with('2'))
            {
                if let Some(schema) = response_schema(response) {
                    write_schema_table(writer, schema, doc, "response")?;
                } else {
                    writeln!(writer, "*No response schema available*")?;
                }
            } else {
                writeln!(writer, "*No success response schema available*")?;
            }
        }

        // Add examples if configured
        if config.include_examples && config.detail_level == DetailLevel::Full {
            write_examples(writer, endpoint, doc)?;
        }
    }

    writeln!(writer)?; // End with a blank line
    Ok(())
}

/// Returns a short endpoint title: operation ID, else a name derived from the
/// summary, else `METHOD /path`.
pub(super) fn get_short_title(endpoint: &Endpoint) -> String {
    if let Some(operation_id) = &endpoint.operation_id {
        // If we have an operation ID, use it
        return operation_id.clone();
    } else if let Some(summary) = &endpoint.summary {
        // If there's a summary, try to extract the operation name (first word or camelCase part)
        if let Some(first_word) = summary.split_whitespace().next() {
            if first_word.chars().any(|c| c.is_uppercase()) {
                // This is likely a camelCase operation name
                return first_word.to_string();
            }
        }
        // If no good first word, just use the whole summary
        return summary.clone();
    }

    // Fallback to method and path
    format!("{} {}", endpoint.method, endpoint.path)
}
