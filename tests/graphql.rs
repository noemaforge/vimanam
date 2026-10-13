//! GraphQL adapter: real-schema fixtures, one test per compatibility rule,
//! SDL/introspection parity, multi-file merging and focused context.
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use vimanam::contract::graphql::{self, Document};
use vimanam::contract::{Change, ChangeKind, Severity};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/graphql")
        .join(name)
}
fn sdl(text: &str) -> Document {
    Document::from_sdl(&[("inline.graphql".into(), text.into())]).unwrap()
}
fn changes(old: &str, new: &str) -> Vec<Change> {
    graphql::diff(&sdl(old), &sdl(new)).unwrap()
}
fn coordinate(c: &Change) -> &str {
    match &c.native {
        vimanam::contract::NativeLocation::Graphql { coordinate } => coordinate,
        _ => panic!("non-GraphQL location"),
    }
}
/// Exactly one fact with this native rule; return it.
fn only(changes: &[Change], native_kind: &str) -> Change {
    let matching: Vec<_> = changes
        .iter()
        .filter(|c| c.native_kind == native_kind)
        .collect();
    assert_eq!(matching.len(), 1, "{native_kind} in {changes:#?}");
    matching[0].clone()
}
fn assert_rule(
    old: &str,
    new: &str,
    native_kind: &str,
    at: &str,
    kind: ChangeKind,
    severity: Severity,
) {
    let all = changes(old, new);
    let c = only(&all, native_kind);
    assert_eq!(coordinate(&c), at);
    assert_eq!(c.kind, kind);
    assert_eq!(c.severity, severity, "{native_kind}");
}
fn summary(changes: &[Change]) -> Vec<Value> {
    let mut rows: Vec<_> = changes
        .iter()
        .map(|c| {
            json!({"coordinate": coordinate(c), "kind": c.kind, "native_kind": c.native_kind, "severity": c.severity})
        })
        .collect();
    rows.sort_by_key(|r| (r["coordinate"].to_string(), r["native_kind"].to_string()));
    rows
}

const BASE: &str = r#"
type Query { user(id: ID!): User  users(first: Int = 10): [User!]! }
type User implements Node { id: ID!  name: String!  role: Role  bio: String }
interface Node { id: ID! }
enum Role { ADMIN USER }
input Filter { name: String  limit: Int = 5 }
union Search = User | Post
type Post { id: ID! }
"#;

// ----------------------------------------------------------------------------
// Real public schema at two versions

#[test]
fn real_schema_matches_checked_in_expected_change_list() {
    let expected: Value =
        serde_json::from_slice(&std::fs::read(fixture("spacex-land-expected.json")).unwrap())
            .unwrap();
    let old = Document::read(&fixture("spacex-land-64a7fa9.graphql")).unwrap();
    let new = Document::read(&fixture("spacex-land-0a482d1.graphql")).unwrap();
    let facts = graphql::diff(&old, &new).unwrap();
    assert_eq!(Value::Array(summary(&facts)), expected);
    // Introspection inputs of the same versions yield the same list and IDs.
    let old_i = Document::read(&fixture("spacex-land-64a7fa9.introspection.json")).unwrap();
    let new_i = Document::read(&fixture("spacex-land-0a482d1.introspection.json")).unwrap();
    assert_eq!(old_i.source, graphql::SourceKind::Introspection);
    let from_introspection = graphql::diff(&old_i, &new_i).unwrap();
    assert_eq!(from_introspection, facts);
}

#[test]
fn sdl_and_introspection_are_the_same_surface() {
    for version in ["64a7fa9", "0a482d1"] {
        let s = Document::read(&fixture(&format!("spacex-land-{version}.graphql"))).unwrap();
        let i = Document::read(&fixture(&format!(
            "spacex-land-{version}.introspection.json"
        )))
        .unwrap();
        assert!(graphql::diff(&s, &i).unwrap().is_empty(), "{version}");
        assert_eq!(graphql::adapt(&s), graphql::adapt(&i), "{version}");
    }
}

#[test]
fn v2_json_summary_and_has_breaking() {
    let old = Document::read(&fixture("spacex-land-64a7fa9.graphql")).unwrap();
    let new = Document::read(&fixture("spacex-land-0a482d1.graphql")).unwrap();
    let doc = vimanam::contract::json::to_json(
        graphql::adapt(&old),
        graphql::adapt(&new),
        graphql::diff(&old, &new).unwrap(),
        "a",
        "b",
        None,
    );
    let value = serde_json::to_value(&doc).unwrap();
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["old"]["contract"]["format"], "graphql");
    assert_eq!(value["summary"]["operations_added"], 4);
    assert_eq!(value["summary"]["breaking"], 1);
    assert_eq!(value["summary"]["non_breaking"], 8);
    // Result.count is reachable from the surviving launchesPastResult field
    // but is a type-level fact, so operations_changed stays zero.
    assert_eq!(value["summary"]["operations_changed"], 0);
    assert!(doc.has_breaking());
    let round: vimanam::contract::json::JsonDiff = serde_json::from_value(value).unwrap();
    assert_eq!(round.changes.len(), 9);
}

// ----------------------------------------------------------------------------
// Breaking rules, one test each

#[test]
fn breaking_removed_type() {
    assert_rule(
        BASE,
        &BASE
            .replace("type Post { id: ID! }", "")
            .replace("| Post", ""),
        "TYPE_REMOVED",
        "Post",
        ChangeKind::TypeRemoved,
        Severity::Breaking,
    );
}

#[test]
fn breaking_removed_field() {
    assert_rule(
        BASE,
        &BASE.replace("bio: String }", "}"),
        "FIELD_REMOVED",
        "User.bio",
        ChangeKind::FieldRemoved,
        Severity::Breaking,
    );
}

#[test]
fn breaking_removed_root_field_is_operation_removed() {
    assert_rule(
        BASE,
        &BASE.replace("users(first: Int = 10): [User!]! ", ""),
        "FIELD_REMOVED",
        "Query.users",
        ChangeKind::OperationRemoved,
        Severity::Breaking,
    );
}

#[test]
fn breaking_removed_argument() {
    assert_rule(
        BASE,
        &BASE.replace("users(first: Int = 10)", "users"),
        "ARG_REMOVED",
        "Query.users(first:)",
        ChangeKind::ArgumentRemoved,
        Severity::Breaking,
    );
}

#[test]
fn breaking_removed_input_field() {
    assert_rule(
        BASE,
        &BASE.replace("name: String  limit", "limit"),
        "INPUT_FIELD_REMOVED",
        "Filter.name",
        ChangeKind::FieldRemoved,
        Severity::Breaking,
    );
}

#[test]
fn breaking_removed_enum_value() {
    assert_rule(
        BASE,
        &BASE.replace("ADMIN USER", "USER"),
        "ENUM_VALUE_REMOVED",
        "Role.ADMIN",
        ChangeKind::EnumValueRemoved,
        Severity::Breaking,
    );
}

#[test]
fn breaking_output_field_made_nullable() {
    assert_rule(
        BASE,
        &BASE.replace("name: String!", "name: String"),
        "FIELD_MADE_NULLABLE",
        "User.name",
        ChangeKind::NullabilityChanged,
        Severity::Breaking,
    );
    // Inner list nullability is directional too.
    assert_rule(
        BASE,
        &BASE.replace("[User!]!", "[User]!"),
        "FIELD_MADE_NULLABLE",
        "Query.users",
        ChangeKind::NullabilityChanged,
        Severity::Breaking,
    );
}

#[test]
fn breaking_input_field_made_non_null_without_default() {
    assert_rule(
        BASE,
        &BASE.replace(
            "input Filter { name: String ",
            "input Filter { name: String! ",
        ),
        "INPUT_FIELD_MADE_NON_NULL",
        "Filter.name",
        ChangeKind::FieldRequiredChanged,
        Severity::Breaking,
    );
}

#[test]
fn breaking_argument_made_non_null_without_default() {
    assert_rule(
        BASE,
        &BASE.replace("users(first: Int = 10)", "users(first: Int!)"),
        "ARG_MADE_NON_NULL",
        "Query.users(first:)",
        ChangeKind::ArgumentRequiredChanged,
        Severity::Breaking,
    );
}

#[test]
fn breaking_added_required_argument() {
    assert_rule(
        BASE,
        &BASE.replace("user(id: ID!)", "user(id: ID!, org: ID!)"),
        "ARG_ADDED_REQUIRED",
        "Query.user(org:)",
        ChangeKind::ArgumentAdded,
        Severity::Breaking,
    );
}

#[test]
fn breaking_added_required_input_field() {
    assert_rule(
        BASE,
        &BASE.replace("limit: Int = 5 }", "limit: Int = 5  org: ID! }"),
        "INPUT_FIELD_ADDED_REQUIRED",
        "Filter.org",
        ChangeKind::FieldAdded,
        Severity::Breaking,
    );
}

#[test]
fn breaking_argument_type_changed() {
    assert_rule(
        BASE,
        &BASE.replace("user(id: ID!)", "user(id: String!)"),
        "ARG_TYPE_CHANGED",
        "Query.user(id:)",
        ChangeKind::ArgumentTypeChanged,
        Severity::Breaking,
    );
}

#[test]
fn breaking_input_field_type_changed() {
    assert_rule(
        BASE,
        &BASE.replace("limit: Int = 5", "limit: [Int] = 5"),
        "INPUT_FIELD_TYPE_CHANGED",
        "Filter.limit",
        ChangeKind::FieldTypeChanged,
        Severity::Breaking,
    );
}

#[test]
fn breaking_output_field_type_changed() {
    assert_rule(
        BASE,
        &BASE.replace("bio: String", "bio: Int"),
        "FIELD_TYPE_CHANGED",
        "User.bio",
        ChangeKind::FieldTypeChanged,
        Severity::Breaking,
    );
}

#[test]
fn breaking_type_removed_from_union() {
    assert_rule(
        BASE,
        &BASE.replace("User | Post", "User"),
        "UNION_MEMBER_REMOVED",
        "Search",
        ChangeKind::ConstraintChanged,
        Severity::Breaking,
    );
}

#[test]
fn breaking_interface_removed_from_object() {
    assert_rule(
        BASE,
        &BASE.replace("type User implements Node", "type User"),
        "INTERFACE_REMOVED",
        "User",
        ChangeKind::ConstraintChanged,
        Severity::Breaking,
    );
}

#[test]
fn breaking_changed_type_kind() {
    let all = changes(
        BASE,
        &BASE
            .replace("type Post { id: ID! }", "input Post { id: ID! }")
            .replace("| Post", ""),
    );
    let c = only(&all, "TYPE_KIND_CHANGED");
    assert_eq!(coordinate(&c), "Post");
    assert_eq!(c.kind, ChangeKind::TypeKindChanged);
    assert_eq!(c.severity, Severity::Breaking);
    assert_eq!(
        c.details,
        json!({"before": "OBJECT", "after": "INPUT_OBJECT"})
    );
}

#[test]
fn breaking_root_type_removed() {
    let old = format!("{BASE} type Mutation {{ ping: Boolean }}");
    let all = changes(&old, BASE);
    assert_eq!(only(&all, "ROOT_TYPE_REMOVED").severity, Severity::Breaking);
    let op = only(&all, "FIELD_REMOVED");
    assert_eq!(coordinate(&op), "Mutation.ping");
    assert_eq!(op.kind, ChangeKind::OperationRemoved);
}

// ----------------------------------------------------------------------------
// Non-breaking and review rules

#[test]
fn non_breaking_rules() {
    for (new, native, kind) in [
        (
            BASE.replace("user(id: ID!)", "user(id: ID!, org: ID)"),
            "ARG_ADDED_OPTIONAL",
            ChangeKind::ArgumentAdded,
        ),
        (
            BASE.replace("user(id: ID!)", "user(id: ID!, n: Int! = 1)"),
            "ARG_ADDED_OPTIONAL",
            ChangeKind::ArgumentAdded,
        ),
        (
            BASE.replace("bio: String }", "bio: String  age: Int }"),
            "FIELD_ADDED",
            ChangeKind::FieldAdded,
        ),
        (
            format!("{BASE} type Extra {{ x: Int }}"),
            "TYPE_ADDED",
            ChangeKind::TypeAdded,
        ),
        (
            BASE.replace("bio: String", "bio: String!"),
            "FIELD_MADE_NON_NULL",
            ChangeKind::NullabilityChanged,
        ),
        (
            BASE.replace("user(id: ID!)", "user(id: ID)"),
            "ARG_MADE_NULLABLE",
            ChangeKind::ArgumentRequiredChanged,
        ),
        (
            BASE.replace("users(first: Int = 10)", "users(first: Int! = 10)"),
            "ARG_MADE_NON_NULL_WITH_DEFAULT",
            ChangeKind::ArgumentRequiredChanged,
        ),
        (
            BASE.replace("limit: Int = 5 }", "limit: Int = 5  org: ID }"),
            "INPUT_FIELD_ADDED_OPTIONAL",
            ChangeKind::FieldAdded,
        ),
        (
            BASE.replace(
                "type User implements Node",
                "type User implements Node & Extra",
            )
            .replace(
                "interface Node",
                "interface Extra { id: ID! } interface Node",
            ),
            "INTERFACE_ADDED",
            ChangeKind::ConstraintChanged,
        ),
    ] {
        let all = changes(BASE, &new);
        let c = only(&all, native);
        assert_eq!(c.kind, kind, "{native}");
        assert_eq!(c.severity, Severity::NonBreaking, "{native}");
        assert!(
            all.iter().all(|c| c.severity != Severity::Breaking),
            "{native}: {all:#?}"
        );
    }
}

#[test]
fn review_enum_value_added_to_output_enum() {
    assert_rule(
        BASE,
        &BASE.replace("ADMIN USER", "ADMIN USER GUEST"),
        "ENUM_VALUE_ADDED_TO_OUTPUT",
        "Role.GUEST",
        ChangeKind::EnumValueAdded,
        Severity::Review,
    );
}

#[test]
fn input_only_enum_value_added_is_non_breaking() {
    let old = format!("{BASE} enum Sort {{ ASC }} input Order {{ sort: Sort }}");
    let new = old.replace("{ ASC }", "{ ASC DESC }");
    let c = only(&changes(&old, &new), "ENUM_VALUE_ADDED");
    assert_eq!(c.severity, Severity::NonBreaking);
}

#[test]
fn review_default_value_changed() {
    assert_rule(
        BASE,
        &BASE.replace("first: Int = 10", "first: Int = 20"),
        "ARG_DEFAULT_CHANGED",
        "Query.users(first:)",
        ChangeKind::DefaultChanged,
        Severity::Review,
    );
    assert_rule(
        BASE,
        &BASE.replace("limit: Int = 5", "limit: Int"),
        "INPUT_FIELD_DEFAULT_CHANGED",
        "Filter.limit",
        ChangeKind::DefaultChanged,
        Severity::Review,
    );
}

#[test]
fn review_union_member_added() {
    let new = format!(
        "{} type Comment {{ id: ID! }}",
        BASE.replace("User | Post", "User | Post | Comment")
    );
    let c = only(&changes(BASE, &new), "UNION_MEMBER_ADDED");
    assert_eq!(c.severity, Severity::Review);
}

#[test]
fn description_only_changes_are_not_reported() {
    let described = r#"
"The root" type Query { "Find" user("which" id: ID!): User  users(first: Int = 10): [User!]! }
"A person" type User implements Node { "id" id: ID!  name: String!  role: Role  bio: String }
interface Node { id: ID! }
enum Role { "admin" ADMIN USER }
input Filter { "n" name: String  limit: Int = 5 }
union Search = User | Post
type Post { id: ID! }
"#;
    assert!(changes(BASE, described).is_empty());
}

#[test]
fn deprecation_is_an_advisory_fact() {
    let all = changes(
        BASE,
        &BASE.replace("bio: String", "bio: String @deprecated"),
    );
    let c = only(&all, "DEPRECATION_ADDED");
    assert_eq!(c.kind, ChangeKind::DeprecatedChanged);
    assert_eq!(c.severity, Severity::NonBreaking);
    assert_eq!(
        c.details["after"]["deprecation_reason"],
        "No longer supported"
    );
    let reason = changes(
        &BASE.replace("bio: String", "bio: String @deprecated"),
        &BASE.replace(
            "bio: String",
            r#"bio: String @deprecated(reason: "Use about")"#,
        ),
    );
    assert_eq!(
        only(&reason, "DEPRECATION_REASON_CHANGED").severity,
        Severity::NonBreaking
    );
}

// ----------------------------------------------------------------------------
// Relay pagination

const CONNECTION: &str = r#"
type PageInfo { hasNextPage: Boolean! endCursor: String }
type UserEdge { node: User cursor: String! }
type UserConnection { edges: [UserEdge] pageInfo: PageInfo! }
"#;

#[test]
fn list_becoming_relay_connection_is_breaking_pagination_change() {
    let old = format!("{BASE}{CONNECTION}");
    let new = old.replace(
        "users(first: Int = 10): [User!]!",
        "users(first: Int = 10, after: String): UserConnection!",
    );
    let all = changes(&old, &new);
    let p = only(&all, "RELAY_CONNECTION_ADOPTED");
    assert_eq!(coordinate(&p), "Query.users");
    assert_eq!(p.kind, ChangeKind::PaginationChanged);
    assert_eq!(p.severity, Severity::Breaking);
    assert_eq!(p.details["before"]["shape"], "list");
    assert_eq!(p.details["after"]["shape"], "connection");
    assert_eq!(p.details["after"]["item"], "User");
    // The underlying type fact is reported separately.
    assert_eq!(
        only(&all, "FIELD_TYPE_CHANGED").severity,
        Severity::Breaking
    );
    // And the reverse.
    assert_eq!(
        only(&changes(&new, &old), "RELAY_CONNECTION_REMOVED").severity,
        Severity::Breaking
    );
}

#[test]
fn connection_default_change_is_review_pagination_change() {
    let old = format!("{BASE}{CONNECTION}").replace(
        "users(first: Int = 10): [User!]!",
        "users(first: Int = 10, after: String): UserConnection!",
    );
    let new = old.replace("first: Int = 10", "first: Int = 50");
    let all = changes(&old, &new);
    let p = only(&all, "RELAY_CONNECTION_CONTROLS_CHANGED");
    assert_eq!(p.severity, Severity::Review);
    assert_eq!(p.details["before"]["controls"]["first"]["default"], "10");
    assert_eq!(p.details["after"]["controls"]["first"]["default"], "50");
    assert_eq!(only(&all, "ARG_DEFAULT_CHANGED").severity, Severity::Review);
}

#[test]
fn connection_shape_without_first_after_is_not_pagination() {
    let old = format!("{BASE}{CONNECTION}");
    let new = old.replace(
        "users(first: Int = 10): [User!]!",
        "users(first: Int = 10): UserConnection!",
    );
    assert!(
        changes(&old, &new)
            .iter()
            .all(|c| c.kind != ChangeKind::PaginationChanged)
    );
}

// ----------------------------------------------------------------------------
// Input handling

#[test]
fn multi_file_sdl_merges_extensions_in_any_order() {
    let merged = Document::from_sdl(&[
        (
            "a.graphql".into(),
            "extend type Query { posts: [Post!]! } extend enum Role { GUEST }".into(),
        ),
        ("b.graphql".into(), BASE.into()),
    ])
    .unwrap();
    let single = sdl(&BASE
        .replace("[User!]! }", "[User!]!  posts: [Post!]! }")
        .replace("ADMIN USER", "ADMIN USER GUEST"));
    assert!(graphql::diff(&single, &merged).unwrap().is_empty());
    assert_eq!(graphql::adapt(&single), graphql::adapt(&merged));
}

#[test]
fn merge_errors_name_the_files() {
    let duplicate = Document::from_sdl(&[
        ("a.graphql".into(), BASE.into()),
        ("b.graphql".into(), "type Post { id: ID! }".into()),
    ])
    .unwrap_err()
    .to_string();
    assert!(
        duplicate.contains("Duplicate GraphQL type Post in b.graphql and a.graphql"),
        "{duplicate}"
    );
    let unknown = Document::from_sdl(&[("a.graphql".into(), "type Query { x: Missing }".into())])
        .unwrap_err()
        .to_string();
    assert!(
        unknown.contains("Unknown GraphQL type Missing referenced by Query.x"),
        "{unknown}"
    );
    let dangling = Document::from_sdl(&[(
        "a.graphql".into(),
        "type Query { x: Int } extend type Nope { y: Int }".into(),
    )])
    .unwrap_err()
    .to_string();
    assert!(
        dangling.contains("extends undefined type Nope"),
        "{dangling}"
    );
    let field = Document::from_sdl(&[(
        "a.graphql".into(),
        "type Query { x: Int } extend type Query { x: Int }".into(),
    )])
    .unwrap_err()
    .to_string();
    assert!(field.contains("Duplicate GraphQL field Query.x"), "{field}");
}

#[test]
fn custom_schema_roots_are_respected() {
    let doc = sdl("schema { query: Root } type Root { a: Int } type Query { b: Int }");
    assert_eq!(doc.roots.get("query").map(String::as_str), Some("Root"));
    let docs = graphql::documentation(&doc);
    assert_eq!(docs.endpoints.len(), 1);
    assert_eq!(docs.endpoints[0].path, "Root.a");
    assert!(docs.schemas.contains_key("Query"));
}

#[test]
fn introspection_defaults_compare_canonically_with_sdl() {
    let s = sdl(
        r#"type Query { f(o: In = {b: 2, a: "x"}, l: [Int] = [1, 2], e: Role = A): Int } input In { a: String b: Int } enum Role { A }"#,
    );
    let intro = json!({"__schema": {
    "queryType": {"name": "Query"}, "mutationType": null, "subscriptionType": null,
    "types": [
        {"kind": "OBJECT", "name": "Query", "fields": [{"name": "f", "args": [
            {"name": "o", "type": {"kind": "INPUT_OBJECT", "name": "In"}, "defaultValue": "{a: \"x\", b: 2}"},
            {"name": "l", "type": {"kind": "LIST", "ofType": {"kind": "SCALAR", "name": "Int"}}, "defaultValue": "[1,2]"},
            {"name": "e", "type": {"kind": "ENUM", "name": "Role"}, "defaultValue": "A"}
        ], "type": {"kind": "SCALAR", "name": "Int"}, "isDeprecated": false}], "interfaces": []},
        {"kind": "INPUT_OBJECT", "name": "In", "inputFields": [
            {"name": "a", "type": {"kind": "SCALAR", "name": "String"}, "defaultValue": null},
            {"name": "b", "type": {"kind": "SCALAR", "name": "Int"}, "defaultValue": null}
        ]},
        {"kind": "ENUM", "name": "Role", "enumValues": [{"name": "A", "isDeprecated": false}]},
        {"kind": "SCALAR", "name": "Int"}, {"kind": "SCALAR", "name": "String"},
        {"kind": "OBJECT", "name": "__Type", "fields": []}
    ]}});
    let i =
        Document::from_introspection(serde_json::to_string(&intro).unwrap().as_bytes()).unwrap();
    assert!(
        graphql::diff(&s, &i).unwrap().is_empty(),
        "{:#?}",
        graphql::diff(&s, &i)
    );
}

#[test]
fn ids_are_stable_and_independent_of_order_and_descriptions() {
    let a = graphql::adapt(&sdl(BASE));
    let reordered = sdl(&format!(
        "{}\n{}",
        "\"x\" type Post { id: ID! }",
        BASE.replace("type Post { id: ID! }", "")
    ));
    let b = graphql::adapt(&reordered);
    let ids = |c: &vimanam::contract::Contract| {
        let mut v: Vec<_> = c.entities.iter().map(|e| e.id.clone()).collect();
        v.sort();
        v
    };
    assert_eq!(ids(&a), ids(&b));
    let first = changes(BASE, &BASE.replace("bio: String }", "}"));
    let second = changes(BASE, &BASE.replace("bio: String }", "}"));
    assert_eq!(first[0].id, second[0].id);
    assert!(first[0].id.starts_with("vc2_"));
}

// ----------------------------------------------------------------------------
// Focused context

#[test]
fn focused_context_renders_changed_root_fields_with_reachable_types_within_budget() {
    let old = Document::read(&fixture("spacex-land-64a7fa9.graphql")).unwrap();
    let new = Document::read(&fixture("spacex-land-0a482d1.graphql")).unwrap();
    let facts = graphql::diff(&old, &new).unwrap();
    let affected = graphql::affected_operations(&new, &facts);
    // Added root fields plus the surviving field reaching the changed Result type.
    assert_eq!(
        affected.into_iter().collect::<Vec<_>>(),
        [
            "Query.historiesResult",
            "Query.launchesPastResult",
            "Query.missionsResult",
            "Query.rocketsResult",
            "Query.shipsResult",
        ]
    );
    let mut unbounded = Vec::new();
    graphql::write_focused_context(&mut unbounded, &new, &facts, None, &mut |_| {}).unwrap();
    let text = String::from_utf8(unbounded.clone()).unwrap();
    assert!(text.contains("### Query.historiesResult"));
    assert!(text.contains("#### Arguments"));
    assert!(text.contains("#### Result"));
    assert!(text.contains("HistoriesResult"), "reachable result type");
    assert!(
        text.contains("totalCount"),
        "transitively reachable Result type"
    );
    assert!(
        !text.contains("### Query.rockets "),
        "unaffected root field"
    );
    assert!(!text.contains("### Query.launches\n"));
    for budget in [400usize, 1200] {
        let mut out = Vec::new();
        let mut notices = vec![];
        graphql::write_focused_context(&mut out, &new, &facts, Some(budget), &mut |n| {
            notices.push(n.to_owned())
        })
        .unwrap();
        assert!(
            notices.iter().any(|n| n.contains("reduced to --detail")),
            "{notices:?}"
        );
        assert!(
            vimanam::markdown::estimate_tokens(&out) <= budget,
            "budget {budget}: {}",
            vimanam::markdown::estimate_tokens(&out)
        );
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("Query.historiesResult")
        );
    }
    assert!(vimanam::markdown::estimate_tokens(&unbounded) > 1200);
}

#[test]
fn focused_context_without_affected_fields_says_so() {
    let mut out = Vec::new();
    let doc = sdl(BASE);
    graphql::write_focused_context(&mut out, &doc, &[], Some(100), &mut |_| {}).unwrap();
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("No surviving root fields")
    );
}

#[test]
fn focused_context_reports_budget_overage() {
    let old = Document::read(&fixture("spacex-land-64a7fa9.graphql")).unwrap();
    let new = Document::read(&fixture("spacex-land-0a482d1.graphql")).unwrap();
    let facts = graphql::diff(&old, &new).unwrap();
    let mut notices = vec![];
    let mut out = Vec::new();
    graphql::write_focused_context(&mut out, &new, &facts, Some(5), &mut |n| {
        notices.push(n.to_owned())
    })
    .unwrap();
    assert!(
        notices.iter().any(|n| n.contains("5-token budget")),
        "{notices:?}"
    );
}

#[test]
fn extend_schema_is_rejected_explicitly() {
    let err = Document::from_sdl(&[(
        "a.graphql".into(),
        "type Query { a: Int } type M { b: Int }\nextend schema { mutation: M }".into(),
    )])
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("Unsupported `extend schema` in a.graphql"),
        "{err}"
    );
    // The words inside descriptions, strings and comments are not schema extensions.
    sdl(r#""extend schema" type Query { a(x: String = "extend schema"): Int } # extend schema"#);
}

#[test]
fn openapi_json_mentioning_schema_key_is_not_introspection() {
    let openapi = br#"{"openapi":"3.0.0","info":{"title":"t","version":"1"},"paths":{},"components":{"schemas":{"__schema":{"type":"object"}}}}"#;
    assert!(!graphql::is_introspection_bytes(openapi));
    assert!(graphql::is_introspection_bytes(
        br#"{"data":{"__schema":{"types":[]}}}"#
    ));
    assert!(graphql::is_introspection_bytes(
        br#"{"__schema":{"types":[]}}"#
    ));
}

#[test]
fn subscription_roots_are_operations() {
    let old = format!("{BASE} type Subscription {{ userChanged(id: ID!): User }}");
    let new = old.replace(
        "userChanged(id: ID!)",
        "userChanged(id: ID!, all: Boolean!)",
    );
    let c = only(&changes(&old, &new), "ARG_ADDED_REQUIRED");
    assert_eq!(coordinate(&c), "Subscription.userChanged(all:)");
    let docs = graphql::documentation(&sdl(&old));
    assert!(
        docs.endpoints
            .iter()
            .any(|e| e.method == "SUBSCRIPTION" && e.path == "Subscription.userChanged")
    );
}
