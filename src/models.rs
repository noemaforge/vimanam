use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// OpenAPI spec model with flexibility for both 2.0 and 3.0 formats
#[derive(Debug, Deserialize, Serialize)]
pub struct OpenApiSpec {
    // Support both "swagger" (2.0) and "openapi" (3.0+) version identifiers
    #[serde(rename = "swagger", alias = "openapi", default)]
    pub spec_version: Option<String>,

    pub info: Info,

    // Tags are optional
    pub tags: Option<Vec<Tag>>,

    // Paths are mandatory; IndexMap preserves spec order for deterministic output
    pub paths: IndexMap<String, PathItem>,

    // Optional servers field (OpenAPI 3.0+)
    pub servers: Option<Vec<Server>>,

    // Optional components field (OpenAPI 3.0+)
    pub components: Option<Components>,

    // Optional security field
    pub security: Option<Vec<HashMap<String, Vec<String>>>>,

    // Capture all other fields we don't explicitly model
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_json::Value>,
}
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum AdditionalProperties {
    Bool(bool),
    Schema(Box<Schema>),
}

/// Deserializes an OpenAPI `type` as either a string (2.0/3.0) or an array of
/// strings (3.1, e.g. `["string", "null"]`), normalizing to the first non-`"null"`
/// type so the rest of the pipeline can keep treating it as a scalar.
fn deserialize_optional_type<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum TypeField {
        Single(String),
        Multi(Vec<String>),
    }

    Ok(match Option::<TypeField>::deserialize(deserializer)? {
        None => None,
        Some(TypeField::Single(s)) => Some(s),
        Some(TypeField::Multi(types)) => types.into_iter().find(|t| t != "null"),
    })
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Info {
    pub title: String,
    pub version: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Tag {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PathItem {
    #[serde(rename = "get", skip_serializing_if = "Option::is_none")]
    pub get: Option<Operation>,
    #[serde(rename = "put", skip_serializing_if = "Option::is_none")]
    pub put: Option<Operation>,
    #[serde(rename = "post", skip_serializing_if = "Option::is_none")]
    pub post: Option<Operation>,
    #[serde(rename = "delete", skip_serializing_if = "Option::is_none")]
    pub delete: Option<Operation>,
    #[serde(rename = "options", skip_serializing_if = "Option::is_none")]
    pub options: Option<Operation>,
    #[serde(rename = "head", skip_serializing_if = "Option::is_none")]
    pub head: Option<Operation>,
    #[serde(rename = "patch", skip_serializing_if = "Option::is_none")]
    pub patch: Option<Operation>,
    #[serde(rename = "trace", skip_serializing_if = "Option::is_none")]
    pub trace: Option<Operation>,
    #[serde(rename = "parameters", skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Vec<Parameter>>,
    // A path item may itself be a `$ref` into `components/pathItems`; capture it
    // so the parser can resolve it instead of silently dropping the operations.
    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

impl PathItem {
    /// The eight HTTP operations paired with their lowercase method name, in a
    /// stable order. Centralizes the method list so callers don't repeat it.
    pub fn operations(&self) -> [(&'static str, &Option<Operation>); 8] {
        [
            ("get", &self.get),
            ("post", &self.post),
            ("put", &self.put),
            ("delete", &self.delete),
            ("options", &self.options),
            ("head", &self.head),
            ("patch", &self.patch),
            ("trace", &self.trace),
        ]
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Operation {
    pub tags: Option<Vec<String>>,
    pub summary: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "operationId")]
    pub operation_id: Option<String>,
    pub parameters: Option<Vec<Parameter>>,
    #[serde(rename = "requestBody", skip_serializing_if = "Option::is_none")]
    pub request_body: Option<RequestBody>,
    // Defaulted so an operation missing `responses` doesn't fail the whole parse.
    #[serde(default)]
    pub responses: IndexMap<String, Response>,
    pub deprecated: Option<bool>,
    #[serde(rename = "security", skip_serializing_if = "Option::is_none")]
    pub security: Option<Vec<HashMap<String, Vec<String>>>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Parameter {
    // A parameter may be a `$ref` into `components/parameters`; capture it so the
    // parser can resolve it. `name`/`in` default to empty so the bare `$ref` form
    // (which omits them) still deserializes — they come from the resolved target.
    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default)]
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "in", default)]
    pub parameter_in: String,
    pub required: Option<bool>,
    pub schema: Option<Schema>,
    // Example carriers. Real OpenAPI 3 parameters may define these directly; the
    // parser also reuses them to ferry a request body's media-type examples into
    // the synthetic `body` parameter so the generator can render them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<IndexMap<String, Example>>,
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Schema {
    #[serde(rename = "title", skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(
        rename = "type",
        default,
        deserialize_with = "deserialize_optional_type",
        skip_serializing_if = "Option::is_none"
    )]
    pub schema_type: Option<String>,
    #[serde(rename = "format", skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(rename = "properties", skip_serializing_if = "Option::is_none")]
    pub properties: Option<IndexMap<String, Schema>>,
    #[serde(rename = "items", skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<Schema>>,
    #[serde(rename = "required", skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
    #[serde(rename = "allOf", skip_serializing_if = "Option::is_none")]
    pub all_of: Option<Vec<Schema>>,
    #[serde(rename = "oneOf", skip_serializing_if = "Option::is_none")]
    pub one_of: Option<Vec<Schema>>,
    #[serde(rename = "anyOf", skip_serializing_if = "Option::is_none")]
    pub any_of: Option<Vec<Schema>>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<serde_json::Value>>,
    #[serde(rename = "nullable", skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,
    #[serde(
        rename = "additionalProperties",
        skip_serializing_if = "Option::is_none"
    )]
    pub additional_properties: Option<AdditionalProperties>,
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Response {
    pub description: Option<String>,
    pub schema: Option<Schema>,
    #[serde(rename = "content", skip_serializing_if = "Option::is_none")]
    pub content: Option<IndexMap<String, MediaType>>,
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_json::Value>,
}

// Server definition for OpenAPI 3.0+
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Server {
    pub url: String,
    pub description: Option<String>,
    pub variables: Option<HashMap<String, ServerVariable>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ServerVariable {
    #[serde(rename = "enum")]
    pub enum_values: Option<Vec<String>>,
    pub default: String,
    pub description: Option<String>,
}

// Components definition for OpenAPI 3.0+
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Components {
    pub schemas: Option<IndexMap<String, Schema>>,
    pub responses: Option<HashMap<String, Response>>,
    pub parameters: Option<HashMap<String, Parameter>>,
    // IndexMap so example references resolve to a deterministically ordered set.
    pub examples: Option<IndexMap<String, Example>>,
    #[serde(rename = "requestBodies")]
    pub request_bodies: Option<HashMap<String, RequestBody>>,
    pub headers: Option<HashMap<String, Header>>,
    // IndexMap preserves spec order so the Authentication section is deterministic
    #[serde(rename = "securitySchemes")]
    pub security_schemes: Option<IndexMap<String, SecurityScheme>>,
    pub links: Option<HashMap<String, Link>>,
    pub callbacks: Option<HashMap<String, Callback>>,
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_json::Value>,
}

// Example struct
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Example {
    // A media-type `examples` entry may be a `$ref` into `components/examples`;
    // capture it so the generator can resolve the reference.
    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub value: Option<serde_json::Value>,
    #[serde(rename = "externalValue", skip_serializing_if = "Option::is_none")]
    pub external_value: Option<String>,
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_json::Value>,
}

// RequestBody struct
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RequestBody {
    // A `requestBody` may itself be a `$ref` into `components/requestBodies`;
    // capture it so the parser can resolve the reference before use. `content`
    // defaults to empty so the `$ref` form (which omits it) still deserializes.
    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub content: IndexMap<String, MediaType>,
    pub required: Option<bool>,
}

// MediaType struct
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MediaType {
    pub schema: Option<Schema>,
    pub example: Option<serde_json::Value>,
    // IndexMap preserves spec order so rendered examples are deterministic.
    pub examples: Option<IndexMap<String, Example>>,
    pub encoding: Option<HashMap<String, Encoding>>,
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_json::Value>,
}

// Encoding struct
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Encoding {
    #[serde(rename = "contentType")]
    pub content_type: Option<String>,
    pub headers: Option<HashMap<String, Header>>,
    pub style: Option<String>,
    pub explode: Option<bool>,
    #[serde(rename = "allowReserved")]
    pub allow_reserved: Option<bool>,
}

// Header struct
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Header {
    pub description: Option<String>,
    pub schema: Option<Schema>,
}

// SecurityScheme struct
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SecurityScheme {
    #[serde(rename = "type")]
    pub security_type: String,
    pub description: Option<String>,
    pub name: Option<String>,
    #[serde(rename = "in")]
    pub location: Option<String>,
    pub scheme: Option<String>,
    #[serde(rename = "bearerFormat")]
    pub bearer_format: Option<String>,
    pub flows: Option<OAuthFlows>,
    #[serde(rename = "openIdConnectUrl")]
    pub open_id_connect_url: Option<String>,
}

// OAuthFlows struct
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OAuthFlows {
    pub implicit: Option<OAuthFlow>,
    pub password: Option<OAuthFlow>,
    #[serde(rename = "clientCredentials")]
    pub client_credentials: Option<OAuthFlow>,
    #[serde(rename = "authorizationCode")]
    pub authorization_code: Option<OAuthFlow>,
}

// OAuthFlow struct
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OAuthFlow {
    #[serde(rename = "authorizationUrl")]
    pub authorization_url: Option<String>,
    #[serde(rename = "tokenUrl")]
    pub token_url: Option<String>,
    #[serde(rename = "refreshUrl")]
    pub refresh_url: Option<String>,
    pub scopes: HashMap<String, String>,
}

// Link struct
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Link {
    #[serde(rename = "operationRef")]
    pub operation_ref: Option<String>,
    #[serde(rename = "operationId")]
    pub operation_id: Option<String>,
    pub parameters: Option<HashMap<String, serde_json::Value>>,
    #[serde(rename = "requestBody")]
    pub request_body: Option<serde_json::Value>,
    pub description: Option<String>,
    pub server: Option<Server>,
}

// Callback struct - simplistic version
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Callback {
    // A more complete version would define this properly
    #[serde(flatten)]
    pub expression: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct Service {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub path: String,
    pub method: String,
    pub services: Vec<String>, // References to service names
    pub summary: Option<String>,
    pub description: Option<String>,
    pub operation_id: Option<String>,
    pub parameters: Vec<Parameter>,
    pub responses: IndexMap<String, Response>,
    pub deprecated: bool,
}

/// Configuration for documentation generation
#[derive(Debug, Clone)]
pub struct DocConfig {
    pub group_by: GroupBy,
    pub service_filter: Option<Vec<String>>,
    pub path_filter: Option<String>,
    pub method_filter: Option<Vec<String>>,
    pub exclude_deprecated: bool,
    pub required_only: bool,
    pub detail_level: DetailLevel,
    pub include_schemas: bool,
    pub include_examples: bool,
    pub include_auth: bool,
    pub include_toc: bool,
    pub sort_method: SortMethod,
    // When set, the generator renders at progressively lower detail until the
    // estimated token count fits this budget (`--max-tokens`).
    pub max_tokens: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GroupBy {
    Service,
    Method,
    Path,
    Flat,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DetailLevel {
    Summary,
    Basic,
    Standard,
    Full,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SortMethod {
    Alphabetical,
    PathLength,
    None,
}

/// Intermediate representation for documentation generation
#[derive(Debug)]
pub struct ApiDocumentation {
    pub title: String,
    pub version: String,
    pub description: Option<String>,
    pub services: Vec<Service>,
    pub endpoints: Vec<Endpoint>,
    pub servers: Vec<String>,
    pub security_schemes: IndexMap<String, String>,
    pub schemas: IndexMap<String, Schema>,
    // Reusable examples (`components/examples`), keyed by name for `$ref` lookups.
    pub examples: IndexMap<String, Example>,
}
