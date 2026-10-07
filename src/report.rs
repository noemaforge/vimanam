//! The spec hygiene report appended after the documentation (`--no-report`
//! suppresses it).
//!
//! [`analyze`] derives a data-oriented [`HygieneReport`] from the intermediate
//! representation, and [`write_report`] renders it as Markdown. The struct is
//! kept public and free of rendering concerns so other front ends (a TUI, for
//! instance) can consume the same counts and lists.
//!
//! # Scope
//!
//! The report covers exactly the endpoint set the rendered body used: the
//! endpoints left after `--service-filter`, `--path-filter`, `--method-filter`
//! and `--exclude-deprecated` are applied (see
//! [`visible_endpoints`](crate::markdown::visible_endpoints)), each counted once
//! even when the service-grouped view renders a multi-tag operation under
//! several services. The service count is the number of distinct services
//! among those endpoints that the service filter keeps, so a filtered document
//! reports only the services it actually contains. Detail lists follow the
//! configured `--sort` order (spec order for `--sort none`).

use std::fmt;
use std::io::Write;

use anyhow::Result;
use indexmap::{IndexMap, IndexSet};

use crate::markdown::{service_is_visible, visible_endpoints};
use crate::models::{ApiDocumentation, DocConfig, Endpoint};

/// An endpoint named the way the report lists it: `METHOD /path`. Also the
/// endpoint identity `diff` attributes changes to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EndpointRef {
    /// Uppercase HTTP method.
    pub method: String,
    /// Exact path template from the spec.
    pub path: String,
}

impl From<&Endpoint> for EndpointRef {
    fn from(endpoint: &Endpoint) -> Self {
        Self {
            // Methods are stored uppercase in the IR already; normalize anyway so
            // the report never depends on that invariant.
            method: endpoint.method.to_uppercase(),
            path: endpoint.path.clone(),
        }
    }
}

impl fmt::Display for EndpointRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.method, self.path)
    }
}

/// An `operationId` shared by more than one endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DuplicateOperationId {
    pub operation_id: String,
    /// The endpoints carrying this id, in report order.
    pub endpoints: Vec<EndpointRef>,
}

/// A parameter with no description, attributed to the endpoint it appears on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UndescribedParameter {
    pub endpoint: EndpointRef,
    pub name: String,
}

/// The outcome of every hygiene check over the visible endpoints. Each check is
/// a list; its count is the list length. Lists are in report order and are
/// never truncated, so the rendering is deterministic for a given spec+flags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HygieneReport {
    /// Endpoints analyzed (the same set the rendered body contains).
    pub endpoint_count: usize,
    /// Distinct services among those endpoints (after `--service-filter`).
    pub service_count: usize,
    /// Endpoints with neither a `summary` nor a `description`.
    pub missing_description: Vec<EndpointRef>,
    /// Endpoints without an `operationId`.
    pub missing_operation_id: Vec<EndpointRef>,
    /// Endpoints whose `responses` map is empty.
    pub no_responses: Vec<EndpointRef>,
    /// Endpoints marked `deprecated: true`.
    pub deprecated: Vec<EndpointRef>,
    /// Endpoints with no tags, attributed to the default service by the parser.
    pub untagged: Vec<EndpointRef>,
    /// `operationId`s used by more than one endpoint, sorted by id. The count
    /// for this check is the number of such ids, not of endpoints involved.
    pub duplicate_operation_ids: Vec<DuplicateOperationId>,
    /// Parameters (across all endpoints, including the synthetic request-body
    /// parameter) that have no description. A request body is listed at most
    /// once per endpoint, however many media types it offers.
    pub undescribed_parameters: Vec<UndescribedParameter>,
}

impl HygieneReport {
    /// The check labels and counts in the order the summary table lists them.
    pub fn counts(&self) -> [(&'static str, usize); 7] {
        [
            ("Missing description", self.missing_description.len()),
            ("Missing operationId", self.missing_operation_id.len()),
            ("No responses documented", self.no_responses.len()),
            ("Deprecated", self.deprecated.len()),
            ("Untagged (no service tag)", self.untagged.len()),
            ("Duplicate operationIds", self.duplicate_operation_ids.len()),
            (
                "Parameters without description",
                self.undescribed_parameters.len(),
            ),
        ]
    }
}

/// A description counts as missing when absent or blank: an empty string
/// documents nothing, so treating it as present would hide the gap.
fn is_blank(text: Option<&String>) -> bool {
    text.is_none_or(|t| t.trim().is_empty())
}

/// Runs every hygiene check over the endpoints the rendered body covers (see
/// the module docs for the exact scoping rules).
pub(crate) fn analyze(doc: &ApiDocumentation, config: &DocConfig) -> HygieneReport {
    let endpoints = visible_endpoints(doc, config);

    let mut report = HygieneReport {
        endpoint_count: endpoints.len(),
        ..HygieneReport::default()
    };

    // Ordered sets/maps keep first-appearance order so the output is stable.
    let mut services: IndexSet<&str> = IndexSet::new();
    let mut by_operation_id: IndexMap<&str, Vec<EndpointRef>> = IndexMap::new();

    for endpoint in endpoints {
        let reference = EndpointRef::from(endpoint);

        for service in &endpoint.services {
            if service_is_visible(service, config) {
                services.insert(service);
            }
        }

        if is_blank(endpoint.summary.as_ref()) && is_blank(endpoint.description.as_ref()) {
            report.missing_description.push(reference.clone());
        }

        match &endpoint.operation_id {
            None => report.missing_operation_id.push(reference.clone()),
            Some(id) => by_operation_id
                .entry(id.as_str())
                .or_default()
                .push(reference.clone()),
        }

        if endpoint.responses.is_empty() {
            report.no_responses.push(reference.clone());
        }

        if endpoint.deprecated {
            report.deprecated.push(reference.clone());
        }

        if endpoint.untagged {
            report.untagged.push(reference.clone());
        }

        // The parser emits one synthetic `body` parameter per `requestBody`
        // media type, all sharing the request body's description, so an
        // undescribed body is reported once per endpoint rather than once per
        // media type. An OAS2 `in: body` parameter arrives the same way (the
        // spec allows one per operation) and is likewise counted once.
        let mut seen_body = false;
        for parameter in &endpoint.parameters {
            if parameter.parameter_in == "body" {
                if seen_body {
                    continue;
                }
                seen_body = true;
            }
            if is_blank(parameter.description.as_ref()) {
                report.undescribed_parameters.push(UndescribedParameter {
                    endpoint: reference.clone(),
                    name: parameter.name.clone(),
                });
            }
        }
    }

    report.service_count = services.len();

    let mut duplicates: Vec<DuplicateOperationId> = by_operation_id
        .into_iter()
        .filter(|(_, endpoints)| endpoints.len() > 1)
        .map(|(operation_id, endpoints)| DuplicateOperationId {
            operation_id: operation_id.to_string(),
            endpoints,
        })
        .collect();
    duplicates.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
    report.duplicate_operation_ids = duplicates;

    report
}

/// Renders the report as Markdown: a horizontal rule separating it from the
/// documentation body, a summary table of every check, then a detail list for
/// each check with a non-zero count, in table order.
///
/// The leading newline assumes the body before it ends with exactly one
/// newline, giving one blank line before the rule; `main::write_output`
/// normalizes the body's trailing newlines to guarantee that.
pub(crate) fn write_report<W: Write>(writer: &mut W, report: &HygieneReport) -> Result<()> {
    writeln!(writer, "\n---\n")?;
    writeln!(writer, "## Spec Hygiene Report\n")?;
    writeln!(
        writer,
        "**{}** across **{}**\n",
        pluralize(report.endpoint_count, "endpoint"),
        pluralize(report.service_count, "service"),
    )?;

    writeln!(writer, "| Check | Count |")?;
    writeln!(writer, "|-------|------:|")?;
    for (label, count) in report.counts() {
        writeln!(writer, "| {label} | {count} |")?;
    }

    write_endpoint_list(writer, "Missing description", &report.missing_description)?;
    write_endpoint_list(writer, "Missing operationId", &report.missing_operation_id)?;
    write_endpoint_list(writer, "No responses documented", &report.no_responses)?;
    write_endpoint_list(writer, "Deprecated", &report.deprecated)?;
    write_endpoint_list(writer, "Untagged (no service tag)", &report.untagged)?;

    if !report.duplicate_operation_ids.is_empty() {
        write_detail_heading(
            writer,
            "Duplicate operationIds",
            report.duplicate_operation_ids.len(),
        )?;
        for duplicate in &report.duplicate_operation_ids {
            writeln!(writer, "- `{}`", duplicate.operation_id)?;
            for endpoint in &duplicate.endpoints {
                writeln!(writer, "  - `{endpoint}`")?;
            }
        }
    }

    if !report.undescribed_parameters.is_empty() {
        write_detail_heading(
            writer,
            "Parameters without description",
            report.undescribed_parameters.len(),
        )?;
        for parameter in &report.undescribed_parameters {
            writeln!(writer, "- `{}` — `{}`", parameter.endpoint, parameter.name)?;
        }
    }

    Ok(())
}

/// Writes a detail section listing endpoints, or nothing when the list is empty.
fn write_endpoint_list<W: Write>(
    writer: &mut W,
    label: &str,
    endpoints: &[EndpointRef],
) -> Result<()> {
    if endpoints.is_empty() {
        return Ok(());
    }
    write_detail_heading(writer, label, endpoints.len())?;
    for endpoint in endpoints {
        writeln!(writer, "- `{endpoint}`")?;
    }
    Ok(())
}

fn write_detail_heading<W: Write>(writer: &mut W, label: &str, count: usize) -> Result<()> {
    writeln!(writer, "\n### {label} ({count})")?;
    Ok(())
}

/// `1 endpoint`, `2 endpoints`. All nouns used here pluralize with a plain `s`.
fn pluralize(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}
