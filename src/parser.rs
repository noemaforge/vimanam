use anyhow::{Context, Result};
use indexmap::{IndexMap, IndexSet};
use log::{debug, warn};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::models::{
    ApiDocumentation, Endpoint, Example, OpenApiSpec, Parameter, Response, Schema, Service,
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

            // Serialize the spec to JSON once so `$ref` resolution can navigate
            // it without re-serializing the (potentially multi-MB) spec per ref.
            let spec_json = serde_json::to_value(&spec)
                .context("Failed to serialize OpenAPI spec for reference resolution")?;

            let endpoints = extract_endpoints(&spec, &spec_json, &services);
            debug!("Extracted {} endpoints", endpoints.len());
            let schemas = extract_schemas(&spec);
            debug!("Extracted {} reusable schemas", schemas.len());
            let examples = extract_examples(&spec);
            debug!("Extracted {} reusable examples", examples.len());

            Ok(ApiDocumentation {
                title: spec.info.title,
                version: spec.info.version,
                description: spec.info.description,
                services,
                endpoints,
                servers,
                security_schemes,
                schemas,
                examples,
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
            for op in path_item
                .operations()
                .into_iter()
                .filter_map(|(_, op)| op.as_ref())
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

/// The service an endpoint is attributed to when its operation declares no
/// (known) tags: the first declared service, or `"API"` if there are none.
fn default_service(services: &[Service]) -> String {
    services
        .first()
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "API".to_string())
}

/// Flattens every operation under `paths` into an [`Endpoint`], merging
/// path-level and operation-level parameters, resolving `$ref`s, and
/// representing an OpenAPI 3.0 `requestBody` as a synthetic `body` parameter.
fn extract_endpoints(
    spec: &OpenApiSpec,
    spec_json: &serde_json::Value,
    services: &[Service],
) -> Vec<Endpoint> {
    let mut endpoints = Vec::new();

    // A map of service names to ensure all endpoints are associated with valid services
    let service_map: HashSet<String> = services.iter().map(|s| s.name.clone()).collect();

    for (path, path_item) in &spec.paths {
        // Get parameters defined at the path level and resolve any references
        let path_parameters = path_item
            .parameters
            .as_ref()
            .map(|params| {
                params
                    .iter()
                    .filter_map(|p| resolve_parameter_ref(spec_json, p))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        for (method, operation_opt) in path_item.operations() {
            if let Some(operation) = operation_opt {
                // Service tags filtered to known services, falling back to the
                // default service when an operation has none (or only unknown ones).
                let service_tags = operation
                    .tags
                    .as_ref()
                    .map(|tags| {
                        tags.iter()
                            .filter(|tag| service_map.contains(*tag))
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .filter(|filtered| !filtered.is_empty())
                    .unwrap_or_else(|| vec![default_service(services)]);

                // Combine path-level and operation-level parameters with reference resolution
                let mut parameters = path_parameters.clone();

                if let Some(op_params) = &operation.parameters {
                    for param in op_params {
                        if let Some(resolved_param) = resolve_parameter_ref(spec_json, param) {
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
                            // Ferry the request body's examples through so the
                            // generator can render them at `--detail full`.
                            example: media_type.example.clone(),
                            examples: media_type.examples.clone(),
                            extensions: HashMap::new(),
                        });
                    }
                }

                // Resolve references in responses
                let resolved_responses: IndexMap<String, Response> = operation
                    .responses
                    .iter()
                    .map(|(status_code, response)| {
                        let resolved = resolve_response_ref(spec_json, response)
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

/// Collects reusable examples from OpenAPI 3 `components.examples` into a
/// deterministic registry, so media-type `examples` entries that are `$ref`s
/// (`#/components/examples/...`) can be resolved during rendering.
fn extract_examples(spec: &OpenApiSpec) -> IndexMap<String, Example> {
    let mut examples = IndexMap::new();

    if let Some(components) = &spec.components {
        if let Some(component_examples) = &components.examples {
            for (name, example) in component_examples {
                examples.insert(name.clone(), example.clone());
            }
        }
    }

    examples
}
