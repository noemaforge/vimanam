//! The `--stats` token-budget dry-run: a per-service table of endpoint counts
//! and estimated token sizes, printed instead of the documentation.
//!
//! [`compute`] derives a data-oriented [`Stats`] from the intermediate
//! representation, and [`write_stats`] renders it as an aligned plain-text
//! table. As with the hygiene report, the struct carries no rendering concerns
//! so other front ends can consume the same numbers.
//!
//! # Scope
//!
//! Rows follow the service-grouped view exactly: one per service the
//! `--service-filter` keeps (see [`service_is_visible`]), in declared (spec)
//! order, skipping services with no visible endpoint. A row's endpoint count is
//! the number of visible endpoints (after `--exclude-deprecated`,
//! `--method-filter` and `--path-filter`) tagged with that service, so a
//! multi-tag operation is counted in each of its services. A row's token
//! estimate is the size of rendering the document with the service filter
//! narrowed to that one service, at the configured detail level, grouping and
//! flags.
//!
//! The TOTAL row counts each visible endpoint once and estimates a single
//! render of the whole filtered document, so it is not necessarily the sum of
//! the rows: shared endpoints, the preamble and the shared "Schema Definitions"
//! section are all counted once there but once per row above.
//!
//! Estimates cover the documentation body only; the spec hygiene report is
//! never part of stats output. When the filters leave no endpoint visible, the
//! table is just the header and an all-zero TOTAL row.

use std::io::Write;

use anyhow::Result;

use crate::markdown::{estimate_tokens, render, service_is_visible, visible_endpoints};
use crate::models::{ApiDocumentation, DocConfig};

/// One row of the table: a service, its visible endpoint count, and the
/// estimated token size of rendering just that service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStats {
    pub name: String,
    pub endpoints: usize,
    pub tokens: usize,
}

/// The full table: per-service rows in render order plus the whole-document
/// totals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub rows: Vec<ServiceStats>,
    /// Distinct visible endpoints; a multi-tag endpoint counts once here even
    /// though it appears in several rows.
    pub total_endpoints: usize,
    /// Estimated tokens of one render of the whole filtered document.
    pub total_tokens: usize,
}

/// Estimates the token size of rendering `doc` under `config` at the
/// configured detail level (no budget stepping: `--stats` conflicts with
/// `--max-tokens`).
fn estimate_render(doc: &ApiDocumentation, config: &DocConfig) -> Result<usize> {
    let mut buffer = Vec::new();
    // Discard omission notices: stats sizes trial renders and must stay silent.
    let _omissions = render(&mut buffer, doc, config)?;
    Ok(estimate_tokens(&buffer))
}

/// Computes the per-service and total statistics (see the module docs for the
/// exact scoping rules).
pub fn compute(doc: &ApiDocumentation, config: &DocConfig) -> Result<Stats> {
    let endpoints = visible_endpoints(doc, config);

    let mut rows = Vec::new();
    // `doc.services` is in declared order, which is the order the
    // service-grouped view renders its sections (it never sorts services).
    for service in &doc.services {
        if !service_is_visible(&service.name, config) {
            continue;
        }

        let count = endpoints
            .iter()
            .filter(|endpoint| endpoint.services.contains(&service.name))
            .count();
        if count == 0 {
            continue;
        }

        // Narrow the service filter to this one service on top of the user's
        // other filters, so the estimate is what `--service-filter <name>`
        // with the same flags would actually produce.
        let mut service_config = config.clone();
        service_config.service_filter = Some(vec![service.name.clone()]);

        rows.push(ServiceStats {
            name: service.name.clone(),
            endpoints: count,
            tokens: estimate_render(doc, &service_config)?,
        });
    }

    // With nothing visible there is no slice to size: the document would be
    // only its preamble, so the total reads as zero rather than that overhead.
    let total_tokens = if endpoints.is_empty() {
        0
    } else {
        estimate_render(doc, config)?
    };

    Ok(Stats {
        rows,
        total_endpoints: endpoints.len(),
        total_tokens,
    })
}

const SERVICE_HEADER: &str = "SERVICE";
const ENDPOINTS_HEADER: &str = "ENDPOINTS";
const TOKENS_HEADER: &str = "~TOKENS";
const TOTAL_LABEL: &str = "TOTAL";
/// Spaces between columns.
const GAP: &str = "   ";

/// The number of characters `value` renders as.
fn digits(value: usize) -> usize {
    value.to_string().len()
}

/// Renders the table as plain text: a header, one line per row, and a TOTAL
/// line, each newline-terminated. The SERVICE column is left-aligned and padded
/// to the widest name; the numeric columns are right-aligned and padded to the
/// wider of their header and their widest value. No blank lines, no Markdown.
pub fn write_stats<W: Write>(writer: &mut W, stats: &Stats) -> Result<()> {
    let name_width = stats
        .rows
        .iter()
        .map(|row| row.name.chars().count())
        .chain([SERVICE_HEADER.len(), TOTAL_LABEL.len()])
        .max()
        .unwrap_or(SERVICE_HEADER.len());
    let endpoints_width = stats
        .rows
        .iter()
        .map(|row| digits(row.endpoints))
        .chain([ENDPOINTS_HEADER.len(), digits(stats.total_endpoints)])
        .max()
        .unwrap_or(ENDPOINTS_HEADER.len());
    let tokens_width = stats
        .rows
        .iter()
        .map(|row| digits(row.tokens))
        .chain([TOKENS_HEADER.len(), digits(stats.total_tokens)])
        .max()
        .unwrap_or(TOKENS_HEADER.len());

    writeln!(
        writer,
        "{SERVICE_HEADER:<name_width$}{GAP}{ENDPOINTS_HEADER:>endpoints_width$}{GAP}{TOKENS_HEADER:>tokens_width$}"
    )?;
    for row in &stats.rows {
        writeln!(
            writer,
            "{:<name_width$}{GAP}{:>endpoints_width$}{GAP}{:>tokens_width$}",
            row.name, row.endpoints, row.tokens
        )?;
    }
    writeln!(
        writer,
        "{TOTAL_LABEL:<name_width$}{GAP}{:>endpoints_width$}{GAP}{:>tokens_width$}",
        stats.total_endpoints, stats.total_tokens
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_to_string(stats: &Stats) -> String {
        let mut buffer = Vec::new();
        write_stats(&mut buffer, stats).unwrap();
        String::from_utf8(buffer).unwrap()
    }

    #[test]
    fn columns_align_to_widest_value() {
        let stats = Stats {
            rows: vec![
                ServiceStats {
                    name: "Findings".into(),
                    endpoints: 42,
                    tokens: 6231,
                },
                ServiceStats {
                    name: "Long Service".into(),
                    endpoints: 7,
                    tokens: 12,
                },
            ],
            total_endpoints: 49,
            total_tokens: 6300,
        };

        assert_eq!(
            render_to_string(&stats),
            "\
SERVICE        ENDPOINTS   ~TOKENS
Findings              42      6231
Long Service           7        12
TOTAL                 49      6300
"
        );
    }

    #[test]
    fn empty_table_is_header_and_zero_total() {
        assert_eq!(
            render_to_string(&Stats::default()),
            "\
SERVICE   ENDPOINTS   ~TOKENS
TOTAL             0         0
"
        );
    }
}
