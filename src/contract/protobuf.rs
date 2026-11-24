//! Protocol Buffers and gRPC adapter. Both `.proto` source compilation and
//! descriptor decoding run in pure Rust without requiring external `protoc`.
use super::*;
use anyhow::{Context, Result};
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, Kind, ReflectMessage};
use protox::{
    Compiler,
    file::{
        ChainFileResolver, File as ProtoxFile, FileResolver, GoogleFileResolver,
        IncludeFileResolver,
    },
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

fn default_value(value: prost_reflect::Value) -> Value {
    use prost_reflect::Value as V;
    let (kind, value) = match value {
        V::Bool(v) => ("bool", json!(v)),
        V::I32(v) => ("i32", json!(v)),
        V::I64(v) => ("i64", json!(v)),
        V::U32(v) => ("u32", json!(v)),
        V::U64(v) => ("u64", json!(v)),
        V::F32(v) if v.is_nan() => ("f32", json!("NaN")),
        V::F32(v) if v == f32::INFINITY => ("f32", json!("Infinity")),
        V::F32(v) if v == f32::NEG_INFINITY => ("f32", json!("-Infinity")),
        V::F32(v) => ("f32", json!(v)),
        V::F64(v) if v.is_nan() => ("f64", json!("NaN")),
        V::F64(v) if v == f64::INFINITY => ("f64", json!("Infinity")),
        V::F64(v) if v == f64::NEG_INFINITY => ("f64", json!("-Infinity")),
        V::F64(v) => ("f64", json!(v)),
        V::String(v) => ("string", json!(v)),
        V::Bytes(v) => (
            "bytes",
            json!(v.iter().map(|b| format!("{b:02x}")).collect::<String>()),
        ),
        V::EnumNumber(v) => ("enum", json!(v)),
        V::Message(_) => ("message", Value::Null),
        V::List(v) => (
            "list",
            json!(v.into_iter().map(default_value).collect::<Vec<_>>()),
        ),
        V::Map(v) => ("map", json!(v.len())),
    };
    json!({"kind":kind,"value":value})
}

/// Decoded descriptors, retaining original bytes (including unknown options).
#[derive(Debug, Clone)]
pub struct Document {
    /// Exact descriptor set, including imported files and unknown fields.
    pub descriptor_bytes: Vec<u8>,
    /// Reflection pool preserving custom options and descriptor metadata.
    pub pool: DescriptorPool,
}
impl Document {
    /// Decode a complete FileDescriptorSet. Imports must be included.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let metadata = prost_types::FileDescriptorSet::decode(bytes)
            .context("Invalid protobuf descriptor encoding")?;
        for file in &metadata.file {
            anyhow::ensure!(
                matches!(
                    file.syntax.as_deref(),
                    None | Some("proto2") | Some("proto3")
                ),
                "Unsupported protobuf syntax {:?} in {:?}: the descriptor adapter supports proto2/proto3; Editions are not yet supported",
                file.syntax(),
                file.name()
            );
        }
        let pool = DescriptorPool::decode(bytes).context("Invalid or incomplete protobuf FileDescriptorSet (include imports when building descriptors)")?;
        anyhow::ensure!(
            pool.files().next().is_some(),
            "Empty protobuf descriptor set"
        );
        Ok(Self {
            descriptor_bytes: bytes.to_vec(),
            pool,
        })
    }
    /// Read `.proto` source or a binary FileDescriptorSet (`.pb`, `.bin`, `.protoset`, `.desc`).
    pub fn read(path: &Path, includes: &[PathBuf]) -> Result<Self> {
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("proto"))
        {
            compile(path, includes)
        } else {
            Self::decode(
                &std::fs::read(path)
                    .with_context(|| format!("Failed to read protobuf descriptors: {path:?}"))?,
            )
        }
    }
}
/// Recognized protobuf source and descriptor filename extensions.
pub fn is_input(path: &Path) -> bool {
    path.extension().and_then(|x| x.to_str()).is_some_and(|e| {
        [
            "proto",
            "pb",
            "bin",
            "binpb",
            "fds",
            "protoset",
            "desc",
            "descriptor",
        ]
        .iter()
        .any(|v| e.eq_ignore_ascii_case(v))
    })
}
struct ResolvedLocalEntry {
    virtual_name: String,
    search_roots: Vec<PathBuf>,
}

fn lexical_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() {
                    out.push(comp.as_os_str());
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

fn lexical_absolute(path: &Path) -> Result<PathBuf> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to determine current working directory")?
            .join(path)
    };
    Ok(lexical_normalize(&joined))
}

fn raw_parent_dir(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    if parent.as_os_str().is_empty() {
        Some(PathBuf::from("."))
    } else {
        Some(parent.to_path_buf())
    }
}

/// Resolve a local `.proto` entry path against user `-I` include roots without
/// breaking symlinked files or symlinked subdirectories under `-I`, and verify
/// canonical shadowing semantics before compilation.
fn resolve_local_entry(path: &Path, includes: &[PathBuf]) -> Result<ResolvedLocalEntry> {
    let canonical_path = path
        .canonicalize()
        .with_context(|| format!("Missing protobuf source: {path:?}"))?;
    anyhow::ensure!(
        !canonical_path.is_dir(),
        "Protobuf source is a directory, expected a file: {path:?}"
    );
    let abs_path = lexical_absolute(path)?;

    let mut roots: Vec<(PathBuf, PathBuf)> = Vec::with_capacity(includes.len());
    for root in includes {
        let canonical = root
            .canonicalize()
            .with_context(|| format!("Invalid protobuf import root: {root:?}"))?;
        anyhow::ensure!(
            canonical.is_dir(),
            "Invalid protobuf import root (not a directory): {root:?}"
        );
        let abs = lexical_absolute(root)?;
        roots.push((abs, canonical));
    }

    let verifies = |canonical_root: &Path, rel: &Path| -> bool {
        !rel.as_os_str().is_empty()
            && proto_virtual_name(rel).is_some()
            && canonical_root.join(rel).canonicalize().ok().as_deref()
                == Some(canonical_path.as_path())
    };

    let entry_parent = raw_parent_dir(path)
        .and_then(|p| p.canonicalize().ok())
        .or_else(|| abs_path.parent().and_then(|p| p.canonicalize().ok()));
    let entry_file_name = path
        .file_name()
        .or_else(|| abs_path.file_name())
        .or_else(|| canonical_path.file_name());
    let parent_canonical_path = entry_parent
        .as_ref()
        .zip(entry_file_name)
        .map(|(dir, name)| dir.join(name));

    // Pass 1: path-preserving match across user `-I` roots (in order), keeping
    // symlinked leaf filenames and symlinked subdirectory prefixes intact.
    let mut virtual_path: Option<PathBuf> = None;
    for (abs_root, canonical_root) in &roots {
        if let Ok(rel) = abs_path.strip_prefix(abs_root)
            && verifies(canonical_root, rel)
        {
            virtual_path = Some(rel.to_path_buf());
            break;
        }
        for anc in abs_path.ancestors().skip(1) {
            if anc.canonicalize().ok().as_deref() == Some(canonical_root.as_path())
                && let Ok(rel) = abs_path.strip_prefix(anc)
                && verifies(canonical_root, rel)
            {
                virtual_path = Some(rel.to_path_buf());
                break;
            }
        }
        if virtual_path.is_some() {
            break;
        }
    }

    // Pass 2: parent-canonical match (handles `..` through a symlink while
    // keeping the leaf symlink unfollowed).
    if virtual_path.is_none()
        && let Some(ref pcp) = parent_canonical_path
    {
        for (_, canonical_root) in &roots {
            if let Ok(rel) = pcp.strip_prefix(canonical_root)
                && verifies(canonical_root, rel)
            {
                virtual_path = Some(rel.to_path_buf());
                break;
            }
        }
    }

    // Pass 3: full-canonical match (e.g. symlink outside `-I` pointing inside `-I`).
    if virtual_path.is_none() {
        for (_, canonical_root) in &roots {
            if let Ok(rel) = canonical_path.strip_prefix(canonical_root)
                && verifies(canonical_root, rel)
            {
                virtual_path = Some(rel.to_path_buf());
                break;
            }
        }
    }

    let virtual_path = match virtual_path {
        Some(rel) => rel,
        None => {
            let name = entry_file_name.context("Missing protobuf input filename")?;
            PathBuf::from(name)
        }
    };
    let virtual_name = proto_virtual_name(&virtual_path)
        .with_context(|| format!("Invalid protobuf virtual path: {virtual_path:?}"))?;

    // User roots precede the entry directory and canonical target directory,
    // matching protoc import semantics while allowing sibling imports next to
    // either the symlink or its target.
    let mut search_roots: Vec<PathBuf> = Vec::with_capacity(roots.len() + 2);
    for (_, canonical) in roots {
        if !search_roots.iter().any(|r| r == &canonical) {
            search_roots.push(canonical);
        }
    }
    let canonical_target_parent = canonical_path
        .parent()
        .context("Missing protobuf input directory")?
        .to_path_buf();
    for fallback in [entry_parent, Some(canonical_target_parent)]
        .into_iter()
        .flatten()
    {
        if !search_roots.iter().any(|r| r == &fallback) {
            search_roots.push(fallback);
        }
    }

    // Canonicalized shadow check across `search_roots`: the first root
    // containing `virtual_path` must resolve to `canonical_path`.
    let mut found_target = false;
    for root in &search_roots {
        let candidate = root.join(&virtual_path);
        match candidate.canonicalize() {
            Ok(resolved) => {
                if resolved != canonical_path {
                    anyhow::bail!(
                        "Protobuf source {:?} ({virtual_name}) is shadowed by {:?} in import root {:?}",
                        path,
                        resolved,
                        root
                    );
                }
                found_target = true;
                break;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                anyhow::bail!(
                    "Protobuf source {:?} ({virtual_name}) is shadowed by unreadable path {:?} in import root {:?}: {err}",
                    path,
                    candidate,
                    root
                );
            }
        }
    }
    anyhow::ensure!(
        found_target,
        "Protobuf source {:?} ({virtual_name}) could not be resolved in any import root",
        path
    );

    Ok(ResolvedLocalEntry {
        virtual_name,
        search_roots,
    })
}

/// Compile `.proto` source in pure Rust using `protox`; no external `protoc` or
/// code generation is required.
pub fn compile(path: &Path, includes: &[PathBuf]) -> Result<Document> {
    let entry = resolve_local_entry(path, includes)?;
    let mut resolver = ChainFileResolver::new();
    for root in entry.search_roots {
        resolver.add(EditionCheckingIncludeResolver::new(root));
    }
    resolver.add(GoogleFileResolver::new());
    compile_with_resolver(resolver, Path::new(&entry.virtual_name))
}

#[cfg(feature = "cli")]
pub(crate) fn compile_bytes(path: &Path, bytes: &[u8], includes: &[PathBuf]) -> Result<Document> {
    let entry = resolve_local_entry(path, includes)?;
    let source = std::str::from_utf8(bytes)
        .with_context(|| format!("Protobuf source is not valid UTF-8: {path:?}"))?
        .to_owned();

    let mut resolver = ChainFileResolver::new();
    resolver.add(MemoryFileResolver {
        name: entry.virtual_name.clone(),
        source,
    });
    for root in entry.search_roots {
        resolver.add(EditionCheckingIncludeResolver::new(root));
    }
    resolver.add(GoogleFileResolver::new());
    compile_with_resolver(resolver, Path::new(&entry.virtual_name))
}

pub(crate) fn compile_with_resolver<R>(resolver: R, entry: &Path) -> Result<Document>
where
    R: FileResolver + 'static,
{
    let mut compiler = Compiler::with_file_resolver(resolver);
    compiler
        .include_imports(true)
        .include_source_info(true)
        .open_file(entry)
        .with_context(|| format!("Failed to compile protobuf source: {}", entry.display()))?;
    Document::decode(&compiler.encode_file_descriptor_set())
}

#[cfg(feature = "cli")]
pub(crate) fn parse_proto_source(name: &str, source: &str) -> Result<ProtoxFile, protox::Error> {
    if has_edition_header(source) {
        return Err(unsupported_edition_error(name));
    }
    ProtoxFile::from_source(name, source)
}

pub(crate) fn proto_virtual_name(path: &Path) -> Option<String> {
    let mut name = String::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => {
                let part = part.to_str()?;
                if !name.is_empty() {
                    name.push('/');
                }
                name.push_str(part);
            }
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    (!name.is_empty()).then_some(name)
}

fn unsupported_edition_error(name: &str) -> protox::Error {
    protox::Error::new(format!(
        "Unsupported protobuf syntax \"editions\" in {name:?}: the descriptor adapter supports proto2/proto3; Editions are not yet supported"
    ))
}

/// Detect a leading Protobuf Editions header (`edition = "...";`) after
/// skipping BOM, whitespace, and `//` or `/* ... */` comments.
fn has_edition_header(source: &str) -> bool {
    let rest = skip_proto_trivia(source.strip_prefix('\u{feff}').unwrap_or(source));
    let Some(after_keyword) = rest.strip_prefix("edition") else {
        return false;
    };
    if after_keyword
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return false;
    }
    skip_proto_trivia(after_keyword).starts_with('=')
}

fn skip_proto_trivia(mut input: &str) -> &str {
    loop {
        input = input.trim_start();
        if let Some(after) = input.strip_prefix("//") {
            input = match after.find('\n') {
                Some(idx) => &after[idx + 1..],
                None => "",
            };
        } else if let Some(after) = input.strip_prefix("/*") {
            input = match after.find("*/") {
                Some(idx) => &after[idx + 2..],
                None => "",
            };
        } else {
            return input;
        }
    }
}

struct EditionCheckingIncludeResolver {
    include: PathBuf,
    inner: IncludeFileResolver,
}

impl EditionCheckingIncludeResolver {
    fn new(include: PathBuf) -> Self {
        Self {
            inner: IncludeFileResolver::new(include.clone()),
            include,
        }
    }
}

impl FileResolver for EditionCheckingIncludeResolver {
    fn resolve_path(&self, path: &Path) -> Option<String> {
        self.inner.resolve_path(path)
    }

    fn open_file(&self, name: &str) -> Result<ProtoxFile, protox::Error> {
        match self.inner.open_file(name) {
            Ok(file) => Ok(file),
            Err(err) if err.is_file_not_found() => Err(err),
            Err(err) => {
                if let Ok(source) = std::fs::read_to_string(self.include.join(name))
                    && has_edition_header(&source)
                {
                    return Err(unsupported_edition_error(name));
                }
                Err(err)
            }
        }
    }
}

#[cfg(feature = "cli")]
struct MemoryFileResolver {
    name: String,
    source: String,
}

#[cfg(feature = "cli")]
impl FileResolver for MemoryFileResolver {
    fn open_file(&self, name: &str) -> Result<ProtoxFile, protox::Error> {
        if name == self.name {
            parse_proto_source(name, &self.source)
        } else {
            Err(protox::Error::file_not_found(name))
        }
    }
}
fn id(kind: EntityKind, name: &str) -> String {
    entity_id(Format::Protobuf, kind, name)
}
fn options(mut message: DynamicMessage) -> Value {
    strip_source_retention(&mut message);
    serde_json::to_value(message).unwrap_or(Value::Null)
}
/// `retention = RETENTION_SOURCE` read reflectively: prost-types predates the
/// field, and a pool whose descriptor.proto lacks it has no source retention.
fn is_source_retained(field_options: &DynamicMessage) -> bool {
    const RETENTION_SOURCE: i32 = 2;
    field_options
        .descriptor()
        .get_field_by_name("retention")
        .is_some_and(|f| field_options.get_field(&f).as_enum_number() == Some(RETENTION_SOURCE))
}
/// Drop applied options declared with source retention, recursively. protoc
/// strips them from descriptor output while protox keeps them, so comparing
/// only what survives into descriptors keeps `.pb`/`.proto` parity (#155).
fn strip_source_retention(message: &mut DynamicMessage) {
    let extensions: Vec<_> = message
        .extensions()
        .map(|(x, _)| x)
        .filter(|x| is_source_retained(&x.options()))
        .collect();
    for x in &extensions {
        message.clear_extension(x);
    }
    let fields: Vec<_> = message
        .fields()
        .map(|(f, _)| f)
        .filter(|f| is_source_retained(&f.options()))
        .collect();
    for f in &fields {
        message.clear_field(f);
    }
    let nested = |value: &mut prost_reflect::Value| match value {
        prost_reflect::Value::Message(m) => strip_source_retention(m),
        prost_reflect::Value::List(items) => items
            .iter_mut()
            .filter_map(|v| v.as_message_mut())
            .for_each(strip_source_retention),
        prost_reflect::Value::Map(entries) => entries
            .values_mut()
            .filter_map(|v| v.as_message_mut())
            .for_each(strip_source_retention),
        _ => {}
    };
    message.fields_mut().for_each(|(_, v)| nested(v));
    message.extensions_mut().for_each(|(_, v)| nested(v));
}
fn node(
    kind: EntityKind,
    name: &str,
    parent: Option<String>,
    number: Option<i32>,
    attrs: Value,
) -> Entity {
    let coordinate = match number {
        Some(n) => format!(
            "{}#{n}{}",
            parent.as_deref().unwrap_or(name),
            if kind == EntityKind::Value {
                format!(":{name}")
            } else {
                String::new()
            }
        ),
        None => name.to_owned(),
    };
    Entity {
        id: id(kind, &coordinate),
        kind,
        name: name.into(),
        parent,
        services: vec![],
        native: NativeLocation::Protobuf {
            full_name: name.into(),
            number,
        },
        attributes: attrs,
        references: vec![],
    }
}
// Number identity uses the native parent FQN, never its hashed entity ID.
fn numbered(kind: EntityKind, name: &str, parent_name: &str, number: i32, attrs: Value) -> Entity {
    let mut e = node(
        kind,
        name,
        Some(id(EntityKind::Type, parent_name)),
        Some(number),
        attrs,
    );
    e.id = id(
        kind,
        &format!(
            "{parent_name}#{number}{}",
            if kind == EntityKind::Value {
                format!(":{name}")
            } else {
                String::new()
            }
        ),
    );
    e
}
fn type_name(kind: Kind) -> String {
    match kind {
        Kind::Message(m) => m.full_name().into(),
        Kind::Enum(e) => e.full_name().into(),
        k => format!("{k:?}").to_lowercase(),
    }
}
/// Adapt all files, including imported and nested symbols, into a sorted neutral surface.
pub fn adapt(doc: &Document) -> Contract {
    let mut entities = vec![];
    let mut packages = std::collections::BTreeSet::new();
    for file in doc.pool.files() {
        packages.insert(file.package_name().to_owned());
        entities.push(file_entity(file));
    }
    entities.extend(packages.into_iter().map(|name| package_entity(&name)));
    for m in doc.pool.all_messages() {
        let d = m.descriptor_proto();
        let parent = m
            .parent_message()
            .map(|p| id(EntityKind::Type, p.full_name()));
        let entity = node(
            EntityKind::Type,
            m.full_name(),
            parent,
            None,
            json!({"kind":"message","package":m.parent_file().package_name(),"syntax":m.parent_file().file_descriptor_proto().syntax(),"options":options(m.options()),"deprecated":d.options.as_ref().is_some_and(|o|o.deprecated()),"reserved_numbers":d.reserved_range.iter().map(|r|json!([r.start(),r.end()])).collect::<Vec<_>>(),"reserved_names":d.reserved_name}),
        );
        entities.push(entity);
        for one in m.oneofs().filter(|o| !o.is_synthetic()) {
            entities.push(node(
                EntityKind::Type,
                one.full_name(),
                Some(id(EntityKind::Type, m.full_name())),
                None,
                json!({"kind":"oneof","options":options(one.options())}),
            ));
        }
        for f in m.fields() {
            let raw = f.field_descriptor_proto();
            let mut e = numbered(
                EntityKind::Field,
                f.full_name(),
                m.full_name(),
                f.number() as i32,
                json!({"type":type_name(f.kind()),"cardinality":format!("{:?}",f.cardinality()).to_lowercase(),"json_name":f.json_name(),"oneof":f.containing_oneof().filter(|o|!o.is_synthetic()).map(|o|o.full_name().to_owned()),"presence":f.supports_presence(),"default":default_value(f.default_value()),"group":f.is_group(),"deprecated":raw.options.as_ref().is_some_and(|o|o.deprecated()),"options":options(f.options())}),
            );
            match f.kind() {
                Kind::Message(t) => e.references.push(id(EntityKind::Type, t.full_name())),
                Kind::Enum(t) => e.references.push(id(EntityKind::Type, t.full_name())),
                _ => {}
            };
            entities.push(e);
        }
    }
    for extension in doc.pool.all_extensions() {
        let raw = extension.field_descriptor_proto();
        let mut e = numbered(
            EntityKind::Field,
            extension.full_name(),
            extension.containing_message().full_name(),
            extension.number() as i32,
            json!({
                "extension":true,"type":type_name(extension.kind()),"cardinality":format!("{:?}",extension.cardinality()).to_lowercase(),"json_name":extension.json_name(),"oneof":null,"presence":extension.supports_presence(),"default":default_value(extension.default_value()),"group":extension.is_group(),"deprecated":raw.options.as_ref().is_some_and(|o|o.deprecated()),"options":options(extension.options())
            }),
        );
        match extension.kind() {
            Kind::Message(t) => e.references.push(id(EntityKind::Type, t.full_name())),
            Kind::Enum(t) => e.references.push(id(EntityKind::Type, t.full_name())),
            _ => {}
        }
        entities.push(e);
    }
    for en in doc.pool.all_enums() {
        let d = en.enum_descriptor_proto();
        let entity = node(
            EntityKind::Type,
            en.full_name(),
            en.parent_message()
                .map(|p| id(EntityKind::Type, p.full_name())),
            None,
            json!({"kind":"enum","package":en.parent_file().package_name(),"syntax":en.parent_file().file_descriptor_proto().syntax(),"options":options(en.options()),"deprecated":d.options.as_ref().is_some_and(|o|o.deprecated()),"reserved_numbers":d.reserved_range.iter().map(|r|json!([r.start(),i64::from(r.end())+1])).collect::<Vec<_>>(),"reserved_names":d.reserved_name}),
        );
        entities.push(entity);
        for v in en.values() {
            entities.push(numbered(EntityKind::Value,v.full_name(),en.full_name(),v.number(),json!({"deprecated":v.enum_value_descriptor_proto().options.as_ref().is_some_and(|o|o.deprecated()),"options":options(v.options())})));
        }
    }
    for service in doc.pool.services() {
        let sid = id(EntityKind::Service, service.full_name());
        let entity = node(
            EntityKind::Service,
            service.full_name(),
            None,
            None,
            json!({"package":service.package_name(),"syntax":service.parent_file_descriptor_proto().syntax(),"options":options(service.options()),"deprecated":service.service_descriptor_proto().options.as_ref().is_some_and(|o|o.deprecated())}),
        );
        entities.push(entity);
        for rpc in service.methods() {
            let mut e = node(
                EntityKind::Operation,
                rpc.full_name(),
                Some(sid.clone()),
                None,
                json!({"input":rpc.input().full_name(),"output":rpc.output().full_name(),"client_streaming":rpc.is_client_streaming(),"server_streaming":rpc.is_server_streaming(),"deprecated":rpc.method_descriptor_proto().options.as_ref().is_some_and(|o|o.deprecated()),"options":options(rpc.options())}),
            );
            e.services.push(sid.clone());
            e.references = vec![
                id(EntityKind::Type, rpc.input().full_name()),
                id(EntityKind::Type, rpc.output().full_name()),
            ];
            entities.push(e);
        }
    }
    entities.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Contract {
        format: Format::Protobuf,
        title: "Protocol Buffers / gRPC".into(),
        version: String::new(),
        entities,
    }
}
/// Protobuf-native classification. Conservative type changes are wire-breaking;
/// field renames retain number identity and break source/JSON compatibility only.
pub struct ProtobufAdapter;
impl Adapter for ProtobufAdapter {
    fn normalize_attributes(&self, e: &Entity) -> Value {
        let mut v = e.attributes.clone();
        if let Some(o) = v.get_mut("options").and_then(Value::as_object_mut) {
            o.remove("deprecated");
        }
        if let Some(o) = v.as_object_mut() {
            o.remove("reserved_numbers");
            o.remove("reserved_names");
        }
        v
    }
    fn structural_kind(&self, _: &Entity, f: &str) -> Option<ChangeKind> {
        match f {
            "name" => Some(ChangeKind::WireIdentityChanged),
            _ => None,
        }
    }
    fn attribute_kind(&self, _: &Entity, p: &str, _: &crate::diff::ValueChangeKind) -> ChangeKind {
        match p {
            "/deprecated" => ChangeKind::DeprecatedChanged,
            "/type" => ChangeKind::FieldTypeChanged,
            p if p.starts_with("/default") => ChangeKind::DefaultChanged,
            "/json_name" | "/oneof" | "/group" | "/client_streaming" | "/server_streaming" => {
                ChangeKind::WireIdentityChanged
            }
            _ => ChangeKind::ConstraintChanged,
        }
    }
    fn classify(&self, e: &Entity, k: ChangeKind, d: &Value) -> (Severity, Vec<String>) {
        let p = d["pointer"].as_str().unwrap_or("");
        let (s, imp) = match k {
            ChangeKind::FieldAdded if e.attributes["cardinality"] == "required" => {
                (Severity::Breaking, vec!["wire", "source"])
            }
            ChangeKind::ServiceAdded
            | ChangeKind::OperationAdded
            | ChangeKind::TypeAdded
            | ChangeKind::FieldAdded
            | ChangeKind::EnumValueAdded => (Severity::NonBreaking, vec![]),
            ChangeKind::DeprecatedChanged | ChangeKind::DefaultChanged => {
                (Severity::Review, vec!["source"])
            }
            ChangeKind::FieldTypeChanged => {
                (Severity::Breaking, vec!["wire", "source", "wire_json"])
            }
            ChangeKind::FieldRemoved | ChangeKind::EnumValueRemoved => {
                (Severity::Breaking, vec!["source"])
            }
            ChangeKind::ServiceRemoved | ChangeKind::OperationRemoved | ChangeKind::TypeRemoved => {
                (Severity::Breaking, vec!["source"])
            }
            ChangeKind::WireIdentityChanged if p == "/oneof" => {
                (Severity::Breaking, vec!["wire", "source"])
            }
            ChangeKind::WireIdentityChanged if p == "/group" => {
                (Severity::Breaking, vec!["wire", "source"])
            }
            // Unary and streaming calls use different gRPC message framing, so
            // existing peers fail at runtime (buf `WIRE` category agrees).
            ChangeKind::WireIdentityChanged
                if p == "/client_streaming" || p == "/server_streaming" =>
            {
                (Severity::Breaking, vec!["wire", "source", "wire_json"])
            }
            ChangeKind::WireIdentityChanged => (Severity::Breaking, vec!["source", "wire_json"]),
            _ if ["/cardinality", "/presence", "/input", "/output", "/syntax"].contains(&p) => {
                (Severity::Breaking, vec!["wire", "source"])
            }
            _ => (Severity::Review, vec![]),
        };
        (s, imp.into_iter().map(str::to_owned).collect())
    }
}
fn rule(c: &Change) -> &str {
    match (c.kind, c.details["pointer"].as_str().unwrap_or("")) {
        (ChangeKind::FieldRemoved, _) => "FIELD_NO_DELETE",
        (ChangeKind::EnumValueRemoved, _) => "ENUM_VALUE_NO_DELETE",
        (ChangeKind::TypeRemoved, _) => "TYPE_NO_DELETE",
        (ChangeKind::ServiceRemoved, _) => "SERVICE_NO_DELETE",
        (ChangeKind::OperationRemoved, _) => "RPC_NO_DELETE",
        (ChangeKind::FieldTypeChanged, _) => "FIELD_SAME_TYPE",
        (ChangeKind::WireIdentityChanged, "/json_name") => "FIELD_SAME_JSON_NAME",
        (ChangeKind::WireIdentityChanged, "/oneof") => "FIELD_SAME_ONEOF",
        (ChangeKind::WireIdentityChanged, "/group") => "FIELD_SAME_GROUP_ENCODING",
        (ChangeKind::WireIdentityChanged, "/client_streaming") => "RPC_SAME_CLIENT_STREAMING",
        (ChangeKind::WireIdentityChanged, "/server_streaming") => "RPC_SAME_SERVER_STREAMING",
        (ChangeKind::WireIdentityChanged, _) => "FIELD_SAME_NAME",
        (_, "/cardinality") => "FIELD_SAME_CARDINALITY",
        (_, "/input") => "RPC_SAME_REQUEST_TYPE",
        (_, "/output") => "RPC_SAME_RESPONSE_TYPE",
        (ChangeKind::DeprecatedChanged, _) => "DEPRECATED_CHANGED",
        _ => "PROTOBUF_SURFACE_CHANGED",
    }
}
fn fact(
    e: &Entity,
    kind: ChangeKind,
    native_kind: &str,
    details: Value,
    impacts: &[&str],
    severity: Severity,
) -> Change {
    let mut c = Change {
        id: String::new(),
        subject: e.id.clone(),
        kind,
        severity,
        impacts: impacts.iter().map(|s| (*s).into()).collect(),
        native: e.native.clone(),
        native_kind: native_kind.into(),
        details,
    };
    c.refresh_id();
    c
}
/// Compare native surfaces with adapter-owned source and wire verdicts.
pub fn diff(old: &Document, new: &Document) -> Result<Vec<Change>> {
    let old_doc = old;
    let new_doc = new;
    let old = adapt(old);
    let new = adapt(new);
    let mut changes = compare(&old, &new, &ProtobufAdapter)?;
    let old_ids: std::collections::HashSet<_> =
        old.entities.iter().map(|e| e.id.as_str()).collect();
    for change in &mut changes {
        if change.kind == ChangeKind::FieldAdded
            && let Some(parent) = new
                .entities
                .iter()
                .find(|e| e.id == change.subject)
                .and_then(|e| e.parent.as_deref())
            && !old_ids.contains(parent)
        {
            // A required field in a newly introduced message has no prior
            // readers to break. Keep it additive just like the message itself.
            change.severity = Severity::NonBreaking;
            change.impacts.clear();
        }
    }
    changes.retain(|c| {
        if c.kind == ChangeKind::TypeRemoved
            && old
                .entities
                .iter()
                .any(|e| e.id == c.subject && e.attributes["kind"] == "file")
        {
            return false;
        }
        if c.kind == ChangeKind::TypeAdded
            && new
                .entities
                .iter()
                .any(|e| e.id == c.subject && e.attributes["kind"] == "file")
        {
            return false;
        }
        true
    });
    for c in &mut changes {
        c.native_kind = if c.details["pointer"]
            .as_str()
            .is_some_and(|p| p.starts_with("/file_options/"))
        {
            "FILE_OPTIONS_CHANGED"
        } else {
            rule(c)
        }
        .into();
        if c.kind == ChangeKind::TypeRemoved
            && let Some(e) = old.entities.iter().find(|e| e.id == c.subject)
        {
            c.native_kind = match e.attributes["kind"].as_str() {
                Some("message") => "MESSAGE_NO_DELETE",
                Some("enum") => "ENUM_NO_DELETE",
                Some("oneof") => "ONEOF_NO_DELETE",
                _ => "TYPE_NO_DELETE",
            }
            .into();
        }
    }
    let old_file_names: std::collections::BTreeSet<_> =
        old_doc.pool.files().map(|f| f.name().to_owned()).collect();
    let new_file_names: std::collections::BTreeSet<_> =
        new_doc.pool.files().map(|f| f.name().to_owned()).collect();
    let old_file_entities: HashMap<_, _> = old
        .entities
        .iter()
        .filter(|e| e.attributes["kind"] == "file")
        .map(|e| (e.name.as_str(), e))
        .collect();
    let new_file_entities: HashMap<_, _> = new
        .entities
        .iter()
        .filter(|e| e.attributes["kind"] == "file")
        .map(|e| (e.name.as_str(), e))
        .collect();
    let mut renamed_pairs = vec![];
    for old_name in old_file_names.difference(&new_file_names) {
        let old_symbols = file_symbols(old_doc, old_name);
        let old_candidates = new_file_names
            .difference(&old_file_names)
            .filter(|new_name| !old_symbols.is_disjoint(&file_symbols(new_doc, new_name)))
            .collect::<Vec<_>>();
        if old_candidates.len() != 1 {
            continue;
        }
        let new_name = old_candidates[0];
        let new_symbols = file_symbols(new_doc, new_name);
        let reverse_candidates = old_file_names
            .difference(&new_file_names)
            .filter(|candidate| !new_symbols.is_disjoint(&file_symbols(old_doc, candidate)))
            .count();
        if reverse_candidates == 1 {
            renamed_pairs.push((old_name, new_name));
        }
    }
    for (old_name, new_name) in renamed_pairs {
        let old_entity = old_file_entities[old_name.as_str()];
        let new_entity = new_file_entities[new_name.as_str()];
        let mut values = vec![];
        crate::diff::diff_values(
            &old_entity.attributes["file_options"],
            &new_entity.attributes["file_options"],
            &mut String::new(),
            &mut values,
        );
        for value in values {
            use crate::diff::ValueChangeKind as V;
            let (before, after) = match &value.kind {
                V::Added(v) => (json!({"present":false}), json!({"present":true,"value":v})),
                V::Removed(v) => (json!({"present":true,"value":v}), json!({"present":false})),
                V::Changed { old, new } => (
                    json!({"present":true,"value":old}),
                    json!({"present":true,"value":new}),
                ),
            };
            changes.push(fact(
                old_entity,
                ChangeKind::ConstraintChanged,
                "FILE_OPTIONS_CHANGED",
                json!({"pointer":format!("/file_options{}",value.pointer),"before":before,"after":after}),
                &[],
                Severity::Review,
            ));
        }
    }
    let new_by_id: HashMap<_, _> = new.entities.iter().map(|e| (&e.id, e)).collect();
    for e in &old.entities {
        if matches!(e.kind, EntityKind::Field | EntityKind::Value) && !new_by_id.contains_key(&e.id)
        {
            let NativeLocation::Protobuf {
                number: Some(number),
                ..
            } = e.native
            else {
                continue;
            };
            if let Some(parent) = e.parent.as_ref().and_then(|p| new_by_id.get(p)) {
                let reserved = parent.attributes["reserved_numbers"]
                    .as_array()
                    .is_some_and(|ranges| {
                        ranges.iter().any(|r| {
                            r[0].as_i64().is_some_and(|lo| i64::from(number) >= lo)
                                && r[1].as_i64().is_some_and(|hi| i64::from(number) < hi)
                        })
                    });
                let reused = new.entities.iter().any(|n|n.parent==e.parent && n.kind==e.kind && matches!(n.native,NativeLocation::Protobuf{number:Some(n),..}if n==number));
                if !reserved && !reused {
                    changes.push(fact(
                        e,
                        ChangeKind::WireIdentityChanged,
                        if e.kind == EntityKind::Field {
                            "FIELD_NO_DELETE_UNLESS_NUMBER_RESERVED"
                        } else {
                            "ENUM_VALUE_NO_DELETE_UNLESS_NUMBER_RESERVED"
                        },
                        json!({"number":number,"reserved":false}),
                        &["wire"],
                        Severity::Breaking,
                    ));
                }
                if e.kind == EntityKind::Field
                    && e.attributes["cardinality"] == "required"
                    && !reused
                {
                    changes.push(fact(
                        e,
                        ChangeKind::WireIdentityChanged,
                        "FIELD_REQUIRED_NO_DELETE",
                        json!({"number":number,"required":true,"reserved":reserved}),
                        &["wire", "source"],
                        Severity::Breaking,
                    ));
                }
                if e.kind == EntityKind::Value && new.entities.iter().any(|n| n.parent==e.parent && n.kind==e.kind && matches!(n.native,NativeLocation::Protobuf{number:Some(n),..}if n==number) && old.entities.iter().any(|previous|previous.name==n.name && previous.native!=n.native)) {
                    changes.push(fact(e,ChangeKind::WireIdentityChanged,"ENUM_VALUE_NUMBER_REUSED",json!({"number":number,"reused":true}),&["wire"],Severity::Breaking));
                }
            }
        }
        if e.kind == EntityKind::Operation
            && let Some(next) = new_by_id.get(&e.id)
        {
            let shape = |contract: &Contract, rpc: &Entity| {
                let fields = |key: &str| {
                    contract
                        .entities
                        .iter()
                        .filter(|f| {
                            f.kind == EntityKind::Field
                                && f.parent
                                    == Some(id(
                                        EntityKind::Type,
                                        rpc.attributes[key].as_str().unwrap_or(""),
                                    ))
                        })
                        .map(|f| f.name.rsplit('.').next().unwrap_or(""))
                        .collect::<Vec<_>>()
                };
                fields("input").contains(&"page_size")
                    && fields("input").contains(&"page_token")
                    && fields("output").contains(&"next_page_token")
            };
            if shape(&old, e) != shape(&new, next) {
                changes.push(fact(
                    e,
                    ChangeKind::PaginationChanged,
                    "RPC_PAGINATION_CHANGED",
                    json!({"before":shape(&old,e),"after":shape(&new,next),"inferred":true}),
                    &["source"],
                    Severity::Review,
                ));
            }
        }
    }
    let old_by_id: HashMap<_, _> = old.entities.iter().map(|e| (&e.id, e)).collect();
    for e in &new.entities {
        if !matches!(e.kind, EntityKind::Field | EntityKind::Value) {
            continue;
        }
        let NativeLocation::Protobuf {
            number: Some(number),
            ..
        } = e.native
        else {
            continue;
        };
        if let Some(parent) = e.parent.as_ref().and_then(|p| old_by_id.get(p)) {
            let reserved = parent.attributes["reserved_numbers"]
                .as_array()
                .is_some_and(|ranges| {
                    ranges.iter().any(|r| {
                        r[0].as_i64().is_some_and(|lo| i64::from(number) >= lo)
                            && r[1].as_i64().is_some_and(|hi| i64::from(number) < hi)
                    })
                });
            let name_reserved =
                parent.attributes["reserved_names"]
                    .as_array()
                    .is_some_and(|names| {
                        names
                            .iter()
                            .any(|n| n.as_str() == e.name.rsplit('.').next())
                    });
            if reserved || name_reserved {
                changes.push(fact(e,ChangeKind::WireIdentityChanged,"RESERVED_IDENTITY_REUSED",json!({"number":number,"number_reserved":reserved,"name_reserved":name_reserved}),if reserved {&["wire","source"]}else{&["source","wire_json"]},Severity::Breaking));
            }
        }
    }
    for file in old_doc.pool.files() {
        if let Some(next) = new_doc.pool.get_file_by_name(file.name())
            && file.package_name() != next.package_name()
        {
            let e = package_entity(file.package_name());
            changes.push(fact(
                &e,
                ChangeKind::WireIdentityChanged,
                "FILE_SAME_PACKAGE",
                json!({"before":file.package_name(),"after":next.package_name()}),
                &["source"],
                Severity::Breaking,
            ));
        }
    }
    // Compilers bundle their own copies of `google/protobuf/*.proto`, so a
    // descriptor set from one protoc release compared with sources compiled by
    // protox (or another protoc) differs inside those files without any change
    // to the user's contract (#155). Drop findings whose subject is defined only
    // in bundled files; user extensions of `google.protobuf.*Options`, applied
    // options and references to well-known types all live in user files and
    // stay reported. Inputs made solely of bundled files keep every finding.
    if has_user_files(old_doc) || has_user_files(new_doc) {
        let old_bundled = bundled_entity_ids(old_doc, &old);
        let new_bundled = bundled_entity_ids(new_doc, &new);
        changes.retain(|c| {
            let in_old = old_by_id.contains_key(&c.subject);
            let in_new = new_by_id.contains_key(&c.subject);
            let bundled = (!in_old || old_bundled.contains(&c.subject))
                && (!in_new || new_bundled.contains(&c.subject));
            !(bundled && (in_old || in_new))
        });
    }
    changes.sort_by(|a, b| a.subject.cmp(&b.subject).then(a.id.cmp(&b.id)));
    changes.dedup_by(|a, b| a.id == b.id);
    Ok(changes)
}

/// Files every protobuf compiler ships itself (well-known types and
/// `descriptor.proto`); their content tracks the compiler release.
fn is_bundled_file(name: &str) -> bool {
    name.starts_with("google/protobuf/")
}

fn has_user_files(doc: &Document) -> bool {
    doc.pool.files().any(|f| !is_bundled_file(f.name()))
}

/// Entity IDs of `contract` whose symbol is defined in a bundled file.
fn bundled_entity_ids(doc: &Document, contract: &Contract) -> std::collections::HashSet<String> {
    let mut home: HashMap<String, bool> = HashMap::new();
    let mut put = |name: &str, file: &str| {
        home.insert(name.to_owned(), is_bundled_file(file));
    };
    for m in doc.pool.all_messages() {
        let file = m.parent_file();
        put(m.full_name(), file.name());
        for f in m.fields() {
            put(f.full_name(), file.name());
        }
        for o in m.oneofs() {
            put(o.full_name(), file.name());
        }
    }
    for e in doc.pool.all_enums() {
        let file = e.parent_file();
        put(e.full_name(), file.name());
        for v in e.values() {
            put(v.full_name(), file.name());
        }
    }
    for s in doc.pool.services() {
        let file = s.parent_file();
        put(s.full_name(), file.name());
        for rpc in s.methods() {
            put(rpc.full_name(), file.name());
        }
    }
    for x in doc.pool.all_extensions() {
        put(x.full_name(), x.parent_file().name());
    }
    let mut package_bundled: HashMap<String, bool> = HashMap::new();
    for f in doc.pool.files() {
        *package_bundled
            .entry(f.package_name().to_owned())
            .or_insert(true) &= is_bundled_file(f.name());
    }
    contract
        .entities
        .iter()
        .filter(|e| match e.attributes["kind"].as_str() {
            Some("file") => is_bundled_file(&e.name),
            Some("package") => package_bundled.get(e.name.as_str()) == Some(&true),
            _ => home.get(&e.name) == Some(&true),
        })
        .map(|e| e.id.clone())
        .collect()
}

fn package_entity(package: &str) -> Entity {
    let mut e = node(
        EntityKind::Type,
        package,
        None,
        None,
        json!({"kind":"package"}),
    );
    e.id = id(EntityKind::Type, &format!("package\0{package}"));
    e
}

fn file_entity(file: prost_reflect::FileDescriptor) -> Entity {
    let name = file.name().to_owned();
    let mut e = node(
        EntityKind::Type,
        &name,
        None,
        None,
        json!({"kind":"file","syntax":file.file_descriptor_proto().syntax(),"file_options":options(file.options())}),
    );
    e.id = id(EntityKind::Type, &format!("file\0{name}"));
    e
}

fn file_symbols(doc: &Document, filename: &str) -> std::collections::BTreeSet<String> {
    let mut symbols = std::collections::BTreeSet::new();
    symbols.extend(
        doc.pool
            .all_messages()
            .filter(|m| m.parent_file().name() == filename)
            .map(|m| m.full_name().to_owned()),
    );
    symbols.extend(
        doc.pool
            .all_enums()
            .filter(|e| e.parent_file().name() == filename)
            .map(|e| e.full_name().to_owned()),
    );
    symbols.extend(
        doc.pool
            .services()
            .filter(|s| s.parent_file().name() == filename)
            .map(|s| s.full_name().to_owned()),
    );
    symbols.extend(
        doc.pool
            .all_extensions()
            .filter(|e| e.parent_file().name() == filename)
            .map(|e| e.full_name().to_owned()),
    );
    symbols
}

/// Clean a protoc `source_code_info` comment string, stripping the single
/// leading space protoc leaves after `//` while preserving paragraph breaks.
fn clean_comment(raw: &str) -> Option<String> {
    let lines: Vec<&str> = raw
        .lines()
        .map(|line| line.strip_prefix(' ').unwrap_or(line).trim_end())
        .collect();
    let start = lines.iter().position(|line| !line.is_empty())?;
    let end = lines.iter().rposition(|line| !line.is_empty())?;
    Some(lines[start..=end].join("\n"))
}

/// Flatten a multiline comment into a single line for table cells and inline lists.
fn inline_comment(comment: &str) -> String {
    comment.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn annotate_block(comment: Option<&str>, note: String) -> Option<String> {
    Some(match comment {
        Some(text) if !text.is_empty() => format!("{text}\n\n{note}"),
        _ => note,
    })
}

fn annotate_inline(comment: Option<&str>, note: String) -> Option<String> {
    Some(match comment.map(inline_comment) {
        Some(text) if !text.is_empty() => format!("{text} ({note})"),
        _ => note,
    })
}

/// Source comments indexed by `(file_name, source_code_info path)`.
/// Used only by [`documentation`]; neutral contract identity and diff facts
/// deliberately ignore comments.
struct SourceComments {
    by_file: HashMap<String, HashMap<Vec<i32>, String>>,
}

impl SourceComments {
    fn from_pool(pool: &DescriptorPool) -> Self {
        let mut by_file = HashMap::new();
        for file in pool.files() {
            let Some(info) = &file.file_descriptor_proto().source_code_info else {
                continue;
            };
            let mut entries: HashMap<Vec<i32>, String> = HashMap::new();
            for loc in &info.location {
                if loc.path.is_empty() {
                    continue;
                }
                let leading = loc.leading_comments.as_deref().and_then(clean_comment);
                let trailing = loc.trailing_comments.as_deref().and_then(clean_comment);
                let combined = match (leading, trailing) {
                    (Some(l), Some(t)) => Some(format!("{l}\n\n{t}")),
                    (Some(l), None) => Some(l),
                    (None, Some(t)) => Some(t),
                    (None, None) => None,
                };
                if let Some(comment) = combined {
                    entries.entry(loc.path.clone()).or_insert(comment);
                }
            }
            if !entries.is_empty() {
                by_file.insert(file.name().to_owned(), entries);
            }
        }
        Self { by_file }
    }

    fn get(&self, file_name: &str, path: &[i32]) -> Option<&str> {
        self.by_file
            .get(file_name)
            .and_then(|entries| entries.get(path))
            .map(String::as_str)
    }
}

/// Convert native descriptors to the renderer IR. RPC is a native operation
/// label, not an HTTP method; `result` is not an HTTP status code.
pub fn documentation(doc: &Document) -> crate::ApiDocumentation {
    use crate::{Endpoint, Parameter, Response, Schema, Service};
    use indexmap::IndexMap;
    let comments = SourceComments::from_pool(&doc.pool);
    let reference = |name: &str| Schema {
        reference: Some(format!("#/components/schemas/{name}")),
        ..Default::default()
    };
    let mut schemas = IndexMap::new();
    let mut messages = doc.pool.all_messages().collect::<Vec<_>>();
    messages.sort_by(|a, b| a.full_name().cmp(b.full_name()));
    for m in messages {
        let mut fields = m.fields().collect::<Vec<_>>();
        fields.sort_by_key(|f| f.number());
        let properties = fields
            .into_iter()
            .map(|f| {
                let mut schema = match f.kind() {
                    Kind::Message(t) => reference(t.full_name()),
                    Kind::Enum(t) => reference(t.full_name()),
                    _ => Schema {
                        schema_type: Some(type_name(f.kind())),
                        ..Default::default()
                    },
                };
                if f.is_list() {
                    schema = Schema {
                        schema_type: Some("array".into()),
                        items: Some(Box::new(schema)),
                        ..Default::default()
                    };
                }
                let field_note = format!(
                    "Field #{}; JSON name `{}`{}{}{}",
                    f.number(),
                    f.json_name(),
                    f.containing_oneof()
                        .filter(|o| !o.is_synthetic())
                        .map(|o| format!("; oneof `{}`", o.name()))
                        .unwrap_or_default(),
                    if f.field_descriptor_proto()
                        .options
                        .as_ref()
                        .is_some_and(|o| o.deprecated())
                    {
                        "; deprecated"
                    } else {
                        ""
                    },
                    if f.supports_presence() {
                        "; explicit presence"
                    } else {
                        ""
                    }
                );
                schema.description =
                    annotate_inline(comments.get(f.parent_file().name(), f.path()), field_note);
                if schema.reference.is_some() {
                    schema.extensions.insert(
                        crate::markdown::PREFER_REF_DESCRIPTION_EXT.into(),
                        json!(true),
                    );
                }
                (f.name().to_owned(), schema)
            })
            .collect();
        schemas.insert(
            m.full_name().into(),
            Schema {
                schema_type: Some("object".into()),
                title: Some(m.full_name().into()),
                description: annotate_block(
                    comments.get(m.parent_file().name(), m.path()),
                    "Protocol Buffers message".into(),
                ),
                properties: Some(properties),
                required: Some(
                    m.fields()
                        .filter(|f| f.cardinality() == prost_reflect::Cardinality::Required)
                        .map(|f| f.name().into())
                        .collect(),
                ),
                ..Default::default()
            },
        );
    }
    let mut enums = doc.pool.all_enums().collect::<Vec<_>>();
    enums.sort_by(|a, b| a.full_name().cmp(b.full_name()));
    for en in enums {
        let mut values = en.values().collect::<Vec<_>>();
        values.sort_by_key(|v| (v.number(), v.name().to_owned()));
        let enum_file = en.parent_file().name().to_owned();
        let values_summary = values
            .iter()
            .map(
                |v| match comments.get(&enum_file, v.path()).map(inline_comment) {
                    Some(text) if !text.is_empty() => {
                        format!("`{}` = {} ({text})", v.name(), v.number())
                    }
                    _ => format!("`{}` = {}", v.name(), v.number()),
                },
            )
            .collect::<Vec<_>>()
            .join(", ");
        schemas.insert(
            en.full_name().into(),
            Schema {
                schema_type: Some("enum".into()),
                title: Some(en.full_name().into()),
                description: annotate_block(comments.get(&enum_file, en.path()), values_summary),
                enum_values: Some(values.iter().map(|v| json!(v.name())).collect()),
                ..Default::default()
            },
        );
    }
    let mut services = vec![];
    let mut endpoints = vec![];
    for s in doc.pool.services() {
        let service_file = s.parent_file().name().to_owned();
        services.push(Service {
            name: s.full_name().into(),
            description: annotate_block(
                comments.get(&service_file, s.path()),
                "gRPC service".into(),
            ),
        });
        for rpc in s.methods() {
            let streaming = match (rpc.is_client_streaming(), rpc.is_server_streaming()) {
                (false, false) => "unary",
                (true, false) => "client streaming",
                (false, true) => "server streaming",
                (true, true) => "bidirectional streaming",
            };
            let mut responses = IndexMap::new();
            responses.insert(
                "result".into(),
                Response {
                    reference: None,
                    description: Some(rpc.output().full_name().into()),
                    schema: Some(reference(rpc.output().full_name())),
                    content: None,
                    extensions: HashMap::new(),
                },
            );
            let rpc_note = format!(
                "{streaming}; input `{}` → result `{}`",
                rpc.input().full_name(),
                rpc.output().full_name()
            );
            endpoints.push(Endpoint {
                path: rpc.full_name().into(),
                method: "RPC".into(),
                services: vec![s.full_name().into()],
                summary: Some(rpc.name().into()),
                description: annotate_block(comments.get(&service_file, rpc.path()), rpc_note),
                operation_id: Some(rpc.full_name().into()),
                parameters: vec![Parameter {
                    reference: None,
                    name: rpc.input().full_name().into(),
                    description: Some("RPC input message".into()),
                    parameter_in: "body".into(),
                    required: Some(true),
                    schema: Some(reference(rpc.input().full_name())),
                    example: None,
                    examples: None,
                    extensions: HashMap::new(),
                }],
                responses,
                deprecated: rpc
                    .method_descriptor_proto()
                    .options
                    .as_ref()
                    .is_some_and(|o| o.deprecated()),
                untagged: false,
            });
        }
    }
    services.sort_by(|a, b| a.name.cmp(&b.name));
    endpoints.sort_by(|a, b| a.path.cmp(&b.path));
    crate::ApiDocumentation{title:"Protocol Buffers / gRPC".into(),version:String::new(),description:Some("Native Protocol Buffers contract. RPC selectors use fully qualified method names; schemas use fully qualified type names.".into()),services,endpoints,servers:vec![],security_schemes:IndexMap::new(),schemas,examples:IndexMap::new()}
}
/// Human-readable diff containing native rules and compatibility dimensions.
pub fn write_diff(
    writer: &mut impl std::io::Write,
    changes: &[Change],
    report: bool,
    old: &Document,
    new: &Document,
) -> Result<()> {
    writeln!(writer, "# Protocol Buffers / gRPC changes\n")?;
    if changes.is_empty() {
        writeln!(writer, "No contract changes.")?;
    }
    for c in changes {
        let NativeLocation::Protobuf { full_name, number } = &c.native else {
            continue;
        };
        writeln!(
            writer,
            "- **{:?}** `{full_name}`{} — `{}` ({})\n  - {:?}: `{}`",
            c.severity,
            number.map(|n| format!(" #{n}")).unwrap_or_default(),
            c.native_kind,
            c.impacts.join(", "),
            c.kind,
            crate::diff::json::canonical_json(&c.details)
        )?;
    }
    if report {
        writeln!(
            writer,
            "\n## Report\n\n| Surface | Old | New |\n|---|---:|---:|\n| Entities | {} | {} |\n| Descriptor files | {} | {} |",
            adapt(old).entities.len(),
            adapt(new).entities.len(),
            old.pool.files().len(),
            new.pool.files().len()
        )?;
    }
    writer.flush()?;
    Ok(())
}
