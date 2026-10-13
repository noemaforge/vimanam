//! Protocol Buffers and gRPC adapter. Source compilation requires `protoc` on
//! PATH; descriptor decoding and comparison do not require external tools.
use super::*;
use anyhow::{Context, Result};
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, Kind};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
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
/// Compile source using the installed official compiler; no code generation occurs.
pub fn compile(path: &Path, includes: &[PathBuf]) -> Result<Document> {
    let path = path
        .canonicalize()
        .with_context(|| format!("Missing protobuf source: {path:?}"))?;
    let temp = tempfile::tempdir()?;
    let output = temp.path().join("descriptor.pb");
    let mut command = Command::new("protoc");
    command
        .arg("--include_imports")
        .arg("--include_source_info")
        .arg(format!("--descriptor_set_out={}", output.display()));
    // User roots precede the input directory, matching protoc import semantics.
    for root in includes {
        command.arg("-I").arg(
            root.canonicalize()
                .with_context(|| format!("Invalid protobuf import root: {root:?}"))?,
        );
    }
    command.arg("-I").arg(path.parent().unwrap()).arg(&path);
    let result = command.output().context("Compiling .proto requires protoc on PATH; alternatively supply a FileDescriptorSet built with --include_imports")?;
    anyhow::ensure!(
        result.status.success(),
        "protoc failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    Document::decode(&std::fs::read(output)?)
}
#[cfg(feature = "cli")]
pub(crate) fn compile_bytes(path: &Path, bytes: &[u8], includes: &[PathBuf]) -> Result<Document> {
    let temp = tempfile::tempdir()?;
    let canonical_path = path.canonicalize()?;
    let virtual_path = includes
        .iter()
        .filter_map(|root| {
            root.canonicalize()
                .ok()
                .and_then(|root| canonical_path.strip_prefix(root).ok())
                .filter(|relative| !relative.as_os_str().is_empty())
                .map(Path::to_path_buf)
        })
        .next()
        .or_else(|| path.file_name().map(PathBuf::from))
        .context("Missing protobuf input filename")?;
    let saved = temp.path().join(virtual_path);
    std::fs::create_dir_all(saved.parent().context("Missing protobuf input directory")?)?;
    std::fs::write(&saved, bytes)?;
    let mut roots = vec![temp.path().to_path_buf()];
    roots.extend_from_slice(includes);
    roots.push(path.canonicalize()?.parent().unwrap().to_path_buf());
    compile(&saved, &roots)
}
fn id(kind: EntityKind, name: &str) -> String {
    entity_id(Format::Protobuf, kind, name)
}
fn options(message: DynamicMessage) -> Value {
    serde_json::to_value(message).unwrap_or(Value::Null)
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
    changes.sort_by(|a, b| a.subject.cmp(&b.subject).then(a.id.cmp(&b.id)));
    changes.dedup_by(|a, b| a.id == b.id);
    Ok(changes)
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

/// Convert native descriptors to the renderer IR. RPC is a native operation
/// label, not an HTTP method; `result` is not an HTTP status code.
pub fn documentation(doc: &Document) -> crate::ApiDocumentation {
    use crate::{Endpoint, Parameter, Response, Schema, Service};
    use indexmap::IndexMap;
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
                schema.description = Some(format!(
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
                ));
                if f.is_list() {
                    schema = Schema {
                        schema_type: Some("array".into()),
                        items: Some(Box::new(schema)),
                        ..Default::default()
                    };
                }
                (f.name().to_owned(), schema)
            })
            .collect();
        schemas.insert(
            m.full_name().into(),
            Schema {
                schema_type: Some("object".into()),
                title: Some(m.full_name().into()),
                description: Some("Protocol Buffers message".into()),
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
        schemas.insert(
            en.full_name().into(),
            Schema {
                schema_type: Some("enum".into()),
                title: Some(en.full_name().into()),
                description: Some(
                    values
                        .iter()
                        .map(|v| format!("`{}` = {}", v.name(), v.number()))
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
                enum_values: Some(values.iter().map(|v| json!(v.name())).collect()),
                ..Default::default()
            },
        );
    }
    let mut services = vec![];
    let mut endpoints = vec![];
    for s in doc.pool.services() {
        services.push(Service {
            name: s.full_name().into(),
            description: Some("gRPC service".into()),
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
            endpoints.push(Endpoint {
                path: rpc.full_name().into(),
                method: "RPC".into(),
                services: vec![s.full_name().into()],
                summary: Some(rpc.name().into()),
                description: Some(format!(
                    "{streaming}; input `{}` → result `{}`",
                    rpc.input().full_name(),
                    rpc.output().full_name()
                )),
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
