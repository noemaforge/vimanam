//! Focused Markdown context from a parsed OpenAPI document.
//!
//! [`generate_markdown`] renders the documentation body at the configured
//! detail and grouping. Exact operations, operationIds, named schemas, and
//! schema subtrees are selected through [`DocConfig`]. Invalid selections
//! fail before any bytes are written. A token budget reduces operation detail;
//! explicit schema selections retain their metadata even when over budget.
//!
//! [`generate_markdown_with_notices`] additionally delivers budget/depth
//! diagnostics to a caller callback. Neither function writes to the console.

mod endpoint;
mod examples;
mod schema;
pub(crate) mod schema_selection;
#[cfg_attr(not(feature = "cli"), allow(dead_code))]
pub(crate) mod split;
mod views;

use std::io::Write;

use anyhow::Result;
use indexmap::IndexMap;

use crate::models::{ApiDocumentation, DetailLevel, DocConfig, GroupBy};

// The report and stats modules reuse the views' notion of which endpoints and
// services the rendered body covers, so their scope always matches the
// document's.
pub(crate) use views::{removing_filters, service_is_visible, visible_endpoints};
// `diff` compares the response schema the renderer would document.
pub(crate) use schema::{PREFER_REF_DESCRIPTION_EXT, response_schema};
// The `--costs` analysis measures schema use sites and per-schema renders.
pub(crate) use schema::SchemaUse;
#[cfg(feature = "cli")]
pub(crate) use schema::{INLINE_MEASURE_PREFIX_ROWS, INLINE_MEASURE_ROW_BUDGET};
#[cfg(feature = "cli")]
pub(crate) use schema::{
    definition_section_tokens, inline_expansion_tokens, short_schema_reference,
};

/// The result of one [`render`] call beyond the bytes themselves: the
/// schema-omission notices plus the per-reference use-site observations
/// collected while rendering (see [`SchemaUse`]). Consumed by
/// `generate_markdown` and the `--costs` analysis; trial renders may discard
/// either.
#[derive(Debug, Clone, Default)]
pub(crate) struct ViewRender {
    pub omissions: Vec<String>,
    #[cfg_attr(not(feature = "cli"), allow(dead_code))]
    pub schema_uses: IndexMap<String, SchemaUse>,
}

/// Renders the documentation to `writer`.
///
/// Invalid operation/schema selectors fail before writing output. With
/// [`DocConfig::max_tokens`] set, operation detail steps down until it fits;
/// explicit schema selections stay intact and carry an overage notice in Markdown.
/// Use [`generate_markdown_with_notices`] to receive diagnostic notices as well.
/// This renders only the body; [`DocConfig::include_report`] is a CLI option.
pub fn generate_markdown<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<()> {
    generate_markdown_with_notices(writer, doc, config, &mut |_| {})
}

/// Render Markdown and deliver budget/depth notices to the caller.
/// Trial renders never emit notices; only the chosen output does.
/// Nothing is written to the process console.
pub fn generate_markdown_with_notices<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
    notice: &mut dyn FnMut(&str),
) -> Result<()> {
    crate::selection::validate(doc, config)?;
    match config.max_tokens {
        Some(budget) => generate_within_budget(writer, doc, config, budget, notice),
        None => {
            let rendered = render(writer, doc, config)?;
            for omission in &rendered.omissions {
                notice(omission);
            }
            Ok(())
        }
    }
}

/// Renders the documentation to `writer`, dispatching on detail level and grouping mode.
///
/// Returns the schema-omission notices collected during this render alongside
/// the observed per-reference use sites ([`ViewRender`]). Callers that discard
/// the buffer (token-budget trials, `--stats` estimates) must not print the
/// notices; callers that write the buffer should deliver the notices to their caller.
///
/// Crate-visible so `--stats` can size trial renders without a token budget.
pub(crate) fn render<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<ViewRender> {
    let resolved = resolve_schema_depth(doc, config);
    let mut rendered = render_resolved(writer, doc, &resolved)?;
    if let Some(bound_notice) = &resolved.notice {
        rendered.omissions.insert(0, bound_notice.clone());
    }
    Ok(rendered)
}

/// A configuration whose effective `--schema-depth` has been decided once,
/// so every measurement of one run (`--stats`, `--costs` rows and slices)
/// uses the same depth as the whole render.
pub(crate) struct ResolvedConfig<'c> {
    pub config: std::borrow::Cow<'c, DocConfig>,
    /// Stderr notice when the depth was chosen automatically (#160, #177).
    pub notice: Option<String>,
    /// Why the depth was chosen automatically, if it was.
    pub bound: Option<AutoBound>,
}

/// Why [`resolve_schema_depth`] bounded schema expansion on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutoBound {
    /// A selected slice reaches more than
    /// [`schema::AUTO_SCHEMA_DEPTH_THRESHOLD`] component schemas (#160).
    ComponentGraph,
    /// `--inline-schemas` would expand more than
    /// [`schema::INLINE_ROW_BUDGET`] schema rows (#177).
    InlineRows,
}

impl AutoBound {
    /// The cause as a clause: "the selection reaches more than ...".
    pub(crate) fn cause(self) -> String {
        match self {
            AutoBound::ComponentGraph => format!(
                "the selection reaches more than {} component schemas",
                schema::AUTO_SCHEMA_DEPTH_THRESHOLD
            ),
            AutoBound::InlineRows => format!(
                "--inline-schemas would expand more than {} schema rows",
                schema::INLINE_ROW_BUDGET
            ),
        }
    }
}

impl ResolvedConfig<'_> {
    /// The automatically chosen depth, if any.
    pub(crate) fn auto_depth(&self) -> Option<usize> {
        self.bound.and(self.config.schema_depth)
    }

    /// The same decision applied to another configuration (a `--stats` row or
    /// `--costs` slice), so it is measured at this run's depth.
    #[cfg(feature = "cli")]
    pub(crate) fn with_config(&self, config: DocConfig) -> ResolvedConfig<'static> {
        ResolvedConfig {
            config: std::borrow::Cow::Owned(config),
            notice: self.notice.clone(),
            bound: self.bound,
        }
    }
}

/// Bounds schema expansion at [`schema::AUTO_SCHEMA_DEPTH`] when
/// `--schema-depth` is not set and either
///
/// * a selected slice's roots reach more than
///   [`schema::AUTO_SCHEMA_DEPTH_THRESHOLD`] component schemas (#160). Slices
///   are standalone `--schema`/`--schema-field` reads and `--operation`/
///   `--operation-id` renders at `--detail full --include-schemas`; or
/// * under `--inline-schemas`, the rendered schema roots (a selection's, or
///   every operation the view renders at `--detail full --include-schemas`,
///   once per rendering, so a multi-tag operation counts once per service)
///   would expand more than [`schema::INLINE_ROW_BUDGET`] rows (#177), counted by
///   [`schema::inline_row_count`] without rendering.
///
/// Anything else, including every linked whole document, is unchanged.
pub(crate) fn resolve_schema_depth<'c>(
    doc: &ApiDocumentation,
    config: &'c DocConfig,
) -> ResolvedConfig<'c> {
    let unchanged = ResolvedConfig {
        config: std::borrow::Cow::Borrowed(config),
        notice: None,
        bound: None,
    };
    if config.schema_depth.is_some() {
        return unchanged;
    }
    let schemas_rendered = config.detail_level == DetailLevel::Full && config.include_schemas;
    let selected;
    // The component-graph rule asks what a slice reaches, so each endpoint
    // counts once; the inline-row rule sizes the output, so an endpoint the
    // view renders under several services counts once per rendering.
    let (graph_roots, row_roots, slice): (Vec<&crate::models::Schema>, Vec<_>, bool) =
        if schema_selection::active(config) {
            let Ok(found) = schema_selection::selections(doc, config) else {
                return unchanged;
            };
            selected = found;
            let roots: Vec<_> = selected.iter().map(|(_, schema)| schema).collect();
            (roots.clone(), roots, true)
        } else if schemas_rendered && (config.operation_selector.is_some() || config.inline_schemas)
        {
            let slice = config.operation_selector.is_some();
            let graph_roots = if slice {
                endpoint_schema_roots(visible_endpoints(doc, config))
            } else {
                Vec::new()
            };
            let row_roots = if config.inline_schemas {
                endpoint_schema_roots(views::rendered_endpoints(doc, config))
            } else {
                Vec::new()
            };
            (graph_roots, row_roots, slice)
        } else {
            return unchanged;
        };
    let threshold = schema::AUTO_SCHEMA_DEPTH_THRESHOLD;
    let bound =
        if slice && schema::reachable_component_count(doc, graph_roots, threshold) > threshold {
            AutoBound::ComponentGraph
        } else if config.inline_schemas
            && schema::inline_row_count(
                doc,
                row_roots,
                schema_selection::active(config),
                schema::INLINE_ROW_BUDGET,
            ) > schema::INLINE_ROW_BUDGET
        {
            AutoBound::InlineRows
        } else {
            return unchanged;
        };
    let depth = schema::AUTO_SCHEMA_DEPTH;
    let mut bounded = config.clone();
    bounded.schema_depth = Some(depth);
    let notice = match bound {
        AutoBound::ComponentGraph => format!(
            "vimanam: selection reaches more than {threshold} component schemas and --schema-depth is not set; bounding schema expansion at --schema-depth {depth} (pass --schema-depth 0..=24 to choose)"
        ),
        AutoBound::InlineRows => format!(
            "vimanam: --inline-schemas would expand more than {} schema rows and --schema-depth is not set; bounding schema expansion at --schema-depth {depth} (pass --schema-depth 0..=24 to choose, or drop --inline-schemas to render each component schema once)",
            schema::INLINE_ROW_BUDGET
        ),
    };
    ResolvedConfig {
        config: std::borrow::Cow::Owned(bounded),
        notice: Some(notice),
        bound: Some(bound),
    }
}

/// The schema roots `endpoints` render at `--detail full --include-schemas`.
fn endpoint_schema_roots(endpoints: Vec<&crate::models::Endpoint>) -> Vec<&crate::models::Schema> {
    endpoints
        .into_iter()
        .flat_map(schema::rendered_endpoint_schemas)
        .collect()
}

/// Renders exactly at the resolved depth, never re-deciding it. When the depth
/// was chosen automatically, a one-line blockquote after the document title
/// says so, so a reader of the Markdown alone (e.g. `-o` output) knows the
/// slice is bounded.
pub(crate) fn render_resolved<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    resolved: &ResolvedConfig,
) -> Result<ViewRender> {
    let Some(depth) = resolved.auto_depth() else {
        return render_configured(writer, doc, &resolved.config);
    };
    let mut buffer = Vec::new();
    let rendered = render_configured(&mut buffer, doc, &resolved.config)?;
    // Every view starts with `# {title}` written verbatim, so a title holding
    // a newline spans several lines; the schema selection escapes its title
    // onto one line. Place the note after the whole title either way.
    let views_title = format!("# {}\n", doc.title);
    let split = if buffer.starts_with(views_title.as_bytes()) {
        views_title.len()
    } else {
        buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1)
    };
    writer.write_all(&buffer[..split])?;
    match resolved.bound {
        Some(AutoBound::InlineRows) => write!(
            writer,
            "\n> Schema expansion is bounded at --schema-depth {depth}: {}. Follow the per-row retrieval commands, pass --schema-depth N (0..=24) to choose another bound, or drop --inline-schemas to render each component schema once.\n",
            AutoBound::InlineRows.cause()
        )?,
        _ => write!(
            writer,
            "\n> Schema expansion is bounded at --schema-depth {depth}: this selection reaches more than {} component schemas. Follow the per-row retrieval commands, or pass --schema-depth N (0..=24) to choose another bound.\n",
            schema::AUTO_SCHEMA_DEPTH_THRESHOLD
        )?,
    }
    let rest = &buffer[split..];
    // Exactly one blank line before what followed the title.
    if !rest.starts_with(b"\n") {
        writeln!(writer)?;
    }
    writer.write_all(rest)?;
    Ok(rendered)
}

fn render_configured<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<ViewRender> {
    if schema_selection::active(config) {
        let omissions = schema_selection::render(writer, doc, config)?;
        return Ok(ViewRender {
            omissions,
            schema_uses: IndexMap::new(),
        });
    }
    // For summary level, just generate the TOC
    if config.detail_level == DetailLevel::Summary {
        views::generate_summary(writer, doc, config)?;
        Ok(ViewRender::default())
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
/// The caller receives notices explaining any detail reduction.
fn generate_within_budget<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
    budget: usize,
    notice: &mut dyn FnMut(&str),
) -> Result<()> {
    if schema_selection::active(config) {
        let mut buffer = Vec::new();
        let rendered = render(&mut buffer, doc, config)?;
        let tokens = estimate_tokens(&buffer);
        if tokens > budget {
            let budget_notice = format!(
                "> Selected schemas exceed the approximate {budget}-token budget (~{tokens} tokens before this notice). Selection and metadata are preserved; narrow --schema-field or explicitly set --schema-depth.\n\n"
            );
            notice(&format!(
                "vimanam: selected schemas exceed approximate {budget}-token budget (~{tokens} tokens); preserving requested selection and metadata"
            ));
            writer.write_all(budget_notice.as_bytes())?;
        }
        writer.write_all(&buffer)?;
        for omission in &rendered.omissions {
            notice(omission);
        }
        return Ok(());
    }
    // Only consider levels at or below the one the caller asked for.
    let start = DETAIL_LADDER
        .iter()
        .position(|level| *level == config.detail_level)
        .unwrap_or(0);

    let mut chosen: Option<(DetailLevel, Vec<u8>, usize, ViewRender)> = None;
    for level in &DETAIL_LADDER[start..] {
        let mut trial_config = config.clone();
        trial_config.detail_level = level.clone();

        let mut buffer = Vec::new();
        let rendered = render(&mut buffer, doc, &trial_config)?;
        let tokens = estimate_tokens(&buffer);

        let fits = tokens <= budget;
        chosen = Some((level.clone(), buffer, tokens, rendered));
        if fits {
            break;
        }
    }

    // The ladder slice is always non-empty, so a candidate is always produced.
    let (level, buffer, tokens, rendered) = chosen.expect("at least one detail level is rendered");

    if level != config.detail_level {
        notice(&format!(
            "vimanam: output exceeded the {budget}-token budget at --detail {}; \
             reduced to --detail {} (~{tokens} tokens).",
            detail_level_name(&config.detail_level),
            detail_level_name(&level),
        ));
    } else if tokens > budget {
        notice(&format!(
            "vimanam: output is ~{tokens} tokens, over the {budget}-token budget; \
             already at the lowest detail level (--detail {}).",
            detail_level_name(&level),
        ));
    }

    writer.write_all(&buffer)?;
    for omission in &rendered.omissions {
        notice(omission);
    }
    Ok(())
}

/// Estimates the token count of rendered output with the common chars/4
/// heuristic. Good enough to pick a detail level; a real tokenizer could
/// replace this later. Shared with `--stats`, whose ~TOKENS column must agree
/// with what `--max-tokens` would measure.
pub fn estimate_tokens(rendered: &[u8]) -> usize {
    String::from_utf8_lossy(rendered)
        .chars()
        .count()
        .div_ceil(4)
}

/// The `--detail` value name for a [`DetailLevel`], for stderr messages and the
/// `--costs` report's mode line.
pub(crate) fn detail_level_name(level: &DetailLevel) -> &'static str {
    match level {
        DetailLevel::Summary => "summary",
        DetailLevel::Basic => "basic",
        DetailLevel::Standard => "standard",
        DetailLevel::Full => "full",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` mutually recursive components with `k` references each, and one
    /// operation tagged with `tags` tags whose request and response bodies
    /// reference them.
    fn multi_tag_doc(n: usize, k: usize, tags: usize) -> ApiDocumentation {
        let reference =
            |i: usize| serde_json::json!({"$ref": format!("#/components/schemas/C{}", i % n)});
        let mut schemas = serde_json::Map::new();
        for i in 0..n {
            let mut properties = serde_json::Map::new();
            properties.insert("id".into(), serde_json::json!({"type": "string"}));
            for j in 1..=k {
                properties.insert(format!("r{j}"), reference(i + j));
            }
            schemas.insert(
                format!("C{i}"),
                serde_json::json!({"type": "object", "properties": properties}),
            );
        }
        let tags: Vec<String> = (0..tags).map(|t| format!("t{t}")).collect();
        let spec = serde_json::json!({
            "openapi": "3.0.0",
            "info": {"title": "Multi", "version": "1"},
            "paths": {"/c": {"post": {
                "tags": tags,
                "requestBody": {"content": {"application/json": {"schema": reference(0)}}},
                "responses": {"200": {
                    "description": "ok",
                    "content": {"application/json": {"schema": reference(1)}}
                }}
            }}},
            "components": {"schemas": schemas},
        });
        crate::parser::parse_openapi_bytes(spec.to_string().as_bytes(), "json", None)
            .expect("multi-tag spec parses")
    }

    fn inline_config() -> DocConfig {
        DocConfig {
            inline_schemas: true,
            ..DocConfig::unfiltered()
        }
    }

    /// The service view renders a multi-tag operation under each service, so
    /// the inline-row estimate counts it once per rendering (#177): one
    /// rendering of this graph fits the budget, four do not.
    #[test]
    fn inline_row_bound_counts_each_service_rendering() {
        fn roots(doc: &ApiDocumentation) -> Vec<&crate::models::Schema> {
            endpoint_schema_roots(views::rendered_endpoints(doc, &inline_config()))
        }
        let single = multi_tag_doc(18, 3, 1);
        let rows = schema::inline_row_count(&single, roots(&single), false, usize::MAX - 1);
        assert!(rows <= schema::INLINE_ROW_BUDGET && rows * 4 > schema::INLINE_ROW_BUDGET);
        assert!(
            resolve_schema_depth(&single, &inline_config())
                .bound
                .is_none()
        );

        let tagged = multi_tag_doc(18, 3, 4);
        assert_eq!(roots(&tagged).len(), 4 * 2);
        let config = inline_config();
        let resolved = resolve_schema_depth(&tagged, &config);
        assert!(matches!(resolved.bound, Some(AutoBound::InlineRows)));
        assert_eq!(
            resolved.config.schema_depth,
            Some(schema::AUTO_SCHEMA_DEPTH)
        );

        // Views that render the operation once, and a service filter that
        // keeps one of its services, leave it under the budget.
        for group_by in [GroupBy::Flat, GroupBy::Method, GroupBy::Path] {
            let config = DocConfig {
                group_by,
                ..inline_config()
            };
            assert!(resolve_schema_depth(&tagged, &config).bound.is_none());
        }
        let config = DocConfig {
            service_filter: Some(vec!["t2".into()]),
            ..inline_config()
        };
        assert!(resolve_schema_depth(&tagged, &config).bound.is_none());
    }
}
