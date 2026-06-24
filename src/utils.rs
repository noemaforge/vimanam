use indexmap::IndexMap;

use crate::models::{OpenApiSpec, Parameter, PathItem, RequestBody, Response};

/// Decodes the escape sequences in a JSON Pointer reference token: `~1` → `/`
/// and `~0` → `~` (RFC 6901). The order matters — `~1` must be decoded before
/// `~0` so that an encoded `~1` is not corrupted.
pub fn decode_json_pointer_token(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

/// Resolves a JSON reference within a pre-serialized OpenAPI specification.
///
/// `spec_json` is the spec serialized to a [`serde_json::Value`] once by the
/// caller (see [`parse_openapi`](crate::parser::parse_openapi)); resolution
/// only navigates it, so it never re-serializes the spec per `$ref`.
pub fn resolve_ref(spec_json: &serde_json::Value, reference: &str) -> Option<serde_json::Value> {
    if !reference.starts_with("#/") {
        return None; // We only support internal references for now
    }

    // Remove the #/ prefix
    let path = &reference[2..];
    let components = path.split('/');

    // Navigate the path
    let mut current = spec_json;
    for component in components {
        // Handle escaped JSON pointer components
        let unescaped = decode_json_pointer_token(component);

        if let Some(obj) = current.as_object() {
            if let Some(value) = obj.get(&unescaped) {
                current = value;
            } else {
                return None; // Component not found
            }
        } else if let Some(arr) = current.as_array() {
            if let Ok(index) = unescaped.parse::<usize>() {
                if index < arr.len() {
                    current = &arr[index];
                } else {
                    return None; // Index out of bounds
                }
            } else {
                return None; // Invalid array index
            }
        } else {
            return None; // Cannot navigate further
        }
    }

    Some(current.clone())
}

/// Resolves a parameter that may be a `$ref` into `components/parameters`,
/// returning the parameter unchanged when it carries no reference. Returns
/// `None` when a `$ref` is present but cannot be resolved.
pub fn resolve_parameter_ref(
    spec_json: &serde_json::Value,
    parameter: &Parameter,
) -> Option<Parameter> {
    if let Some(reference) = &parameter.reference {
        resolve_ref(spec_json, reference).and_then(|resolved| serde_json::from_value(resolved).ok())
    } else {
        Some(parameter.clone())
    }
}

/// Resolves a response reference to a concrete response
pub fn resolve_response_ref(
    spec_json: &serde_json::Value,
    response: &Response,
) -> Option<Response> {
    if let Some(extensions) = response.extensions.get("$ref") {
        if let Some(reference) = extensions.as_str() {
            if let Some(resolved) = resolve_ref(spec_json, reference) {
                return serde_json::from_value(resolved).ok();
            }
        }
    }
    Some(response.clone())
}

/// Resolves a request body that may itself be a `$ref` into
/// `components/requestBodies`, returning the concrete request body. A bare
/// `requestBody: { "$ref": ... }` carries no `content`, so without this the
/// synthetic body parameter would be dropped (or the spec would fail to parse).
pub fn resolve_request_body_ref(
    spec_json: &serde_json::Value,
    request_body: &RequestBody,
) -> Option<RequestBody> {
    if let Some(reference) = &request_body.reference {
        if let Some(resolved) = resolve_ref(spec_json, reference) {
            return serde_json::from_value(resolved).ok();
        }
    }
    Some(request_body.clone())
}

/// Resolves a path item that may be a `$ref` into `components/pathItems`.
///
/// Returns `Some(item)` for an inline item (no `$ref`) or a successfully resolved
/// reference, and `None` only when a `$ref` is present but cannot be resolved — so
/// the caller can warn and skip rather than silently emitting an empty path item.
pub fn resolve_path_item_ref(
    spec_json: &serde_json::Value,
    path_item: &PathItem,
) -> Option<PathItem> {
    match &path_item.reference {
        None => Some(path_item.clone()),
        Some(reference) => resolve_ref(spec_json, reference)
            .and_then(|resolved| serde_json::from_value(resolved).ok()),
    }
}

/// Extracts servers from the OpenAPI spec
pub fn extract_servers(spec: &OpenApiSpec) -> Vec<String> {
    let mut servers = Vec::new();

    // Check for servers array (OpenAPI 3.0+)
    if let Some(server_list) = &spec.servers {
        for server in server_list {
            servers.push(server.url.clone());
        }
    }
    // Check for host + basePath (OpenAPI 2.0)
    else if let Some(host) = spec.extensions.get("host") {
        if let Some(host_str) = host.as_str() {
            let mut base_url = if host_str.starts_with("http") {
                host_str.to_string()
            } else {
                format!("https://{}", host_str)
            };

            // Add basePath if present
            if let Some(base_path) = spec.extensions.get("basePath") {
                if let Some(path_str) = base_path.as_str() {
                    if !base_url.ends_with('/') && !path_str.starts_with('/') {
                        base_url.push('/');
                    }
                    base_url.push_str(path_str);
                }
            }

            servers.push(base_url);
        }
    }

    // Fallback to a default if empty
    if servers.is_empty() {
        servers.push("https://api.example.com".to_string());
    }

    servers
}

/// Extracts security schemes from the OpenAPI spec.
///
/// Returns an [`IndexMap`] so the `## Authentication` section is emitted in a
/// stable order, preserving the output-determinism invariant.
pub fn extract_security_schemes(spec: &OpenApiSpec) -> IndexMap<String, String> {
    let mut schemes = IndexMap::new();

    // OpenAPI 3.0+: components.securitySchemes
    if let Some(components) = &spec.components {
        if let Some(security_schemes) = &components.security_schemes {
            for (name, scheme) in security_schemes {
                let desc = format!(
                    "{} ({})",
                    scheme.description.as_deref().unwrap_or(""),
                    scheme.security_type
                );
                schemes.insert(name.clone(), desc);
            }
        }
    }

    // OpenAPI 2.0: securityDefinitions
    if let Some(security_defs) = spec.extensions.get("securityDefinitions") {
        if let Some(defs_map) = security_defs.as_object() {
            for (name, def) in defs_map {
                if let Some(def_obj) = def.as_object() {
                    let type_str = def_obj
                        .get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("unknown");

                    let desc = def_obj
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("");

                    schemes.insert(name.clone(), format!("{} ({})", desc, type_str));
                }
            }
        }
    }

    schemes
}

/// Cleans a string for use as an ID or anchor in Markdown.
///
/// Lowercases, maps every run of non-`[alphanumeric-_]` characters to a single
/// dash, and trims leading/trailing dashes. A single `.replace("--", "-")` only
/// collapses pairs, so runs of 3+ dashes (e.g. from `"a///b"`) would survive;
/// folding character-by-character collapses any-length runs in one pass.
pub fn clean_for_id(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut last_was_dash = false;

    for c in input.to_lowercase().chars() {
        if c.is_alphanumeric() || c == '_' {
            result.push(c);
            last_was_dash = false;
        } else if !last_was_dash {
            // Any disallowed character (including a literal '-') folds into a
            // single separating dash, collapsing consecutive runs.
            result.push('-');
            last_was_dash = true;
        }
    }

    result.trim_matches('-').to_string()
}

/// Extracts the primary content type from responses
pub fn extract_content_type(response: &Response) -> Option<String> {
    if let Some(content) = &response.content {
        if !content.is_empty() {
            return content.keys().next().map(|s| s.to_string());
        }
    }

    // For OpenAPI 2.0, infer from schema
    if response.schema.is_some() {
        return Some("application/json".to_string());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::clean_for_id;

    #[test]
    fn clean_for_id_basic_cases() {
        assert_eq!(clean_for_id("Pets_ListPets"), "pets_listpets");
        assert_eq!(clean_for_id("GET /pets"), "get-pets");
        assert_eq!(clean_for_id("list-pets"), "list-pets");
    }

    // Regression test for #15: a single `.replace("--", "-")` left runs of 3+
    // dashes intact; folding must collapse any-length runs to one dash.
    #[test]
    fn clean_for_id_collapses_long_dash_runs() {
        assert_eq!(clean_for_id("a///b"), "a-b");
        assert_eq!(clean_for_id("a / / b"), "a-b");
        assert_eq!(clean_for_id("foo----bar"), "foo-bar");
    }

    #[test]
    fn clean_for_id_trims_edge_dashes() {
        assert_eq!(clean_for_id("/pets/"), "pets");
        assert_eq!(clean_for_id("**bold**"), "bold");
    }
}
