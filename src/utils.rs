use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use indexmap::IndexMap;

use crate::models::{OpenApiSpec, Parameter, PathItem, RequestBody, Response};

/// Maximum `$ref` chain / nesting depth the resolver follows before giving up.
const MAX_REF_DEPTH: usize = 64;

/// Resolves internal `$ref`s against a spec serialized to JSON exactly once.
///
/// The whole spec is serialized into a single `serde_json::Value` at construction
/// and shared via `Arc`, so repeated resolution is cheap. (Previously every lookup
/// re-serialized the entire spec — O(spec_size) per `$ref`.)
#[derive(Debug, Clone)]
pub struct Resolver {
    root: Arc<serde_json::Value>,
}

impl Resolver {
    /// Serializes the spec once. If serialization somehow fails, the resolver is
    /// inert and every lookup returns `None`.
    pub fn new(spec: &OpenApiSpec) -> Self {
        let root = serde_json::to_value(spec).unwrap_or(serde_json::Value::Null);
        Resolver {
            root: Arc::new(root),
        }
    }

    /// Resolves a `#/...` JSON pointer to its target, following `$ref` chains and
    /// guarding against cycles and runaway depth.
    pub fn resolve(&self, reference: &str) -> Option<serde_json::Value> {
        let mut seen = HashSet::new();
        self.resolve_inner(reference, &mut seen, 0)
    }

    fn resolve_inner(
        &self,
        reference: &str,
        seen: &mut HashSet<String>,
        depth: usize,
    ) -> Option<serde_json::Value> {
        // Break cycles and bound depth.
        if depth >= MAX_REF_DEPTH || !seen.insert(reference.to_string()) {
            return None;
        }

        let value = self.lookup(reference)?;

        // Follow ref-to-ref chains (e.g. a component that is itself a `$ref`).
        if let Some(next) = value
            .as_object()
            .and_then(|o| o.get("$ref"))
            .and_then(|r| r.as_str())
        {
            return self.resolve_inner(next, seen, depth + 1);
        }

        Some(value)
    }

    /// One JSON-pointer navigation step (no chain following).
    fn lookup(&self, reference: &str) -> Option<serde_json::Value> {
        let path = reference.strip_prefix("#/")?;
        let mut current = self.root.as_ref();
        for component in path.split('/') {
            // Handle escaped JSON pointer components.
            let unescaped = component.replace("~1", "/").replace("~0", "~");
            if let Some(obj) = current.as_object() {
                current = obj.get(&unescaped)?;
            } else if let Some(arr) = current.as_array() {
                let index = unescaped.parse::<usize>().ok()?;
                current = arr.get(index)?;
            } else {
                return None;
            }
        }
        Some(current.clone())
    }

    /// Resolves a parameter that may be a `$ref`; returns it unchanged when inline.
    pub fn resolve_parameter(&self, parameter: &Parameter) -> Option<Parameter> {
        match self.ref_target(&parameter.extensions) {
            Some(resolved) => serde_json::from_value(resolved).ok(),
            None => Some(parameter.clone()),
        }
    }

    /// Resolves a response that may be a `$ref`.
    pub fn resolve_response(&self, response: &Response) -> Option<Response> {
        match self.ref_target(&response.extensions) {
            Some(resolved) => serde_json::from_value(resolved).ok(),
            None => Some(response.clone()),
        }
    }

    /// Resolves a request body that may be a `$ref`.
    pub fn resolve_request_body(&self, body: &RequestBody) -> Option<RequestBody> {
        match self.ref_target(&body.extensions) {
            Some(resolved) => serde_json::from_value(resolved).ok(),
            None => Some(body.clone()),
        }
    }

    /// Resolves a path item that may be a `$ref`.
    pub fn resolve_path_item(&self, item: &PathItem) -> Option<PathItem> {
        match self.ref_target(&item.extensions) {
            Some(resolved) => serde_json::from_value(resolved).ok(),
            None => Some(item.clone()),
        }
    }

    /// If an `extensions` map carries a `$ref`, resolve its target.
    fn ref_target(
        &self,
        extensions: &HashMap<String, serde_json::Value>,
    ) -> Option<serde_json::Value> {
        let reference = extensions.get("$ref")?.as_str()?;
        self.resolve(reference)
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

/// Extracts security schemes from the OpenAPI spec, preserving declaration order.
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

/// Cleans a string for use as an ID or anchor in Markdown
pub fn clean_for_id(input: &str) -> String {
    input
        .to_lowercase()
        .replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', "-")
        .replace("--", "-")
        .trim_matches('-')
        .to_string()
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
