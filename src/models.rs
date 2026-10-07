use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// OpenAPI spec model with flexibility for both 2.0 and 3.0 formats
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct OpenApiSpec {
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
/// Policy for extra properties of an object schema.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum AdditionalProperties {
    /// Permit or forbid extra object properties.
    Bool(bool),
    /// Constrain extra object properties with a schema.
    Schema(Box<Schema>),
}

/// Deserializes an OpenAPI `type` as either a string (2.0/3.0) or an array of
/// strings (3.1, e.g. `["string", "null"]`), preserving all non-`"null"` types
/// as a pipe-separated string so union schemas are represented in full.
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
        Some(TypeField::Multi(types)) => {
            let non_null_types: Vec<String> = types.into_iter().filter(|t| t != "null").collect();
            if non_null_types.is_empty() {
                None
            } else {
                Some(non_null_types.join(" | "))
            }
        }
    })
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct Info {
    pub title: String,
    pub version: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct Tag {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct PathItem {
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
pub(crate) struct Operation {
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

/// A parameter declaration, or a synthetic OAS3 request-body parameter.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Parameter {
    // A parameter may be a `$ref` into `components/parameters`; capture it so the
    // parser can resolve it. `name`/`in` default to empty so the bare `$ref` form
    // (which omits them) still deserializes — they come from the resolved target.
    /// A `$ref` URI into the source specification.
    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// The declared name.
    #[serde(default)]
    pub name: String,
    /// Optional source description.
    pub description: Option<String>,
    /// Parameter location (`in`) from the specification.
    #[serde(rename = "in", default)]
    pub parameter_in: String,
    /// Requiredness from the source declaration.
    pub required: Option<bool>,
    /// Declared schema for this value.
    pub schema: Option<Schema>,
    // Example carriers. Real OpenAPI 3 parameters may define these directly; the
    // parser also reuses them to ferry a request body's media-type examples into
    // the synthetic `body` parameter so the generator can render them.
    /// Single source example, including explicit JSON null.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,
    /// Named source examples in declaration order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<IndexMap<String, Example>>,
    /// Unrecognized source fields preserved as JSON values.
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_json::Value>,
}

// `Default` is for constructing schemas in code (tests build partial schemas
// via `..Schema::default()`); serde does not consult it during deserialization
// — missing fields fall back to `Option`/empty-map defaults on their own.
/// A source schema with property order and unknown metadata preserved.
#[derive(Debug, Default, Deserialize, Serialize, Clone)]
pub struct Schema {
    /// Declared title.
    #[serde(rename = "title", skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional source description.
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(
        rename = "type",
        default,
        deserialize_with = "deserialize_optional_type",
        skip_serializing_if = "Option::is_none"
    )]
    /// Type keyword, normalized from supported OAS 3.1 type arrays.
    pub schema_type: Option<String>,
    /// Optional format annotation such as `int64` or `date-time`.
    #[serde(rename = "format", skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// A `$ref` URI into the source specification.
    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Object properties in declaration order.
    #[serde(rename = "properties", skip_serializing_if = "Option::is_none")]
    pub properties: Option<IndexMap<String, Schema>>,
    /// Schema of each array item.
    #[serde(rename = "items", skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<Schema>>,
    /// Names of required object properties.
    #[serde(rename = "required", skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
    /// Composition members that must all match.
    #[serde(rename = "allOf", skip_serializing_if = "Option::is_none")]
    pub all_of: Option<Vec<Schema>>,
    /// Composition members of which exactly one must match.
    #[serde(rename = "oneOf", skip_serializing_if = "Option::is_none")]
    pub one_of: Option<Vec<Schema>>,
    /// Composition members of which at least one must match.
    #[serde(rename = "anyOf", skip_serializing_if = "Option::is_none")]
    pub any_of: Option<Vec<Schema>>,
    /// Allowed values in declaration order.
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<serde_json::Value>>,
    /// Whether null is explicitly permitted.
    #[serde(rename = "nullable", skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,
    #[serde(
        rename = "additionalProperties",
        skip_serializing_if = "Option::is_none"
    )]
    /// Whether extra object properties are allowed, or their schema.
    pub additional_properties: Option<AdditionalProperties>,
    /// Unrecognized source fields preserved as JSON values.
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_json::Value>,
}

/// A response declaration with its schema and media types.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Response {
    // A response may be given as a `$ref` into `components/responses`;
    // `resolve_response_ref` resolves it during parsing.
    /// A `$ref` URI into the source specification.
    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Optional source description.
    pub description: Option<String>,
    /// Declared schema for this value.
    pub schema: Option<Schema>,
    /// Media types in declaration order.
    #[serde(rename = "content", skip_serializing_if = "Option::is_none")]
    pub content: Option<IndexMap<String, MediaType>>,
    /// Unrecognized source fields preserved as JSON values.
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_json::Value>,
}

// Server definition for OpenAPI 3.0+
#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct Server {
    pub url: String,
    pub description: Option<String>,
    pub variables: Option<HashMap<String, ServerVariable>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct ServerVariable {
    #[serde(rename = "enum")]
    pub enum_values: Option<Vec<String>>,
    pub default: String,
    pub description: Option<String>,
}

// Components definition for OpenAPI 3.0+
#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct Components {
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
/// A named example declaration or reference.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Example {
    // A media-type `examples` entry may be a `$ref` into `components/examples`;
    // capture it so the generator can resolve the reference.
    /// A `$ref` URI into the source specification.
    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Optional operation summary.
    pub summary: Option<String>,
    /// Optional source description.
    pub description: Option<String>,
    /// Inline example value.
    pub value: Option<serde_json::Value>,
    /// URI of an externally stored example.
    #[serde(rename = "externalValue", skip_serializing_if = "Option::is_none")]
    pub external_value: Option<String>,
    /// Unrecognized source fields preserved as JSON values.
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_json::Value>,
}

// RequestBody struct
#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct RequestBody {
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
/// Schema, examples, and encoding for a media type.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MediaType {
    /// Declared schema for this value.
    pub schema: Option<Schema>,
    /// Single source example, including explicit JSON null.
    pub example: Option<serde_json::Value>,
    // IndexMap preserves spec order so rendered examples are deterministic.
    /// Named source examples in declaration order.
    pub examples: Option<IndexMap<String, Example>>,
    /// Per-property encoding declarations.
    pub encoding: Option<HashMap<String, Encoding>>,
    /// Unrecognized source fields preserved as JSON values.
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_json::Value>,
}

// Encoding struct
/// Serialization metadata for one media-type property.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Encoding {
    /// Content type for this encoded property.
    #[serde(rename = "contentType")]
    pub content_type: Option<String>,
    /// Header declarations for this encoding.
    pub headers: Option<HashMap<String, Header>>,
    /// Serialization style declared by the spec.
    pub style: Option<String>,
    /// Whether arrays and objects expand into separate parameters.
    pub explode: Option<bool>,
    /// Whether reserved characters may remain unescaped.
    #[serde(rename = "allowReserved")]
    pub allow_reserved: Option<bool>,
}

// Header struct
/// Header description and schema.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Header {
    /// Optional source description.
    pub description: Option<String>,
    /// Declared schema for this value.
    pub schema: Option<Schema>,
}

// SecurityScheme struct
#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct SecurityScheme {
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
pub(crate) struct OAuthFlows {
    pub implicit: Option<OAuthFlow>,
    pub password: Option<OAuthFlow>,
    #[serde(rename = "clientCredentials")]
    pub client_credentials: Option<OAuthFlow>,
    #[serde(rename = "authorizationCode")]
    pub authorization_code: Option<OAuthFlow>,
}

// OAuthFlow struct
#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct OAuthFlow {
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
pub(crate) struct Link {
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
pub(crate) struct Callback {
    // A more complete version would define this properly
    #[serde(flatten)]
    pub expression: HashMap<String, serde_json::Value>,
}

/// A tag-derived service used to group operations.
#[derive(Debug, Clone)]
pub struct Service {
    /// The declared name.
    pub name: String,
    /// Optional source description.
    pub description: Option<String>,
}

/// An operation normalized across OpenAPI versions.
#[derive(Debug, Clone)]
pub struct Endpoint {
    /// Exact path template from the spec, without Swagger 2 basePath.
    pub path: String,
    /// Uppercase HTTP method.
    pub method: String,
    /// Service/tag names assigned to the operation.
    pub services: Vec<String>, // References to service names
    /// Optional operation summary.
    pub summary: Option<String>,
    /// Optional source description.
    pub description: Option<String>,
    /// Optional operationId exactly as declared.
    pub operation_id: Option<String>,
    /// Merged parameters, including synthetic OAS3 request-body parameters.
    pub parameters: Vec<Parameter>,
    /// Responses keyed by status code in declaration order.
    pub responses: IndexMap<String, Response>,
    /// Whether the operation is marked deprecated.
    pub deprecated: bool,
    // True when the operation declared no usable tags and the parser attributed
    // it to the default service. `services` alone can't reveal this: the default
    // is the first declared tag, so an untagged endpoint looks identical to one
    // explicitly tagged with it. The spec hygiene report reads this flag.
    /// Whether missing usable tags required attribution to a default service.
    pub untagged: bool,
}

/// Configuration for documentation generation
#[derive(Debug, Clone)]
pub struct DocConfig {
    /// How rendered operations are grouped.
    pub group_by: GroupBy,
    /// Optional service/tag names to include.
    pub service_filter: Option<Vec<String>>,
    /// Optional substring that must occur in the operation path.
    pub path_filter: Option<String>,
    /// Optional uppercase HTTP methods to include.
    pub method_filter: Option<Vec<String>>,
    // Exact operations picked with `--operation`/`--operation-id`; `None` when
    // neither flag was given. ANDed with the other filters.
    /// Exact operation selections; matching either set includes the operation.
    pub operation_selector: Option<OperationSelector>,
    /// Omit deprecated operations.
    pub exclude_deprecated: bool,
    /// Show only required parameters at standard/full detail.
    pub required_only: bool,
    /// Requested detail; token budgets may reduce it for operation reads.
    pub detail_level: DetailLevel,
    /// Include schemas at full detail.
    pub include_schemas: bool,
    // Expand every `$ref` inline at each use site instead of linking to a shared
    // "Schema Definitions" section (the fully self-contained output).
    /// Expand references at each use site instead of linking definitions.
    pub inline_schemas: bool,
    /// Maximum schema traversal edges from a root, in `0..=24`.
    pub schema_depth: Option<usize>,
    /// Named schemas to render independently of endpoint reachability.
    pub schema_names: Vec<String>,
    /// Subtree selectors in `NAME#JSON_POINTER` form, retaining ancestor metadata.
    pub schema_fields: Vec<String>,
    /// Source label used in retrieval commands; defaults to `spec.json`.
    pub source_path: Option<String>,
    /// Include examples at full detail.
    pub include_examples: bool,
    /// Include server and authentication metadata.
    pub include_auth: bool,
    /// Include a table of contents.
    pub include_toc: bool,
    /// Order of rendered operations.
    pub sort_method: SortMethod,
    // When set, the generator renders at progressively lower detail until the
    // estimated token count fits this budget (`--max-tokens`).
    /// Approximate body budget using characters/4; explicit schema reads remain intact.
    pub max_tokens: Option<usize>,
    // Append the spec hygiene report after the documentation (on by default;
    // `--no-report` turns it off).
    /// Append hygiene in the CLI adapter; body-only Markdown rendering ignores this flag.
    pub include_report: bool,
}

impl DocConfig {
    /// A configuration that covers the whole spec at the richest detail level:
    /// no filters, `--detail full --include-schemas`, default grouping and
    /// sorting, no token budget, no hygiene report. `diff --report` uses it so
    /// the hygiene and token deltas describe the complete documents.
    pub fn unfiltered() -> Self {
        Self {
            group_by: GroupBy::Service,
            service_filter: None,
            path_filter: None,
            method_filter: None,
            operation_selector: None,
            exclude_deprecated: false,
            required_only: false,
            detail_level: DetailLevel::Full,
            include_schemas: true,
            inline_schemas: false,
            schema_depth: None,
            schema_names: Vec::new(),
            schema_fields: Vec::new(),
            source_path: None,
            include_examples: false,
            include_auth: false,
            include_toc: true,
            sort_method: SortMethod::Alphabetical,
            max_tokens: None,
            include_report: false,
        }
    }
}

/// One `--operation` value: an uppercased HTTP method and a path template that
/// must equal [`Endpoint::path`] byte for byte.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OperationRef {
    /// Uppercase HTTP method.
    pub method: String,
    /// Exact path template from the spec, without Swagger 2 basePath.
    pub path: String,
}

impl OperationRef {
    /// Whether the exact selector includes the given endpoint.
    pub fn matches(&self, endpoint: &Endpoint) -> bool {
        endpoint.method == self.method && endpoint.path == self.path
    }
}

impl std::fmt::Display for OperationRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.method, self.path)
    }
}

/// The exact-operation selection from `--operation` and `--operation-id`. An
/// endpoint is selected when it matches any entry of either set (a union).
/// `IndexSet` keeps the order the values were given, so the unmatched-selector
/// error lists them deterministically.
#[derive(Debug, Clone, Default)]
pub struct OperationSelector {
    /// Exact method/path selections in insertion order.
    pub operations: IndexSet<OperationRef>,
    /// Case-sensitive operationId selections in insertion order.
    pub operation_ids: IndexSet<String>,
}

impl OperationSelector {
    /// Whether the exact selector includes the given endpoint.
    pub fn matches(&self, endpoint: &Endpoint) -> bool {
        self.operations.iter().any(|op| op.matches(endpoint))
            || endpoint
                .operation_id
                .as_ref()
                .is_some_and(|id| self.operation_ids.contains(id))
    }
}

/// Grouping strategy for Markdown operation context.
#[derive(Debug, Clone, PartialEq)]
pub enum GroupBy {
    /// Group by tag-derived service.
    Service,
    /// Group by HTTP method.
    Method,
    /// Group by path hierarchy.
    Path,
    /// Render a flat operation list.
    Flat,
}

/// Amount of information included in operation context.
#[derive(Debug, Clone, PartialEq)]
pub enum DetailLevel {
    /// Render only operation navigation.
    Summary,
    /// Render operation headings and summaries.
    Basic,
    /// Also render descriptions and parameter tables.
    Standard,
    /// Render full detail, including enabled schemas and examples.
    Full,
}

/// Operation ordering within a rendered group.
#[derive(Debug, Clone, PartialEq)]
pub enum SortMethod {
    /// Sort lexically by operation path.
    Alphabetical,
    /// Sort by path length.
    PathLength,
    /// Preserve spec declaration order.
    None,
}

/// Intermediate representation for documentation generation
#[derive(Debug)]
pub struct ApiDocumentation {
    /// Declared title.
    pub title: String,
    /// API version declared by the source spec.
    pub version: String,
    /// Optional source description.
    pub description: Option<String>,
    /// Declared and inferred services in source order.
    pub services: Vec<Service>,
    /// Operations in source order.
    pub endpoints: Vec<Endpoint>,
    /// Resolved server URLs.
    pub servers: Vec<String>,
    /// Authentication scheme descriptions keyed by name.
    pub security_schemes: IndexMap<String, String>,
    /// Reusable schemas keyed by name in declaration order.
    pub schemas: IndexMap<String, Schema>,
    // Reusable examples (`components/examples`), keyed by name for `$ref` lookups.
    /// Named source examples in declaration order.
    pub examples: IndexMap<String, Example>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "cli"), allow(dead_code))]
pub(crate) enum SplitMode {
    Service,
    Tag,
    Endpoint,
}
