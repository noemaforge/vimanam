//! GraphQL adapter for SDL (`.graphql`/`.graphqls`, single or merged files) and
//! introspection JSON (`__schema` results). Root fields of the query, mutation
//! and subscription roots are operations; named types, fields, arguments, input
//! fields and enum values keep schema coordinates such as `Type.field(arg:)`.
//!
//! Breaking rules are directional: an output field that becomes nullable breaks
//! readers, while an input that becomes non-null without a default breaks
//! callers. Federation directives, schema stitching and executable documents are
//! out of scope.
use super::*;
use anyhow::{Context, Result, bail};
use graphql_parser::schema as ast;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// Default deprecation reason defined by the GraphQL specification.
pub const DEFAULT_DEPRECATION_REASON: &str = "No longer supported";
const BUILTIN_SCALARS: &[&str] = &["Int", "Float", "String", "Boolean", "ID"];
const PAGINATION_CONTROLS: &[&str] = &["first", "after", "last", "before"];

/// Where a schema came from. Both sources map to the same model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// Schema definition language, possibly merged from several files.
    Sdl,
    /// Introspection query result.
    Introspection,
}

/// GraphQL type reference with list and non-null wrappers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeRef {
    /// A named type.
    Named(String),
    /// `[T]`.
    List(Box<TypeRef>),
    /// `T!`.
    NonNull(Box<TypeRef>),
}
impl TypeRef {
    /// Innermost named type.
    pub fn named(&self) -> &str {
        match self {
            Self::Named(n) => n,
            Self::List(t) | Self::NonNull(t) => t.named(),
        }
    }
    /// Whether the outermost wrapper is non-null.
    pub fn is_non_null(&self) -> bool {
        matches!(self, Self::NonNull(_))
    }
    /// Whether any list wrapper is present.
    pub fn is_list(&self) -> bool {
        match self {
            Self::Named(_) => false,
            Self::List(_) => true,
            Self::NonNull(t) => t.is_list(),
        }
    }
    fn strip_non_null(&self) -> TypeRef {
        match self {
            Self::Named(n) => Self::Named(n.clone()),
            Self::List(t) => Self::List(Box::new(t.strip_non_null())),
            Self::NonNull(t) => t.strip_non_null(),
        }
    }
}
impl fmt::Display for TypeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named(n) => write!(f, "{n}"),
            Self::List(t) => write!(f, "[{t}]"),
            Self::NonNull(t) => write!(f, "{t}!"),
        }
    }
}

/// Deprecation state; SDL `@deprecated` without a reason gets the spec default.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Deprecation {
    /// Whether the member is deprecated.
    pub deprecated: bool,
    /// Declared or default reason.
    pub reason: Option<String>,
}

/// Argument or input field.
#[derive(Debug, Clone, PartialEq)]
pub struct InputValueDef {
    /// Name.
    pub name: String,
    /// Description (annotation only, never compared).
    pub description: Option<String>,
    /// Declared type.
    pub ty: TypeRef,
    /// Default value printed as a canonical GraphQL literal.
    pub default: Option<String>,
    /// Deprecation.
    pub deprecation: Deprecation,
}
impl InputValueDef {
    /// Required: non-null without a default.
    pub fn required(&self) -> bool {
        self.ty.is_non_null() && self.default.is_none()
    }
}

/// Object or interface field.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDef {
    /// Name.
    pub name: String,
    /// Description (annotation only).
    pub description: Option<String>,
    /// Arguments in declaration order.
    pub args: Vec<InputValueDef>,
    /// Result type.
    pub ty: TypeRef,
    /// Deprecation.
    pub deprecation: Deprecation,
}

/// Enum value.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumValueDef {
    /// Name.
    pub name: String,
    /// Description (annotation only).
    pub description: Option<String>,
    /// Deprecation.
    pub deprecation: Deprecation,
}

/// Named type kind, spelled as in introspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TypeKind {
    /// Scalar.
    Scalar,
    /// Object.
    Object,
    /// Interface.
    Interface,
    /// Union.
    Union,
    /// Enum.
    Enum,
    /// Input object.
    InputObject,
}
impl TypeKind {
    fn label(self) -> &'static str {
        match self {
            Self::Scalar => "SCALAR",
            Self::Object => "OBJECT",
            Self::Interface => "INTERFACE",
            Self::Union => "UNION",
            Self::Enum => "ENUM",
            Self::InputObject => "INPUT_OBJECT",
        }
    }
}

/// A named type. Only members relevant to the kind are populated.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeDef {
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: TypeKind,
    /// Description (annotation only).
    pub description: Option<String>,
    /// Object/interface fields.
    pub fields: Vec<FieldDef>,
    /// Input object fields.
    pub input_fields: Vec<InputValueDef>,
    /// Enum values.
    pub values: Vec<EnumValueDef>,
    /// Implemented interfaces.
    pub interfaces: Vec<String>,
    /// Union members.
    pub members: Vec<String>,
}
impl TypeDef {
    fn new(name: &str, kind: TypeKind, description: Option<String>) -> Self {
        Self {
            name: name.into(),
            kind,
            description,
            fields: vec![],
            input_fields: vec![],
            values: vec![],
            interfaces: vec![],
            members: vec![],
        }
    }
}

/// Root operation kinds.
pub const ROOTS: [&str; 3] = ["query", "mutation", "subscription"];

/// Parsed GraphQL schema. Built-in scalars and `__` introspection types are
/// excluded so SDL and introspection inputs describe the same surface.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// Input source kind.
    pub source: SourceKind,
    /// Root type names keyed by `query`/`mutation`/`subscription`.
    pub roots: BTreeMap<String, String>,
    /// Named types sorted by name.
    pub types: BTreeMap<String, TypeDef>,
}

impl Document {
    /// Root operation keyword for a type name, if it is a root.
    pub fn root_of(&self, type_name: &str) -> Option<&str> {
        self.roots
            .iter()
            .find(|(_, name)| name.as_str() == type_name)
            .map(|(kind, _)| kind.as_str())
    }

    /// Parse and merge SDL sources `(label, text)`. Type extensions may appear
    /// in any file; duplicate definitions and unknown type references fail.
    pub fn from_sdl(sources: &[(String, String)]) -> Result<Self> {
        let mut types: BTreeMap<String, (TypeDef, String)> = BTreeMap::new();
        let mut roots = BTreeMap::new();
        let mut schema_seen: Option<String> = None;
        let documents = sources
            .iter()
            .map(|(label, text)| {
                if has_schema_extension(text) {
                    bail!(
                        "Unsupported `extend schema` in {label}: declare root operation types in a single `schema {{ ... }}` definition"
                    );
                }
                ast::parse_schema::<String>(text)
                    .map(|doc| (label, doc))
                    .map_err(|e| anyhow::anyhow!("Invalid GraphQL SDL in {label}: {e}"))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut extensions = vec![];
        for (label, doc) in &documents {
            for definition in &doc.definitions {
                match definition {
                    ast::Definition::SchemaDefinition(s) => {
                        if let Some(previous) = &schema_seen {
                            bail!("Duplicate GraphQL schema definition in {label} and {previous}");
                        }
                        schema_seen = Some((*label).clone());
                        for (kind, name) in
                            ROOTS.iter().zip([&s.query, &s.mutation, &s.subscription])
                        {
                            if let Some(name) = name {
                                roots.insert((*kind).to_owned(), name.clone());
                            }
                        }
                    }
                    ast::Definition::TypeDefinition(t) => {
                        let def = sdl_type(t);
                        if is_builtin(&def.name) {
                            continue;
                        }
                        if let Some((_, previous)) = types.get(&def.name) {
                            bail!(
                                "Duplicate GraphQL type {} in {label} and {previous}",
                                def.name
                            );
                        }
                        types.insert(def.name.clone(), (def, (*label).clone()));
                    }
                    ast::Definition::TypeExtension(e) => extensions.push((label.as_str(), e)),
                    ast::Definition::DirectiveDefinition(_) => {}
                }
            }
        }
        for (label, extension) in extensions {
            apply_extension(&mut types, label, extension)?;
        }
        if schema_seen.is_none() {
            for (kind, name) in ROOTS.iter().zip(["Query", "Mutation", "Subscription"]) {
                if types.contains_key(name) {
                    roots.insert((*kind).to_owned(), name.to_owned());
                }
            }
        }
        let document = Self {
            source: SourceKind::Sdl,
            roots,
            types: types.into_iter().map(|(k, (t, _))| (k, t)).collect(),
        };
        document.validate()?;
        Ok(document)
    }

    /// Parse an introspection result: `{"data":{"__schema":…}}` or `{"__schema":…}`.
    pub fn from_introspection(bytes: &[u8]) -> Result<Self> {
        let value: Value =
            serde_json::from_slice(bytes).context("Invalid GraphQL introspection JSON")?;
        let schema = value
            .get("data")
            .and_then(|d| d.get("__schema"))
            .or_else(|| value.get("__schema"))
            .context("GraphQL introspection JSON must contain __schema or data.__schema")?;
        let mut roots = BTreeMap::new();
        for (kind, key) in ROOTS
            .iter()
            .zip(["queryType", "mutationType", "subscriptionType"])
        {
            if let Some(name) = schema
                .get(key)
                .and_then(|t| t.get("name"))
                .and_then(Value::as_str)
            {
                roots.insert((*kind).to_owned(), name.to_owned());
            }
        }
        let mut types = BTreeMap::new();
        for t in schema
            .get("types")
            .and_then(Value::as_array)
            .context("Introspection __schema.types must be an array")?
        {
            let name = str_field(t, "name")?;
            if is_builtin(name) {
                continue;
            }
            let kind = match str_field(t, "kind")? {
                "SCALAR" => TypeKind::Scalar,
                "OBJECT" => TypeKind::Object,
                "INTERFACE" => TypeKind::Interface,
                "UNION" => TypeKind::Union,
                "ENUM" => TypeKind::Enum,
                "INPUT_OBJECT" => TypeKind::InputObject,
                other => bail!("Unknown introspection type kind {other:?} for {name}"),
            };
            let mut def = TypeDef::new(name, kind, opt_str(t, "description"));
            for f in array(t, "fields") {
                def.fields.push(FieldDef {
                    name: str_field(f, "name")?.into(),
                    description: opt_str(f, "description"),
                    args: array(f, "args")
                        .map(introspection_input)
                        .collect::<Result<_>>()?,
                    ty: introspection_type(f.get("type").context("Field without type")?)?,
                    deprecation: introspection_deprecation(f),
                });
            }
            def.input_fields = array(t, "inputFields")
                .map(introspection_input)
                .collect::<Result<_>>()?;
            for v in array(t, "enumValues") {
                def.values.push(EnumValueDef {
                    name: str_field(v, "name")?.into(),
                    description: opt_str(v, "description"),
                    deprecation: introspection_deprecation(v),
                });
            }
            def.interfaces = array(t, "interfaces")
                .map(|i| str_field(i, "name").map(str::to_owned))
                .collect::<Result<_>>()?;
            if kind == TypeKind::Union {
                def.members = array(t, "possibleTypes")
                    .map(|i| str_field(i, "name").map(str::to_owned))
                    .collect::<Result<_>>()?;
            }
            if types.insert(name.to_owned(), def).is_some() {
                bail!("Duplicate GraphQL type {name} in introspection result");
            }
        }
        let document = Self {
            source: SourceKind::Introspection,
            roots,
            types,
        };
        document.validate()?;
        Ok(document)
    }

    /// Parse exact bytes: an introspection result when the JSON has
    /// `__schema`, SDL otherwise. `label` names the source in errors.
    pub fn from_bytes(bytes: &[u8], label: &str) -> Result<Self> {
        if is_introspection_bytes(bytes) {
            Self::from_introspection(bytes)
        } else {
            let text = std::str::from_utf8(bytes)
                .with_context(|| format!("GraphQL SDL must be UTF-8: {label}"))?;
            Self::from_sdl(&[(label.to_owned(), text.to_owned())])
        }
    }

    /// Read a `.graphql`/`.graphqls` file, an introspection `.json` file, or a
    /// directory whose SDL files (recursively, sorted by path) are merged.
    pub fn read(path: &Path) -> Result<Self> {
        if path.is_dir() {
            return Self::from_sdl(&read_sdl_dir(path)?);
        }
        let bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read GraphQL schema: {path:?}"))?;
        Self::from_bytes(&bytes, &path.display().to_string())
    }

    fn validate(&self) -> Result<()> {
        for (kind, name) in &self.roots {
            let Some(t) = self.types.get(name) else {
                bail!("GraphQL {kind} root type {name} is not defined");
            };
            anyhow::ensure!(
                t.kind == TypeKind::Object,
                "GraphQL {kind} root type {name} must be an object type"
            );
        }
        let known = |name: &str| BUILTIN_SCALARS.contains(&name) || self.types.contains_key(name);
        for t in self.types.values() {
            let mut referenced: Vec<(String, &str)> = vec![];
            for f in &t.fields {
                referenced.push((format!("{}.{}", t.name, f.name), f.ty.named()));
                for a in &f.args {
                    referenced.push((format!("{}.{}({}:)", t.name, f.name, a.name), a.ty.named()));
                }
            }
            for f in &t.input_fields {
                referenced.push((format!("{}.{}", t.name, f.name), f.ty.named()));
            }
            for i in t.interfaces.iter().chain(&t.members) {
                referenced.push((t.name.clone(), i));
            }
            for (coordinate, name) in referenced {
                anyhow::ensure!(
                    known(name),
                    "Unknown GraphQL type {name} referenced by {coordinate}"
                );
            }
        }
        Ok(())
    }
}

/// `extend schema` outside strings and comments (graphql-parser 0.4 cannot
/// parse it); detected so the error is explicit rather than a syntax error.
fn has_schema_extension(text: &str) -> bool {
    let mut previous_word: Option<&str> = None;
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if let Some(body) = rest.strip_prefix("\"\"\"") {
            rest = body.find("\"\"\"").map_or("", |i| &body[i + 3..]);
            previous_word = None;
        } else if c == '"' {
            let mut escaped = false;
            let end = rest[1..].char_indices().find(|&(_, ch)| {
                let done = ch == '"' && !escaped;
                escaped = ch == '\\' && !escaped;
                done
            });
            rest = end.map_or("", |(i, _)| &rest[i + 2..]);
            previous_word = None;
        } else if c == '#' {
            rest = rest.find('\n').map_or("", |i| &rest[i..]);
        } else if c.is_ascii_alphanumeric() || c == '_' {
            let end = rest
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                .unwrap_or(rest.len());
            let word = &rest[..end];
            if previous_word == Some("extend") && word == "schema" {
                return true;
            }
            previous_word = Some(word);
            rest = &rest[end..];
        } else {
            // Whitespace and commas separate tokens; punctuation breaks a pair.
            if !(c.is_whitespace() || c == ',') {
                previous_word = None;
            }
            rest = &rest[c.len_utf8()..];
        }
    }
    false
}

fn is_builtin(name: &str) -> bool {
    name.starts_with("__") || BUILTIN_SCALARS.contains(&name)
}

/// Read every SDL file below a directory as `(path, text)`, sorted by path.
pub fn read_sdl_dir(dir: &Path) -> Result<Vec<(String, String)>> {
    let files = sdl_files(dir)?;
    anyhow::ensure!(
        !files.is_empty(),
        "No .graphql or .graphqls files in directory {dir:?}"
    );
    files
        .into_iter()
        .map(|file| {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("Failed to read GraphQL SDL: {file:?}"))?;
            Ok((file.display().to_string(), text))
        })
        .collect()
}

/// SDL files below a directory, sorted by path for deterministic merging.
/// Symlinks are followed; a directory reached twice (including symlink loops)
/// is listed once, and a file reached through several paths is merged once,
/// under its first path in sort order. A dangling SDL-named link is an error.
pub fn sdl_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = vec![];
    let mut visited = std::collections::HashSet::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let canonical = d
            .canonicalize()
            .with_context(|| format!("Failed to resolve {d:?}"))?;
        if !visited.insert(canonical) {
            continue;
        }
        for entry in std::fs::read_dir(&d).with_context(|| format!("Failed to list {d:?}"))? {
            let path = entry?.path();
            let metadata = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(e) if is_sdl_path(&path) => {
                    return Err(e).with_context(|| format!("Unreadable GraphQL SDL {path:?}"));
                }
                Err(_) => continue,
            };
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() && is_sdl_path(&path) {
                out.push(path);
            }
        }
    }
    out.sort();
    let mut seen = std::collections::HashSet::new();
    let mut unique = vec![];
    for path in out {
        let canonical = path
            .canonicalize()
            .with_context(|| format!("Failed to resolve {path:?}"))?;
        if seen.insert(canonical) {
            unique.push(path);
        }
    }
    Ok(unique)
}

/// `.graphql` or `.graphqls`, case-insensitively.
pub fn is_sdl_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("graphql") || e.eq_ignore_ascii_case("graphqls"))
}

/// Whether bytes are an introspection result (`__schema` object at the top
/// level or under `data`). A substring check runs first so ordinary OpenAPI
/// JSON is never parsed twice.
pub fn is_introspection_bytes(bytes: &[u8]) -> bool {
    if !bytes.windows(10).any(|w| w == b"\"__schema\"") {
        return false;
    }
    serde_json::from_slice::<Value>(bytes).is_ok_and(|v| {
        v.get("__schema").is_some_and(Value::is_object)
            || v.get("data")
                .and_then(|d| d.get("__schema"))
                .is_some_and(Value::is_object)
    })
}

/// Whether a path is GraphQL input: an SDL file, a directory containing SDL
/// files, or a `.json` introspection result.
pub fn is_input(path: &Path) -> bool {
    if is_sdl_path(path) {
        return true;
    }
    if path.is_dir() {
        return sdl_files(path).is_ok_and(|f| !f.is_empty());
    }
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        && std::fs::read(path).is_ok_and(|b| is_introspection_bytes(&b))
}

fn str_field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("Introspection entry missing string {key:?}"))
}
fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}
fn array<'a>(v: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    v.get(key).and_then(Value::as_array).into_iter().flatten()
}
fn introspection_type(v: &Value) -> Result<TypeRef> {
    Ok(match str_field(v, "kind")? {
        "NON_NULL" => TypeRef::NonNull(Box::new(introspection_type(
            v.get("ofType").context("NON_NULL without ofType")?,
        )?)),
        "LIST" => TypeRef::List(Box::new(introspection_type(
            v.get("ofType").context("LIST without ofType")?,
        )?)),
        _ => TypeRef::Named(str_field(v, "name")?.into()),
    })
}
fn introspection_deprecation(v: &Value) -> Deprecation {
    let deprecated = v.get("isDeprecated").and_then(Value::as_bool) == Some(true);
    Deprecation {
        deprecated,
        reason: if deprecated {
            Some(
                opt_str(v, "deprecationReason")
                    .unwrap_or_else(|| DEFAULT_DEPRECATION_REASON.into()),
            )
        } else {
            None
        },
    }
}
fn introspection_input(v: &Value) -> Result<InputValueDef> {
    Ok(InputValueDef {
        name: str_field(v, "name")?.into(),
        description: opt_str(v, "description"),
        ty: introspection_type(v.get("type").context("Input value without type")?)?,
        default: v
            .get("defaultValue")
            .and_then(Value::as_str)
            .map(canonical_literal)
            .transpose()?,
        deprecation: introspection_deprecation(v),
    })
}

/// Canonical printed form of a GraphQL literal (object keys sorted), so SDL
/// and introspection defaults compare equal.
pub fn canonical_literal(literal: &str) -> Result<String> {
    let source = format!("query($v: V = {literal}) {{ v }}");
    let document = graphql_parser::parse_query::<String>(&source)
        .map_err(|e| anyhow::anyhow!("Invalid GraphQL default value {literal:?}: {e}"))?;
    let Some(graphql_parser::query::Definition::Operation(
        graphql_parser::query::OperationDefinition::Query(q),
    )) = document.definitions.first()
    else {
        bail!("Invalid GraphQL default value {literal:?}");
    };
    q.variable_definitions
        .first()
        .and_then(|v| v.default_value.as_ref())
        .map(print_value)
        .with_context(|| format!("Invalid GraphQL default value {literal:?}"))
}

fn print_value(v: &graphql_parser::query::Value<'_, String>) -> String {
    use graphql_parser::query::Value as V;
    match v {
        V::Variable(n) => format!("${n}"),
        // graphql-parser stores integers as i64, so `as_i64` always succeeds.
        V::Int(n) => n.as_i64().map(|n| n.to_string()).unwrap_or_default(),
        V::Float(f) => {
            let mut s = f.to_string();
            if !s.contains(['.', 'e', 'E', 'N', 'i']) {
                s.push_str(".0");
            }
            s
        }
        V::String(s) => serde_json::to_string(s).unwrap_or_default(),
        V::Boolean(b) => b.to_string(),
        V::Null => "null".into(),
        V::Enum(e) => e.clone(),
        V::List(items) => format!(
            "[{}]",
            items.iter().map(print_value).collect::<Vec<_>>().join(", ")
        ),
        V::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{k}: {}", print_value(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn sdl_typeref(t: &ast::Type<'_, String>) -> TypeRef {
    match t {
        ast::Type::NamedType(n) => TypeRef::Named(n.clone()),
        ast::Type::ListType(t) => TypeRef::List(Box::new(sdl_typeref(t))),
        ast::Type::NonNullType(t) => TypeRef::NonNull(Box::new(sdl_typeref(t))),
    }
}
fn sdl_deprecation(directives: &[ast::Directive<'_, String>]) -> Deprecation {
    let Some(d) = directives.iter().find(|d| d.name == "deprecated") else {
        return Deprecation::default();
    };
    let reason = d
        .arguments
        .iter()
        .find(|(name, _)| name == "reason")
        .and_then(|(_, v)| match v {
            ast::Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_else(|| DEFAULT_DEPRECATION_REASON.into());
    Deprecation {
        deprecated: true,
        reason: Some(reason),
    }
}
fn sdl_input(v: &ast::InputValue<'_, String>) -> InputValueDef {
    InputValueDef {
        name: v.name.clone(),
        description: v.description.clone(),
        ty: sdl_typeref(&v.value_type),
        default: v.default_value.as_ref().map(print_value),
        deprecation: sdl_deprecation(&v.directives),
    }
}
fn sdl_field(f: &ast::Field<'_, String>) -> FieldDef {
    FieldDef {
        name: f.name.clone(),
        description: f.description.clone(),
        args: f.arguments.iter().map(sdl_input).collect(),
        ty: sdl_typeref(&f.field_type),
        deprecation: sdl_deprecation(&f.directives),
    }
}
fn sdl_value(v: &ast::EnumValue<'_, String>) -> EnumValueDef {
    EnumValueDef {
        name: v.name.clone(),
        description: v.description.clone(),
        deprecation: sdl_deprecation(&v.directives),
    }
}
fn sdl_type(t: &ast::TypeDefinition<'_, String>) -> TypeDef {
    match t {
        ast::TypeDefinition::Scalar(s) => {
            TypeDef::new(&s.name, TypeKind::Scalar, s.description.clone())
        }
        ast::TypeDefinition::Object(o) => {
            let mut d = TypeDef::new(&o.name, TypeKind::Object, o.description.clone());
            d.fields = o.fields.iter().map(sdl_field).collect();
            d.interfaces = o.implements_interfaces.clone();
            d
        }
        ast::TypeDefinition::Interface(o) => {
            let mut d = TypeDef::new(&o.name, TypeKind::Interface, o.description.clone());
            d.fields = o.fields.iter().map(sdl_field).collect();
            d.interfaces = o.implements_interfaces.clone();
            d
        }
        ast::TypeDefinition::Union(u) => {
            let mut d = TypeDef::new(&u.name, TypeKind::Union, u.description.clone());
            d.members = u.types.clone();
            d
        }
        ast::TypeDefinition::Enum(e) => {
            let mut d = TypeDef::new(&e.name, TypeKind::Enum, e.description.clone());
            d.values = e.values.iter().map(sdl_value).collect();
            d
        }
        ast::TypeDefinition::InputObject(i) => {
            let mut d = TypeDef::new(&i.name, TypeKind::InputObject, i.description.clone());
            d.input_fields = i.fields.iter().map(sdl_input).collect();
            d
        }
    }
}

fn extend<T>(
    list: &mut Vec<T>,
    items: impl IntoIterator<Item = T>,
    key: impl Fn(&T) -> &str,
    what: &str,
    owner: &str,
    label: &str,
) -> Result<()> {
    for item in items {
        anyhow::ensure!(
            !list.iter().any(|existing| key(existing) == key(&item)),
            "Duplicate GraphQL {what} {owner}.{} from extension in {label}",
            key(&item)
        );
        list.push(item);
    }
    Ok(())
}

fn apply_extension(
    types: &mut BTreeMap<String, (TypeDef, String)>,
    label: &str,
    extension: &ast::TypeExtension<'_, String>,
) -> Result<()> {
    use ast::TypeExtension as X;
    let (name, kind) = match extension {
        X::Scalar(e) => (&e.name, TypeKind::Scalar),
        X::Object(e) => (&e.name, TypeKind::Object),
        X::Interface(e) => (&e.name, TypeKind::Interface),
        X::Union(e) => (&e.name, TypeKind::Union),
        X::Enum(e) => (&e.name, TypeKind::Enum),
        X::InputObject(e) => (&e.name, TypeKind::InputObject),
    };
    let Some((def, _)) = types.get_mut(name) else {
        bail!("GraphQL extension in {label} extends undefined type {name}");
    };
    anyhow::ensure!(
        def.kind == kind,
        "GraphQL extension in {label} extends {name} as {} but it is {}",
        kind.label(),
        def.kind.label()
    );
    fn field_name(f: &FieldDef) -> &str {
        &f.name
    }
    fn input_name(f: &InputValueDef) -> &str {
        &f.name
    }
    fn value_name(v: &EnumValueDef) -> &str {
        &v.name
    }
    fn string(s: &String) -> &str {
        s
    }
    match extension {
        X::Scalar(_) => {}
        X::Object(e) => {
            extend(
                &mut def.fields,
                e.fields.iter().map(sdl_field),
                field_name,
                "field",
                name,
                label,
            )?;
            extend(
                &mut def.interfaces,
                e.implements_interfaces.iter().cloned(),
                string,
                "interface",
                name,
                label,
            )?;
        }
        X::Interface(e) => {
            extend(
                &mut def.fields,
                e.fields.iter().map(sdl_field),
                field_name,
                "field",
                name,
                label,
            )?;
            extend(
                &mut def.interfaces,
                e.implements_interfaces.iter().cloned(),
                string,
                "interface",
                name,
                label,
            )?;
        }
        X::Union(e) => {
            extend(
                &mut def.members,
                e.types.iter().cloned(),
                string,
                "union member",
                name,
                label,
            )?;
        }
        X::Enum(e) => {
            extend(
                &mut def.values,
                e.values.iter().map(sdl_value),
                value_name,
                "enum value",
                name,
                label,
            )?;
        }
        X::InputObject(e) => {
            extend(
                &mut def.input_fields,
                e.fields.iter().map(sdl_input),
                input_name,
                "input field",
                name,
                label,
            )?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Neutral surface

fn id(kind: EntityKind, coordinate: &str) -> String {
    entity_id(Format::Graphql, kind, coordinate)
}
fn location(coordinate: &str) -> NativeLocation {
    NativeLocation::Graphql {
        coordinate: coordinate.into(),
    }
}
fn with_deprecation(mut attrs: Value, d: &Deprecation) -> Value {
    attrs["deprecated"] = json!(d.deprecated);
    attrs["deprecation_reason"] = json!(d.reason);
    attrs
}
fn type_reference(doc: &Document, ty: &TypeRef) -> Vec<String> {
    let name = ty.named();
    if doc.types.contains_key(name) {
        vec![id(EntityKind::Type, name)]
    } else {
        vec![]
    }
}
fn entity(
    kind: EntityKind,
    coordinate: &str,
    parent: Option<String>,
    attributes: Value,
    references: Vec<String>,
) -> Entity {
    Entity {
        id: id(kind, coordinate),
        kind,
        name: coordinate.into(),
        parent,
        services: vec![],
        native: location(coordinate),
        attributes,
        references,
    }
}
fn input_attributes(v: &InputValueDef) -> Value {
    with_deprecation(
        json!({"type": v.ty.to_string(), "default": v.default, "required": v.required()}),
        &v.deprecation,
    )
}
fn output_attributes(f: &FieldDef) -> Value {
    with_deprecation(json!({"type": f.ty.to_string()}), &f.deprecation)
}

/// Adapt a schema to the neutral surface. Root fields are operations under
/// their root service; other fields, arguments, input fields and enum values
/// use schema coordinates. Descriptions never enter identities or attributes.
pub fn adapt(doc: &Document) -> Contract {
    let mut entities = vec![];
    for (kind, root) in &doc.roots {
        entities.push(entity(
            EntityKind::Service,
            root,
            None,
            json!({"operation_type": kind}),
            vec![id(EntityKind::Type, root)],
        ));
    }
    for t in doc.types.values() {
        let type_id = id(EntityKind::Type, &t.name);
        let mut interfaces = t.interfaces.clone();
        interfaces.sort();
        let mut members = t.members.clone();
        members.sort();
        let root = doc.root_of(&t.name);
        let mut attrs = json!({"kind": t.kind, "interfaces": interfaces, "members": members});
        if let Some(root) = root {
            attrs["root"] = json!(root);
        }
        entities.push(entity(
            EntityKind::Type,
            &t.name,
            None,
            attrs,
            t.interfaces
                .iter()
                .chain(&t.members)
                .map(|n| id(EntityKind::Type, n))
                .collect(),
        ));
        for f in &t.fields {
            let coordinate = format!("{}.{}", t.name, f.name);
            let mut e = if let Some(root) = root {
                let service = id(EntityKind::Service, &t.name);
                let mut attrs = output_attributes(f);
                attrs["operation_type"] = json!(root);
                let mut e = entity(
                    EntityKind::Operation,
                    &coordinate,
                    Some(service.clone()),
                    attrs,
                    type_reference(doc, &f.ty),
                );
                e.services = vec![service];
                e
            } else {
                entity(
                    EntityKind::Field,
                    &coordinate,
                    Some(type_id.clone()),
                    output_attributes(f),
                    type_reference(doc, &f.ty),
                )
            };
            if let Some(p) = pagination_evidence(doc, f) {
                e.attributes["pagination"] = p;
            }
            let parent = e.id.clone();
            entities.push(e);
            for a in &f.args {
                entities.push(entity(
                    EntityKind::Argument,
                    &format!("{coordinate}({}:)", a.name),
                    Some(parent.clone()),
                    input_attributes(a),
                    type_reference(doc, &a.ty),
                ));
            }
        }
        for f in &t.input_fields {
            entities.push(entity(
                EntityKind::Field,
                &format!("{}.{}", t.name, f.name),
                Some(type_id.clone()),
                input_attributes(f),
                type_reference(doc, &f.ty),
            ));
        }
        for v in &t.values {
            entities.push(entity(
                EntityKind::Value,
                &format!("{}.{}", t.name, v.name),
                Some(type_id.clone()),
                with_deprecation(json!({}), &v.deprecation),
                vec![],
            ));
        }
    }
    Contract {
        format: Format::Graphql,
        title: "GraphQL schema".into(),
        version: String::new(),
        entities,
    }
}

// ---------------------------------------------------------------------------
// Pagination

/// Pagination evidence for an output field. A list result is
/// `{"shape":"list"}`. A Relay connection requires a named object/interface
/// result with `pageInfo` and `edges` whose type has `node`, plus `first` and
/// `after` (or `last` and `before`) arguments; its evidence records the item type and the
/// `first`/`after`/`last`/`before` control types and defaults. Backward-only
/// connections (`last` and `before`) qualify too. GraphQL has no
/// standard declaration of page-size limits, so limits are not inferred.
pub fn pagination_evidence(doc: &Document, field: &FieldDef) -> Option<Value> {
    if field.ty.is_list() {
        return Some(json!({"shape": "list", "item": field.ty.named()}));
    }
    let connection = doc.types.get(field.ty.named())?;
    if !matches!(connection.kind, TypeKind::Object | TypeKind::Interface) {
        return None;
    }
    let edges = connection.fields.iter().find(|f| f.name == "edges")?;
    connection.fields.iter().find(|f| f.name == "pageInfo")?;
    let edge = doc.types.get(edges.ty.named())?;
    let node = edge.fields.iter().find(|f| f.name == "node")?;
    let has = |n: &str| field.args.iter().any(|a| a.name == n);
    // Forward (`first`/`after`) or backward-only (`last`/`before`) Relay paging.
    if !((has("first") && has("after")) || (has("last") && has("before"))) {
        return None;
    }
    let controls: BTreeMap<_, _> = field
        .args
        .iter()
        .filter(|a| PAGINATION_CONTROLS.contains(&a.name.as_str()))
        .map(|a| {
            (
                a.name.clone(),
                json!({"type": a.ty.to_string(), "default": a.default}),
            )
        })
        .collect();
    Some(json!({
        "shape": "connection",
        "connection": connection.name,
        "item": node.ty.named(),
        "controls": controls,
    }))
}

// ---------------------------------------------------------------------------
// Diff

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compat {
    Safe,
    Breaking,
}

/// Output position: the new type must be at least as strict as the old one.
fn output_compat(old: &TypeRef, new: &TypeRef) -> Compat {
    use TypeRef::*;
    match (old, new) {
        (Named(a), Named(b)) if a == b => Compat::Safe,
        (List(a), List(b)) | (NonNull(a), NonNull(b)) => output_compat(a, b),
        (_, NonNull(b)) if !old.is_non_null() => output_compat(old, b),
        _ => Compat::Breaking,
    }
}

/// Input position: the new type must accept every value the old one did. An
/// outer non-null wrapper with a default does not oblige callers to send it.
fn input_compat(old: &TypeRef, new: &TypeRef, new_has_default: bool) -> Compat {
    use TypeRef::*;
    match (old, new) {
        (Named(a), Named(b)) if a == b => Compat::Safe,
        (List(a), List(b)) | (NonNull(a), NonNull(b)) => input_compat(a, b, false),
        (NonNull(a), _) => input_compat(a, new, false),
        (_, NonNull(b)) if new_has_default => input_compat(old, b, false),
        _ => Compat::Breaking,
    }
}

fn verdict(compat: Compat) -> Severity {
    match compat {
        Compat::Safe => Severity::NonBreaking,
        Compat::Breaking => Severity::Breaking,
    }
}

struct Facts<'a> {
    old: &'a Document,
    new: &'a Document,
    changes: Vec<Change>,
}

impl Facts<'_> {
    fn push(
        &mut self,
        entity_kind: EntityKind,
        coordinate: &str,
        (kind, native_kind, severity): (ChangeKind, &str, Severity),
        details: Value,
    ) {
        let mut change = Change {
            id: String::new(),
            subject: id(entity_kind, coordinate),
            kind,
            severity,
            impacts: vec!["source".into()],
            native: location(coordinate),
            native_kind: native_kind.into(),
            details,
        };
        change.refresh_id();
        self.changes.push(change);
    }

    fn deprecation(
        &mut self,
        entity_kind: EntityKind,
        c: &str,
        old: &Deprecation,
        new: &Deprecation,
    ) {
        if old == new {
            return;
        }
        let native = match (old.deprecated, new.deprecated) {
            (false, true) => "DEPRECATION_ADDED",
            (true, false) => "DEPRECATION_REMOVED",
            _ => "DEPRECATION_REASON_CHANGED",
        };
        self.push(
            entity_kind,
            c,
            (ChangeKind::DeprecatedChanged, native, Severity::NonBreaking),
            json!({
                "before": with_deprecation(json!({}), old),
                "after": with_deprecation(json!({}), new),
            }),
        );
    }

    /// Arguments (`argument`) and input-object fields share the input rules.
    fn inputs(
        &mut self,
        owner: &str,
        old: &[InputValueDef],
        new: &[InputValueDef],
        argument: bool,
    ) {
        let (entity_kind, label) = if argument {
            (EntityKind::Argument, "ARG")
        } else {
            (EntityKind::Field, "INPUT_FIELD")
        };
        let (removed, added, required, retyped) = if argument {
            (
                ChangeKind::ArgumentRemoved,
                ChangeKind::ArgumentAdded,
                ChangeKind::ArgumentRequiredChanged,
                ChangeKind::ArgumentTypeChanged,
            )
        } else {
            (
                ChangeKind::FieldRemoved,
                ChangeKind::FieldAdded,
                ChangeKind::FieldRequiredChanged,
                ChangeKind::FieldTypeChanged,
            )
        };
        let coordinate = |name: &str| {
            if argument {
                format!("{owner}({name}:)")
            } else {
                format!("{owner}.{name}")
            }
        };
        for o in old {
            let c = coordinate(&o.name);
            let Some(n) = new.iter().find(|n| n.name == o.name) else {
                self.push(
                    entity_kind,
                    &c,
                    (removed, &format!("{label}_REMOVED"), Severity::Breaking),
                    json!({"before": input_attributes(o)}),
                );
                continue;
            };
            if o.ty != n.ty {
                let compat = input_compat(&o.ty, &n.ty, n.default.is_some());
                let same_named_shape = o.ty.strip_non_null() == n.ty.strip_non_null();
                let (kind, native) = if !same_named_shape {
                    (retyped, format!("{label}_TYPE_CHANGED"))
                } else {
                    let kind = if o.ty.is_non_null() != n.ty.is_non_null() {
                        required
                    } else {
                        ChangeKind::NullabilityChanged
                    };
                    let native = match compat {
                        Compat::Breaking => format!("{label}_MADE_NON_NULL"),
                        Compat::Safe if n.ty.is_non_null() && !o.ty.is_non_null() => {
                            format!("{label}_MADE_NON_NULL_WITH_DEFAULT")
                        }
                        Compat::Safe => format!("{label}_MADE_NULLABLE"),
                    };
                    (kind, native)
                };
                self.push(
                    entity_kind,
                    &c,
                    (kind, &native, verdict(compat)),
                    json!({"before": o.ty.to_string(), "after": n.ty.to_string(), "default": n.default}),
                );
            }
            if o.default != n.default {
                self.push(
                    entity_kind,
                    &c,
                    (
                        ChangeKind::DefaultChanged,
                        &format!("{label}_DEFAULT_CHANGED"),
                        Severity::Review,
                    ),
                    json!({"before": o.default, "after": n.default}),
                );
            }
            self.deprecation(entity_kind, &c, &o.deprecation, &n.deprecation);
        }
        for n in new.iter().filter(|n| !old.iter().any(|o| o.name == n.name)) {
            let req = n.required();
            self.push(
                entity_kind,
                &coordinate(&n.name),
                (
                    added,
                    &format!(
                        "{label}_ADDED_{}",
                        if req { "REQUIRED" } else { "OPTIONAL" }
                    ),
                    if req {
                        Severity::Breaking
                    } else {
                        Severity::NonBreaking
                    },
                ),
                json!({"after": input_attributes(n)}),
            );
        }
    }

    /// Object/interface fields; `root` fields are operations.
    fn output_fields(&mut self, owner: &str, old: &[FieldDef], new: &[FieldDef], root: bool) {
        let entity_kind = if root {
            EntityKind::Operation
        } else {
            EntityKind::Field
        };
        let (removed, added) = if root {
            (ChangeKind::OperationRemoved, ChangeKind::OperationAdded)
        } else {
            (ChangeKind::FieldRemoved, ChangeKind::FieldAdded)
        };
        for o in old {
            let c = format!("{owner}.{}", o.name);
            let Some(n) = new.iter().find(|n| n.name == o.name) else {
                self.push(
                    entity_kind,
                    &c,
                    (removed, "FIELD_REMOVED", Severity::Breaking),
                    json!({"before": output_attributes(o)}),
                );
                continue;
            };
            if o.ty != n.ty {
                let compat = output_compat(&o.ty, &n.ty);
                let only_nullability = o.ty.strip_non_null() == n.ty.strip_non_null();
                let (kind, native) = match (only_nullability, compat) {
                    (false, _) => (ChangeKind::FieldTypeChanged, "FIELD_TYPE_CHANGED"),
                    (true, Compat::Breaking) => {
                        (ChangeKind::NullabilityChanged, "FIELD_MADE_NULLABLE")
                    }
                    (true, Compat::Safe) => (ChangeKind::NullabilityChanged, "FIELD_MADE_NON_NULL"),
                };
                self.push(
                    entity_kind,
                    &c,
                    (kind, native, verdict(compat)),
                    json!({"before": o.ty.to_string(), "after": n.ty.to_string()}),
                );
            }
            self.deprecation(entity_kind, &c, &o.deprecation, &n.deprecation);
            self.inputs(&c, &o.args, &n.args, true);
            self.pagination(entity_kind, &c, o, n);
        }
        for n in new.iter().filter(|n| !old.iter().any(|o| o.name == n.name)) {
            self.push(
                entity_kind,
                &format!("{owner}.{}", n.name),
                (added, "FIELD_ADDED", Severity::NonBreaking),
                json!({"after": output_attributes(n)}),
            );
        }
    }

    fn pagination(&mut self, entity_kind: EntityKind, c: &str, o: &FieldDef, n: &FieldDef) {
        let before = pagination_evidence(self.old, o);
        let after = pagination_evidence(self.new, n);
        if before == after {
            return;
        }
        let shape = |v: &Option<Value>| {
            v.as_ref()
                .and_then(|v| v["shape"].as_str())
                .map(str::to_owned)
        };
        let (native, severity) = match (shape(&before).as_deref(), shape(&after).as_deref()) {
            (Some("list"), Some("connection")) => ("RELAY_CONNECTION_ADOPTED", Severity::Breaking),
            (Some("connection"), Some("list")) => ("RELAY_CONNECTION_REMOVED", Severity::Breaking),
            (Some("connection"), Some("connection")) => {
                ("RELAY_CONNECTION_CONTROLS_CHANGED", Severity::Review)
            }
            // List item changes and other evidence transitions are already
            // covered by the underlying type and argument facts.
            _ => return,
        };
        self.push(
            entity_kind,
            c,
            (ChangeKind::PaginationChanged, native, severity),
            json!({"confidence": "declared", "before": before, "after": after}),
        );
    }

    fn named_type(&mut self, o: &TypeDef, n: &TypeDef, outputs: &BTreeSet<&str>) {
        let t = &o.name;
        let set_facts =
            |old: &[String], new: &[String], what: &str, removed: Severity, added: Severity| {
                let mut facts = vec![];
                for m in old.iter().filter(|m| !new.contains(m)) {
                    facts.push((
                        format!("{what}_REMOVED"),
                        removed,
                        json!({what.to_lowercase(): m, "before": true, "after": false}),
                    ));
                }
                for m in new.iter().filter(|m| !old.contains(m)) {
                    facts.push((
                        format!("{what}_ADDED"),
                        added,
                        json!({what.to_lowercase(): m, "before": false, "after": true}),
                    ));
                }
                facts
            };
        let mut set = set_facts(
            &o.interfaces,
            &n.interfaces,
            "INTERFACE",
            Severity::Breaking,
            Severity::NonBreaking,
        );
        // Clients switching exhaustively over a union can meet a new member.
        set.extend(set_facts(
            &o.members,
            &n.members,
            "UNION_MEMBER",
            Severity::Breaking,
            Severity::Review,
        ));
        for (native, severity, details) in set {
            self.push(
                EntityKind::Type,
                t,
                (ChangeKind::ConstraintChanged, &native, severity),
                details,
            );
        }
        for v in &o.values {
            let c = format!("{t}.{}", v.name);
            match n.values.iter().find(|x| x.name == v.name) {
                None => self.push(
                    EntityKind::Value,
                    &c,
                    (
                        ChangeKind::EnumValueRemoved,
                        "ENUM_VALUE_REMOVED",
                        Severity::Breaking,
                    ),
                    json!({}),
                ),
                Some(x) => self.deprecation(EntityKind::Value, &c, &v.deprecation, &x.deprecation),
            }
        }
        for v in n
            .values
            .iter()
            .filter(|v| !o.values.iter().any(|x| x.name == v.name))
        {
            let output = outputs.contains(t.as_str());
            self.push(
                EntityKind::Value,
                &format!("{t}.{}", v.name),
                if output {
                    (
                        ChangeKind::EnumValueAdded,
                        "ENUM_VALUE_ADDED_TO_OUTPUT",
                        Severity::Review,
                    )
                } else {
                    (
                        ChangeKind::EnumValueAdded,
                        "ENUM_VALUE_ADDED",
                        Severity::NonBreaking,
                    )
                },
                json!({"output": output}),
            );
        }
        let old_root = self.old.root_of(t).is_some();
        let new_root = self.new.root_of(t).is_some();
        if old_root == new_root {
            self.output_fields(t, &o.fields, &n.fields, old_root);
        } else {
            // Becoming (or ceasing to be) a root moves fields between field
            // and operation identities: report removal and addition.
            self.output_fields(t, &o.fields, &[], old_root);
            self.output_fields(t, &[], &n.fields, new_root);
        }
        self.inputs(t, &o.input_fields, &n.input_fields, false);
    }
}

/// Named types used by any output field. Enum additions on them are review
/// because clients may switch over values exhaustively.
fn output_types(doc: &Document) -> BTreeSet<&str> {
    doc.types
        .values()
        .flat_map(|t| t.fields.iter().map(|f| f.ty.named()))
        .collect()
}

/// Compare two schemas with the GraphQL compatibility policy. Facts are
/// sorted by subject then ID; description-only changes produce no facts.
pub fn diff(old: &Document, new: &Document) -> Result<Vec<Change>> {
    let mut facts = Facts {
        old,
        new,
        changes: vec![],
    };
    for kind in ROOTS {
        let (o, n) = (old.roots.get(kind), new.roots.get(kind));
        if o == n {
            continue;
        }
        let details = json!({"operation_type": kind, "before": o, "after": n});
        if let Some(o) = o {
            facts.push(
                EntityKind::Service,
                o,
                (
                    ChangeKind::ServiceRemoved,
                    "ROOT_TYPE_REMOVED",
                    Severity::Breaking,
                ),
                details.clone(),
            );
        }
        if let Some(n) = n {
            facts.push(
                EntityKind::Service,
                n,
                (
                    ChangeKind::ServiceAdded,
                    "ROOT_TYPE_ADDED",
                    Severity::NonBreaking,
                ),
                details,
            );
        }
    }
    let outputs = output_types(new);
    for o in old.types.values() {
        match new.types.get(&o.name) {
            None => {
                facts.push(
                    EntityKind::Type,
                    &o.name,
                    (ChangeKind::TypeRemoved, "TYPE_REMOVED", Severity::Breaking),
                    json!({"kind": o.kind}),
                );
                if old.root_of(&o.name).is_some() {
                    facts.output_fields(&o.name, &o.fields, &[], true);
                }
            }
            Some(n) if n.kind != o.kind => facts.push(
                EntityKind::Type,
                &o.name,
                (
                    ChangeKind::TypeKindChanged,
                    "TYPE_KIND_CHANGED",
                    Severity::Breaking,
                ),
                json!({"before": o.kind, "after": n.kind}),
            ),
            Some(n) => facts.named_type(o, n, &outputs),
        }
    }
    for n in new
        .types
        .values()
        .filter(|n| !old.types.contains_key(&n.name))
    {
        facts.push(
            EntityKind::Type,
            &n.name,
            (ChangeKind::TypeAdded, "TYPE_ADDED", Severity::NonBreaking),
            json!({"kind": n.kind}),
        );
        if new.root_of(&n.name).is_some() {
            facts.output_fields(&n.name, &[], &n.fields, true);
        }
    }
    let mut changes = facts.changes;
    changes.sort_by(|a, b| a.subject.cmp(&b.subject).then(a.id.cmp(&b.id)));
    changes.dedup_by(|a, b| a.id == b.id);
    Ok(changes)
}

// ---------------------------------------------------------------------------
// Rendering

/// Operation methods used by the renderer IR for GraphQL root fields.
pub const OPERATION_METHODS: [&str; 3] = ["QUERY", "MUTATION", "SUBSCRIPTION"];

fn schema_for(doc: &Document, ty: &TypeRef) -> crate::Schema {
    use crate::Schema;
    match ty {
        TypeRef::NonNull(t) => schema_for(doc, t),
        TypeRef::List(t) => Schema {
            schema_type: Some("array".into()),
            items: Some(Box::new(schema_for(doc, t))),
            ..Default::default()
        },
        TypeRef::Named(n) if doc.types.contains_key(n) && doc.root_of(n).is_none() => Schema {
            reference: Some(format!("#/components/schemas/{n}")),
            ..Default::default()
        },
        TypeRef::Named(n) => Schema {
            schema_type: Some(n.clone()),
            ..Default::default()
        },
    }
}
fn annotate(description: Option<&str>, note: String) -> Option<String> {
    Some(match description.map(str::trim) {
        Some(d) if !d.is_empty() => format!("{d} ({note})"),
        _ => note,
    })
}
fn deprecated_note(d: &Deprecation) -> String {
    match (&d.deprecated, &d.reason) {
        (true, Some(reason)) => format!("; deprecated: {reason}"),
        (true, None) => "; deprecated".into(),
        _ => String::new(),
    }
}
fn default_note(default: &Option<String>) -> String {
    default
        .as_ref()
        .map(|d| format!("; default `{d}`"))
        .unwrap_or_default()
}
fn object_schema(doc: &Document, t: &TypeDef, schema: &mut crate::Schema) {
    let mut properties = indexmap::IndexMap::new();
    let mut required = vec![];
    for f in &t.fields {
        let mut s = schema_for(doc, &f.ty);
        let args: Vec<_> = f
            .args
            .iter()
            .map(|a| {
                let default = a
                    .default
                    .as_ref()
                    .map(|d| format!(" = {d}"))
                    .unwrap_or_default();
                format!("{}: {}{default}", a.name, a.ty)
            })
            .collect();
        let args = if args.is_empty() {
            String::new()
        } else {
            format!("; arguments `({})`", args.join(", "))
        };
        s.description = annotate(
            f.description.as_deref(),
            format!("`{}`{args}{}", f.ty, deprecated_note(&f.deprecation)),
        );
        if f.ty.is_non_null() {
            required.push(f.name.clone());
        }
        properties.insert(f.name.clone(), s);
    }
    for f in &t.input_fields {
        let mut s = schema_for(doc, &f.ty);
        s.description = annotate(
            f.description.as_deref(),
            format!(
                "`{}`{}{}",
                f.ty,
                default_note(&f.default),
                deprecated_note(&f.deprecation)
            ),
        );
        if f.required() {
            required.push(f.name.clone());
        }
        properties.insert(f.name.clone(), s);
    }
    schema.schema_type = Some("object".into());
    schema.properties = Some(properties);
    schema.required = Some(required);
}

/// Convert to the renderer IR. Root fields become operations whose method is
/// the root keyword (`QUERY`, `MUTATION`, `SUBSCRIPTION`) and whose path is the
/// schema coordinate (`Query.user`); arguments are `argument` parameters and
/// the field type is the `result`. Non-root named types become schemas.
pub fn documentation(doc: &Document) -> crate::ApiDocumentation {
    use crate::{Endpoint, Parameter, Response, Schema, Service};
    use indexmap::IndexMap;
    let mut schemas = IndexMap::new();
    for t in doc
        .types
        .values()
        .filter(|t| doc.root_of(&t.name).is_none())
    {
        let mut schema = Schema {
            title: Some(t.name.clone()),
            ..Default::default()
        };
        let mut note = format!(
            "GraphQL {}",
            t.kind.label().to_lowercase().replace('_', " ")
        );
        if !t.interfaces.is_empty() {
            note.push_str(&format!("; implements {}", t.interfaces.join(" & ")));
        }
        match t.kind {
            TypeKind::Scalar => schema.schema_type = Some("scalar".into()),
            TypeKind::Enum => {
                schema.schema_type = Some("enum".into());
                schema.enum_values = Some(t.values.iter().map(|v| json!(v.name)).collect());
                let deprecated: Vec<_> = t
                    .values
                    .iter()
                    .filter(|v| v.deprecation.deprecated)
                    .map(|v| format!("`{}`{}", v.name, deprecated_note(&v.deprecation)))
                    .collect();
                if !deprecated.is_empty() {
                    note.push_str(&format!("; deprecated values: {}", deprecated.join(", ")));
                }
            }
            TypeKind::Union => {
                note.push_str(&format!("; one of {}", t.members.join(" | ")));
                schema.one_of = Some(
                    t.members
                        .iter()
                        .map(|m| schema_for(doc, &TypeRef::Named(m.clone())))
                        .collect(),
                );
            }
            TypeKind::Object | TypeKind::Interface | TypeKind::InputObject => {
                object_schema(doc, t, &mut schema)
            }
        }
        schema.description = annotate(t.description.as_deref(), note);
        schemas.insert(t.name.clone(), schema);
    }
    let mut services = vec![];
    let mut endpoints = vec![];
    for (kind, method) in ROOTS.iter().zip(OPERATION_METHODS) {
        let Some(t) = doc.roots.get(*kind).and_then(|r| doc.types.get(r)) else {
            continue;
        };
        services.push(Service {
            name: t.name.clone(),
            description: Some(format!("GraphQL {kind} root")),
        });
        for f in &t.fields {
            let coordinate = format!("{}.{}", t.name, f.name);
            let mut responses = IndexMap::new();
            responses.insert(
                "result".into(),
                Response {
                    reference: None,
                    description: Some(format!("`{}`", f.ty)),
                    schema: Some(schema_for(doc, &f.ty)),
                    content: None,
                    extensions: std::collections::HashMap::new(),
                },
            );
            let parameters = f
                .args
                .iter()
                .map(|a| Parameter {
                    reference: None,
                    name: a.name.clone(),
                    description: annotate(
                        a.description.as_deref(),
                        format!(
                            "`{}`{}{}",
                            a.ty,
                            default_note(&a.default),
                            deprecated_note(&a.deprecation)
                        ),
                    ),
                    parameter_in: "argument".into(),
                    required: Some(a.required()),
                    schema: Some(schema_for(doc, &a.ty)),
                    example: None,
                    examples: None,
                    extensions: std::collections::HashMap::new(),
                })
                .collect();
            endpoints.push(Endpoint {
                path: coordinate.clone(),
                method: method.into(),
                services: vec![t.name.clone()],
                summary: Some(
                    f.description
                        .as_deref()
                        .and_then(|d| d.lines().map(str::trim).find(|l| !l.is_empty()))
                        .map_or_else(|| f.name.clone(), str::to_owned),
                ),
                description: Some(format!(
                    "GraphQL {kind} field `{coordinate}` returning `{}`{}",
                    f.ty,
                    deprecated_note(&f.deprecation)
                )),
                operation_id: Some(coordinate),
                parameters,
                responses,
                deprecated: f.deprecation.deprecated,
                untagged: false,
            });
        }
    }
    crate::ApiDocumentation {
        title: "GraphQL schema".into(),
        version: String::new(),
        description: Some(
            "Native GraphQL schema. Operation selectors use the root keyword and schema coordinate (for example `QUERY Query.user`); schemas use GraphQL type names.".into(),
        ),
        services,
        endpoints,
        servers: vec![],
        security_schemes: IndexMap::new(),
        schemas,
        examples: IndexMap::new(),
    }
}

/// Surviving root fields of `doc` affected by `changes`: a fact at or below
/// the root field itself, or on a type reachable from its arguments/result.
pub fn affected_operations(doc: &Document, changes: &[Change]) -> BTreeSet<String> {
    let mut touched = BTreeSet::new();
    let mut operations = BTreeSet::new();
    for change in changes {
        let NativeLocation::Graphql { coordinate } = &change.native else {
            continue;
        };
        let mut parts = coordinate.splitn(2, '.');
        let type_name = parts.next().unwrap_or_default();
        let field = parts.next().map(|f| f.split('(').next().unwrap_or(f));
        match (doc.root_of(type_name), field) {
            (Some(_), Some(field)) => {
                if doc.types[type_name].fields.iter().any(|f| f.name == field) {
                    operations.insert(format!("{type_name}.{field}"));
                }
            }
            // A root service/type fact itself does not focus every field.
            (Some(_), None) => {}
            (None, _) => {
                touched.insert(type_name.to_owned());
            }
        }
    }
    if touched.is_empty() {
        return operations;
    }
    for root in doc.roots.values() {
        for f in &doc.types[root].fields {
            let mut seen = BTreeSet::new();
            let mut stack: Vec<&str> = std::iter::once(f.ty.named())
                .chain(f.args.iter().map(|a| a.ty.named()))
                .collect();
            while let Some(name) = stack.pop() {
                if !seen.insert(name) {
                    continue;
                }
                if touched.contains(name) {
                    operations.insert(format!("{root}.{}", f.name));
                    break;
                }
                if let Some(t) = doc
                    .types
                    .get(name)
                    .filter(|t| doc.root_of(&t.name).is_none())
                {
                    for x in &t.fields {
                        stack.push(x.ty.named());
                        stack.extend(x.args.iter().map(|a| a.ty.named()));
                    }
                    stack.extend(t.input_fields.iter().map(|x| x.ty.named()));
                    stack.extend(t.members.iter().chain(&t.interfaces).map(String::as_str));
                }
            }
        }
    }
    operations
}

/// Focused Markdown for the root fields of `new` affected by `changes`, with
/// their reachable types, through the shared renderer and its `max_tokens`
/// budget (which lowers detail until the context fits). Budget notices, such
/// as an overage at the lowest detail level, go to `notice`.
pub fn write_focused_context(
    writer: &mut impl std::io::Write,
    new: &Document,
    changes: &[Change],
    max_tokens: Option<usize>,
    notice: &mut dyn FnMut(&str),
) -> Result<()> {
    let operations = affected_operations(new, changes);
    if operations.is_empty() {
        writeln!(
            writer,
            "No surviving root fields are affected by these changes."
        )?;
        return Ok(());
    }
    let doc = documentation(new);
    let mut config = crate::DocConfig::unfiltered();
    config.detail_level = crate::DetailLevel::Full;
    config.include_schemas = true;
    config.max_tokens = max_tokens;
    let mut selector = crate::OperationSelector::default();
    for endpoint in doc
        .endpoints
        .iter()
        .filter(|e| operations.contains(&e.path))
    {
        selector.operations.insert(crate::OperationRef {
            method: endpoint.method.clone(),
            path: endpoint.path.clone(),
        });
    }
    config.operation_selector = Some(selector);
    crate::markdown::generate_markdown_with_notices(writer, &doc, &config, notice)
}

/// Human-readable diff grouped by verdict, with native rule names and an
/// optional report table.
pub fn write_diff(
    writer: &mut impl std::io::Write,
    changes: &[Change],
    report: bool,
    old: &Document,
    new: &Document,
) -> Result<()> {
    writeln!(writer, "# GraphQL schema changes\n")?;
    if changes.is_empty() {
        writeln!(writer, "No contract changes.\n")?;
    }
    for (title, severity) in [
        ("Breaking", Severity::Breaking),
        ("Needs review", Severity::Review),
        ("Non-breaking", Severity::NonBreaking),
    ] {
        let coordinate = |c: &Change| match &c.native {
            NativeLocation::Graphql { coordinate } => coordinate.clone(),
            _ => String::new(),
        };
        let mut rows: Vec<_> = changes.iter().filter(|c| c.severity == severity).collect();
        // Readers scan by coordinate; JSON keeps the stable subject/ID order.
        rows.sort_by(|a, b| coordinate(a).cmp(&coordinate(b)).then(a.id.cmp(&b.id)));
        if rows.is_empty() {
            continue;
        }
        writeln!(writer, "## {title} ({})\n", rows.len())?;
        for c in rows {
            if let NativeLocation::Graphql { coordinate } = &c.native {
                writeln!(
                    writer,
                    "- `{coordinate}` — `{}` ({}): `{}`",
                    c.native_kind,
                    serde_json::to_value(c.kind)?.as_str().unwrap_or_default(),
                    crate::diff::json::canonical_json(&c.details)
                )?;
            }
        }
        writeln!(writer)?;
    }
    if report {
        let root_fields = |d: &Document| -> usize {
            d.roots
                .values()
                .filter_map(|r| d.types.get(r))
                .map(|t| t.fields.len())
                .sum()
        };
        writeln!(
            writer,
            "## Report\n\n| Surface | Old | New |\n|---|---:|---:|\n| Entities | {} | {} |\n| Named types | {} | {} |\n| Root fields | {} | {} |\n",
            adapt(old).entities.len(),
            adapt(new).entities.len(),
            old.types.len(),
            new.types.len(),
            root_fields(old),
            root_fields(new),
        )?;
    }
    writer.flush()?;
    Ok(())
}
