use std::collections::{HashMap, HashSet};
use std::io::Write;

use anyhow::Result;

use crate::models::{ApiDocumentation, DetailLevel, DocConfig, Endpoint, GroupBy};
use crate::utils::{clean_for_id, extract_content_type};

/// Renders the documentation to `writer`, dispatching on detail level and grouping mode.
pub fn generate_markdown<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<()> {
    // For summary level, just generate the TOC
    if config.detail_level == DetailLevel::Summary {
        generate_summary(writer, doc, config)
    } else {
        // For other detail levels, use the existing grouping logic
        match config.group_by {
            GroupBy::Service => generate_by_service(writer, doc, config),
            GroupBy::Method => generate_by_method(writer, doc, config),
            GroupBy::Flat => generate_flat(writer, doc, config),
        }
    }
}

/// Generates the `--detail summary` view: a compact list of services and their operations.
fn generate_summary<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<()> {
    // Write title
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

    // Filter services if needed
    let services = if let Some(filter) = &config.service_filter {
        let filter_set: HashSet<_> = filter.iter().collect();
        doc.services
            .iter()
            .filter(|s| filter_set.contains(&s.name))
            .collect::<Vec<_>>()
    } else {
        doc.services.iter().collect()
    };

    // Group endpoints by service
    let mut service_endpoints: HashMap<&str, Vec<&Endpoint>> = HashMap::new();
    for endpoint in &doc.endpoints {
        // Skip deprecated endpoints if configured
        if config.exclude_deprecated && endpoint.deprecated {
            continue;
        }

        // Apply method filter if configured
        if let Some(methods) = &config.method_filter {
            if !methods.contains(&endpoint.method) {
                continue;
            }
        }

        // Apply path filter if configured
        if let Some(path_pattern) = &config.path_filter {
            if !endpoint.path.contains(path_pattern) {
                continue;
            }
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
            match config.sort_method {
                crate::models::SortMethod::Alphabetical => {
                    sorted_ops.sort_by(|a, b| {
                        a.operation_id
                            .clone()
                            .unwrap_or_default()
                            .cmp(&b.operation_id.clone().unwrap_or_default())
                    });
                }
                crate::models::SortMethod::PathLength => {
                    sorted_ops.sort_by_key(|a| a.path.len());
                }
                crate::models::SortMethod::None => {}
            }

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
fn generate_by_service<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<()> {
    // Write title
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

    // Filter services if needed
    let services = if let Some(filter) = &config.service_filter {
        let filter_set: HashSet<_> = filter.iter().collect();
        doc.services
            .iter()
            .filter(|s| filter_set.contains(&s.name))
            .collect::<Vec<_>>()
    } else {
        doc.services.iter().collect()
    };

    // Group endpoints by service - MOVED THIS UP before TOC generation
    let mut service_endpoints: HashMap<&str, Vec<&Endpoint>> = HashMap::new();
    for endpoint in &doc.endpoints {
        // Skip deprecated endpoints if configured
        if config.exclude_deprecated && endpoint.deprecated {
            continue;
        }

        // Apply method filter if configured
        if let Some(methods) = &config.method_filter {
            if !methods.contains(&endpoint.method) {
                continue;
            }
        }

        // Apply path filter if configured
        if let Some(path_pattern) = &config.path_filter {
            if !endpoint.path.contains(path_pattern) {
                continue;
            }
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
                match config.sort_method {
                    crate::models::SortMethod::Alphabetical => {
                        sorted_ops.sort_by(|a, b| {
                            a.summary
                                .clone()
                                .unwrap_or_default()
                                .cmp(&b.summary.clone().unwrap_or_default())
                        });
                    }
                    crate::models::SortMethod::PathLength => {
                        sorted_ops.sort_by_key(|a| a.path.len());
                    }
                    crate::models::SortMethod::None => {}
                }

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
            match config.sort_method {
                crate::models::SortMethod::Alphabetical => {
                    sorted_endpoints.sort_by(|a, b| a.path.cmp(&b.path));
                }
                crate::models::SortMethod::PathLength => {
                    sorted_endpoints.sort_by_key(|a| a.path.len());
                }
                crate::models::SortMethod::None => {}
            }

            for endpoint in sorted_endpoints {
                write_endpoint(writer, endpoint, config, true)?;
            }
        } else {
            writeln!(writer, "\nNo endpoints found for this service.\n")?;
        }
    }

    Ok(())
}

/// Generates documentation grouped by HTTP method, one `##` section per method.
fn generate_by_method<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<()> {
    // Write title
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

    // Group endpoints by method
    let mut method_endpoints: HashMap<&str, Vec<&Endpoint>> = HashMap::new();
    for endpoint in &doc.endpoints {
        // Skip deprecated endpoints if configured
        if config.exclude_deprecated && endpoint.deprecated {
            continue;
        }

        // Apply service filter if configured
        if let Some(services) = &config.service_filter {
            if !endpoint.services.iter().any(|s| services.contains(s)) {
                continue;
            }
        }

        // Apply path filter if configured
        if let Some(path_pattern) = &config.path_filter {
            if !endpoint.path.contains(path_pattern) {
                continue;
            }
        }

        // Apply method filter if configured
        if let Some(methods) = &config.method_filter {
            if !methods.contains(&endpoint.method) {
                continue;
            }
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
                match config.sort_method {
                    crate::models::SortMethod::Alphabetical => {
                        sorted_endpoints.sort_by(|a, b| a.path.cmp(&b.path));
                    }
                    crate::models::SortMethod::PathLength => {
                        sorted_endpoints.sort_by_key(|a| a.path.len());
                    }
                    crate::models::SortMethod::None => {}
                }

                for endpoint in sorted_endpoints {
                    write_endpoint(writer, endpoint, config, true)?;
                }
            }
        }
    }

    Ok(())
}

/// Generates a flat endpoint list (`--flat`) with no grouping hierarchy.
fn generate_flat<W: Write>(
    writer: &mut W,
    doc: &ApiDocumentation,
    config: &DocConfig,
) -> Result<()> {
    // Write title
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

    // Collect endpoints, applying the same filters as the grouped views
    let mut endpoints: Vec<&Endpoint> = doc
        .endpoints
        .iter()
        .filter(|endpoint| {
            if config.exclude_deprecated && endpoint.deprecated {
                return false;
            }
            if let Some(services) = &config.service_filter {
                if !endpoint.services.iter().any(|s| services.contains(s)) {
                    return false;
                }
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
        })
        .collect();

    match config.sort_method {
        crate::models::SortMethod::Alphabetical => {
            endpoints.sort_by(|a, b| a.path.cmp(&b.path).then(a.method.cmp(&b.method)));
        }
        crate::models::SortMethod::PathLength => {
            endpoints.sort_by_key(|a| a.path.len());
        }
        crate::models::SortMethod::None => {}
    }

    writeln!(writer, "## Endpoints\n")?;
    for endpoint in endpoints {
        write_endpoint(writer, endpoint, config, true)?;
    }

    Ok(())
}

/// Writes a single endpoint section; the amount of detail depends on `config.detail_level`.
fn write_endpoint<W: Write>(
    writer: &mut W,
    endpoint: &Endpoint,
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
                    param.name(),
                    param.location(),
                    required_str,
                    desc
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
                .find(|p| p.location() == "body" && p.schema.is_some());

            if let Some(param) = body_param {
                if let Some(schema) = &param.schema {
                    if let Some(schema_type) = &schema.schema_type {
                        writeln!(writer, "```json\n// Schema type: {}\n```", schema_type)?;
                    } else if let Some(ref_val) = &schema.reference {
                        writeln!(writer, "```json\n// Reference: {}\n```", ref_val)?;
                    }
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
                if let Some(schema) = &response.schema {
                    if let Some(schema_type) = &schema.schema_type {
                        writeln!(writer, "```json\n// Schema type: {}\n```", schema_type)?;
                    } else if let Some(ref_val) = &schema.reference {
                        writeln!(writer, "```json\n// Reference: {}\n```", ref_val)?;
                    }
                } else if let Some(content) = &response.content {
                    if let Some((content_type, media_type)) = content.iter().next() {
                        if media_type.schema.is_some() {
                            writeln!(writer, "```json\n// Content type: {}\n```", content_type)?;
                        }
                    }
                } else {
                    writeln!(writer, "*No response schema available*")?;
                }
            } else {
                writeln!(writer, "*No success response schema available*")?;
            }
        }

        // Add examples if configured
        if config.include_examples && config.detail_level == DetailLevel::Full {
            writeln!(writer, "\n#### Examples")?;
            writeln!(writer, "*Examples would be included here if available*")?;
        }
    }

    writeln!(writer)?; // End with a blank line
    Ok(())
}

/// Returns a short endpoint title: operation ID, else a name derived from the
/// summary, else `METHOD /path`.
fn get_short_title(endpoint: &Endpoint) -> String {
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
