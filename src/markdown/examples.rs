//! Renders request/response examples as fenced JSON blocks at `--detail full
//! --include-examples`, resolving `$ref`s into `components/examples`.

use std::io::Write;

use anyhow::Result;
use indexmap::IndexMap;

use crate::models::{ApiDocumentation, Endpoint, Example};
use crate::utils::decode_json_pointer_token;

/// A single rendered example block: either an inline JSON value or a pointer to
/// an `externalValue` URL.
enum ExampleBlock {
    Json {
        label: String,
        value: serde_json::Value,
    },
    External {
        label: String,
        url: String,
    },
}

/// Writes the `#### Examples` section for an endpoint, pulling examples from the
/// request body and from each response's media types (resolving `$ref`s into
/// `components/examples`) and rendering them as fenced JSON blocks.
pub(super) fn write_examples<W: Write>(
    writer: &mut W,
    endpoint: &Endpoint,
    doc: &ApiDocumentation,
) -> Result<()> {
    let mut blocks: Vec<ExampleBlock> = Vec::new();

    // Request body examples ride on the synthetic `body` parameter.
    if let Some(body) = endpoint
        .parameters
        .iter()
        .find(|p| p.parameter_in == "body")
    {
        collect_examples("Request", &body.example, &body.examples, doc, &mut blocks);
    }

    // Response examples, per status code and media type.
    for (code, response) in &endpoint.responses {
        if let Some(content) = &response.content {
            for (content_type, media) in content {
                let prefix = format!("Response `{}` (`{}`)", code, content_type);
                collect_examples(&prefix, &media.example, &media.examples, doc, &mut blocks);
            }
        }
    }

    writeln!(writer, "\n#### Examples")?;

    if blocks.is_empty() {
        writeln!(writer, "*No examples available*")?;
        return Ok(());
    }

    for block in blocks {
        match block {
            ExampleBlock::Json { label, value } => {
                writeln!(writer, "\n**{}**\n", label)?;
                writeln!(writer, "```json")?;
                let pretty =
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
                writeln!(writer, "{}", pretty)?;
                writeln!(writer, "```")?;
            }
            ExampleBlock::External { label, url } => {
                writeln!(writer, "\n**{}**\n", label)?;
                writeln!(writer, "External value: <{}>", url)?;
            }
        }
    }

    Ok(())
}

/// Appends example blocks for one media type (a single `example` and/or a named
/// `examples` map) under `label_prefix`, resolving any `$ref` entries.
fn collect_examples(
    label_prefix: &str,
    example: &Option<serde_json::Value>,
    examples: &Option<IndexMap<String, Example>>,
    doc: &ApiDocumentation,
    blocks: &mut Vec<ExampleBlock>,
) {
    if let Some(value) = example {
        blocks.push(ExampleBlock::Json {
            label: label_prefix.to_string(),
            value: value.clone(),
        });
    }

    if let Some(named) = examples {
        for (name, raw) in named {
            let Some(resolved) = resolve_example(raw, doc) else {
                continue;
            };
            let label = format!("{} — {}", label_prefix, name);

            if let Some(value) = &resolved.value {
                blocks.push(ExampleBlock::Json {
                    label,
                    value: value.clone(),
                });
            } else if let Some(url) = &resolved.external_value {
                blocks.push(ExampleBlock::External {
                    label,
                    url: url.clone(),
                });
            }
        }
    }
}

/// Resolves an [`Example`] that may be a `$ref` into `components/examples`.
/// Returns `None` if the reference can't be resolved.
fn resolve_example<'a>(example: &'a Example, doc: &'a ApiDocumentation) -> Option<&'a Example> {
    if let Some(reference) = &example.reference {
        if let Some(name) = reference.strip_prefix("#/components/examples/") {
            return doc.examples.get(&decode_json_pointer_token(name));
        }
        return None;
    }

    Some(example)
}
