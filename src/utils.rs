use indexmap::IndexMap;
use serde_json::Value;

use crate::models::{
    ApiDocumentation, OpenApiSpec, Parameter, PathItem, RequestBody, Response, Schema,
};

/// Decodes the escape sequences in a JSON Pointer reference token: `~1` → `/`
/// and `~0` → `~` (RFC 6901). The order matters — `~1` must be decoded before
/// `~0` so that an encoded `~1` is not corrupted.
pub(crate) fn decode_json_pointer_token(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

/// Resolves a JSON reference within a pre-serialized OpenAPI specification.
///
/// `spec_json` is the spec serialized to a [`serde_json::Value`] once by the
/// caller (see [`parse_openapi`](crate::parser::parse_openapi)); resolution
/// only navigates it, so it never re-serializes the spec per `$ref`.
pub(crate) fn resolve_ref(
    spec_json: &serde_json::Value,
    reference: &str,
) -> Option<serde_json::Value> {
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

        current = match current {
            // Component not found -> None
            serde_json::Value::Object(obj) => obj.get(&unescaped)?,
            // Invalid or out-of-bounds index -> None
            serde_json::Value::Array(arr) => arr.get(unescaped.parse::<usize>().ok()?)?,
            // Cannot navigate further
            _ => return None,
        };
    }

    Some(current.clone())
}

/// Resolves a parameter that may be a `$ref` into `components/parameters`,
/// returning the parameter unchanged when it carries no reference. Returns
/// `None` when a `$ref` is present but cannot be resolved.
pub(crate) fn resolve_parameter_ref(
    spec_json: &serde_json::Value,
    parameter: &Parameter,
) -> Option<Parameter> {
    if let Some(reference) = &parameter.reference {
        resolve_ref(spec_json, reference).and_then(|resolved| serde_json::from_value(resolved).ok())
    } else {
        Some(parameter.clone())
    }
}

/// Resolves a response that may be a `$ref` into `components/responses`,
/// returning the response unchanged when it carries no reference. Returns
/// `None` when a `$ref` is present but cannot be resolved.
pub(crate) fn resolve_response_ref(
    spec_json: &serde_json::Value,
    response: &Response,
) -> Option<Response> {
    match &response.reference {
        None => Some(response.clone()),
        Some(reference) => resolve_ref(spec_json, reference)
            .and_then(|resolved| serde_json::from_value(resolved).ok()),
    }
}

/// Resolves a request body that may itself be a `$ref` into
/// `components/requestBodies`, returning the concrete request body. A bare
/// `requestBody: { "$ref": ... }` carries no `content`, so without this the
/// synthetic body parameter would be dropped (or the spec would fail to parse).
pub(crate) fn resolve_request_body_ref(
    spec_json: &serde_json::Value,
    request_body: &RequestBody,
) -> Option<RequestBody> {
    if let Some(reference) = &request_body.reference
        && let Some(resolved) = resolve_ref(spec_json, reference)
    {
        return serde_json::from_value(resolved).ok();
    }
    Some(request_body.clone())
}

/// Resolves a path item that may be a `$ref` into `components/pathItems`.
///
/// Returns `Some(item)` for an inline item (no `$ref`) or a successfully resolved
/// reference, and `None` only when a `$ref` is present but cannot be resolved — so
/// the caller can warn and skip rather than silently emitting an empty path item.
pub(crate) fn resolve_path_item_ref(
    spec_json: &serde_json::Value,
    path_item: &PathItem,
) -> Option<PathItem> {
    match &path_item.reference {
        None => Some(path_item.clone()),
        Some(reference) => resolve_ref(spec_json, reference)
            .and_then(|resolved| serde_json::from_value(resolved).ok()),
    }
}

/// Looks up a component schema by `$ref`. Supports the OpenAPI 3 form
/// (`#/components/schemas/Name`) and the Swagger 2 form (`#/definitions/Name`),
/// both of which the parser collects into [`ApiDocumentation::schemas`].
/// Returns `None` for any other reference shape.
pub(crate) fn resolve_schema_reference<'a>(
    reference: &str,
    doc: &'a ApiDocumentation,
) -> Option<&'a Schema> {
    if let Some(name) = reference.strip_prefix("#/components/schemas/") {
        return doc.schemas.get(&decode_json_pointer_token(name));
    }

    if let Some(name) = reference.strip_prefix("#/definitions/") {
        return doc.schemas.get(&decode_json_pointer_token(name));
    }

    None
}

/// Maximum number of `$ref` expansions on one chain in
/// [`resolve_schema_value`]. Termination is already guaranteed by the cycle
/// guard (`ref_stack`), so this is purely a size guard against a spec whose
/// acyclic reference chains are absurdly long; plain object and array nesting
/// does not count towards it. Deliberately far above the renderer's nesting
/// cap: the diff must see a change however deep it sits.
const MAX_RESOLVE_DEPTH: usize = 64;

/// Materialises `schema` as a self-contained JSON value with every component
/// `$ref` replaced by its target, recursively.
///
/// This is the resolved-shape view that `diff` compares: two operations whose
/// objects are byte-identical still differ here when a shared schema they
/// reference changed. Resolution rules:
///
/// - An object of the form `{"$ref": r, ...siblings}` is replaced by the target
///   of `r`, with any sibling keys merged over it (siblings win).
/// - A reference already being expanded on the current chain is left as the
///   literal `{"$ref": r}` object, so self-referential schemas terminate.
/// - A reference that cannot be resolved is likewise left literal, so two specs
///   that are equally unresolved compare equal instead of diverging.
/// - Expansion stops after [`MAX_RESOLVE_DEPTH`] `$ref` hops on one chain;
///   anything beyond is kept as written. Descending plain objects and arrays
///   does not consume the budget.
pub(crate) fn resolve_schema_value(schema: &Schema, doc: &ApiDocumentation) -> Value {
    let value = serde_json::to_value(schema).unwrap_or(Value::Null);
    let mut ref_stack = Vec::new();
    inline_refs(value, doc, &mut ref_stack, 0)
}

/// `depth` counts `$ref` expansions on the current chain, not JSON nesting.
fn inline_refs(
    value: Value,
    doc: &ApiDocumentation,
    ref_stack: &mut Vec<String>,
    depth: usize,
) -> Value {
    match value {
        Value::Object(mut map) => {
            let reference = match map.get("$ref") {
                Some(Value::String(reference)) => Some(reference.clone()),
                _ => None,
            };

            if let Some(reference) = reference {
                if depth >= MAX_RESOLVE_DEPTH || ref_stack.contains(&reference) {
                    // Cycle (or an implausibly long chain): keep the pointer
                    // as a marker and stop here.
                    return Value::Object(map);
                }
                let Some(target) = resolve_schema_reference(&reference, doc) else {
                    return Value::Object(map);
                };

                // Start from the target and lay any sibling keys over it.
                let mut merged = match serde_json::to_value(target) {
                    Ok(Value::Object(target_map)) => target_map,
                    _ => serde_json::Map::new(),
                };
                map.shift_remove("$ref");
                for (key, sibling) in map {
                    merged.insert(key, sibling);
                }

                ref_stack.push(reference);
                let resolved = inline_refs(Value::Object(merged), doc, ref_stack, depth + 1);
                ref_stack.pop();
                return resolved;
            }

            let mut resolved = serde_json::Map::with_capacity(map.len());
            for (key, child) in map {
                resolved.insert(key, inline_refs(child, doc, ref_stack, depth));
            }
            Value::Object(resolved)
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| inline_refs(item, doc, ref_stack, depth))
                .collect(),
        ),
        other => other,
    }
}

/// Keys that document a schema without changing the shape it accepts. They are
/// dropped before comparison so a reworded description is not a "change".
/// `deprecated` is tracked at the endpoint level instead.
const ANNOTATION_KEYS: [&str; 5] = ["description", "title", "example", "examples", "deprecated"];

/// Keywords whose value is a map of *names* to schemas. The names are data, so
/// annotation stripping must not touch them (a property may be called
/// `description`); only the schemas underneath are canonicalised.
const SCHEMA_MAP_KEYS: [&str; 4] = ["properties", "patternProperties", "definitions", "$defs"];

/// Keywords whose value is a single nested schema.
const SCHEMA_KEYS: [&str; 3] = ["items", "additionalProperties", "not"];

/// Keywords whose value is a list of schemas.
const SCHEMA_LIST_KEYS: [&str; 3] = ["allOf", "oneOf", "anyOf"];

/// Keywords whose value is an order-insensitive set.
const SET_KEYS: [&str; 2] = ["required", "enum"];

/// Rewrites a (resolved) schema value into a canonical form so that two
/// semantically equal schemas compare equal and, when they differ, the
/// differences are reported in a stable order:
///
/// - `required` and `enum` arrays are sorted (their order carries no meaning);
/// - `description`, `title`, `example`, `examples`, `deprecated` and every
///   `x-*` extension are removed from schema objects (never from the property
///   name maps under `properties`, where such a key is a field name);
/// - object keys are sorted, so the walk order never depends on how the model
///   serialised its unmodelled fields.
pub(crate) fn canonicalize_schema_value(value: &mut Value) {
    canonicalize_schema(value);
}

fn canonicalize_schema(value: &mut Value) {
    let Value::Object(map) = value else {
        return;
    };

    map.retain(|key, _| !(key.starts_with("x-") || ANNOTATION_KEYS.contains(&key.as_str())));

    for (key, child) in map.iter_mut() {
        if SCHEMA_MAP_KEYS.contains(&key.as_str()) {
            if let Value::Object(named) = child {
                for (_, nested) in named.iter_mut() {
                    canonicalize_schema(nested);
                }
                named.sort_keys();
            }
        } else if SCHEMA_KEYS.contains(&key.as_str()) {
            canonicalize_schema(child);
        } else if SCHEMA_LIST_KEYS.contains(&key.as_str()) {
            if let Value::Array(items) = child {
                for item in items.iter_mut() {
                    canonicalize_schema(item);
                }
            }
        } else if SET_KEYS.contains(&key.as_str())
            && let Value::Array(items) = child
        {
            items.sort_by_cached_key(|item| item.to_string());
        }
    }

    map.sort_keys();
}

/// Extracts servers from the OpenAPI spec
pub(crate) fn extract_servers(spec: &OpenApiSpec) -> Vec<String> {
    let mut servers = Vec::new();

    // Check for servers array (OpenAPI 3.0+)
    if let Some(server_list) = &spec.servers {
        for server in server_list {
            servers.push(server.url.clone());
        }
    }
    // Check for host + basePath (OpenAPI 2.0)
    else if let Some(host) = spec.extensions.get("host")
        && let Some(host_str) = host.as_str()
    {
        let mut base_url = if host_str.starts_with("http") {
            host_str.to_string()
        } else {
            // Swagger 2.0 lists its transfer protocols in `schemes`; use the
            // first entry and assume https only when the spec doesn't say.
            let scheme = spec
                .extensions
                .get("schemes")
                .and_then(|schemes| schemes.as_array())
                .and_then(|schemes| schemes.first())
                .and_then(|scheme| scheme.as_str())
                .unwrap_or("https");
            format!("{}://{}", scheme, host_str)
        };

        // Add basePath if present
        if let Some(base_path) = spec.extensions.get("basePath")
            && let Some(path_str) = base_path.as_str()
        {
            if !base_url.ends_with('/') && !path_str.starts_with('/') {
                base_url.push('/');
            }
            base_url.push_str(path_str);
        }

        servers.push(base_url);
    }

    servers
}

/// Extracts security schemes from the OpenAPI spec.
///
/// Returns an [`IndexMap`] so the `## Authentication` section is emitted in a
/// stable order, preserving the output-determinism invariant.
pub(crate) fn extract_security_schemes(spec: &OpenApiSpec) -> IndexMap<String, String> {
    let mut schemes = IndexMap::new();

    // OpenAPI 3.0+: components.securitySchemes
    if let Some(components) = &spec.components
        && let Some(security_schemes) = &components.security_schemes
    {
        for (name, scheme) in security_schemes {
            let desc = format!(
                "{} ({})",
                scheme.description.as_deref().unwrap_or(""),
                scheme.security_type
            );
            schemes.insert(name.clone(), desc);
        }
    }

    // OpenAPI 2.0: securityDefinitions
    if let Some(security_defs) = spec.extensions.get("securityDefinitions")
        && let Some(defs_map) = security_defs.as_object()
    {
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

    schemes
}

/// Cleans a string for use as an ID or anchor in Markdown.
///
/// Lowercases, maps every run of non-`[alphanumeric-_]` characters to a single
/// dash, and trims leading/trailing dashes. A single `.replace("--", "-")` only
/// collapses pairs, so runs of 3+ dashes (e.g. from `"a///b"`) would survive;
/// folding character-by-character collapses any-length runs in one pass.
pub(crate) fn clean_for_id(input: &str) -> String {
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
pub(crate) fn extract_content_type(response: &Response) -> Option<String> {
    if let Some(content) = &response.content
        && !content.is_empty()
    {
        return content.keys().next().map(|s| s.to_string());
    }

    // For OpenAPI 2.0, infer from schema
    if response.schema.is_some() {
        return Some("application/json".to_string());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{canonicalize_schema_value, clean_for_id, resolve_schema_value};
    use crate::models::{ApiDocumentation, Schema};
    use indexmap::IndexMap;
    use serde_json::{Value, json};

    /// A documentation shell holding only the given component schemas.
    fn doc_with_schemas(schemas: &[(&str, Value)]) -> ApiDocumentation {
        ApiDocumentation {
            title: "Test".into(),
            version: "1".into(),
            description: None,
            services: Vec::new(),
            endpoints: Vec::new(),
            servers: Vec::new(),
            security_schemes: IndexMap::new(),
            schemas: schemas
                .iter()
                .map(|(name, value)| {
                    (
                        (*name).to_string(),
                        serde_json::from_value::<Schema>(value.clone()).unwrap(),
                    )
                })
                .collect(),
            examples: IndexMap::new(),
        }
    }

    fn schema(value: Value) -> Schema {
        serde_json::from_value(value).unwrap()
    }

    fn canonical(value: Value) -> Value {
        let mut value = value;
        canonicalize_schema_value(&mut value);
        value
    }

    #[test]
    fn resolve_schema_value_inlines_nested_component_refs() {
        // Mirrors `Pet.allOf[0]` in schema_refs_oas3.json: the ref inside allOf
        // must be expanded, and so must refs inside the expansion.
        let doc = doc_with_schemas(&[
            (
                "Category",
                json!({"type": "object", "properties": {"id": {"type": "string"}}}),
            ),
            (
                "CreatePetRequest",
                json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "category": {"$ref": "#/components/schemas/Category"}
                    }
                }),
            ),
        ]);
        let pet = schema(json!({
            "allOf": [
                {"$ref": "#/components/schemas/CreatePetRequest"},
                {"type": "object", "properties": {"id": {"type": "string"}}}
            ]
        }));

        let resolved = resolve_schema_value(&pet, &doc);

        assert_eq!(
            resolved.pointer("/allOf/0/properties/category/properties/id/type"),
            Some(&json!("string"))
        );
        assert!(!resolved.to_string().contains("$ref"));
    }

    #[test]
    fn resolve_schema_value_terminates_on_self_reference() {
        let doc = doc_with_schemas(&[(
            "Node",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "next": {"$ref": "#/components/schemas/Node"}
                }
            }),
        )]);
        let root = schema(json!({"$ref": "#/components/schemas/Node"}));

        let resolved = resolve_schema_value(&root, &doc);

        // One level is expanded; the re-entry is left as the literal pointer.
        assert_eq!(
            resolved.pointer("/properties/id/type"),
            Some(&json!("string"))
        );
        assert_eq!(
            resolved.pointer("/properties/next"),
            Some(&json!({"$ref": "#/components/schemas/Node"}))
        );
    }

    #[test]
    fn resolve_schema_value_keeps_unresolvable_ref_literal() {
        let doc = doc_with_schemas(&[]);
        let root = schema(json!({
            "type": "object",
            "properties": {"owner": {"$ref": "#/components/schemas/Missing"}}
        }));

        let resolved = resolve_schema_value(&root, &doc);

        assert_eq!(
            resolved.pointer("/properties/owner"),
            Some(&json!({"$ref": "#/components/schemas/Missing"}))
        );
    }

    #[test]
    fn resolve_schema_value_merges_ref_siblings_over_target() {
        let doc = doc_with_schemas(&[("Id", json!({"type": "string", "format": "uuid"}))]);
        let root = schema(json!({"$ref": "#/components/schemas/Id", "format": "ulid"}));

        let resolved = resolve_schema_value(&root, &doc);

        assert_eq!(resolved, json!({"type": "string", "format": "ulid"}));
    }

    #[test]
    fn resolve_schema_value_supports_swagger_definitions() {
        let doc = doc_with_schemas(&[("Pet", json!({"type": "object"}))]);
        let root = schema(json!({"$ref": "#/definitions/Pet"}));

        assert_eq!(resolve_schema_value(&root, &doc), json!({"type": "object"}));
    }

    /// A component chain `L0 -> L1 -> ... -> L<hops>` where every link is an
    /// object whose `next` property references the following link and the last
    /// link is a scalar of type `leaf_type`.
    fn chain_doc(hops: usize, leaf_type: &str) -> ApiDocumentation {
        let mut schemas: Vec<(String, Value)> = (0..hops)
            .map(|index| {
                (
                    format!("L{index}"),
                    json!({
                        "type": "object",
                        "properties": {
                            "next": {"$ref": format!("#/components/schemas/L{}", index + 1)}
                        }
                    }),
                )
            })
            .collect();
        schemas.push((format!("L{hops}"), json!({"type": leaf_type})));
        let borrowed: Vec<(&str, Value)> = schemas
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        doc_with_schemas(&borrowed)
    }

    #[test]
    fn resolve_schema_value_follows_long_ref_chains() {
        // The depth budget counts `$ref` hops, not JSON nesting: a leaf twelve
        // references deep (twice the renderer's nesting cap in JSON levels)
        // must still be materialised so the diff can see it change.
        let root = schema(json!({"$ref": "#/components/schemas/L0"}));
        let leaf = "/properties/next".repeat(12) + "/type";

        let old = resolve_schema_value(&root, &chain_doc(12, "string"));
        let new = resolve_schema_value(&root, &chain_doc(12, "integer"));

        assert_eq!(old.pointer(&leaf), Some(&json!("string")));
        assert_eq!(new.pointer(&leaf), Some(&json!("integer")));
        assert_ne!(old, new);
        assert!(!old.to_string().contains("$ref"));
    }

    #[test]
    fn resolve_schema_value_stops_after_max_ref_hops() {
        let root = schema(json!({"$ref": "#/components/schemas/L0"}));
        let resolved = resolve_schema_value(&root, &chain_doc(70, "string"));

        // The 64th hop is expanded; the 65th is kept as a literal pointer.
        let last_expanded = "/properties/next".repeat(63) + "/type";
        assert_eq!(resolved.pointer(&last_expanded), Some(&json!("object")));
        let first_kept = "/properties/next".repeat(64);
        assert_eq!(
            resolved.pointer(&first_kept),
            Some(&json!({"$ref": "#/components/schemas/L64"}))
        );
    }

    #[test]
    fn canonicalize_makes_required_order_irrelevant() {
        let a = canonical(json!({"type": "object", "required": ["b", "a"]}));
        let b = canonical(json!({"type": "object", "required": ["a", "b"]}));
        assert_eq!(a, b);
    }

    #[test]
    fn canonicalize_makes_enum_order_irrelevant() {
        let a = canonical(json!({"type": "string", "enum": ["x", "y"]}));
        let b = canonical(json!({"type": "string", "enum": ["y", "x"]}));
        assert_eq!(a, b);
    }

    #[test]
    fn canonicalize_ignores_description_only_changes() {
        let a = canonical(json!({
            "type": "object",
            "title": "Old",
            "description": "Old words",
            "example": {"id": 1},
            "x-internal": true,
            "properties": {"id": {"type": "string", "description": "Old id"}}
        }));
        let b = canonical(json!({
            "type": "object",
            "title": "New",
            "description": "New words",
            "properties": {"id": {"type": "string", "description": "New id"}}
        }));
        assert_eq!(a, b);
        assert_eq!(
            a,
            json!({"properties": {"id": {"type": "string"}}, "type": "object"})
        );
    }

    #[test]
    fn canonicalize_keeps_properties_named_like_annotations() {
        // A field called `description` is data, not an annotation.
        let value = canonical(json!({
            "type": "object",
            "properties": {
                "description": {"type": "string"},
                "x-rate": {"type": "integer"}
            }
        }));
        assert_eq!(
            value.pointer("/properties/description/type"),
            Some(&json!("string"))
        );
        assert_eq!(
            value.pointer("/properties/x-rate/type"),
            Some(&json!("integer"))
        );
    }

    #[test]
    fn canonicalize_sorts_object_keys() {
        let value = canonical(json!({"type": "string", "minimum": 1, "format": "int32"}));
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["format", "minimum", "type"]);
    }

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
