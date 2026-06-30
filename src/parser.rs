use anyhow::{Context, Result};
use indexmap::{IndexMap, IndexSet};
use log::{debug, warn};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use crate::models::{
    ApiDocumentation, Endpoint, Example, OpenApiSpec, Parameter, Response, Schema, Service,
};
use crate::utils::{
    extract_security_schemes, extract_servers, resolve_parameter_ref, resolve_path_item_ref,
    resolve_request_body_ref, resolve_response_ref,
};

/// Parses an OpenAPI 2.0/3.0 file (JSON or YAML) into the spec-version-agnostic
/// [`ApiDocumentation`] intermediate representation. On deserialization
/// failure, re-parses as generic JSON/YAML to produce a targeted error message.
pub fn parse_openapi<P: AsRef<Path>>(path: P) -> Result<ApiDocumentation> {
    let path_ref = path.as_ref();
    let file = File::open(path_ref).context("Failed to open OpenAPI file")?;
    let mut reader = BufReader::new(file);

    // Read entire file content first
    let mut content = String::new();
    reader.read_to_string(&mut content)?;

    // Determine file format based on extension (case-insensitive)
    let file_extension = path_ref
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    // Prefer YAML parser when extension suggests YAML, but fall back to JSON parser
    // if that fails (since YAML is a superset of JSON). This handles files with
    // unusual/no extensions and makes parsing more robust.
    let spec: OpenApiSpec = if file_extension == "yaml" || file_extension == "yml" {
        parse_yaml_spec(&content).or_else(|yaml_err| {
            parse_json_spec(&content).map_err(|json_err| {
                anyhow::anyhow!(
                    "YAML parse failed: {}; JSON fallback also failed: {}",
                    yaml_err,
                    json_err
                )
            })
        })?
    } else {
        // Try JSON first; if it fails, fall back to YAML parser (handles JSON too)
        parse_json_spec(&content).or_else(|json_err| {
            parse_yaml_spec(&content).map_err(|yaml_err| {
                anyhow::anyhow!(
                    "JSON parse failed: {}; YAML fallback also failed: {}",
                    json_err,
                    yaml_err
                )
            })
        })?
    };

    // Validate the parsed spec
    validate_openapi(&spec, path_ref)?;

    // Serialize the spec to JSON once so `$ref` resolution can navigate
    // it without re-serializing the (potentially multi-MB) spec per ref.
    let spec_json = serde_json::to_value(&spec)
        .context("Failed to serialize OpenAPI spec for reference resolution")?;

    // Extract services (ref-aware: tags inside `$ref` path items count too)
    let services = extract_services(&spec, &spec_json);
    debug!("Extracted {} services", services.len());

    // Extract servers information
    let servers = extract_servers(&spec);
    debug!("Extracted {} server URLs", servers.len());

    // Extract security schemes
    let security_schemes = extract_security_schemes(&spec);
    debug!("Extracted {} security schemes", security_schemes.len());

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

/// Parse content as OpenAPI spec using the given deserializer, with enhanced error messages.
/// The `deserializer` function takes the content string and returns either the parsed spec
/// or an error. The `generic_deserializer` parses to `serde_json::Value` for structural
/// validation. The `format_name` is used in error messages.
fn parse_spec<F, G>(
    content: &str,
    deserializer: F,
    generic_deserializer: G,
    format_name: &str,
) -> Result<OpenApiSpec>
where
    F: FnOnce(&str) -> Result<OpenApiSpec, anyhow::Error>,
    G: FnOnce(&str) -> Result<serde_json::Value, anyhow::Error>,
{
    deserializer(content).map_err(|err| {
        // Try to parse as generic value to provide better error messages
        match generic_deserializer(content) {
            Ok(json) => {
                // Check for common structural issues
                if !json.is_object() {
                    return anyhow::anyhow!("Root element is not a {} object", format_name);
                }

                let obj = json.as_object().unwrap();

                if !obj.contains_key("swagger") && !obj.contains_key("openapi") {
                    return anyhow::anyhow!(
                        "Missing 'swagger' or 'openapi' field - not a valid OpenAPI specification"
                    );
                }

                if !obj.contains_key("paths") {
                    return anyhow::anyhow!(
                        "Missing 'paths' field - not a valid OpenAPI specification"
                    );
                }

                if !obj.contains_key("info") {
                    return anyhow::anyhow!(
                        "Missing 'info' field - not a valid OpenAPI specification"
                    );
                }

                // If we got here, there's a structural issue with the spec
                anyhow::anyhow!("Invalid OpenAPI specification structure: {}", err)
            }
            Err(_) => {
                // Not even valid format
                anyhow::anyhow!("File is not valid {}: {}", format_name, err)
            }
        }
    })
}

/// Parse YAML content as OpenAPI spec with enhanced error messages.
fn parse_yaml_spec(content: &str) -> Result<OpenApiSpec> {
    parse_spec(
        content,
        |s| serde_yaml::from_str(s).map_err(anyhow::Error::new),
        |s| serde_yaml::from_str(s).map_err(anyhow::Error::new),
        "YAML",
    )
}

/// Parse JSON content as OpenAPI spec with enhanced error messages.
fn parse_json_spec(content: &str) -> Result<OpenApiSpec> {
    parse_spec(
        content,
        |s| serde_json::from_str(s).map_err(anyhow::Error::new),
        |s| serde_json::from_str(s).map_err(anyhow::Error::new),
        "JSON",
    )
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
fn extract_services(spec: &OpenApiSpec, spec_json: &serde_json::Value) -> Vec<Service> {
    let mut services = Vec::new();
    // Names already added, so declared tags and operation tags don't duplicate;
    // first-appearance order keeps output deterministic.
    let mut seen: IndexSet<String> = IndexSet::new();

    // Declared tags first, preserving their descriptions and order.
    if let Some(tags) = &spec.tags {
        for tag in tags {
            if seen.insert(tag.name.clone()) {
                services.push(Service {
                    name: tag.name.clone(),
                    description: tag.description.clone(),
                });
            }
        }
    }

    // Union with every tag actually used by an operation — including operations
    // inside `$ref` path items — so an operation tagged with an undeclared tag
    // gets its own service instead of being silently reassigned to the first one.
    for (_, path_item) in &spec.paths {
        let resolved_path_item;
        let path_item = if path_item.reference.is_some() {
            match resolve_path_item_ref(spec_json, path_item) {
                Some(item) => {
                    resolved_path_item = item;
                    &resolved_path_item
                }
                None => continue,
            }
        } else {
            path_item
        };

        for op in path_item
            .operations()
            .into_iter()
            .filter_map(|(_, op)| op.as_ref())
        {
            if let Some(tags) = &op.tags {
                for tag in tags {
                    if seen.insert(tag.clone()) {
                        services.push(Service {
                            name: tag.clone(),
                            description: None,
                        });
                    }
                }
            }
        }
    }

    // Fall back to a single default service if the spec declares no tags at all.
    if services.is_empty() {
        services.push(Service {
            name: "API".to_string(),
            description: None,
        });
    }

    services
}

/// The service an endpoint is attributed to when its operation declares no tags
/// at all: the first declared service, or `"API"` if there are none.
fn default_service(services: &[Service]) -> String {
    services
        .first()
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "API".to_string())
}

/// De-duplicates parameters on `(name, in)`, keeping the last occurrence so an
/// operation-level parameter overrides a path-level one of the same name and
/// location (OpenAPI's override rule). First-seen position is preserved.
fn dedup_parameters(parameters: Vec<Parameter>) -> Vec<Parameter> {
    let mut by_key: IndexMap<(String, String), Parameter> = IndexMap::new();
    for parameter in parameters {
        let key = (parameter.name.clone(), parameter.parameter_in.clone());
        by_key.insert(key, parameter);
    }
    by_key.into_values().collect()
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
        // A path item may itself be a `$ref`; resolve it (warn + skip when it
        // can't be resolved) so its operations aren't silently dropped.
        let resolved_path_item;
        let path_item = if path_item.reference.is_some() {
            match resolve_path_item_ref(spec_json, path_item) {
                Some(item) => {
                    resolved_path_item = item;
                    &resolved_path_item
                }
                None => {
                    warn!("Unresolved $ref for path '{}'; skipping", path);
                    continue;
                }
            }
        } else {
            path_item
        };

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

                // De-duplicate on (name, in): an operation-level parameter
                // overrides a path-level one with the same name and location.
                let mut parameters = dedup_parameters(parameters);

                // Handle request body as a parameter (for OpenAPI 3.0).
                // Bodies are optional unless the spec says required: true.
                if let Some(req_body) = &operation.request_body {
                    // A `requestBody` may be a `$ref`; resolve it before reading
                    // its content, otherwise the synthetic body is dropped.
                    let req_body = resolve_request_body_ref(spec_json, req_body)
                        .unwrap_or_else(|| req_body.clone());

                    // Iterate over all media types instead of just the first one
                    for (content_type, media_type) in &req_body.content {
                        let param_name = if req_body.content.len() > 1 {
                            format!("requestBody ({})", content_type)
                        } else {
                            "requestBody".to_string()
                        };
                        parameters.push(Parameter {
                            reference: None,
                            name: param_name,
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

/// Collects reusable schemas from OpenAPI 3 `components.schemas` and OpenAPI 2 `definitions`
/// into a deterministic registry.
fn extract_schemas(spec: &OpenApiSpec) -> IndexMap<String, Schema> {
    let mut schemas = IndexMap::new();

    if let Some(components) = &spec.components
        && let Some(component_schemas) = &components.schemas
    {
        for (name, schema) in component_schemas {
            schemas.insert(name.clone(), schema.clone());
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

    if let Some(components) = &spec.components
        && let Some(component_examples) = &components.examples
    {
        for (name, example) in component_examples {
            examples.insert(name.clone(), example.clone());
        }
    }

    examples
}
