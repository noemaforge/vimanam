use std::collections::{HashMap, HashSet};
use std::io::Write;

use anyhow::Result;

use crate::models::{
    ApiDocumentation, DetailLevel, DocConfig, Endpoint, GroupBy, Response, Schema, SortMethod,
};
use crate::utils::{clean_for_id, extract_content_type};

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

        // Apply service filter if configured (case-insensitive)
        if let Some(services) = &config.service_filter {
            if !endpoint
                .services
                .iter()
                .any(|s| service_matches_filter(s, services))
            {
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
                sort_endpoints(&mut sorted_endpoints, &config.sort_method);

                for endpoint in sorted_endpoints {
                    write_endpoint(writer, endpoint, doc, config, true)?;
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
                if !endpoint
                    .services
                    .iter()
                    .any(|s| service_matches_filter(s, services))
                {
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

    sort_endpoints(&mut endpoints, &config.sort_method);

    writeln!(writer, "## Endpoints\n")?;
    for endpoint in endpoints {
        write_endpoint(writer, endpoint, doc, config, true)?;
    }

    Ok(())
}

/// Writes a single endpoint section; the amount of detail depends on `config.detail_level`.
fn write_endpoint<W: Write>(
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

#[derive(Debug)]
struct SchemaRow {
    field: String,
    type_name: String,
    required: String,
    description: String,
}

fn response_schema(response: &Response) -> Option<&Schema> {
    if let Some(schema) = &response.schema {
        return Some(schema);
    }

    response
        .content
        .as_ref()
        .and_then(|content| content.values().find_map(|media| media.schema.as_ref()))
}

fn write_schema_table<W: Write>(
    writer: &mut W,
    schema: &Schema,
    doc: &ApiDocumentation,
    root_label: &str,
) -> Result<()> {
    let mut rows = Vec::new();
    let mut ref_stack = Vec::new();
    collect_schema_rows(schema, doc, root_label, None, &mut rows, &mut ref_stack, 0);

    if rows.is_empty() {
        writeln!(writer, "*No schema fields available*")?;
        return Ok(());
    }

    writeln!(writer, "| Field | Type | Required | Description |")?;
    writeln!(writer, "|------|------|---------:|-------------|")?;
    for row in rows {
        writeln!(
            writer,
            "| `{}` | {} | {} | {} |",
            row.field,
            escape_table_cell(&row.type_name),
            row.required,
            escape_table_cell(&row.description)
        )?;
    }

    Ok(())
}

fn collect_schema_rows(
    schema: &Schema,
    doc: &ApiDocumentation,
    field: &str,
    required: Option<bool>,
    rows: &mut Vec<SchemaRow>,
    ref_stack: &mut Vec<String>,
    depth: usize,
) {
    const MAX_DEPTH: usize = 24;

    if depth >= MAX_DEPTH {
        rows.push(SchemaRow {
            field: field.to_string(),
            type_name: "truncated".to_string(),
            required: required_to_string(required).to_string(),
            description: "Maximum schema depth reached; nested expansion stopped".to_string(),
        });
        return;
    }

    if let Some(reference) = &schema.reference {
        if ref_stack.contains(reference) {
            rows.push(SchemaRow {
                field: field.to_string(),
                type_name: format!("ref {}", short_schema_reference(reference)),
                required: required_to_string(required).to_string(),
                description: "Cycle detected while expanding schema reference".to_string(),
            });
            return;
        }

        if let Some(resolved) = resolve_schema_reference(reference, doc) {
            ref_stack.push(reference.clone());
            collect_schema_rows(resolved, doc, field, required, rows, ref_stack, depth + 1);
            ref_stack.pop();
            return;
        }

        rows.push(SchemaRow {
            field: field.to_string(),
            type_name: format!("ref {}", short_schema_reference(reference)),
            required: required_to_string(required).to_string(),
            description: format!("Unresolved schema reference: {}", reference),
        });
        return;
    }

    let description = schema.description.as_deref().unwrap_or("-");
    rows.push(SchemaRow {
        field: field.to_string(),
        type_name: schema_type_label(schema),
        required: required_to_string(required).to_string(),
        description: description.to_string(),
    });

    if let Some(properties) = &schema.properties {
        let required_fields: HashSet<&str> = schema
            .required
            .as_ref()
            .map(|items| items.iter().map(String::as_str).collect())
            .unwrap_or_default();

        for (name, child_schema) in properties {
            let child_field = format_field(field, name);
            collect_schema_rows(
                child_schema,
                doc,
                &child_field,
                Some(required_fields.contains(name.as_str())),
                rows,
                ref_stack,
                depth + 1,
            );
        }
    }

    if let Some(items) = &schema.items {
        let item_field = format!("{}[]", field);
        collect_schema_rows(items, doc, &item_field, None, rows, ref_stack, depth + 1);
    }

    if let Some(all_of) = &schema.all_of {
        for (index, variant) in all_of.iter().enumerate() {
            let variant_field = format!("{}.allOf[{}]", field, index);
            collect_schema_rows(
                variant,
                doc,
                &variant_field,
                required,
                rows,
                ref_stack,
                depth + 1,
            );
        }
    }

    if let Some(one_of) = &schema.one_of {
        for (index, variant) in one_of.iter().enumerate() {
            let variant_field = format!("{}.oneOf[{}]", field, index);
            collect_schema_rows(
                variant,
                doc,
                &variant_field,
                required,
                rows,
                ref_stack,
                depth + 1,
            );
        }
    }

    if let Some(any_of) = &schema.any_of {
        for (index, variant) in any_of.iter().enumerate() {
            let variant_field = format!("{}.anyOf[{}]", field, index);
            collect_schema_rows(
                variant,
                doc,
                &variant_field,
                required,
                rows,
                ref_stack,
                depth + 1,
            );
        }
    }
}

fn resolve_schema_reference<'a>(reference: &str, doc: &'a ApiDocumentation) -> Option<&'a Schema> {
    if let Some(name) = reference.strip_prefix("#/components/schemas/") {
        return doc.schemas.get(&decode_json_pointer_token(name));
    }

    if let Some(name) = reference.strip_prefix("#/definitions/") {
        return doc.schemas.get(&decode_json_pointer_token(name));
    }

    None
}

fn schema_type_label(schema: &Schema) -> String {
    let mut label = if let Some(schema_type) = &schema.schema_type {
        if schema_type == "array" {
            let item_label = schema
                .items
                .as_deref()
                .map(schema_type_hint)
                .unwrap_or_else(|| "unknown".to_string());
            format!("array<{}>", item_label)
        } else if let Some(format) = &schema.format {
            format!("{}({})", schema_type, format)
        } else {
            schema_type.clone()
        }
    } else if schema.properties.is_some() {
        "object".to_string()
    } else if schema.items.is_some() {
        let item_label = schema
            .items
            .as_deref()
            .map(schema_type_hint)
            .unwrap_or_else(|| "unknown".to_string());
        format!("array<{}>", item_label)
    } else if schema.all_of.as_ref().is_some_and(|v| !v.is_empty()) {
        "allOf".to_string()
    } else if schema.one_of.as_ref().is_some_and(|v| !v.is_empty()) {
        "oneOf".to_string()
    } else if schema.any_of.as_ref().is_some_and(|v| !v.is_empty()) {
        "anyOf".to_string()
    } else if let Some(enum_values) = &schema.enum_values {
        format!("enum[{}]", enum_values.len())
    } else {
        "unknown".to_string()
    };

    if schema.nullable.unwrap_or(false) {
        label.push_str(" | null");
    }

    label
}

fn schema_type_hint(schema: &Schema) -> String {
    if let Some(reference) = &schema.reference {
        return format!("ref {}", short_schema_reference(reference));
    }

    if let Some(schema_type) = &schema.schema_type {
        return schema_type.clone();
    }

    if schema.properties.is_some() {
        return "object".to_string();
    }

    if schema.items.is_some() {
        return "array".to_string();
    }

    "unknown".to_string()
}

fn format_field(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        return child.to_string();
    }

    format!("{}.{}", parent, child)
}

fn required_to_string(required: Option<bool>) -> &'static str {
    match required {
        Some(true) => "Yes",
        Some(false) => "No",
        None => "-",
    }
}

fn short_schema_reference(reference: &str) -> String {
    reference
        .rsplit('/')
        .next()
        .map(decode_json_pointer_token)
        .unwrap_or_else(|| reference.to_string())
}

fn decode_json_pointer_token(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

fn escape_table_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', "<br/>")
}
