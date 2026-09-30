//! Markdown generation.
//!
//! [`generate_markdown`] is the entry point. It dispatches on detail level and
//! grouping mode (see [`render`]) and, when `--max-tokens` is set, fits the
//! output to a token budget (see [`generate_within_budget`]).
//!
//! The work is split across submodules: [`views`] renders each grouping mode,
//! [`endpoint`] renders one endpoint, and [`schema`]/[`examples`] render the
//! schema and example sections within an endpoint.

mod endpoint;
mod examples;
mod schema;
pub(crate) mod schema_selection;
pub(crate) mod split;
mod views;

use std::io::Write;

use anyhow::Result;

use crate::models::{ApiDocumentation, DetailLevel, DocConfig, GroupBy};

// The report and stats modules reuse the views' notion of which endpoints and
// services the rendered body covers, so their scope always matches the
// document's.
pub(crate) use views::{removing_filters, service_is_visible, visible_endpoints};
// `diff` compares the response schema the renderer would document.
pub(crate) use schema::response_schema;

use schema::emit_omissions;

/// Renders the documentation to `writer`.
///
/// With `--max-tokens` set, this fits the output to the budget (see
/// [`generate_within_budget`]); otherwise it renders directly at the configured
/// detail level via [`render`].
pub fn generate_markdown<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<()> {
    match config.max_tokens {
        Some(budget) => generate_within_budget(writer, doc, config, budget),
        None => {
            let omissions = render(writer, doc, config)?;
            emit_omissions(&omissions);
            Ok(())
        }
    }
}

/// Renders the documentation to `writer`, dispatching on detail level and grouping mode.
///
/// Returns schema-omission notices collected during this render. Callers that
/// discard the buffer (token-budget trials, `--stats` estimates) must not print
/// them; callers that write the buffer should pass them to [`emit_omissions`].
///
/// Crate-visible so `--stats` can size trial renders without a token budget.
pub(crate) fn render<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<Vec<String>> {
    if schema_selection::active(config) {
        return schema_selection::render(writer, doc, config);
    }
    // For summary level, just generate the TOC
    if config.detail_level == DetailLevel::Summary {
        views::generate_summary(writer, doc, config)?;
        Ok(Vec::new())
    } else {
        // For other detail levels, use the existing grouping logic
        match config.group_by {
            GroupBy::Service => views::generate_by_service(writer, doc, config),
            GroupBy::Method => views::generate_by_method(writer, doc, config),
            GroupBy::Path => views::generate_by_path(writer, doc, config),
            GroupBy::Flat => views::generate_flat(writer, doc, config),
        }
    }
}

/// Detail levels ordered from most to least verbose; the token-budget search
/// steps down this ladder.
const DETAIL_LADDER: [DetailLevel; 4] = [
    DetailLevel::Full,
    DetailLevel::Standard,
    DetailLevel::Basic,
    DetailLevel::Summary,
];

/// Renders the documentation, stepping the detail level down the
/// [`DETAIL_LADDER`] (starting from the configured level) until the estimated
/// token count fits `budget`. If nothing fits, the lowest level is emitted.
/// What happened is reported on stderr so a caller knows the output was trimmed.
fn generate_within_budget<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
    budget: usize,
) -> Result<()> {
    if schema_selection::active(config) {
        let mut buffer = Vec::new();
        let omissions = render(&mut buffer, doc, config)?;
        let tokens = estimate_tokens(&buffer);
        if tokens > budget {
            let notice = format!(
                "> Selected schemas exceed the approximate {budget}-token budget (~{tokens} tokens before this notice). Selection and metadata are preserved; narrow --schema-field or explicitly set --schema-depth.\n\n"
            );
            eprintln!(
                "vimanam: selected schemas exceed approximate {budget}-token budget (~{tokens} tokens); preserving requested selection and metadata"
            );
            writer.write_all(notice.as_bytes())?;
        }
        writer.write_all(&buffer)?;
        emit_omissions(&omissions);
        return Ok(());
    }
    // Only consider levels at or below the one the caller asked for.
    let start = DETAIL_LADDER
        .iter()
        .position(|level| *level == config.detail_level)
        .unwrap_or(0);

    let mut chosen: Option<(DetailLevel, Vec<u8>, usize, Vec<String>)> = None;
    for level in &DETAIL_LADDER[start..] {
        let mut trial_config = config.clone();
        trial_config.detail_level = level.clone();

        let mut buffer = Vec::new();
        let omissions = render(&mut buffer, doc, &trial_config)?;
        let tokens = estimate_tokens(&buffer);

        let fits = tokens <= budget;
        chosen = Some((level.clone(), buffer, tokens, omissions));
        if fits {
            break;
        }
    }

    // The ladder slice is always non-empty, so a candidate is always produced.
    let (level, buffer, tokens, omissions) = chosen.expect("at least one detail level is rendered");

    if level != config.detail_level {
        eprintln!(
            "vimanam: output exceeded the {budget}-token budget at --detail {}; \
             reduced to --detail {} (~{tokens} tokens).",
            detail_level_name(&config.detail_level),
            detail_level_name(&level),
        );
    } else if tokens > budget {
        eprintln!(
            "vimanam: output is ~{tokens} tokens, over the {budget}-token budget; \
             already at the lowest detail level (--detail {}).",
            detail_level_name(&level),
        );
    }

    writer.write_all(&buffer)?;
    emit_omissions(&omissions);
    Ok(())
}

/// Estimates the token count of rendered output with the common chars/4
/// heuristic. Good enough to pick a detail level; a real tokenizer could
/// replace this later. Shared with `--stats`, whose ~TOKENS column must agree
/// with what `--max-tokens` would measure.
pub(crate) fn estimate_tokens(rendered: &[u8]) -> usize {
    String::from_utf8_lossy(rendered)
        .chars()
        .count()
        .div_ceil(4)
}

/// The `--detail` value name for a [`DetailLevel`], for stderr messages.
fn detail_level_name(level: &DetailLevel) -> &'static str {
    match level {
        DetailLevel::Summary => "summary",
        DetailLevel::Basic => "basic",
        DetailLevel::Standard => "standard",
        DetailLevel::Full => "full",
    }
}
