//! The `--costs` token-cost analysis: per-endpoint and per-schema estimated
//! rendering costs, reference amplification and hotspots, printed instead of
//! the documentation.
//!
//! All numbers reuse the [`estimate_tokens`] chars/4 heuristic over real
//! renders; none are tokenizer counts or billing figures. Three distinct
//! measurements are reported, and the output labels each one so they cannot be
//! confused:
//!
//! * **TOTAL** — one render of the whole filtered document, exactly what the
//!   conversion pipeline (or `--max-tokens`) would measure.
//! * **Endpoint rows** — one render narrowed to that endpoint alone (its
//!   operation plus the document frame and the schema definitions it links),
//!   the same slice semantics as `--stats`. Rows overlap through the shared
//!   frame and shared schema definitions, so they neither sum to nor subtract
//!   from TOTAL. A multi-tag endpoint is measured once, under its first
//!   service, while a service-grouped document renders it once per tag.
//! * **Schema rows** — the linked-mode definition cost (read once) and, as a
//!   separate clearly-labeled analysis, the modeled cost of inline expansion:
//!   use sites × one measured, cycle-guarded expansion. Use sites are the
//!   linked render's rows — body rows plus the reference rows inside each
//!   rendered definition — so the model can differ from real
//!   `--inline-schemas` output in either direction (see `write_costs`).
//!
//! Like the hygiene report, [`CostReport`] is data-only so other front ends
//! can reuse the analysis; [`write_costs`] renders it deterministically.

use std::cmp::Ordering;
use std::io::Write;

use anyhow::Result;
use indexmap::IndexSet;

use crate::markdown::{
    SchemaUse, definition_section_tokens, detail_level_name, estimate_tokens,
    inline_expansion_tokens, render, short_schema_reference, visible_endpoints,
};
use crate::models::{
    AdditionalProperties, ApiDocumentation, DocConfig, Endpoint, OperationRef, OperationSelector,
    Schema,
};
use crate::utils::resolve_schema_reference;

/// Estimated cost of reading one endpoint: a whole render narrowed to that
/// endpoint alone (see the module docs for what the slice includes).
#[derive(Debug, Clone, PartialEq)]
pub struct EndpointCost {
    /// `METHOD /path/template`, the same label `diff` and `--operation` use.
    pub operation: String,
    /// Every tag of the endpoint, in spec order. The cost is measured under
    /// the first; the rest explain where the endpoint also renders.
    pub services: Vec<String>,
    pub tokens: usize,
    /// Slice tokens as a percentage of TOTAL. Shares overlap across rows and
    /// are not additive.
    pub share: f64,
}

/// Cost of one component schema as actually rendered, plus the separate
/// inline-expansion model.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaCost {
    /// Short schema name as rendered in headings.
    pub name: String,
    /// Rendered rows linking (or, inline, expanding) this schema.
    pub uses: usize,
    /// Tokens of the Schema Definitions entry rendered once (linked mode).
    /// The hypothetical read-once cost in `--inline-schemas` mode, which emits
    /// no definitions.
    pub definition_tokens: usize,
    /// Tokens of one measured inline expansion at a use site (cycle-guarded).
    pub inline_per_use_tokens: usize,
    /// Model: `uses × inline_per_use_tokens` — what expanding the schema at
    /// every use site would cost, not the cost of any single render.
    pub inline_total_tokens: usize,
    /// `inline_total / definition_tokens`: how much more expensive reading
    /// every use site inline is than reading the definition once. `None` when
    /// the definition renders empty.
    pub amplification: Option<f64>,
    /// The schema participates in a reference cycle (transitively reaches
    /// itself). Inline expansion cuts such cycles with a one-row notice.
    pub cyclic: bool,
}

/// The complete analysis: mode, TOTAL, endpoint and schema rows, and
/// deterministic hotspot rankings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CostReport {
    /// The `--detail` value name the costs were measured at (e.g. `full`).
    pub detail_name: String,
    /// Whether the analyzed configuration renders schemas inline.
    pub inline_schemas: bool,
    /// Tokens of one render of the whole filtered document; 0 when nothing is
    /// visible.
    pub total_tokens: usize,
    pub endpoints: Vec<EndpointCost>,
    /// Schemas with at least one rendered use, in first-encounter order.
    pub schemas: Vec<SchemaCost>,
    /// Indices into `endpoints`, most expensive first (ties keep row order).
    pub endpoint_hotspots: Vec<usize>,
    /// Indices into `schemas` by descending amplification ratio (schemas with
    /// no ratio — empty definitions — last), then `inline_total_tokens`, then
    /// `definition_tokens`, all descending, then first-encounter order.
    pub schema_hotspots: Vec<usize>,
}

/// How many hotspots each ranking reports.
const HOTSPOT_LIMIT: usize = 5;

pub fn compute(doc: &ApiDocumentation, config: &DocConfig) -> Result<CostReport> {
    // One whole-document render: the TOTAL plus the exact use-site
    // observations (which schemas the render linked, and how often).
    let mut whole = Vec::new();
    let rendered = render(&mut whole, doc, config)?;
    let visible = visible_endpoints(doc, config);
    let total_tokens = if visible.is_empty() {
        0
    } else {
        estimate_tokens(&whole)
    };

    let endpoints: Vec<EndpointCost> = visible
        .iter()
        .map(|endpoint| {
            let tokens = estimate_slice(doc, config, endpoint)?;
            let share = share_of(tokens, total_tokens);
            Ok(EndpointCost {
                operation: format!("{} {}", endpoint.method, endpoint.path),
                services: endpoint.services.clone(),
                tokens,
                share,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let mut schemas = Vec::new();
    for (reference, use_stats) in &rendered.schema_uses {
        schemas.push(schema_cost(doc, config, reference, use_stats));
    }

    let endpoint_hotspots = hotspot_indices(&endpoints, HOTSPOT_LIMIT, |cost| (cost.tokens, 0));
    let schema_hotspots = schema_hotspot_indices(&schemas, HOTSPOT_LIMIT);

    Ok(CostReport {
        detail_name: detail_level_name(&config.detail_level).to_string(),
        inline_schemas: config.inline_schemas,
        total_tokens,
        endpoints,
        schemas,
        endpoint_hotspots,
        schema_hotspots,
    })
}

/// Estimates the endpoint's slice: a whole render with the exact-operation
/// selector narrowed to this endpoint and the service filter to its first
/// service (mirroring how `--stats` narrows to one service). The user's other
/// filters, detail level and schema mode all stay in effect.
fn estimate_slice(
    doc: &ApiDocumentation,
    config: &DocConfig,
    endpoint: &Endpoint,
) -> Result<usize> {
    let mut slice_config = config.clone();
    let mut operations = IndexSet::new();
    operations.insert(OperationRef {
        method: endpoint.method.clone(),
        path: endpoint.path.clone(),
    });
    slice_config.operation_selector = Some(OperationSelector {
        operations,
        operation_ids: IndexSet::new(),
    });
    if let Some(first) = endpoint.services.first() {
        slice_config.service_filter = Some(vec![first.clone()]);
    }

    let mut buffer = Vec::new();
    render(&mut buffer, doc, &slice_config)?;
    Ok(estimate_tokens(&buffer))
}

/// Builds one schema row from the observed use sites plus two small measured
/// renders (the definition entry, one inline expansion).
fn schema_cost(
    doc: &ApiDocumentation,
    config: &DocConfig,
    reference: &str,
    use_stats: &SchemaUse,
) -> SchemaCost {
    let definition_depth = definition_depth(config, use_stats.min_depth);
    let definition_tokens = definition_section_tokens(doc, config, reference, definition_depth);
    let inline_per_use_tokens = inline_expansion_tokens(doc, config, reference);
    let inline_total_tokens = use_stats.count * inline_per_use_tokens;
    let amplification =
        (definition_tokens > 0).then(|| inline_total_tokens as f64 / definition_tokens as f64);

    SchemaCost {
        name: short_schema_reference(reference),
        uses: use_stats.count,
        definition_tokens,
        inline_per_use_tokens,
        inline_total_tokens,
        amplification,
        cyclic: schema_reaches_itself(doc, reference),
    }
}

/// The depth the Schema Definitions entry renders at: always 0 without
/// `--schema-depth` (the renderer seeds definitions at depth 0), and one edge
/// below the shallowest discovered use when pre-discovery is active (see
/// [`crate::markdown::schema::SchemaContext::discover`]).
fn definition_depth(config: &DocConfig, min_use_depth: usize) -> usize {
    match (config.inline_schemas, config.schema_depth) {
        // Inline mode emits no definitions; measure the hypothetical
        // "read once" cost as a full expansion from the root.
        (true, _) => 0,
        (false, Some(_)) => min_use_depth + 1,
        (false, None) => 0,
    }
}

/// Whether the schema graph reachable from `reference` returns to it. Edges
/// are `$ref`s found anywhere inside a schema; cycles elsewhere in the graph
/// are cut by the visited set.
fn schema_reaches_itself(doc: &ApiDocumentation, reference: &str) -> bool {
    fn walk(
        doc: &ApiDocumentation,
        reference: &str,
        target: &str,
        visited: &mut IndexSet<String>,
    ) -> bool {
        if !visited.insert(reference.to_string()) {
            return false;
        }
        let Some(resolved) = resolve_schema_reference(reference, doc) else {
            return false;
        };
        let mut child_refs = Vec::new();
        collect_child_refs(resolved, &mut child_refs);
        child_refs
            .iter()
            .any(|child| child == target || walk(doc, child, target, visited))
    }

    let mut visited = IndexSet::new();
    walk(doc, reference, reference, &mut visited)
}

/// Every `$ref` inside `schema`'s own structure (properties, items,
/// additionalProperties, compositions). A `$ref` node is an edge; its target
/// is resolved lazily by the caller, so lazy reference graphs terminate.
fn collect_child_refs(schema: &Schema, refs: &mut Vec<String>) {
    if let Some(reference) = &schema.reference {
        refs.push(reference.clone());
        return;
    }
    if let Some(properties) = &schema.properties {
        for child in properties.values() {
            collect_child_refs(child, refs);
        }
    }
    if let Some(items) = &schema.items {
        collect_child_refs(items, refs);
    }
    if let Some(AdditionalProperties::Schema(child)) = &schema.additional_properties {
        collect_child_refs(child, refs);
    }
    for variants in [&schema.all_of, &schema.one_of, &schema.any_of] {
        for variant in variants.iter().flatten() {
            collect_child_refs(variant, refs);
        }
    }
}

/// Indices of the `limit` largest items by a two-level descending key (ties
/// resolved by ascending index, i.e. row order).
fn hotspot_indices<T>(items: &[T], limit: usize, key: impl Fn(&T) -> (usize, usize)) -> Vec<usize> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| key(&items[b]).cmp(&key(&items[a])).then(a.cmp(&b)));
    order.truncate(limit);
    order
}

/// Indices of the `limit` most amplifying schemas: descending amplification
/// ratio first (a 10x-amplified small schema outranks a 1.1x large one), with
/// ratio-less rows — empty definitions — last; ties break by descending
/// `inline_total_tokens`, then `definition_tokens`, then row order.
fn schema_hotspot_indices(schemas: &[SchemaCost], limit: usize) -> Vec<usize> {
    /// Descending ratio order; `None` (empty definition) sorts last.
    fn ratio_desc(a: &Option<f64>, b: &Option<f64>) -> Ordering {
        match (a, b) {
            (Some(x), Some(y)) => y.total_cmp(x),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }

    let mut order: Vec<usize> = (0..schemas.len()).collect();
    order.sort_by(|&a, &b| {
        ratio_desc(&schemas[a].amplification, &schemas[b].amplification)
            .then(
                schemas[b]
                    .inline_total_tokens
                    .cmp(&schemas[a].inline_total_tokens),
            )
            .then(
                schemas[b]
                    .definition_tokens
                    .cmp(&schemas[a].definition_tokens),
            )
            .then(a.cmp(&b))
    });
    order.truncate(limit);
    order
}

/// `tokens` as a percentage of `total`; 0 when the total is 0.
fn share_of(tokens: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        tokens as f64 / total as f64 * 100.0
    }
}

/// The endpoint's operation label with a ` (+tag)` suffix for every tag after
/// the first: multi-tag endpoints are measured once, under their first
/// service, and the suffix names where they also render.
fn endpoint_label(cost: &EndpointCost) -> String {
    let mut label = cost.operation.clone();
    for tag in cost.services.iter().skip(1) {
        label.push_str(&format!(" (+{tag})"));
    }
    label
}

const MODE_HEADER: &str = "TOKEN COST ANALYSIS";
const ENDPOINTS_HEADER: &str = "ENDPOINTS";
const SCHEMAS_HEADER: &str = "SCHEMAS";
const HOTSPOTS_HEADER: &str = "HOTSPOTS";
const TOTAL_LABEL: &str = "TOTAL";
const TOKENS_HEADER: &str = "~TOKENS";
const SHARE_HEADER: &str = "SHARE";
const OPERATION_HEADER: &str = "OPERATION";
const SCHEMA_HEADER: &str = "SCHEMA";
const USES_HEADER: &str = "USES";
const DEF_HEADER: &str = "DEF ~TOKENS";
const PER_USE_HEADER: &str = "INLINE/USE";
const INLINE_TOTAL_HEADER: &str = "INLINE TOTAL";
const AMPLIFICATION_HEADER: &str = "AMPLIF.";
const NOTES_HEADER: &str = "NOTES";
const CYCLE_NOTE: &str = "cycle";
/// Spaces between columns.
const GAP: &str = "  ";

/// The number of characters `value` renders as.
fn digits(value: usize) -> usize {
    value.to_string().len()
}

/// Writes the analysis as plain text: an explanation of the cost model, the
/// mode line with TOTAL, the endpoint table, the schema table, and the
/// hotspot rankings. Deterministic given the same spec and flags.
pub fn write_costs<W: Write>(writer: &mut W, report: &CostReport) -> Result<()> {
    writeln!(writer, "{MODE_HEADER}")?;
    writeln!(
        writer,
        "~TOKENS are chars/4 estimates over rendered output — not tokenizer counts and not billing figures."
    )?;
    let schema_mode = if report.inline_schemas {
        "inline (every use site expands; no definitions section)"
    } else {
        "linked (definitions rendered once; use sites link)"
    };
    writeln!(
        writer,
        "Mode: --detail {}, schemas {}. {TOTAL_LABEL}: ~{} tokens (one render of the whole filtered document).",
        report.detail_name, schema_mode, report.total_tokens
    )?;
    writeln!(writer)?;

    writeln!(writer, "{ENDPOINTS_HEADER}")?;
    if report.endpoints.is_empty() {
        writeln!(writer, "(no endpoints visible after filters)")?;
    } else {
        writeln!(
            writer,
            "One render per endpoint alone: its service section, the document frame and the schema"
        )?;
        writeln!(
            writer,
            "definitions it links. Rows overlap through the frame and shared schemas, so they neither"
        )?;
        writeln!(
            writer,
            "sum to nor subtract from TOTAL. Multi-tag endpoints are measured once, under their first"
        )?;
        writeln!(
            writer,
            "service; a service-grouped document renders them once per tag. A ` (+tag)` suffix marks"
        )?;
        writeln!(writer, "the extra tags.")?;
        write_endpoint_table(writer, report)?;
    }
    writeln!(writer)?;

    writeln!(writer, "{SCHEMAS_HEADER}")?;
    if report.schemas.is_empty() {
        writeln!(
            writer,
            "(no component schemas rendered at this detail level and filters)"
        )?;
    } else {
        writeln!(
            writer,
            "DEF is the Schema Definitions entry read once. INLINE TOTAL is a separate analysis: the"
        )?;
        writeln!(
            writer,
            "modeled cost of expanding the schema at every use site (uses × one measured, cycle-guarded"
        )?;
        writeln!(
            writer,
            "expansion), not the cost of the output in linked mode. USES counts the linked render's"
        )?;
        writeln!(
            writer,
            "rows: body rows plus the reference rows inside each rendered definition. Internal rows"
        )?;
        writeln!(
            writer,
            "have no inline counterpart, and a schema referenced only inside another definition expands"
        )?;
        writeln!(
            writer,
            "with it, so real --inline-schemas output can differ from the model in either direction."
        )?;
        writeln!(
            writer,
            "A cycle note marks a schema that transitively reaches itself; inline expansion cuts it."
        )?;
        write_schema_table(writer, report)?;
    }
    writeln!(writer)?;

    writeln!(writer, "{HOTSPOTS_HEADER}")?;
    writeln!(
        writer,
        "Most expensive slices; shares are of TOTAL and overlap with each other."
    )?;
    if report.endpoint_hotspots.is_empty() && report.schema_hotspots.is_empty() {
        writeln!(writer, "(nothing to rank)")?;
    } else {
        for (rank, &index) in report.endpoint_hotspots.iter().enumerate() {
            let cost = &report.endpoints[index];
            writeln!(
                writer,
                "{}. {} — ~{} tokens ({:.1}% of TOTAL)",
                rank + 1,
                endpoint_label(cost),
                cost.tokens,
                cost.share
            )?;
        }
        if !report.schema_hotspots.is_empty() {
            writeln!(
                writer,
                "Most amplifying schemas (inline model vs reading the definition once):"
            )?;
            for (rank, &index) in report.schema_hotspots.iter().enumerate() {
                let cost = &report.schemas[index];
                writeln!(
                    writer,
                    "{}. {} — {} use(s), ~{} tokens inline vs ~{} read once{}",
                    rank + 1,
                    cost.name,
                    cost.uses,
                    cost.inline_total_tokens,
                    cost.definition_tokens,
                    if cost.cyclic { " (cycle)" } else { "" }
                )?;
            }
        }
    }

    Ok(())
}

fn write_endpoint_table<W: Write>(writer: &mut W, report: &CostReport) -> Result<()> {
    let share_cells: Vec<String> = report
        .endpoints
        .iter()
        .map(|cost| format!("{:.1}%", cost.share))
        .collect();
    let share_width = share_cells
        .iter()
        .map(String::len)
        .chain([SHARE_HEADER.len()])
        .max()
        .unwrap_or(SHARE_HEADER.len());
    let tokens_width = report
        .endpoints
        .iter()
        .map(|cost| digits(cost.tokens))
        .chain([TOKENS_HEADER.len()])
        .max()
        .unwrap_or(TOKENS_HEADER.len());

    // Only intermediate columns are padded: the trailing OPERATION column is
    // bare, so no row ends in whitespace.
    writeln!(
        writer,
        "{TOKENS_HEADER:>tokens_width$}{GAP}{SHARE_HEADER:>share_width$}{GAP}{OPERATION_HEADER}"
    )?;
    for (cost, share) in report.endpoints.iter().zip(&share_cells) {
        writeln!(
            writer,
            "{:>tokens_width$}{GAP}{:>share_width$}{GAP}{}",
            cost.tokens,
            share,
            endpoint_label(cost)
        )?;
    }
    Ok(())
}

fn write_schema_table<W: Write>(writer: &mut W, report: &CostReport) -> Result<()> {
    let name_width = report
        .schemas
        .iter()
        .map(|cost| cost.name.chars().count())
        .chain([SCHEMA_HEADER.len()])
        .max()
        .unwrap_or(SCHEMA_HEADER.len());
    let def_width = report
        .schemas
        .iter()
        .map(|cost| digits(cost.definition_tokens))
        .chain([DEF_HEADER.len()])
        .max()
        .unwrap_or(DEF_HEADER.len());
    let per_use_width = report
        .schemas
        .iter()
        .map(|cost| digits(cost.inline_per_use_tokens))
        .chain([PER_USE_HEADER.len()])
        .max()
        .unwrap_or(PER_USE_HEADER.len());
    let total_width = report
        .schemas
        .iter()
        .map(|cost| digits(cost.inline_total_tokens))
        .chain([INLINE_TOTAL_HEADER.len()])
        .max()
        .unwrap_or(INLINE_TOTAL_HEADER.len());
    // The NOTES column only exists when at least one row carries a note, so
    // ordinary rows never end in trailing whitespace.
    let has_notes = report.schemas.iter().any(|cost| cost.cyclic);

    write!(
        writer,
        "{SCHEMA_HEADER:<name_width$}{GAP}{USES_HEADER:>4}{GAP}{DEF_HEADER:>def_width$}{GAP}{PER_USE_HEADER:>per_use_width$}{GAP}{INLINE_TOTAL_HEADER:>total_width$}{GAP}{AMPLIFICATION_HEADER:>8}"
    )?;
    if has_notes {
        write!(writer, "{GAP}{NOTES_HEADER}")?;
    }
    writeln!(writer)?;
    for cost in &report.schemas {
        let amplification = match cost.amplification {
            Some(value) => format!("{value:.1}x"),
            None => "-".to_string(),
        };
        write!(
            writer,
            "{:<name_width$}{GAP}{:>4}{GAP}{:>def_width$}{GAP}{:>per_use_width$}{GAP}{:>total_width$}{GAP}{:>8}",
            cost.name,
            cost.uses,
            cost.definition_tokens,
            cost.inline_per_use_tokens,
            cost.inline_total_tokens,
            amplification
        )?;
        if has_notes && cost.cyclic {
            write!(writer, "{GAP}{CYCLE_NOTE}")?;
        }
        writeln!(writer)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_to_string(report: &CostReport) -> String {
        let mut buffer = Vec::new();
        write_costs(&mut buffer, report).unwrap();
        String::from_utf8(buffer).unwrap()
    }

    fn sample_report() -> CostReport {
        CostReport {
            detail_name: "full".into(),
            inline_schemas: false,
            total_tokens: 2000,
            endpoints: vec![
                EndpointCost {
                    operation: "GET /users/{id}".into(),
                    services: vec!["users".into()],
                    tokens: 100,
                    share: share_of(100, 2000),
                },
                EndpointCost {
                    operation: "POST /users".into(),
                    services: vec!["users".into(), "items".into()],
                    tokens: 400,
                    share: share_of(400, 2000),
                },
            ],
            schemas: vec![
                SchemaCost {
                    name: "Pet".into(),
                    uses: 3,
                    definition_tokens: 456,
                    inline_per_use_tokens: 450,
                    inline_total_tokens: 1350,
                    amplification: Some(1350.0 / 456.0),
                    cyclic: false,
                },
                SchemaCost {
                    name: "Node".into(),
                    uses: 1,
                    definition_tokens: 890,
                    inline_per_use_tokens: 120,
                    inline_total_tokens: 120,
                    amplification: Some(120.0 / 890.0),
                    cyclic: true,
                },
            ],
            endpoint_hotspots: vec![1, 0],
            schema_hotspots: vec![0, 1],
        }
    }

    #[test]
    fn hotspots_rank_descending_with_row_order_ties() {
        let items = vec![10usize, 30, 30, 5];
        let order = hotspot_indices(&items, 4, |&value| (value, 0));
        assert_eq!(order, vec![1, 2, 0, 3]);
        // The limit truncates after the stable ordering.
        assert_eq!(hotspot_indices(&items, 2, |&value| (value, 0)), vec![1, 2]);
    }

    #[test]
    fn hotspots_use_the_secondary_key_before_row_order() {
        let items = vec![(5usize, 100usize), (5, 200)];
        let order = hotspot_indices(&items, 2, |&(_, definition)| (0, definition));
        assert_eq!(order, vec![1, 0]);
    }

    #[test]
    fn schema_hotspots_rank_by_ratio_over_absolute_size() {
        let schemas = vec![
            // 1.1x amplification on the largest absolute inline total.
            SchemaCost {
                name: "Large".into(),
                uses: 1,
                definition_tokens: 1000,
                inline_per_use_tokens: 1100,
                inline_total_tokens: 1100,
                amplification: Some(1.1),
                cyclic: false,
            },
            // 10x amplification on a small absolute inline total.
            SchemaCost {
                name: "Small".into(),
                uses: 10,
                definition_tokens: 10,
                inline_per_use_tokens: 10,
                inline_total_tokens: 100,
                amplification: Some(10.0),
                cyclic: false,
            },
            // Ties Small on ratio and wins the inline-total tie-break.
            SchemaCost {
                name: "Twin".into(),
                uses: 1,
                definition_tokens: 100,
                inline_per_use_tokens: 1000,
                inline_total_tokens: 1000,
                amplification: Some(10.0),
                cyclic: false,
            },
            // No ratio (empty definition): always last.
            SchemaCost {
                name: "Empty".into(),
                uses: 5,
                definition_tokens: 0,
                inline_per_use_tokens: 4,
                inline_total_tokens: 20,
                amplification: None,
                cyclic: false,
            },
        ];
        // Ratio beats absolute size, ratio ties break on inline total, and
        // ratio-less rows rank last.
        assert_eq!(schema_hotspot_indices(&schemas, 4), vec![2, 1, 0, 3]);
        assert_eq!(schema_hotspot_indices(&schemas, 2), vec![2, 1]);
    }

    #[test]
    fn share_is_zero_without_a_total() {
        assert_eq!(share_of(123, 0), 0.0);
        assert_eq!(share_of(50, 200), 25.0);
    }

    #[test]
    fn report_labels_estimates_and_separates_the_inline_model() {
        let text = render_to_string(&sample_report());
        assert!(text.starts_with("TOKEN COST ANALYSIS\n"));
        assert!(text.contains("chars/4 estimates"));
        assert!(text.contains("not billing figures"));
        // The inline total is labeled a model, not the output's cost.
        assert!(text.contains("not the cost of the output in linked mode"));
        // Multi-tag measurement is documented.
        assert!(text.contains("measured once, under their first"));
    }

    #[test]
    fn report_prints_endpoint_rows_and_hotspots() {
        let text = render_to_string(&sample_report());
        assert!(text.contains("GET /users/{id}"), "{text}");
        // The multi-tag endpoint carries a suffix per tag after the first.
        assert!(text.contains("POST /users (+items)"), "{text}");
        assert!(
            text.contains("1. POST /users (+items) — ~400 tokens (20.0% of TOTAL)"),
            "{text}"
        );
        assert!(
            text.contains("2. GET /users/{id} — ~100 tokens (5.0% of TOTAL)"),
            "{text}"
        );
        assert!(
            text.contains("1. Pet — 3 use(s), ~1350 tokens inline vs ~456 read once"),
            "{text}"
        );
    }

    #[test]
    fn report_marks_cyclic_schemas() {
        let text = render_to_string(&sample_report());
        // The cycle note appears in the schema table and in the hotspot line.
        assert!(text.contains("cycle"), "{text}");
    }

    #[test]
    fn empty_report_is_headers_and_notes_only() {
        let text = render_to_string(&CostReport::default());
        assert!(
            text.contains("(no endpoints visible after filters)"),
            "{text}"
        );
        assert!(text.contains("(no component schemas rendered"), "{text}");
        assert!(text.contains("(nothing to rank)"), "{text}");
        assert!(text.contains("TOTAL: ~0 tokens"), "{text}");
    }

    #[test]
    fn inline_mode_is_labeled_in_the_mode_line() {
        let mut report = sample_report();
        report.inline_schemas = true;
        let text = render_to_string(&report);
        assert!(text.contains("schemas inline"), "{text}");
    }
}
