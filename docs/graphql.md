# GraphQL contracts

Vimanam reads GraphQL schemas as:

- **SDL**: a `.graphql` or `.graphqls` file, or a directory. Every SDL file
  below a directory (recursively, sorted by path, symlinks not followed) is
  merged; `extend type`/`extend enum`/… may appear in any file. Duplicate type
  definitions, duplicate members from extensions, extensions of undefined
  types and references to undefined types are errors naming the file.
- **Introspection JSON**: the result of an introspection query, either
  `{"data": {"__schema": …}}` or `{"__schema": …}`. A `.json` input is GraphQL
  only when it contains a `__schema` object, so OpenAPI JSON is unaffected.

Built-in scalars and `__` introspection types are excluded from both sources,
deprecation without a reason uses the specification default
(`No longer supported`), and defaults are printed as canonical GraphQL
literals, so SDL and introspection of the same schema produce the same surface
and no changes. Without a `schema { … }` definition the roots are `Query`,
`Mutation` and `Subscription` when defined. Directive definitions and applied
directives other than `@deprecated` are not modeled. `extend schema` is
rejected with an explicit error (the parser cannot read it); declare root types
in one `schema { … }` definition.

Capture introspection with input-value deprecation enabled (graphql-js
`inputValueDeprecation: true`, or `args(includeDeprecated: true)` and
`inputFields(includeDeprecated: true)` in the query). Without it, servers omit
deprecated arguments and input fields, which then compare as removed against
SDL or a fuller capture.

```sh
vimanam schema.graphql --detail full --include-schemas
vimanam schema/ --operation 'QUERY Query.user' --max-tokens 1600
vimanam introspection.json --schema User
vimanam schema.graphql --output-mode skill --detail full --include-schemas -o context/
vimanam diff old.graphql new.graphql --report --fail-on-breaking
vimanam diff old.graphql new.graphql --format json
vimanam diff old.graphql new.graphql --context --max-tokens 2000
vimanam diff --from-ref v1 --to-ref v2 --spec api/schema.graphql
```

## Mapping

| GraphQL | Neutral |
|---|---|
| `Query` / `Mutation` / `Subscription` root type | service, plus the root named type |
| Root field `Query.user` | operation (`services` = its root) |
| Object, interface, input, enum, union, scalar | named type (`attributes.kind`, sorted `interfaces`/`members`) |
| Field `User.name`, input field `UserInput.name` | field |
| Argument `Query.user(id:)` | argument (parent = its field/operation) |
| Enum value `Role.ADMIN` | value |
| `@deprecated(reason:)` | `deprecated` / `deprecation_reason` attributes; `deprecated_changed` |

Entity IDs hash the schema coordinate (`ve2_`), so descriptions and declaration
order never change them. Native locations carry the coordinate. A root type
renamed through `schema { query: … }` changes its operation coordinates and is
reported as removal plus addition.

## Rules

Impacts are `source`. Native rule names appear in `native_kind`.

| Change | Severity | Native kind |
|---|---|---|
| Type removed | breaking | `TYPE_REMOVED` |
| Type kind changed | breaking | `TYPE_KIND_CHANGED` |
| Root type removed | breaking | `ROOT_TYPE_REMOVED` |
| Field / root field removed | breaking | `FIELD_REMOVED` |
| Argument removed | breaking | `ARG_REMOVED` |
| Input field removed | breaking | `INPUT_FIELD_REMOVED` |
| Enum value removed | breaking | `ENUM_VALUE_REMOVED` |
| Output field made nullable (any wrapper) | breaking | `FIELD_MADE_NULLABLE` |
| Output field type changed | breaking | `FIELD_TYPE_CHANGED` |
| Argument / input field made non-null without a default | breaking | `ARG_MADE_NON_NULL`, `INPUT_FIELD_MADE_NON_NULL` |
| Argument / input field type changed | breaking | `ARG_TYPE_CHANGED`, `INPUT_FIELD_TYPE_CHANGED` |
| Required argument / input field added | breaking | `ARG_ADDED_REQUIRED`, `INPUT_FIELD_ADDED_REQUIRED` |
| Type removed from a union | breaking | `UNION_MEMBER_REMOVED` |
| Interface removed from an object/interface | breaking | `INTERFACE_REMOVED` |
| List field became a Relay connection, or back | breaking | `RELAY_CONNECTION_ADOPTED`, `RELAY_CONNECTION_REMOVED` |
| Type, field, root type or optional argument/input field added | non-breaking | `TYPE_ADDED`, `FIELD_ADDED`, `ROOT_TYPE_ADDED`, `*_ADDED_OPTIONAL` |
| Output field made non-null | non-breaking | `FIELD_MADE_NON_NULL` |
| Input made nullable, or non-null with a default | non-breaking | `*_MADE_NULLABLE`, `*_MADE_NON_NULL_WITH_DEFAULT` |
| Interface added | non-breaking | `INTERFACE_ADDED` |
| Enum value added to an input-only enum | non-breaking | `ENUM_VALUE_ADDED` |
| Deprecation added, removed or reason changed | non-breaking | `DEPRECATION_*` |
| Enum value added to an enum used by an output field | review | `ENUM_VALUE_ADDED_TO_OUTPUT` |
| Type added to a union | review | `UNION_MEMBER_ADDED` |
| Default value changed | review | `ARG_DEFAULT_CHANGED`, `INPUT_FIELD_DEFAULT_CHANGED` |
| Relay connection controls changed | review | `RELAY_CONNECTION_CONTROLS_CHANGED` |

Description-only changes are not reported. A removed type does not also list
its members; a removed root type also lists its removed operations. Type
changes are compared wrapper by wrapper: output positions may only become
stricter, input positions may only become looser.

## Pagination

A field is a Relay connection when its named result type (object or
interface) has `pageInfo` and `edges`, the edge type has `node`, and the field
accepts `first` and `after`. Evidence records the connection and node types
and the `first`/`after`/`last`/`before` argument types and defaults; a list
result records `shape: list`. A list field becoming a connection (or the
reverse) is a breaking `pagination_changed` fact with `confidence: declared`,
beside the underlying `field_type_changed` fact. A change of connection
control types, defaults or presence is `review`. GraphQL has no standard
declaration of page-size limits, so limits are not inferred.

## Output

`diff` Markdown groups facts by verdict, sorted by coordinate; `--report` adds
entity, type and root-field counts. `--format json` is contract v2;
`--contract 1` and `--breaking wire|source` are rejected before output is
created. `--fail-on-breaking` exits 3 when any breaking fact exists.

`--context` (Markdown only) appends focused context for the new schema's root
fields affected by the changes: a fact on a root field or its arguments, or on
any type reachable from its arguments or result. They are rendered at full
detail with schemas, and `--max-tokens N` lowers detail until the context fits.
Removed root fields have no new-schema context.

Conversion renders root fields as operations with `QUERY`, `MUTATION` or
`SUBSCRIPTION` and the coordinate (`--operation 'QUERY Query.user'` or
`--operation-id Query.user`), an *Arguments* table and a *Result*. Non-root
types are schemas (`--schema User`). Budgets, schema depth, split and Skill
trees reuse the shared renderer.

## Git refs

`--from-ref`/`--to-ref` use the committed bytes of a single SDL or
introspection file selected with `--spec` (or `--from-spec`/`--to-spec`);
automatic candidate discovery remains OpenAPI-only. Directory merging is
available for file inputs only.

## Not covered

Federation directives, schema stitching and executable documents (client
operations) are out of scope; client impact is the caller's job, as it is for
OpenAPI.
