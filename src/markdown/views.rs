//! The grouping views: one `generate_*` function per grouping mode, plus the
//! filtering and sorting helpers they share. Each renders the document preamble
//! (title, servers, auth) and delegates per-endpoint rendering to
//! [`write_endpoint`](super::endpoint::write_endpoint).

use std::collections::HashMap;
use std::io::Write;

use anyhow::Result;
use indexmap::IndexMap;

use crate::models::{ApiDocumentation, DocConfig, Endpoint, SortMethod};
use crate::utils::clean_for_id;

use super::endpoint::{get_short_title, write_endpoint};

/// Sorts endpoints in place using the configured method.
///
/// Every view sorts through this single function so that, within a document,
/// the table of contents and the body sections always share one ordering
/// (otherwise TOC anchor links can point to a different sequence than the
/// sections themselves).
fn sort_endpoints(endpoints: &mut [&Endpoint], sort_method: &SortMethod) {
    match sort_method {
        SortMethod::Alphabetical => {
            endpoints.sort_by(|a, b| a.path.cmp(&b.path).then(a.method.cmp(&b.method)));
        }
        SortMethod::PathLength => {
            endpoints.sort_by_key(|a| a.path.len());
        }
        SortMethod::None => {}
    }
}

/// Case-insensitive membership test for `--service-filter` against a service
/// (tag) name. Mirrors the case-insensitivity of `--method-filter` so that a
/// case mismatch doesn't silently produce empty output.
fn service_matches_filter(name: &str, filter: &[String]) -> bool {
    filter.iter().any(|entry| entry.eq_ignore_ascii_case(name))
}

/// Whether an endpoint survives the deprecated, method, and path filters that
/// every view applies. The service filter is handled separately
/// ([`passes_service_filter`]): the service-grouped views narrow their service
/// list instead of filtering endpoints.
fn passes_filters(endpoint: &Endpoint, config: &DocConfig) -> bool {
    if config.exclude_deprecated && endpoint.deprecated {
        return false;
    }
    if let Some(methods) = &config.method_filter {
        if !methods.contains(&endpoint.method) {
            return false;
        }
    }
    if let Some(path_pattern) = &config.path_filter {
        if !endpoint.path.contains(path_pattern) {
            return false;
        }
    }
    true
}

/// Whether an endpoint belongs to one of the `--service-filter` services. Always
/// true when no service filter is set.
fn passes_service_filter(endpoint: &Endpoint, config: &DocConfig) -> bool {
    match &config.service_filter {
        Some(filter) => endpoint
            .services
            .iter()
            .any(|s| service_matches_filter(s, filter)),
        None => true,
    }
}

/// Writes the document preamble shared by every view: title, description, API
/// version, and—when `--include-auth` is set—the server URLs and authentication
/// schemes.
fn write_preamble<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<()> {
    writeln!(writer, "# {}", doc.title)?;
    if let Some(description) = &doc.description {
        writeln!(writer, "\n{}\n", description)?;
    }
    writeln!(writer, "API Version: {}\n", doc.version)?;

    // Add server URLs if available
    if !doc.servers.is_empty() && config.include_auth {
        writeln!(writer, "## Server URLs")?;
        for server in &doc.servers {
            writeln!(writer, "* {}", server)?;
        }
        writeln!(writer)?;
    }

    // Add security schemes if available
    if !doc.security_schemes.is_empty() && config.include_auth {
        writeln!(writer, "## Authentication")?;
        for (name, desc) in &doc.security_schemes {
            writeln!(writer, "* **{}**: {}", name, desc)?;
        }
        writeln!(writer)?;
    }

    Ok(())
}

/// Generates the `--detail summary` view: a compact list of services and their operations.
pub(super) fn generate_summary<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<()> {
    write_preamble(writer, doc, config)?;

    // Filter services if needed (case-insensitive)
    let services = if let Some(filter) = &config.service_filter {
        doc.services
            .iter()
            .filter(|s| service_matches_filter(&s.name, filter))
            .collect::<Vec<_>>()
    } else {
        doc.services.iter().collect()
    };

    // Group endpoints by service
    let mut service_endpoints: HashMap<&str, Vec<&Endpoint>> = HashMap::new();
    for endpoint in &doc.endpoints {
        if !passes_filters(endpoint, config) {
            continue;
        }

        for service_name in &endpoint.services {
            service_endpoints
                .entry(service_name)
                .or_default()
                .push(endpoint);
        }
    }

    // Write Services List
    writeln!(writer, "## Services")?;

    for service in &services {
        writeln!(writer, "- {}", service.name)?;

        // Add operation links under each service
        if let Some(endpoints) = service_endpoints.get(&service.name as &str) {
            let mut sorted_ops = endpoints.clone();
            sort_endpoints(&mut sorted_ops, &config.sort_method);

            for endpoint in sorted_ops {
                let op_name = if let Some(operation_id) = &endpoint.operation_id {
                    // Clean up the operation ID by removing the service name prefix if present
                    if operation_id.starts_with(&format!("{}_", service.name)) {
                        // Remove the "ServiceName_" prefix
                        operation_id.replacen(&format!("{}_", service.name), "", 1)
                    } else {
                        operation_id.clone()
                    }
                } else {
                    // Fallback if no operation ID
                    format!("{} {}", endpoint.method, endpoint.path)
                };

                writeln!(writer, "  * {}", op_name)?;
            }
        }
    }

    Ok(())
}

/// Generates documentation grouped by service (tag), one `##` section per service.
pub(super) fn generate_by_service<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<()> {
    write_preamble(writer, doc, config)?;

    // Filter services if needed (case-insensitive)
    let services = if let Some(filter) = &config.service_filter {
        doc.services
            .iter()
            .filter(|s| service_matches_filter(&s.name, filter))
            .collect::<Vec<_>>()
    } else {
        doc.services.iter().collect()
    };

    // Group endpoints by service - MOVED THIS UP before TOC generation
    let mut service_endpoints: HashMap<&str, Vec<&Endpoint>> = HashMap::new();
    for endpoint in &doc.endpoints {
        if !passes_filters(endpoint, config) {
            continue;
        }

        for service_name in &endpoint.services {
            service_endpoints
                .entry(service_name)
                .or_default()
                .push(endpoint);
        }
    }

    // Table of Contents (if enabled)
    if config.include_toc {
        writeln!(writer, "## Services\n")?;
        for service in &services {
            let anchor = clean_for_id(&service.name);
            writeln!(writer, "- [{}](#{anchor})", service.name)?;

            // Add operation links under each service
            if let Some(endpoints) = service_endpoints.get(&service.name as &str) {
                let mut sorted_ops = endpoints.clone();
                sort_endpoints(&mut sorted_ops, &config.sort_method);

                for endpoint in sorted_ops {
                    // Extract a shorter title for the TOC entry
                    let op_title = get_short_title(endpoint);
                    let op_anchor = clean_for_id(&op_title);
                    writeln!(writer, "  * [{}](#{op_anchor})", op_title)?;
                }
            }
        }
        writeln!(writer)?;
    }

    // Write each service section
    for service in &services {
        // Create anchor but use it directly in the writeln! call
        let anchor = clean_for_id(&service.name);
        writeln!(writer, "## {} {{#{}}}", service.name, anchor)?;

        if let Some(description) = &service.description {
            writeln!(writer, "\n{}", description)?;
        }

        // Get endpoints for this service
        if let Some(endpoints) = service_endpoints.get(&service.name as &str) {
            // Sort endpoints as configured
            let mut sorted_endpoints = endpoints.clone();
            sort_endpoints(&mut sorted_endpoints, &config.sort_method);

            for endpoint in sorted_endpoints {
                write_endpoint(writer, endpoint, doc, config, true)?;
            }
        } else {
            writeln!(writer, "\nNo endpoints found for this service.\n")?;
        }
    }

    Ok(())
}

/// Generates documentation grouped by HTTP method, one `##` section per method.
pub(super) fn generate_by_method<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<()> {
    write_preamble(writer, doc, config)?;

    // Group endpoints by method
    let mut method_endpoints: HashMap<&str, Vec<&Endpoint>> = HashMap::new();
    for endpoint in &doc.endpoints {
        if !passes_filters(endpoint, config) || !passes_service_filter(endpoint, config) {
            continue;
        }

        method_endpoints
            .entry(&endpoint.method)
            .or_default()
            .push(endpoint);
    }

    // Table of Contents (if enabled)
    if config.include_toc {
        writeln!(writer, "## HTTP Methods\n")?;
        for method in [
            "GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "HEAD", "TRACE",
        ] {
            if let Some(endpoints) = method_endpoints.get(method) {
                if !endpoints.is_empty() {
                    let anchor = clean_for_id(method);
                    writeln!(writer, "- [{}](#{anchor})", method)?;
                }
            }
        }
        writeln!(writer)?;
    }

    // Write each method section
    for method in [
        "GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "HEAD", "TRACE",
    ] {
        if let Some(endpoints) = method_endpoints.get(method) {
            if !endpoints.is_empty() {
                let anchor = clean_for_id(method);
                writeln!(writer, "## {} {{#{}}}", method, anchor)?;

                // Sort endpoints as configured
                let mut sorted_endpoints = endpoints.clone();
                sort_endpoints(&mut sorted_endpoints, &config.sort_method);

                for endpoint in sorted_endpoints {
                    write_endpoint(writer, endpoint, doc, config, true)?;
                }
            }
        }
    }

    Ok(())
}

/// Generates documentation grouped by path (`--group-by path`), one `##`
/// section per path with its methods listed underneath. Paths appear in spec
/// order (first appearance), preserving the output-determinism invariant.
pub(super) fn generate_by_path<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<()> {
    write_preamble(writer, doc, config)?;

    // Group endpoints by path. IndexMap keeps first-appearance (spec) order.
    let mut path_endpoints: IndexMap<&str, Vec<&Endpoint>> = IndexMap::new();
    for endpoint in &doc.endpoints {
        if !passes_filters(endpoint, config) || !passes_service_filter(endpoint, config) {
            continue;
        }

        path_endpoints
            .entry(endpoint.path.as_str())
            .or_default()
            .push(endpoint);
    }

    // Table of Contents (if enabled)
    if config.include_toc {
        writeln!(writer, "## Paths\n")?;
        for (path, endpoints) in &path_endpoints {
            let anchor = clean_for_id(path);
            writeln!(writer, "- [{}](#{anchor})", path)?;

            let mut sorted_ops = endpoints.clone();
            sort_endpoints(&mut sorted_ops, &config.sort_method);
            for endpoint in sorted_ops {
                let op_title = get_short_title(endpoint);
                let op_anchor = clean_for_id(&op_title);
                writeln!(writer, "  * [{}](#{op_anchor})", op_title)?;
            }
        }
        writeln!(writer)?;
    }

    // Write each path section
    for (path, endpoints) in &path_endpoints {
        let anchor = clean_for_id(path);
        writeln!(writer, "## {} {{#{}}}", path, anchor)?;

        let mut sorted_endpoints = endpoints.clone();
        sort_endpoints(&mut sorted_endpoints, &config.sort_method);
        for endpoint in sorted_endpoints {
            write_endpoint(writer, endpoint, doc, config, true)?;
        }
    }

    Ok(())
}

/// Generates a flat endpoint list (`--flat`) with no grouping hierarchy.
pub(super) fn generate_flat<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<()> {
    write_preamble(writer, doc, config)?;

    // Collect endpoints, applying the same filters as the grouped views
    let mut endpoints: Vec<&Endpoint> = doc
        .endpoints
        .iter()
        .filter(|endpoint| {
            passes_filters(endpoint, config) && passes_service_filter(endpoint, config)
        })
        .collect();

    sort_endpoints(&mut endpoints, &config.sort_method);

    writeln!(writer, "## Endpoints\n")?;
    for endpoint in endpoints {
        write_endpoint(writer, endpoint, doc, config, true)?;
    }

    Ok(())
}
