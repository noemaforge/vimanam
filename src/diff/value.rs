// ── generic value differ ────────────────────────────────────────────────────

use serde_json::Value;

use crate::utils::decode_json_pointer_token;

/// One difference between two canonical JSON values.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueChange {
    /// JSON pointer (RFC 6901) from the schema root, e.g. `/properties/pricing`
    /// or `/items/type`. Elements of the order-insensitive `required` and
    /// `enum` sets are addressed as `/required/<name>` and `/enum/<value>`.
    /// The empty pointer is the root itself.
    pub pointer: String,
    pub kind: ValueChangeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ValueChangeKind {
    Added(Value),
    Removed(Value),
    Changed { old: Value, new: Value },
}

// ── schema grammar ──────────────────────────────────────────────────────────

/// What a JSON pointer into a schema addresses, as far as the severity rules
/// care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Location {
    /// A `type` keyword.
    Type,
    /// A member of a `required` array.
    RequiredElement,
    /// A member of an `enum` array.
    EnumElement,
    /// A named property (the schema under `properties/<name>`).
    Property,
    /// An `additionalProperties` keyword.
    AdditionalProperties,
    /// A `nullable` keyword.
    Nullable,
    /// Anything else (`format`, `minimum`, an `allOf` variant, ...).
    Other,
}

/// The grammatical role of one node in a schema tree. Both the differ
/// ([`diff_values`]) and the pointer classifier ([`locate`]) walk the tree with
/// this state, so keyword semantics (set comparison of `required`, per-member
/// reporting of `properties`, "`type` is a type") apply only where the JSON
/// Schema grammar puts a keyword — never to a *property* that happens to be
/// called `properties`, `required`, `enum` or `type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Node {
    /// A schema object: its keys are keywords.
    Schema,
    /// A `properties`/`patternProperties` map: its keys are field names, its
    /// values schemas.
    PropertyMap,
    /// An `allOf`/`oneOf`/`anyOf` list: its elements are schemas.
    SchemaList,
    /// A `required`/`enum` array: compared as a set of opaque members.
    Set(Location),
    /// Anything else (`default`, `example`, a set member, an unknown keyword's
    /// value): compared generically, with no keyword semantics.
    Opaque,
}

impl Node {
    /// The role of the child reached from `self` through `segment` (an object
    /// key or array index).
    fn child(self, segment: &str) -> Node {
        match self {
            Node::Schema => match segment {
                "properties" | "patternProperties" => Node::PropertyMap,
                "allOf" | "oneOf" | "anyOf" => Node::SchemaList,
                "items" | "additionalProperties" | "not" => Node::Schema,
                "required" => Node::Set(Location::RequiredElement),
                "enum" => Node::Set(Location::EnumElement),
                _ => Node::Opaque,
            },
            Node::PropertyMap | Node::SchemaList => Node::Schema,
            Node::Set(_) | Node::Opaque => Node::Opaque,
        }
    }

    /// What the child reached from `self` through `segment` *is*, for the
    /// severity rules.
    fn locate(self, segment: &str) -> Location {
        match self {
            Node::Schema => match segment {
                "type" => Location::Type,
                "nullable" => Location::Nullable,
                "additionalProperties" => Location::AdditionalProperties,
                _ => Location::Other,
            },
            Node::PropertyMap => Location::Property,
            Node::Set(element) => element,
            Node::SchemaList | Node::Opaque => Location::Other,
        }
    }
}

/// Classifies `pointer` by walking it with the schema grammar in mind: under
/// `properties` the next segment is a field name (so a field called `type` is a
/// property, not a type keyword), under `allOf` it is an index, and so on.
pub(crate) fn locate(pointer: &str) -> Location {
    let mut node = Node::Schema;
    let mut location = Location::Other;
    for segment in pointer.split('/').skip(1) {
        let segment = decode_json_pointer_token(segment);
        location = node.locate(&segment);
        node = node.child(&segment);
    }
    location
}

/// The value to compare `present` against when the key for `child` exists on
/// only one side of a schema object: an empty set for `required`/`enum` and an
/// empty map for `properties`, so that members are reported one by one
/// (`/required/x`, `/properties/x`) instead of as a single opaque change at
/// `/required` or `/properties`. `None` for every other node, which is then
/// reported whole.
fn empty_counterpart(child: Node, present: &Value) -> Option<Value> {
    match child {
        Node::Set(_) if present.is_array() => Some(Value::Array(Vec::new())),
        Node::PropertyMap if present.is_object() => Some(Value::Object(serde_json::Map::new())),
        _ => None,
    }
}

/// Encodes a JSON Pointer reference token (RFC 6901): `~` → `~0`, `/` → `~1`.
fn encode_json_pointer_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// The pointer segment used for a member of a `required`/`enum` set: the bare
/// string for string members, compact JSON otherwise.
fn set_element_segment(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Appends `/segment` to `path`, returning the length to truncate back to.
fn push_segment(path: &mut String, segment: &str) -> usize {
    let mark = path.len();
    path.push('/');
    path.push_str(&encode_json_pointer_token(segment));
    mark
}

/// Reports every difference between `old` and `new` into `out`, addressing each
/// by JSON pointer under `path` (pass an empty `String` for the root). Both
/// values are taken to be schemas.
///
/// Objects are compared key-wise: the old object's keys in order, then any key
/// only the new object has. Where the schema grammar says a keyword sits, the
/// `required` and `enum` arrays are compared as sets (a missing array counts as
/// empty), reporting each member added or removed at `/required/<member>`;
/// likewise a `properties` map present on one side only reports each property
/// (`/properties/<name>`) rather than the map as a whole. A *property* named
/// `required` or `properties` gets none of that treatment (see [`Node`]).
/// Every other array is compared index-wise, so a longer list reports `Added`
/// at the extra indices (`/allOf/2`) and a shorter one `Removed`. Scalars that
/// differ are `Changed`.
pub fn diff_values(old: &Value, new: &Value, path: &mut String, out: &mut Vec<ValueChange>) {
    diff_nodes(old, new, Node::Schema, path, out);
}

fn diff_nodes(old: &Value, new: &Value, node: Node, path: &mut String, out: &mut Vec<ValueChange>) {
    match (old, new) {
        (Value::Object(old_map), Value::Object(new_map)) => {
            for (key, old_child) in old_map {
                let child = node.child(key);
                let mark = push_segment(path, key);
                match (new_map.get(key), empty_counterpart(child, old_child)) {
                    (Some(new_child), _) => diff_nodes(old_child, new_child, child, path, out),
                    (None, Some(empty)) => diff_nodes(old_child, &empty, child, path, out),
                    (None, None) => out.push(ValueChange {
                        pointer: path.clone(),
                        kind: ValueChangeKind::Removed(old_child.clone()),
                    }),
                }
                path.truncate(mark);
            }
            for (key, new_child) in new_map {
                if old_map.contains_key(key) {
                    continue;
                }
                let child = node.child(key);
                let mark = push_segment(path, key);
                match empty_counterpart(child, new_child) {
                    Some(empty) => diff_nodes(&empty, new_child, child, path, out),
                    None => out.push(ValueChange {
                        pointer: path.clone(),
                        kind: ValueChangeKind::Added(new_child.clone()),
                    }),
                }
                path.truncate(mark);
            }
        }
        (Value::Array(_), Value::Array(_)) if matches!(node, Node::Set(_)) => {
            diff_sets(old, new, path, out)
        }
        (Value::Array(old_items), Value::Array(new_items)) => {
            let common = old_items.len().min(new_items.len());
            for index in 0..common {
                let segment = index.to_string();
                let child = node.child(&segment);
                let mark = push_segment(path, &segment);
                diff_nodes(&old_items[index], &new_items[index], child, path, out);
                path.truncate(mark);
            }
            for (index, item) in old_items.iter().enumerate().skip(common) {
                let mark = push_segment(path, &index.to_string());
                out.push(ValueChange {
                    pointer: path.clone(),
                    kind: ValueChangeKind::Removed(item.clone()),
                });
                path.truncate(mark);
            }
            for (index, item) in new_items.iter().enumerate().skip(common) {
                let mark = push_segment(path, &index.to_string());
                out.push(ValueChange {
                    pointer: path.clone(),
                    kind: ValueChangeKind::Added(item.clone()),
                });
                path.truncate(mark);
            }
        }
        _ if old == new => {}
        _ => out.push(ValueChange {
            pointer: path.clone(),
            kind: ValueChangeKind::Changed {
                old: old.clone(),
                new: new.clone(),
            },
        }),
    }
}

/// Set difference for `required`/`enum`: members only in `old` are `Removed`,
/// members only in `new` are `Added`, each addressed as `<path>/<member>`.
fn diff_sets(old: &Value, new: &Value, path: &mut String, out: &mut Vec<ValueChange>) {
    let old_items = old.as_array().map(Vec::as_slice).unwrap_or_default();
    let new_items = new.as_array().map(Vec::as_slice).unwrap_or_default();

    for item in old_items.iter().filter(|item| !new_items.contains(item)) {
        let mark = push_segment(path, &set_element_segment(item));
        out.push(ValueChange {
            pointer: path.clone(),
            kind: ValueChangeKind::Removed(item.clone()),
        });
        path.truncate(mark);
    }
    for item in new_items.iter().filter(|item| !old_items.contains(item)) {
        let mark = push_segment(path, &set_element_segment(item));
        out.push(ValueChange {
            pointer: path.clone(),
            kind: ValueChangeKind::Added(item.clone()),
        });
        path.truncate(mark);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::test_support::{added, changed, removed};
    use crate::diff::value_changes;
    use serde_json::json;

    // ── diff_values ──────────────────────────────────────────────────────

    #[test]
    fn diff_values_reports_object_keys_added_removed_changed() {
        let changes = value_changes(
            &json!({"a": 1, "b": {"c": "x"}, "d": true}),
            &json!({"a": 1, "b": {"c": "y"}, "e": null}),
        );
        assert_eq!(
            changes,
            vec![
                changed("/b/c", json!("x"), json!("y")),
                removed("/d", json!(true)),
                added("/e", Value::Null),
            ]
        );
    }

    #[test]
    fn diff_values_compares_generic_arrays_by_index() {
        let changes = value_changes(
            &json!({"allOf": [{"type": "object"}, {"type": "string"}]}),
            &json!({"allOf": [{"type": "object"}, {"type": "integer"}, {"format": "x"}]}),
        );
        assert_eq!(
            changes,
            vec![
                changed("/allOf/1/type", json!("string"), json!("integer")),
                added("/allOf/2", json!({"format": "x"})),
            ]
        );

        let shorter = value_changes(&json!({"oneOf": [1, 2]}), &json!({"oneOf": [1]}));
        assert_eq!(shorter, vec![removed("/oneOf/1", json!(2))]);
    }

    #[test]
    fn diff_values_compares_required_as_a_set() {
        let changes = value_changes(
            &json!({"required": ["a", "b"]}),
            &json!({"required": ["b", "c"]}),
        );
        assert_eq!(
            changes,
            vec![
                removed("/required/a", json!("a")),
                added("/required/c", json!("c")),
            ]
        );
    }

    #[test]
    fn diff_values_treats_missing_required_as_empty_set() {
        assert_eq!(
            value_changes(
                &json!({"type": "object"}),
                &json!({"type": "object", "required": ["a"]})
            ),
            vec![added("/required/a", json!("a"))]
        );
        assert_eq!(
            value_changes(&json!({"required": ["a"]}), &json!({})),
            vec![removed("/required/a", json!("a"))]
        );
    }

    #[test]
    fn diff_values_treats_missing_properties_as_empty_map() {
        assert_eq!(
            value_changes(
                &json!({"type": "object"}),
                &json!({"type": "object", "properties": {"name": {"type": "string"}}})
            ),
            vec![added("/properties/name", json!({"type": "string"}))]
        );
        assert_eq!(
            value_changes(
                &json!({"properties": {"a": {}, "b": {}}}),
                &json!({"type": "object"})
            ),
            vec![
                removed("/properties/a", json!({})),
                removed("/properties/b", json!({})),
                added("/type", json!("object")),
            ]
        );
    }

    #[test]
    fn diff_values_compares_enum_as_a_set_with_non_string_members() {
        let changes = value_changes(
            &json!({"properties": {"n": {"enum": [1, 2]}}}),
            &json!({"properties": {"n": {"enum": [2, 3]}}}),
        );
        assert_eq!(
            changes,
            vec![
                removed("/properties/n/enum/1", json!(1)),
                added("/properties/n/enum/3", json!(3)),
            ]
        );
    }

    #[test]
    fn diff_values_escapes_pointer_tokens() {
        let changes = value_changes(
            &json!({"properties": {"a/b": {"type": "string"}}}),
            &json!({"properties": {}}),
        );
        assert_eq!(
            changes,
            vec![removed("/properties/a~1b", json!({"type": "string"}))]
        );
    }

    #[test]
    fn diff_values_reports_nothing_for_equal_values() {
        let value =
            json!({"type": "object", "required": ["a"], "properties": {"a": {"type": "x"}}});
        assert!(value_changes(&value, &value).is_empty());
    }

    #[test]
    fn diff_values_keyword_semantics_resume_inside_a_field_named_like_a_keyword() {
        // Under `/properties/required` we are back in a schema, so *its*
        // `required` array is a set again.
        let changes = value_changes(
            &json!({"properties": {"required": {"type": "object", "required": ["a"]}}}),
            &json!({"properties": {"required": {"type": "object", "required": ["a", "b"]}}}),
        );
        assert_eq!(
            changes,
            vec![added("/properties/required/required/b", json!("b"))]
        );
        assert_eq!(locate(&changes[0].pointer), Location::RequiredElement);
    }

    #[test]
    fn diff_values_does_not_apply_set_semantics_outside_schema_positions() {
        // `default` is opaque: an array under it is compared by index even when
        // it is keyed `required` or `enum` further down.
        let changes = value_changes(
            &json!({"default": {"required": ["a", "b"]}}),
            &json!({"default": {"required": ["b", "a"]}}),
        );
        assert_eq!(
            changes,
            vec![
                changed("/default/required/0", json!("a"), json!("b")),
                changed("/default/required/1", json!("b"), json!("a")),
            ]
        );
    }

    // ── locate ───────────────────────────────────────────────────────────

    #[test]
    fn locate_distinguishes_keywords_from_property_names() {
        assert_eq!(locate("/type"), Location::Type);
        assert_eq!(locate("/properties/type"), Location::Property);
        assert_eq!(locate("/properties/type/type"), Location::Type);
        assert_eq!(locate("/items/type"), Location::Type);
        assert_eq!(locate("/allOf/0/type"), Location::Type);
        assert_eq!(locate("/allOf/0"), Location::Other);
        assert_eq!(locate("/required/x"), Location::RequiredElement);
        assert_eq!(locate("/properties/required"), Location::Property);
        assert_eq!(locate("/properties/s/enum/a"), Location::EnumElement);
        assert_eq!(
            locate("/additionalProperties"),
            Location::AdditionalProperties
        );
        assert_eq!(locate("/additionalProperties/type"), Location::Type);
        assert_eq!(locate("/nullable"), Location::Nullable);
        assert_eq!(locate("/format"), Location::Other);
        assert_eq!(locate("/default/type"), Location::Other);
        assert_eq!(locate(""), Location::Other);
    }
}
