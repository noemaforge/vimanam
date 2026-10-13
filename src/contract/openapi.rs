//! OpenAPI adapter. Legacy endpoint diff rules are retained; independent named
//! type facts are review because a type can occur in both input and output.
use super::*;
use crate::diff::{self, Location, ValueChange, ValueChangeKind};
use crate::utils::resolve_schema_value;
use crate::{ApiDocumentation, Endpoint, Schema};
use serde_json::json;

/// Conservative compatibility policy for context-free surface comparisons.
pub struct OpenApiAdapter;
impl Adapter for OpenApiAdapter {
    fn normalize_attributes(&self, subject: &Entity) -> Value {
        semantic_attributes(&subject.attributes)
    }
    fn change_location(&self, subject: &Entity, suffix: &str) -> NativeLocation {
        let mut native = subject.native.clone();
        if let NativeLocation::Openapi { pointer, .. } = &mut native {
            pointer.push_str(if subject.kind == EntityKind::Argument {
                suffix.strip_prefix("/schema").unwrap_or(suffix)
            } else {
                suffix
            });
        }
        native
    }

    fn attribute_kind(
        &self,
        subject: &Entity,
        pointer: &str,
        kind: &ValueChangeKind,
    ) -> ChangeKind {
        if subject.kind == EntityKind::Type && pointer == "/type" {
            ChangeKind::TypeKindChanged
        } else if is_schema_deprecation_pointer(pointer) {
            ChangeKind::DeprecatedChanged
        } else {
            schema_kind(
                &ValueChange {
                    pointer: pointer.into(),
                    kind: kind.clone(),
                },
                subject.kind == EntityKind::Argument,
            )
        }
    }

    fn structural_kind(&self, subject: &Entity, field: &str) -> Option<ChangeKind> {
        match field {
            "native" => Some(ChangeKind::WireIdentityChanged),
            "references" => Some(ChangeKind::ConstraintChanged),
            "services" | "parent" | "name" => Some(ChangeKind::ConstraintChanged),
            _ if subject.kind == EntityKind::Operation => None,
            _ => Some(ChangeKind::ConstraintChanged),
        }
    }

    fn classify(&self, subject: &Entity, kind: ChangeKind, _: &Value) -> (Severity, Vec<String>) {
        let severity = match kind {
            ChangeKind::OperationRemoved => Severity::Breaking,
            ChangeKind::OperationAdded | ChangeKind::TypeAdded | ChangeKind::ServiceAdded => {
                Severity::NonBreaking
            }
            ChangeKind::ArgumentAdded if subject.attributes["required"] == true => {
                Severity::Breaking
            }
            _ => Severity::Review,
        };
        (severity, vec!["source".into()])
    }
}

fn escape(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}
fn operation_coordinate(e: &Endpoint) -> String {
    format!("/paths/{}/{}", escape(&e.path), e.method.to_lowercase())
}
fn native(
    e: Option<&Endpoint>,
    pointer: &str,
    parameter_in: Option<String>,
    status: Option<String>,
    resolved: bool,
) -> NativeLocation {
    NativeLocation::Openapi {
        method: e.map(|e| e.method.clone()),
        path: e.map(|e| e.path.clone()),
        parameter_in,
        status,
        pointer: pointer.into(),
        resolved,
    }
}
fn node(
    kind: EntityKind,
    name: &str,
    coordinate: &str,
    parent: Option<String>,
    native: NativeLocation,
    attributes: Value,
) -> Entity {
    Entity {
        id: entity_id(Format::Openapi, kind, coordinate),
        kind,
        name: name.into(),
        parent,
        services: vec![],
        native,
        attributes,
        references: vec![],
    }
}
fn schema_value(schema: &Schema) -> Value {
    serde_json::to_value(schema).expect("schema serializes")
}
fn refs(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(reference) = map.get("$ref").and_then(Value::as_str)
                && let Some(name) = reference
                    .strip_prefix("#/components/schemas/")
                    .or_else(|| reference.strip_prefix("#/definitions/"))
            {
                let id = entity_id(
                    Format::Openapi,
                    EntityKind::Type,
                    &format!(
                        "/types/{}",
                        escape(&crate::utils::decode_json_pointer_token(name))
                    ),
                );
                if !out.contains(&id) {
                    out.push(id);
                }
            }
            for value in map.values() {
                refs(value, out);
            }
        }
        Value::Array(values) => {
            for value in values {
                refs(value, out);
            }
        }
        _ => {}
    }
}
fn schema_nodes(
    entities: &mut Vec<Entity>,
    value: &Value,
    root: &str,
    parent: &str,
    e: Option<&Endpoint>,
    pointer: &str,
) {
    if let Some(properties) = value.get("properties").and_then(Value::as_object) {
        for (name, schema) in properties {
            let p = format!("{pointer}/properties/{}", escape(name));
            let coordinate = format!("{root}{p}");
            let mut field = node(
                EntityKind::Field,
                name,
                &coordinate,
                Some(parent.into()),
                native(e, &p, None, None, false),
                schema.clone(),
            );
            let required = value
                .get("required")
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().any(|v| v == name));
            if let Some(attributes) = field.attributes.as_object_mut() {
                attributes.insert("x-vimanam-required".into(), json!(required));
            }
            refs(schema, &mut field.references);
            let id = field.id.clone();
            entities.push(field);
            schema_nodes(entities, schema, root, &id, e, &p);
        }
    }
    if let Some(values) = value.get("enum").and_then(Value::as_array) {
        for value in values {
            let name = canonical_json(value);
            let coordinate = format!("{root}{pointer}/enum/{name}");
            entities.push(node(
                EntityKind::Value,
                &name,
                &coordinate,
                Some(parent.into()),
                native(e, pointer, None, None, false),
                json!({"value":value}),
            ));
        }
    }
    if let Some(items) = value.get("items") {
        schema_nodes(
            entities,
            items,
            root,
            parent,
            e,
            &format!("{pointer}/items"),
        );
    }
    for key in ["allOf", "oneOf", "anyOf"] {
        if let Some(items) = value.get(key).and_then(Value::as_array) {
            for (i, item) in items.iter().enumerate() {
                schema_nodes(
                    entities,
                    item,
                    root,
                    parent,
                    e,
                    &format!("{pointer}/{key}/{i}"),
                );
            }
        }
    }
    if let Some(additional) = value.get("additionalProperties").filter(|v| v.is_object()) {
        schema_nodes(
            entities,
            additional,
            root,
            parent,
            e,
            &format!("{pointer}/additionalProperties"),
        );
    }
}

/// Adapt the existing parsed OpenAPI IR into a neutral surface. Refs remain
/// edges; named schemas are not expanded into operation-local copies.
pub fn adapt(api: &ApiDocumentation) -> Contract {
    let mut entities = vec![];
    for service in &api.services {
        let coordinate = format!("/services/{}", escape(&service.name));
        entities.push(node(
            EntityKind::Service,
            &service.name,
            &coordinate,
            None,
            native(None, &coordinate, None, None, false),
            json!({}),
        ));
    }
    for e in &api.endpoints {
        let coordinate = operation_coordinate(e);
        let mut operation = node(
            EntityKind::Operation,
            e.operation_id.as_deref().unwrap_or(&e.path),
            &coordinate,
            None,
            native(Some(e), "", None, None, false),
            json!({"operation_id":e.operation_id,"deprecated":e.deprecated}),
        );
        operation.services = e
            .services
            .iter()
            .map(|s| {
                entity_id(
                    Format::Openapi,
                    EntityKind::Service,
                    &format!("/services/{}", escape(s)),
                )
            })
            .collect();
        let parent = operation.id.clone();
        entities.push(operation);
        for p in &e.parameters {
            let path = format!(
                "{coordinate}/arguments/{}/{}",
                escape(&p.parameter_in),
                escape(&p.name)
            );
            let value = p
                .schema
                .as_ref()
                .map(schema_value)
                .unwrap_or_else(|| json!(p.extensions));
            let mut argument = node(
                EntityKind::Argument,
                &p.name,
                &path,
                Some(parent.clone()),
                native(Some(e), "", Some(p.parameter_in.clone()), None, false),
                json!({"required":p.required.unwrap_or(false),"schema":value}),
            );
            refs(&value, &mut argument.references);
            let id = argument.id.clone();
            entities.push(argument);
            schema_nodes(&mut entities, &value, &path, &id, Some(e), "");
        }
        for (status, response) in &e.responses {
            let path = format!("{coordinate}/results/{}", escape(status));
            let value = crate::markdown::response_schema(response)
                .map(schema_value)
                .unwrap_or(Value::Null);
            let mut result = node(
                EntityKind::Result,
                status,
                &path,
                Some(parent.clone()),
                native(Some(e), "", None, Some(status.clone()), false),
                value.clone(),
            );
            refs(&value, &mut result.references);
            let id = result.id.clone();
            entities.push(result);
            schema_nodes(&mut entities, &value, &path, &id, Some(e), "");
        }
    }
    for (name, schema) in &api.schemas {
        let coordinate = format!("/types/{}", escape(name));
        let value = schema_value(schema);
        let mut ty = node(
            EntityKind::Type,
            name,
            &coordinate,
            None,
            native(None, &coordinate, None, None, false),
            value.clone(),
        );
        refs(&value, &mut ty.references);
        let id = ty.id.clone();
        entities.push(ty);
        schema_nodes(&mut entities, &value, &coordinate, &id, None, "");
    }
    Contract {
        format: Format::Openapi,
        title: api.title.clone(),
        version: api.version.clone(),
        entities,
    }
}

/// Parse an OpenAPI file and return its neutral surface.
pub fn parse(path: impl AsRef<std::path::Path>) -> anyhow::Result<Contract> {
    Ok(adapt(&crate::parse_openapi(path)?))
}

/// Parse OpenAPI bytes and return its neutral surface.
pub fn parse_bytes(bytes: &[u8], extension: &str) -> anyhow::Result<Contract> {
    Ok(adapt(&crate::parse_openapi_bytes(bytes, extension, None)?))
}

pub(super) fn schema_kind(change: &ValueChange, argument: bool) -> ChangeKind {
    let added = matches!(change.kind, ValueChangeKind::Added(_));
    match diff::locate(&change.pointer) {
        Location::Property => {
            if added {
                ChangeKind::FieldAdded
            } else if matches!(change.kind, ValueChangeKind::Removed(_)) {
                ChangeKind::FieldRemoved
            } else {
                ChangeKind::FieldTypeChanged
            }
        }
        Location::Type => {
            if argument {
                ChangeKind::ArgumentTypeChanged
            } else {
                ChangeKind::FieldTypeChanged
            }
        }
        Location::RequiredElement => ChangeKind::FieldRequiredChanged,
        Location::EnumElement => {
            if added {
                ChangeKind::EnumValueAdded
            } else {
                ChangeKind::EnumValueRemoved
            }
        }
        Location::Nullable => ChangeKind::NullabilityChanged,
        _ => {
            if change.pointer.ends_with("/default") {
                ChangeKind::DefaultChanged
            } else {
                ChangeKind::ConstraintChanged
            }
        }
    }
}
fn verdict(s: diff::Severity) -> Severity {
    match s {
        diff::Severity::Breaking => Severity::Breaking,
        diff::Severity::NonBreaking => Severity::NonBreaking,
        diff::Severity::Review => Severity::Review,
    }
}

/// Compare OpenAPI contracts through the neutral change model. Existing
/// operation-specific severity rules are reused, not inferred from neutral kind.
/// Unused and shared named type facts are additional review records; affected
/// operation records provide directional breaking verdicts.
pub fn diff(old: &ApiDocumentation, new: &ApiDocumentation) -> Vec<Change> {
    let legacy = diff::diff(old, new);
    let json = diff::json::to_json(&legacy, None, "", "");
    let mut changes = vec![];
    for (change, record) in legacy.changes.iter().zip(json.changes) {
        use diff::ChangeKind as K;
        let e = old
            .endpoints
            .iter()
            .chain(&new.endpoints)
            .find(|e| e.method == change.endpoint.method && e.path == change.endpoint.path)
            .expect("diff endpoint exists");
        let root = operation_coordinate(e);
        let mut subject = entity_id(Format::Openapi, EntityKind::Operation, &root);
        let mut parameter_in = None;
        let mut status = None;
        let mut pointer = String::new();
        let mut resolved = false;
        let kind = match &change.kind {
            K::EndpointAdded => ChangeKind::OperationAdded,
            K::EndpointRemoved { .. } => ChangeKind::OperationRemoved,
            K::OperationIdChanged { .. } => ChangeKind::OperationRenamed,
            K::DeprecatedChanged { .. } => ChangeKind::DeprecatedChanged,
            K::ParameterAdded { name, location, .. }
            | K::ParameterRemoved { name, location }
            | K::ParameterRequiredChanged { name, location, .. }
            | K::ParameterSchemaChanged { name, location, .. } => {
                parameter_in = Some(location.clone());
                subject = entity_id(
                    Format::Openapi,
                    EntityKind::Argument,
                    &format!("{root}/arguments/{}/{}", escape(location), escape(name)),
                );
                match &change.kind {
                    K::ParameterAdded { .. } => ChangeKind::ArgumentAdded,
                    K::ParameterRemoved { .. } => ChangeKind::ArgumentRemoved,
                    K::ParameterRequiredChanged { .. } => ChangeKind::ArgumentRequiredChanged,
                    K::ParameterSchemaChanged { change, .. } => {
                        pointer = change.pointer.clone();
                        resolved = true;
                        schema_kind(change, true)
                    }
                    _ => unreachable!(),
                }
            }
            K::ParameterLocationChanged { .. } => ChangeKind::WireIdentityChanged,
            K::ResponseAdded { status: s } | K::ResponseRemoved { status: s } => {
                status = Some(s.clone());
                subject = entity_id(
                    Format::Openapi,
                    EntityKind::Result,
                    &format!("{root}/results/{}", escape(s)),
                );
                if matches!(change.kind, K::ResponseAdded { .. }) {
                    ChangeKind::ResultAdded
                } else {
                    ChangeKind::ResultRemoved
                }
            }
            K::RequestSchemaChanged { change } => {
                parameter_in = Some("body".into());
                pointer = change.pointer.clone();
                resolved = true;
                schema_kind(change, false)
            }
            K::ResponseSchemaChanged { status: s, change } => {
                status = Some(s.clone());
                subject = entity_id(
                    Format::Openapi,
                    EntityKind::Result,
                    &format!("{root}/results/{}", escape(s)),
                );
                pointer = change.pointer.clone();
                resolved = true;
                schema_kind(change, false)
            }
        };
        let mut c = Change {
            id: String::new(),
            subject,
            kind,
            severity: verdict(diff::severity(change)),
            impacts: vec!["source".into()],
            native: native(Some(e), &pointer, parameter_in, status, resolved),
            native_kind: serde_json::to_value(record.kind)
                .unwrap()
                .as_str()
                .unwrap()
                .into(),
            details: serde_json::to_value(record.details).unwrap(),
        };
        c.refresh_id();
        changes.push(c);
    }
    // Named type changes remain independent of reachability. Review avoids
    // claiming a directional verdict for unused or mixed-use definitions.
    let old_surface = adapt(old);
    let new_surface = adapt(new);
    let types = |c: &Contract| Contract {
        entities: c
            .entities
            .iter()
            .filter(|e| matches!(e.kind, EntityKind::Type | EntityKind::Service))
            .cloned()
            .collect(),
        ..c.clone()
    };
    changes.extend(
        compare(&types(&old_surface), &types(&new_surface), &OpenApiAdapter).expect("same format"),
    );
    // The mature endpoint differ intentionally omits some neutral facts (for
    // example query defaults and deprecated inline response fields). Compare
    // operation-local schema nodes and append only facts outside its coverage.
    let operations = |c: &Contract| Contract {
        entities: c
            .entities
            .iter()
            .filter(|e| {
                matches!(
                    e.kind,
                    EntityKind::Operation
                        | EntityKind::Argument
                        | EntityKind::Field
                        | EntityKind::Result
                )
            })
            .cloned()
            .collect(),
        ..c.clone()
    };
    let swagger_parameter_ids: std::collections::HashSet<String> = old
        .endpoints
        .iter()
        .chain(&new.endpoints)
        .flat_map(|e| {
            let root = operation_coordinate(e);
            e.parameters
                .iter()
                .filter(|p| p.schema.is_none())
                .map(move |p| {
                    entity_id(
                        Format::Openapi,
                        EntityKind::Argument,
                        &format!(
                            "{root}/arguments/{}/{}",
                            escape(&p.parameter_in),
                            escape(&p.name)
                        ),
                    )
                })
        })
        .collect();
    let supplemental = compare(
        &operations(&old_surface),
        &operations(&new_surface),
        &OpenApiAdapter,
    )
    .expect("same format")
    .into_iter()
    .filter(|candidate| {
        let pointer = candidate
            .details
            .get("pointer")
            .and_then(Value::as_str)
            .unwrap_or("");
        (candidate.kind == ChangeKind::DeprecatedChanged
            && has_operation_schema_root(&candidate.subject, &old_surface, &new_surface))
            || (candidate.kind == ChangeKind::ConstraintChanged
                && candidate.details.get("structural") == Some(&json!("services")))
            || (candidate.kind == ChangeKind::DefaultChanged
                && swagger_parameter_ids.contains(&candidate.subject))
            || (candidate.kind == ChangeKind::ConstraintChanged
                && swagger_parameter_ids.contains(&candidate.subject)
                && [
                    "/maximum",
                    "/minimum",
                    "/maxLength",
                    "/minLength",
                    "/maxItems",
                    "/minItems",
                    "/pattern",
                    "/multipleOf",
                ]
                .iter()
                .any(|suffix| pointer.ends_with(suffix)))
    });
    for candidate in supplemental {
        let mut candidate = candidate;
        if candidate.kind == ChangeKind::DeprecatedChanged {
            promote_schema_fact_to_use_root(&mut candidate, &old_surface, &new_surface);
        }
        if !changes.iter().any(|existing| {
            existing.subject == candidate.subject
                && existing.kind == candidate.kind
                && existing.details.get("pointer") == candidate.details.get("pointer")
        }) {
            changes.push(candidate);
        }
    }
    // Refs are edges in the public surface, so also inspect their resolved
    // operation uses for deprecation changes that the legacy schema differ
    // strips. The subject and native coordinate remain use-site specific.
    for before in &old.endpoints {
        let Some(after) = new
            .endpoints
            .iter()
            .find(|e| e.method == before.method && e.path == before.path)
        else {
            continue;
        };
        let root = operation_coordinate(before);
        for parameter in &before.parameters {
            let Some(next) = after
                .parameters
                .iter()
                .find(|p| p.name == parameter.name && p.parameter_in == parameter.parameter_in)
            else {
                continue;
            };
            let left = parameter
                .schema
                .as_ref()
                .map(|s| resolve_schema_value(s, old))
                .unwrap_or_else(|| json!(parameter.extensions));
            let right = next
                .schema
                .as_ref()
                .map(|s| resolve_schema_value(s, new))
                .unwrap_or_else(|| json!(next.extensions));
            let subject = entity_id(
                Format::Openapi,
                EntityKind::Argument,
                &format!(
                    "{root}/arguments/{}/{}",
                    escape(&parameter.parameter_in),
                    escape(&parameter.name)
                ),
            );
            if parameter.schema.as_ref().is_some_and(schema_contains_ref)
                || next.schema.as_ref().is_some_and(schema_contains_ref)
            {
                append_deprecation_facts(
                    &mut changes,
                    &subject,
                    before,
                    &left,
                    &right,
                    Some(parameter.parameter_in.clone()),
                    None,
                );
            }
        }
        for (status, response) in &before.responses {
            let Some(next) = after.responses.get(status) else {
                continue;
            };
            let (Some(left), Some(right)) = (
                crate::markdown::response_schema(response),
                crate::markdown::response_schema(next),
            ) else {
                continue;
            };
            let has_ref = schema_contains_ref(left) || schema_contains_ref(right);
            let left = resolve_schema_value(left, old);
            let right = resolve_schema_value(right, new);
            let subject = entity_id(
                Format::Openapi,
                EntityKind::Result,
                &format!("{root}/results/{}", escape(status)),
            );
            if has_ref {
                append_deprecation_facts(
                    &mut changes,
                    &subject,
                    before,
                    &left,
                    &right,
                    None,
                    Some(status.clone()),
                );
            }
        }
    }
    for e in &old.endpoints {
        if let Some(next) = new
            .endpoints
            .iter()
            .find(|n| n.method == e.method && n.path == e.path)
        {
            let before = pagination(e, old);
            let after = pagination(next, new);
            if before != after && (before.is_some() || after.is_some()) {
                let mut c = Change {
                    id: String::new(),
                    subject: entity_id(
                        Format::Openapi,
                        EntityKind::Operation,
                        &operation_coordinate(e),
                    ),
                    kind: ChangeKind::PaginationChanged,
                    severity: Severity::Review,
                    impacts: vec!["source".into()],
                    native: native(Some(e), "", None, None, true),
                    native_kind: "pagination_inferred".into(),
                    details: json!({"before":before,"after":after,"confidence":"inferred","evidence_rule":"list+request_controls+continuation"}),
                };
                c.refresh_id();
                changes.push(c);
            }
        }
    }
    changes
}

fn promote_schema_fact_to_use_root(change: &mut Change, old: &Contract, new: &Contract) {
    fn find<'a>(contract: &'a Contract, id: &str) -> Option<&'a Entity> {
        contract.entities.iter().find(|e| e.id == id)
    }
    let mut current = find(new, &change.subject).or_else(|| find(old, &change.subject));
    while let Some(entity) = current {
        if matches!(entity.kind, EntityKind::Argument | EntityKind::Result) {
            change.subject = entity.id.clone();
            if let NativeLocation::Openapi { pointer, .. } = &change.native {
                change.details["pointer"] = json!(pointer);
            }
            change.refresh_id();
            return;
        }
        current = entity
            .parent
            .as_deref()
            .and_then(|id| find(new, id).or_else(|| find(old, id)));
    }
}

fn has_operation_schema_root(subject: &str, old: &Contract, new: &Contract) -> bool {
    fn find<'a>(contract: &'a Contract, id: &str) -> Option<&'a Entity> {
        contract.entities.iter().find(|e| e.id == id)
    }
    let mut current = find(new, subject).or_else(|| find(old, subject));
    while let Some(entity) = current {
        if matches!(entity.kind, EntityKind::Argument | EntityKind::Result) {
            return true;
        }
        if entity.kind == EntityKind::Type {
            return false;
        }
        current = entity
            .parent
            .as_deref()
            .and_then(|id| find(new, id).or_else(|| find(old, id)));
    }
    false
}

fn schema_contains_ref(schema: &Schema) -> bool {
    fn has_ref(value: &Value) -> bool {
        match value {
            Value::Object(map) => map.contains_key("$ref") || map.values().any(has_ref),
            Value::Array(items) => items.iter().any(has_ref),
            _ => false,
        }
    }
    has_ref(&schema_value(schema))
}

fn append_deprecation_facts(
    changes: &mut Vec<Change>,
    subject: &str,
    endpoint: &Endpoint,
    before: &Value,
    after: &Value,
    parameter_in: Option<String>,
    status: Option<String>,
) {
    let before = semantic_attributes(before);
    let after = semantic_attributes(after);
    let mut values = Vec::new();
    diff::diff_values(&before, &after, &mut String::new(), &mut values);
    for value in values
        .into_iter()
        .filter(|v| is_schema_deprecation_pointer(&v.pointer))
    {
        let (before, after) = match &value.kind {
            ValueChangeKind::Added(v) => {
                (json!({"present":false}), json!({"present":true,"value":v}))
            }
            ValueChangeKind::Removed(v) => {
                (json!({"present":true,"value":v}), json!({"present":false}))
            }
            ValueChangeKind::Changed { old, new } => (
                json!({"present":true,"value":old}),
                json!({"present":true,"value":new}),
            ),
        };
        let native = native(
            Some(endpoint),
            &value.pointer,
            parameter_in.clone(),
            status.clone(),
            true,
        );
        let details = json!({"pointer":value.pointer,"before":before,"after":after});
        let mut change = Change {
            id: String::new(),
            subject: subject.into(),
            kind: ChangeKind::DeprecatedChanged,
            severity: Severity::Review,
            impacts: vec!["source".into()],
            native,
            native_kind: "schema_deprecation".into(),
            details,
        };
        change.refresh_id();
        if !changes.iter().any(|existing| {
            existing.kind == change.kind
                && existing.details.get("pointer") == change.details.get("pointer")
                && matches!(&existing.native, NativeLocation::Openapi { method:Some(m), path:Some(p), parameter_in:pi, status:s, .. }
                    if m == &endpoint.method && p == &endpoint.path && pi == &parameter_in && s == &status)
        }) {
            changes.push(change);
        }
    }
}

fn is_schema_deprecation_pointer(pointer: &str) -> bool {
    let Some(tokens) = pointer.strip_prefix('/').map(|p| {
        p.split('/')
            .map(|t| t.replace("~1", "/").replace("~0", "~"))
            .collect::<Vec<_>>()
    }) else {
        return false;
    };
    let mut schema = true;
    let mut next_is_schema = false;
    for token in tokens {
        if next_is_schema {
            next_is_schema = false;
            schema = true;
            continue;
        }
        if schema && token == "deprecated" {
            return true;
        }
        if !schema {
            continue;
        }
        if [
            "properties",
            "patternProperties",
            "definitions",
            "$defs",
            "items",
            "not",
            "additionalProperties",
            "if",
            "then",
            "else",
            "allOf",
            "oneOf",
            "anyOf",
        ]
        .contains(&token.as_str())
        {
            next_is_schema = true;
        } else if ["default", "enum", "required", "example", "examples"].contains(&token.as_str()) {
            schema = false;
        }
    }
    false
}
fn pagination(e: &Endpoint, api: &ApiDocumentation) -> Option<Value> {
    let mut controls:Vec<Value>=e.parameters.iter().filter(|p|p.parameter_in=="query"&&["page","size","pageSize","page_size","pageToken","page_token","cursor","limit","offset"].contains(&p.name.as_str())).map(|p|json!({"name":p.name,"schema":semantic_attributes(&p.schema.as_ref().map(schema_value).unwrap_or_else(||json!(p.extensions)))})).collect();
    controls.sort_by_key(canonical_json);
    if controls.is_empty() {
        return None;
    }
    let mut responses: Vec<Value> = e
        .responses
        .iter()
        .filter(|(s, _)| s.starts_with('2'))
        .filter_map(|(s, r)| {
            let schema = crate::markdown::response_schema(r)?;
            let value = resolve_schema_value(schema, api);
            let properties = value.get("properties")?.as_object()?;
            let list = properties
                .values()
                .any(|v| v.get("type").is_some_and(|t| t == "array"));
            let continuation: Vec<&String> = properties
                .keys()
                .filter(|key| {
                    [
                        "nextPageToken",
                        "next_page_token",
                        "nextPage",
                        "next_page",
                        "nextCursor",
                        "next_cursor",
                    ]
                    .contains(&key.as_str())
                })
                .collect();
            (list && !continuation.is_empty())
                .then(|| json!({"status":s,"continuation":continuation}))
        })
        .collect();
    responses.sort_by_key(canonical_json);
    (!responses.is_empty()).then(|| json!({"controls":controls,"responses":responses}))
}
