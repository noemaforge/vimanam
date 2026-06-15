use anyhow::{Context, Result};
use indexmap::{IndexMap, IndexSet};
use log::{debug, warn};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::models::{
    ApiDocumentation, Endpoint, OpenApiSpec, Parameter, Response, Schema, Service,
};
use crate::utils::{
    extract_security_schemes, extract_servers, resolve_parameter_ref, resolve_response_ref,
};

/// Parses an OpenAPI 2.0/3.0 JSON file into the spec-version-agnostic
/// [`ApiDocumentation`] intermediate representation. On deserialization
/// failure, re-parses as generic JSON to produce a targeted error message.
pub fn parse_openapi<P: AsRef<Path>>(path: P) -> Result<ApiDocumentation> {
    let path_ref = path.as_ref();
    let file = File::open(path_ref).context("Failed to open OpenAPI file")?;
    let mut reader = BufReader::new(file);

    // First, try to parse as OpenAPI spec
    match serde_json::from_reader(&mut reader) as Result<OpenApiSpec, _> {
        Ok(spec) => {
            // Validate the parsed spec
            validate_openapi(&spec, path_ref)?;

            // Extract services and endpoints
            let services = extract_services(&spec);
            debug!("Extracted {} services", services.len());

            // Extract servers information
            let servers = extract_servers(&spec);
            debug!("Extracted {} server URLs", servers.len());

            // Extract security schemes
            let security_schemes = extract_security_schemes(&spec);
            debug!("Extracted {} security schemes", security_schemes.len());

            let endpoints = extract_endpoints(&spec, &services);
            debug!("Extracted {} endpoints", endpoints.len());
            let schemas = extract_schemas(&spec);
            debug!("Extracted {} reusable schemas", schemas.len());

            Ok(ApiDocumentation {
                title: spec.info.title,
                version: spec.info.version,
                description: spec.info.description,
                services,
                endpoints,
                servers,
                security_schemes,
                schemas,
            })
        }
        Err(err) => {
            // Rewind the file to try other parsing methods
            reader.seek(SeekFrom::Start(0))?;

            // Read the file content for better error analysis
            let mut content = String::new();
            reader.read_to_string(&mut content)?;

            // Try to parse as generic JSON to provide better error messages
            match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(json) => {
                    // Check for common issues
                    if !json.is_object() {
                        return Err(anyhow::anyhow!("Root element is not a JSON object"));
                    }

                    let obj = json.as_object().unwrap();

                    if !obj.contains_key("swagger") && !obj.contains_key("openapi") {
                        return Err(anyhow::anyhow!(
                            "Missing 'swagger' or 'openapi' field - not a valid OpenAPI specification"
                        ));
                    }

                    if !obj.contains_key("paths") {
                        return Err(anyhow::anyhow!(
                            "Missing 'paths' field - not a valid OpenAPI specification"
                        ));
                    }

                    if !obj.contains_key("info") {
                        return Err(anyhow::anyhow!(
                            "Missing 'info' field - not a valid OpenAPI specification"
                        ));
                    }

                    // If we got here, there's a structural issue with the spec
                    Err(anyhow::anyhow!(
                        "Invalid OpenAPI specification structure: {}",
                        err
                    ))
                }
                Err(_) => {
                    // Not even valid JSON
                    Err(anyhow::anyhow!("File is not valid JSON: {}", err))
                }
            }
        }
    }
}

/// Logs warnings for missing-but-tolerated spec fields (version, title, paths).
fn validate_openapi(spec: &OpenApiSpec, path: &Path) -> Result<()> {
    // Log the OpenAPI version
    if let Some(version) = &spec.spec_version {
        debug!("OpenAPI specification version: {}", version);
    } else {
        warn!(
            "OpenAPI specification version not found in {}, continuing anyway",
            path.display()
        );
    }

    // Check for required fields
    if spec.info.title.is_empty() {
        warn!("OpenAPI specification is missing a title");
    }

    if spec.info.version.is_empty() {
        warn!("OpenAPI specification is missing a version");
    }

    if spec.paths.is_empty() {
        warn!("OpenAPI specification has no paths defined");
    }

    Ok(())
}

/// Derives "services" from spec-level tags, falling back to per-operation
/// tags, then to a single default `"API"` service.
fn extract_services(spec: &OpenApiSpec) -> Vec<Service> {
    // Extract services from tags
    let mut services = Vec::new();

    if let Some(tags) = &spec.tags {
        for tag in tags {
            services.push(Service {
                name: tag.name.clone(),
                description: tag.description.clone(),
            });
        }
    }

    // If no tags, try to infer services from endpoint tags.
    // IndexSet keeps first-appearance order so output is deterministic.
    if services.is_empty() {
        let mut service_names = IndexSet::new();

        for (_, path_item) in &spec.paths {
            for op in [
                &path_item.get,
                &path_item.post,
                &path_item.put,
                &path_item.delete,
                &path_item.options,
                &path_item.head,
                &path_item.patch,
                &path_item.trace,
            ]
            .into_iter()
            .flatten()
            {
                if let Some(tags) = &op.tags {
                    for tag in tags {
                        service_names.insert(tag.clone());
                    }
                }
            }
        }

        // If still no services found, add an "API" default service
        if service_names.is_empty() {
            service_names.insert("API".to_string());
        }

        // Convert IndexSet to Vec of Services
        for name in service_names {
            services.push(Service {
                name,
                description: None,
            });
        }
    }

    services
}

/// Flattens every operation under `paths` into an [`Endpoint`], merging
/// path-level and operation-level parameters, resolving `$ref`s, and
/// representing an OpenAPI 3.0 `requestBody` as a synthetic `body` parameter.
fn extract_endpoints(spec: &OpenApiSpec, services: &[Service]) -> Vec<Endpoint> {
    let mut endpoints = Vec::new();

    // A map of service names to ensure all endpoints are associated with valid services
    let service_map: HashSet<String> = services.iter().map(|s| s.name.clone()).collect();

    for (path, path_item) in &spec.paths {
        let operations = [
            ("get", &path_item.get),
            ("post", &path_item.post),
            ("put", &path_item.put),
            ("delete", &path_item.delete),
            ("options", &path_item.options),
            ("head", &path_item.head),
            ("patch", &path_item.patch),
            ("trace", &path_item.trace),
        ];

        // Get parameters defined at the path level and resolve any references
        let path_parameters = path_item
            .parameters
            .as_ref()
            .map(|params| {
                params
                    .iter()
                    .filter_map(|p| resolve_parameter_ref(spec, p))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        for (method, operation_opt) in operations {
            if let Some(operation) = operation_opt {
                // Extract service tags with fallback
                let service_tags = if let Some(tags) = &operation.tags {
                    // Filter to only include valid services
                    let filtered_tags: Vec<String> = tags
                        .iter()
                        .filter(|tag| service_map.contains(*tag))
                        .cloned()
                        .collect();

                    // If all tags were filtered out, use fallback
                    if filtered_tags.is_empty() {
                        if !service_map.is_empty() {
                            vec![services[0].name.clone()]
                        } else {
                            vec!["API".to_string()]
                        }
                    } else {
                        filtered_tags
                    }
                } else {
                    // If no tags, use the first service or "API"
                    if !service_map.is_empty() {
                        vec![services[0].name.clone()]
                    } else {
                        vec!["API".to_string()]
                    }
                };

                // Combine path-level and operation-level parameters with reference resolution
                let mut parameters = path_parameters.clone();

                if let Some(op_params) = &operation.parameters {
                    for param in op_params {
                        if let Some(resolved_param) = resolve_parameter_ref(spec, param) {
                            parameters.push(resolved_param);
                        }
                    }
                }

                // Handle request body as a parameter (for OpenAPI 3.0).
                // Bodies are optional unless the spec says required: true.
                if let Some(req_body) = &operation.request_body {
                    if let Some((_, media_type)) = req_body.content.first() {
                        parameters.push(Parameter {
                            name: "requestBody".to_string(),
                            description: req_body.description.clone(),
                            parameter_in: "body".to_string(),
                            required: Some(req_body.required.unwrap_or(false)),
                            schema: media_type.schema.clone(),
                            extensions: HashMap::new(),
                        });
                    }
                }

                // Resolve references in responses
                let resolved_responses: IndexMap<String, Response> = operation
                    .responses
                    .iter()
                    .map(|(status_code, response)| {
                        let resolved = resolve_response_ref(spec, response)
                            .unwrap_or_else(|| response.clone());
                        (status_code.clone(), resolved)
                    })
                    .collect();

                endpoints.push(Endpoint {
                    path: path.clone(),
                    method: method.to_uppercase(),
                    services: service_tags,
                    summary: operation.summary.clone(),
                    description: operation.description.clone(),
                    operation_id: operation.operation_id.clone(),
                    parameters,
                    responses: resolved_responses,
                    deprecated: operation.deprecated.unwrap_or(false),
                });
            }
        }
    }

    endpoints
}

/// Collects reusable schemas from OpenAPI 3 `components.schemas` and
/// OpenAPI 2 `definitions` into a deterministic registry.
fn extract_schemas(spec: &OpenApiSpec) -> IndexMap<String, Schema> {
    let mut schemas = IndexMap::new();

    if let Some(components) = &spec.components {
        if let Some(component_schemas) = &components.schemas {
            for (name, schema) in component_schemas {
                schemas.insert(name.clone(), schema.clone());
            }
        }
    }

    if let Some(definitions) = spec
        .extensions
        .get("definitions")
        .and_then(|v| v.as_object())
    {
        for (name, raw_schema) in definitions {
            if schemas.contains_key(name) {
                continue;
            }

            match serde_json::from_value::<Schema>(raw_schema.clone()) {
                Ok(schema) => {
                    schemas.insert(name.clone(), schema);
                }
                Err(err) => {
                    warn!(
                        "Skipping definition '{}': failed to parse schema: {}",
                        name, err
                    );
                }
            }
        }
    }

    schemas
}
