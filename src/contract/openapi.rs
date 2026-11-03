//! OpenAPI adapter. Legacy endpoint diff rules are retained; independent named
//! type facts are review because a type can occur in both input and output.
use super::*;
use crate::diff::{self, Location, ValueChange, ValueChangeKind};
use crate::utils::resolve_schema_value;
use crate::{ApiDocumentation, Endpoint, Schema};
use serde_json::json;
use std::collections::{HashMap, HashSet};

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
            ChangeKind::OperationRemoved | ChangeKind::OperationMoved => Severity::Breaking,
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
                .unwrap_or_else(|| extensions_value(&p.extensions));
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
///
/// Duplicate entity IDs in an adapted surface are returned as an error.
/// Operations match by exact method and path; see [`diff_with`] for moves.
pub fn diff(old: &ApiDocumentation, new: &ApiDocumentation) -> anyhow::Result<Vec<Change>> {
    diff_with(old, new, diff::EndpointMatching::Exact)
}

/// [`diff()`] with explicit operation matching.
///
/// With [`diff::EndpointMatching::Moves`], an operation paired across a path
/// move ([`diff::diff_with`]) emits one breaking `operation_moved` fact whose
/// subject is the operation at its **new** coordinate, with the old method,
/// path, pairing rule, differing operationIds and old entity ID
/// (`from_operation`) in details. Inner facts (arguments, results, schema,
/// deprecation, pagination) compare the pair as one operation and follow the
/// exact-mode sides: removals (`argument_removed`, `result_removed`,
/// `field_removed`, `enum_value_removed`) use old-side subjects and native
/// locations, which resolve in the old surface; additions and changed facts
/// use the new-side ones, which resolve in the new surface.
pub fn diff_with(
    old: &ApiDocumentation,
    new: &ApiDocumentation,
    matching: diff::EndpointMatching,
) -> anyhow::Result<Vec<Change>> {
    diff_from(old, new, &diff::diff_with(old, new, matching))
}

/// [`diff_with`] over an already computed endpoint diff of the same specs, so
/// callers that also render it compute operation moves once.
pub(crate) fn diff_from(
    old: &ApiDocumentation,
    new: &ApiDocumentation,
    legacy: &diff::SpecDiff,
) -> anyhow::Result<Vec<Change>> {
    // Moved operations: (method, old path) -> new path.
    let moved: HashMap<(&str, &str), &str> = legacy
        .changes
        .iter()
        .filter_map(|c| match &c.kind {
            diff::ChangeKind::EndpointMoved { from, .. } => Some((
                (from.method.as_str(), from.path.as_str()),
                c.endpoint.path.as_str(),
            )),
            _ => None,
        })
        .collect();
    // The old spec with each moved operation re-keyed to its new path, so the
    // supplemental passes below compare a moved pair as one operation.
    let aligned;
    let original_old = old;
    let old = if moved.is_empty() {
        old
    } else {
        let mut copy = old.clone();
        for endpoint in &mut copy.endpoints {
            if let Some(path) = moved.get(&(endpoint.method.as_str(), endpoint.path.as_str())) {
                endpoint.path = (*path).to_string();
            }
        }
        aligned = copy;
        &aligned
    };
    let changes = compare_aligned(old, new, legacy)?;
    if moved.is_empty() {
        return Ok(changes);
    }
    Ok(restore_removal_sides(
        changes,
        original_old,
        old,
        new,
        &moved,
    ))
}

/// Re-keys removal facts on moved operations from the aligned old copy (old
/// operation at the new path) back to the real old subject and native
/// location, so they resolve in the old surface exactly as in exact mode.
fn restore_removal_sides(
    mut changes: Vec<Change>,
    original_old: &ApiDocumentation,
    aligned_old: &ApiDocumentation,
    new: &ApiDocumentation,
    moved: &HashMap<(&str, &str), &str>,
) -> Vec<Change> {
    let original = adapt(original_old);
    let aligned = adapt(aligned_old);
    debug_assert_eq!(original.entities.len(), aligned.entities.len());
    let subjects: HashMap<&str, &str> = aligned
        .entities
        .iter()
        .zip(&original.entities)
        .filter(|(a, o)| a.id != o.id)
        .map(|(a, o)| (a.id.as_str(), o.id.as_str()))
        .collect();
    let new_ids: HashSet<String> = adapt(new).entities.into_iter().map(|e| e.id).collect();
    // (method, new path) -> old path
    let old_paths: HashMap<(&str, &str), &str> = moved
        .iter()
        .map(|(&(method, from), &to)| ((method, to), from))
        .collect();
    for change in &mut changes {
        let removal = matches!(
            change.kind,
            ChangeKind::ArgumentRemoved
                | ChangeKind::ResultRemoved
                | ChangeKind::FieldRemoved
                | ChangeKind::EnumValueRemoved
        );
        let Some(&subject) = subjects.get(change.subject.as_str()) else {
            continue;
        };
        if !removal && new_ids.contains(&change.subject) {
            continue;
        }
        change.subject = subject.to_string();
        if let NativeLocation::Openapi {
            method: Some(method),
            path: Some(path),
            ..
        } = &mut change.native
            && let Some(&from) = old_paths.get(&(method.as_str(), path.as_str()))
        {
            *path = from.to_string();
        }
        change.refresh_id();
    }
    changes
}

/// The fact passes of [`diff_from`] over specs whose moved operations share a
/// path, so every pair is compared as one operation.
fn compare_aligned(
    old: &ApiDocumentation,
    new: &ApiDocumentation,
    legacy: &diff::SpecDiff,
) -> anyhow::Result<Vec<Change>> {
    // Endpoint lookup for each legacy change: old spec first, then new.
    let mut by_key: HashMap<(&str, &str), &Endpoint> = HashMap::new();
    for e in old.endpoints.iter().chain(&new.endpoints) {
        by_key
            .entry((e.method.as_str(), e.path.as_str()))
            .or_insert(e);
    }
    let new_by_key: HashMap<(&str, &str), &Endpoint> = new
        .endpoints
        .iter()
        .map(|e| ((e.method.as_str(), e.path.as_str()), e))
        .collect();
    let json = diff::json::to_json(legacy, None, "", "");
    let mut changes = vec![];
    for (change, record) in legacy.changes.iter().zip(json.changes) {
        use diff::ChangeKind as K;
        let e = *by_key
            .get(&(
                change.endpoint.method.as_str(),
                change.endpoint.path.as_str(),
            ))
            .expect("diff endpoint exists");
        let root = operation_coordinate(e);
        let mut subject = entity_id(Format::Openapi, EntityKind::Operation, &root);
        let mut parameter_in = None;
        let mut status = None;
        let mut pointer = String::new();
        let mut resolved = false;
        let mut details = serde_json::to_value(record.details).unwrap();
        let kind = match &change.kind {
            K::EndpointAdded => ChangeKind::OperationAdded,
            K::EndpointMoved { from, .. } => {
                details["from_operation"] = json!(entity_id(
                    Format::Openapi,
                    EntityKind::Operation,
                    &format!(
                        "/paths/{}/{}",
                        escape(&from.path),
                        from.method.to_lowercase()
                    ),
                ));
                ChangeKind::OperationMoved
            }
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
                        // `deprecated` under keywords the legacy canonicalizer
                        // does not walk (`if`/`then`/`else`) is a deprecation
                        // fact from the supplemental pass, not a constraint.
                        if is_schema_deprecation_pointer(&change.pointer) {
                            continue;
                        }
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
                if is_schema_deprecation_pointer(&change.pointer) {
                    continue;
                }
                // Legacy comparison uses only the first `in: body` parameter.
                parameter_in = Some("body".into());
                pointer = change.pointer.clone();
                resolved = true;
                if let Some(name) = first_body_name(e) {
                    subject = body_argument_id(e, name);
                }
                schema_kind(change, true)
            }
            K::ResponseSchemaChanged { status: s, change } => {
                if is_schema_deprecation_pointer(&change.pointer) {
                    continue;
                }
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
            details,
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
    changes.extend(compare(
        &types(&old_surface),
        &types(&new_surface),
        &OpenApiAdapter,
    )?);
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
    let first_body_ids: std::collections::HashSet<String> = old
        .endpoints
        .iter()
        .chain(&new.endpoints)
        .filter_map(|endpoint| {
            first_body_name(endpoint).map(|name| body_argument_id(endpoint, name))
        })
        .collect();
    let supplemental = compare(
        &operations(&old_surface),
        &operations(&new_surface),
        &OpenApiAdapter,
    )?
    .into_iter()
    .filter(|candidate| {
        let pointer = candidate
            .details
            .get("pointer")
            .and_then(Value::as_str)
            .unwrap_or("");
        (candidate.kind == ChangeKind::DeprecatedChanged
            && has_operation_schema_root(&candidate.subject, &old_surface, &new_surface)
            && !later_request_body_subject(
                &candidate.subject,
                &old_surface,
                &new_surface,
                &first_body_ids,
            ))
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
        let Some(&after) = new_by_key.get(&(before.method.as_str(), before.path.as_str())) else {
            continue;
        };
        let root = operation_coordinate(before);
        for parameter in &before.parameters {
            if parameter.parameter_in == "body"
                && first_body_name(before) != Some(parameter.name.as_str())
            {
                continue;
            }
            let Some(next) = after
                .parameters
                .iter()
                .find(|p| p.name == parameter.name && p.parameter_in == parameter.parameter_in)
            else {
                continue;
            };
            if next.parameter_in == "body" && first_body_name(after) != Some(next.name.as_str()) {
                continue;
            }
            let left = parameter
                .schema
                .as_ref()
                .map(|s| resolve_schema_value(s, old))
                .unwrap_or_else(|| extensions_value(&parameter.extensions));
            let right = next
                .schema
                .as_ref()
                .map(|s| resolve_schema_value(s, new))
                .unwrap_or_else(|| extensions_value(&next.extensions));
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
        if let Some(&next) = new_by_key.get(&(e.method.as_str(), e.path.as_str())) {
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
                    details: {
                        // The side that now holds evidence describes it; for a
                        // removal, the old evidence does.
                        let (confidence, rule) = pagination_strength(
                            after.as_ref().or(before.as_ref()).expect("one side"),
                        );
                        json!({"before":before,"after":after,"confidence":confidence,"evidence_rule":rule})
                    },
                };
                c.refresh_id();
                changes.push(c);
            }
        }
    }
    Ok(changes)
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

fn first_body_name(endpoint: &Endpoint) -> Option<&str> {
    endpoint
        .parameters
        .iter()
        .find(|parameter| parameter.parameter_in == "body")
        .map(|parameter| parameter.name.as_str())
}

fn body_argument_id(endpoint: &Endpoint, name: &str) -> String {
    entity_id(
        Format::Openapi,
        EntityKind::Argument,
        &format!(
            "{}/arguments/{}/{}",
            operation_coordinate(endpoint),
            escape("body"),
            escape(name)
        ),
    )
}

fn later_request_body_subject(
    subject: &str,
    old: &Contract,
    new: &Contract,
    first_body_ids: &std::collections::HashSet<String>,
) -> bool {
    fn find<'a>(contract: &'a Contract, id: &str) -> Option<&'a Entity> {
        contract.entities.iter().find(|entity| entity.id == id)
    }
    let mut current = find(new, subject).or_else(|| find(old, subject));
    while let Some(entity) = current {
        if entity.kind == EntityKind::Argument {
            let body = matches!(
                &entity.native,
                NativeLocation::Openapi {
                    parameter_in: Some(location),
                    ..
                } if location == "body"
            );
            return body && !first_body_ids.contains(&entity.id);
        }
        if matches!(
            entity.kind,
            EntityKind::Result | EntityKind::Type | EntityKind::Operation | EntityKind::Service
        ) {
            return false;
        }
        current = entity
            .parent
            .as_deref()
            .and_then(|id| find(new, id).or_else(|| find(old, id)));
    }
    false
}

/// True when `pointer` addresses a schema `deprecated` keyword.
///
/// Name maps (`properties`, `patternProperties`, `definitions`, `$defs`) and
/// composition lists (`allOf`, `oneOf`, `anyOf`) consume the next token.
/// Single-schema keywords (`items`, `not`, `additionalProperties`, `if`,
/// `then`, `else`) are already inside the child schema, so that token is
/// tested. `schema` is the argument-attribute wrapper around a body schema.
fn is_schema_deprecation_pointer(pointer: &str) -> bool {
    let Some(path) = pointer.strip_prefix('/') else {
        return false;
    };
    if path.is_empty() {
        return false;
    }
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Node {
        Schema,
        Name,
        Index,
        Opaque,
    }
    let mut node = Node::Schema;
    for token in path.split('/').map(crate::utils::decode_json_pointer_token) {
        if node == Node::Schema && token == "deprecated" {
            return true;
        }
        node = match node {
            Node::Schema => match token.as_str() {
                "properties" | "patternProperties" | "definitions" | "$defs" => Node::Name,
                "allOf" | "oneOf" | "anyOf" => Node::Index,
                "items" | "not" | "additionalProperties" | "if" | "then" | "else" | "schema" => {
                    Node::Schema
                }
                _ => Node::Opaque,
            },
            Node::Name | Node::Index => Node::Schema,
            Node::Opaque => Node::Opaque,
        };
    }
    false
}
/// Request controls that establish an endpoint as possibly paginated. Names are
/// exact (unchanged since 2.0.0, so existing evidence and IDs are stable).
const PAGINATION_CONTROLS: &[&str] = &[
    "page",
    "size",
    "pageSize",
    "page_size",
    "pageToken",
    "page_token",
    "cursor",
    "limit",
    "offset",
];
/// The exact top-level continuation names 2.0.0 recognised. A match on these
/// keeps the 2.0.0 evidence shape (see [`pagination`]).
const LEGACY_CONTINUATIONS: &[&str] = &[
    "nextPageToken",
    "next_page_token",
    "nextPage",
    "next_page",
    "nextCursor",
    "next_cursor",
];
/// Controls that carry a continuation token back to the server; they
/// corroborate a generically named continuation field.
const TOKEN_CONTROLS: &[&str] = &["cursor", "pageToken", "page_token"];
/// Continuation field names that are specific enough on their own, compared
/// after [`pagination_key`] normalisation.
const SPECIFIC_CONTINUATIONS: &[&str] = &[
    "nextpagetoken",
    "nextpage",
    "nextcursor",
    "nextpagecursor",
    "nexttoken",
    "nextmarker",
    "nextlink",
    "nexturl",
    "nextpageurl",
    "nextpagelink",
    "aftercursor",
    "continuationtoken",
    "continuation",
    "lastevaluatedkey",
    "odatanextlink",
];
/// Common words that are continuations only with corroboration: a string
/// field (not a date) plus a token request control or an end-of-results flag.
const GENERIC_CONTINUATIONS: &[&str] = &["cursor", "next", "after", "marker"];
/// Boolean end-of-results (or more-results) flags: supporting evidence only.
const END_FLAGS: &[&str] = &[
    "hasmore",
    "hasnext",
    "hasnextpage",
    "hasmoreresults",
    "moreresults",
    "endofstream",
    "endofresults",
    "islast",
    "islastpage",
    "lastpage",
    "complete",
    "iscomplete",
    "done",
    "drained",
    "truncated",
    "istruncated",
];

/// Case-insensitive name key ignoring `_`, `-`, `@` and `.` separators, so
/// `next_cursor`, `nextCursor` and `@odata.nextLink` compare by their words.
fn pagination_key(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '_' | '-' | '@' | '.'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn schema_has_type(schema: &Value, ty: &str) -> bool {
    match schema.get("type") {
        Some(Value::String(t)) => t == ty,
        Some(Value::Array(types)) => types.iter().any(|t| t == ty),
        _ => false,
    }
}

/// A schema-less (Swagger 2 non-body) parameter's keywords as a JSON object
/// with sorted keys. `extensions` is a `HashMap`, and `preserve_order` would
/// otherwise serialize its random iteration order, making v2 JSON bytes vary
/// between runs (IDs hash canonical JSON and were already stable).
fn extensions_value(extensions: &HashMap<String, Value>) -> Value {
    let mut entries: Vec<(&String, &Value)> = extensions.iter().collect();
    entries.sort_by_key(|(key, _)| *key);
    Value::Object(
        entries
            .into_iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

/// A generic continuation must be an opaque string, not a timestamp filter.
fn opaque_string(schema: &Value) -> bool {
    schema_has_type(schema, "string")
        && !schema
            .get("format")
            .and_then(Value::as_str)
            .is_some_and(|f| matches!(f, "date" | "date-time" | "time"))
}

/// Inferred OpenAPI pagination evidence for one operation, or `None`.
///
/// Requires at least one exact [`PAGINATION_CONTROLS`] query parameter and a
/// 2xx response whose ref-resolved schema has a direct array property and a
/// continuation field, at the top level or one object level down
/// (`links/next`). Specific names qualify alone; generic names need a token
/// control or an end flag. Evidence records the controls and, per response,
/// the matched continuation paths (top-level names bare, nested ones as
/// pointer tokens joined by `/`), plus `generic_continuation` and `end_flags`
/// keys only when present.
///
/// Stability: when the 2.0.0 rule (a top-level property whose `type` is the
/// string `"array"` plus one of [`LEGACY_CONTINUATIONS`] at the top level)
/// matches any response, the evidence is exactly the 2.0.0 evidence, built from
/// only the responses that rule matched. The broader rules apply only when it
/// matches none. This is per side: a 2.0.0 fact keeps its `vc2_` ID when the
/// rule matches both sides, or one side while this returns `None` for the
/// other. If the other side has broader-only evidence, it replaces 2.0.0's
/// `null` and the fact's ID changes (pinned by
/// `evidence_2_0_0_on_one_side_with_broader_evidence_on_the_other_changes_the_id`).
fn pagination(e: &Endpoint, api: &ApiDocumentation) -> Option<Value> {
    let query = || e.parameters.iter().filter(|p| p.parameter_in == "query");
    let mut controls: Vec<Value> = query()
        .filter(|p| PAGINATION_CONTROLS.contains(&p.name.as_str()))
        .map(|p| {
            json!({"name":p.name,"schema":semantic_attributes(&p.schema.as_ref().map(schema_value).unwrap_or_else(||extensions_value(&p.extensions)))})
        })
        .collect();
    controls.sort_by_key(canonical_json);
    if controls.is_empty() {
        return None;
    }
    let resolved: Vec<(&String, Value)> = e
        .responses
        .iter()
        .filter(|(s, _)| s.starts_with('2'))
        .filter_map(|(s, r)| {
            crate::markdown::response_schema(r).map(|schema| (s, resolve_schema_value(schema, api)))
        })
        .collect();
    let mut legacy: Vec<Value> = resolved
        .iter()
        .filter_map(|(s, value)| {
            let properties = value.get("properties")?.as_object()?;
            let list = properties
                .values()
                .any(|v| v.get("type").is_some_and(|t| t == "array"));
            let mut continuation: Vec<&String> = properties
                .keys()
                .filter(|key| LEGACY_CONTINUATIONS.contains(&key.as_str()))
                .collect();
            continuation.sort();
            (list && !continuation.is_empty())
                .then(|| json!({"status":s,"continuation":continuation}))
        })
        .collect();
    if !legacy.is_empty() {
        legacy.sort_by_key(canonical_json);
        return Some(json!({"controls":controls,"responses":legacy}));
    }
    let token_control = query().any(|p| TOKEN_CONTROLS.contains(&p.name.as_str()));
    let mut responses: Vec<Value> = resolved
        .iter()
        .filter_map(|(s, value)| {
            let properties = value.get("properties")?.as_object()?;
            let list = properties.values().any(|v| schema_has_type(v, "array"));
            if !list {
                return None;
            }
            // Candidate `(path, key, schema)`: direct properties, then one
            // object level down.
            let mut fields: Vec<(String, String, &Value)> = vec![];
            for (name, schema) in properties {
                fields.push((name.clone(), pagination_key(name), schema));
                if let Some(inner) = schema.get("properties").and_then(Value::as_object) {
                    for (child, schema) in inner {
                        let path = format!("{}/{}", escape(name), escape(child));
                        fields.push((path, pagination_key(child), schema));
                    }
                }
            }
            let select = |accept: &dyn Fn(&str, &Value) -> bool| -> Vec<String> {
                let mut paths: Vec<String> = fields
                    .iter()
                    .filter(|(_, key, schema)| accept(key, schema))
                    .map(|(path, _, _)| path.clone())
                    .collect();
                paths.sort();
                paths
            };
            let end_flags = select(&|key, schema| {
                schema_has_type(schema, "boolean") && END_FLAGS.contains(&key)
            });
            let mut specific = select(&|key, _| SPECIFIC_CONTINUATIONS.contains(&key));
            let corroborated = token_control || !end_flags.is_empty();
            let generic = select(&|key, schema| {
                corroborated && opaque_string(schema) && GENERIC_CONTINUATIONS.contains(&key)
            });
            if specific.is_empty() && generic.is_empty() {
                return None;
            }
            specific.extend(generic.iter().cloned());
            specific.sort();
            let mut evidence = json!({"status":s,"continuation":specific});
            if !generic.is_empty() {
                evidence["generic_continuation"] = json!(generic);
            }
            if !end_flags.is_empty() {
                evidence["end_flags"] = json!(end_flags);
            }
            Some(evidence)
        })
        .collect();
    responses.sort_by_key(canonical_json);
    (!responses.is_empty()).then(|| json!({"controls":controls,"responses":responses}))
}

/// `(confidence, evidence_rule)` for one side's evidence. A generic
/// continuation needs one corroboration to be inferred at all; an end-of-results
/// flag beyond that minimum raises confidence to `corroborated`. Never
/// `declared`: OpenAPI has no pagination declaration.
fn pagination_strength(evidence: &Value) -> (&'static str, String) {
    let responses = evidence["responses"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    // The strongest response: specific names beat generic-only ones.
    let specific = responses.iter().any(|r| {
        r["continuation"].as_array().map_or(0, Vec::len)
            > r["generic_continuation"].as_array().map_or(0, Vec::len)
    });
    let end_flag = responses.iter().any(|r| r.get("end_flags").is_some());
    let token = evidence["controls"].as_array().is_some_and(|controls| {
        controls.iter().any(|c| {
            c["name"]
                .as_str()
                .is_some_and(|n| TOKEN_CONTROLS.contains(&n))
        })
    });
    let mut rule = String::from("list+request_controls+");
    rule.push_str(if specific {
        "continuation"
    } else {
        "generic_continuation"
    });
    if !specific && token {
        rule.push_str("+token_control");
    }
    if end_flag {
        rule.push_str("+end_flag");
    }
    let corroborated = end_flag && (specific || token);
    (
        if corroborated {
            "corroborated"
        } else {
            "inferred"
        },
        rule,
    )
}
