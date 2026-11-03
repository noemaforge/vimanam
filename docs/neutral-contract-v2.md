# Neutral contract and JSON v2

`diff --format json` defaults to contract 2. `--contract 1` selects the unchanged OpenAPI v1 serializer and `vc1_` identity construction; `--contract 2` makes the default explicit. Markdown remains the existing OpenAPI report. A v2 JSON exit gate uses v2 adapter verdicts; a v1 gate uses v1 verdicts. The package version is independent of the JSON schema version. These additions preserve `parse_openapi`, `diff::diff`, and `diff::json::to_json`.

## Surface

`contract::Contract` holds a source format, title, version and ordered `entities`. Each entity has a category, name, stable ID, parent, service memberships, references, native location and adapter attributes. A flat node forest with explicit edges allows shared and cyclic named types without copies. Operations may belong to several services. Arguments/results belong to operations; fields and enum values belong to their schema root or enclosing field. A reusable type is a root; inline schemas expose member nodes under the argument/result containing them. Native schema attributes retain constraints, defaults, deprecation, composition and `$ref` identity. `references` contains stable named type IDs, including unresolved local targets; the absence of a target is inspectable rather than silently discarded.

Entity IDs are `ve2_` plus SHA-256 of canonical JSON `{v:2, format, kind, coordinate}`. Canonical JSON sorts object keys recursively, leaves arrays ordered, and uses compact serde_json encoding. Coordinates are exact case-sensitive native identities, scoped by their container. OpenAPI uses escaped `/services/NAME`, `/paths/PATH/method`, `/types/NAME`, and child argument/result/property coordinates. HTTP operation identity remains method plus exact path, excluding Swagger `basePath`; changing operationId changes its name, not identity. OpenAPI 2 definitions and OpenAPI 3 components share `/types/NAME`. Parameter coordinates include location and name. Enum coordinates use canonical JSON of the actual value, so null and the string "null" differ. Composition branches use their array index. Descriptions, versions and unrelated declaration order do not enter entity IDs. A rename of an entity's identity is remove/add unless its adapter explicitly identifies it as a rename or move.

### OpenAPI operation moves

The CLI (Markdown and v2 JSON) pairs OpenAPI operations that moved path; `--exact-paths`, `--format json --contract 1` and the library defaults (`openapi::diff`, `json::openapi`, `diff::diff`) keep exact matching, and `diff_with`/`openapi_with` take `diff::EndpointMatching::Moves`. After exact `(method, path)` matching, leftover removed and added operations of the same method pair by `operationId`, then by path equality after replacing whole `v<digits>` segments (`/api/v1/items` ↔ `/api/v2/items`, not `/v1beta`). A key qualifies only when it is unique across the whole old document and the whole new document (not merely among the leftovers) and both holders are leftovers, so pairing is one-to-one. Ambiguity keeps `operation_removed`/`operation_added`: old `/v1/items` + `/v2/items` against new `/v2/items` + `/v3/items` is an exact `/v2/items` match plus a removal and an addition, never a v1 → v3 move.

A pair emits one `operation_moved` fact (native kind `endpoint_moved`, breaking, since callers of the old path break). Its subject is the operation entity at the **new** coordinate; details hold `old_method`, `old_path`, `matched_by` (`operation_id` or `version_segment`), `old_operation_id`/`new_operation_id` only when they differ, and `from_operation`, the old operation's entity ID. Inner facts compare the pair as one operation and take their subjects and native locations from the side that holds them, as in exact mode: removals (`argument_removed`, `result_removed`, `field_removed`, `enum_value_removed`) use old-side subjects and native locations, which resolve in `old.contract.entities`; additions and changed facts (type, requiredness, constraint, default, deprecation, pagination) use new-side ones, which resolve in `new.contract.entities` and are the identities a later comparison from the new spec reuses. IDs are hashed from these subjects and details as usual, so they are deterministic. `operations_changed` counts the moved operation once; it is neither added nor removed. The surfaces in `old`/`new` stay unaligned.

Changes carry `subject` (an entity ID), a neutral `kind`, `severity`, compatibility `impacts`, `native`, `native_kind`, and lossless `details`. A schema change may attribute to the containing schema root; its pointer identifies the member. Native OpenAPI coordinates distinguish declared surface pointers (`resolved:false`) from pointers into ref-expanded normalized schemas (`resolved:true`). The latter are not input-file pointers. GraphQL locations carry schema coordinates; protobuf locations carry fully qualified names and optional numbers. Format-specific information remains in attributes and details rather than contaminating neutral categories.

Change IDs are `vc2_` plus SHA-256 of canonical `{v:2, subject, kind, details}`. Severity, impact dimensions, generator version, raw-file hashes and native display attribution are excluded. Details are factual, not prose. Composite pagination evidence is part of the fact. Identity construction is versioned independently from a package release. Exact v1 output and IDs remain available through the original serializer.

## Mapping designed for three adapters

| Surface | OpenAPI (implemented) | GraphQL (implemented) | Protobuf (implemented) |
|---|---|---|---|
| Service | Declared/inferred tag; many-to-many operation membership | Query, Mutation, Subscription root | Fully qualified service |
| Operation | METHOD + exact path | Root schema field coordinate | Fully qualified service RPC |
| Argument | Parameter location/name, synthetic request body | Field argument `Type.field(arg:)` | Request message reference; its members are input fields |
| Result | Response status and first schema | Root return type/reference | Output message/reference |
| Named type | Components/definitions schema | Object, interface, input, enum, union, scalar | Message, enum, nested type |
| Field | Property; nested items/composition/additionalProperties member | Object/interface/input field | Field or oneof member |
| Value | Canonical JSON enum member | Enum coordinate | Enum fully qualified name plus native number |
| Reference | `$ref` to a local named type | Named type and list/non-null wrapper attributes | Message/enum FQN, recursive references |
| Native attributes | Full schema grammar, operationId, requiredness | Type kind, wrappers, interfaces, union members, argument defaults | Syntax/edition, number, JSON name, oneof, reserved numbers, streaming mode |

GraphQL service and type roots may refer to the same named root type through an edge. Custom scalar/directive semantics remain native. Protobuf field coordinates use containing FQN and field name for source identity; the number is retained separately as wire identity. A field rename at the same number can be recognized by that adapter and produce source change plus wire evidence without misclassifying a safe wire rename. Negative enum numbers fit the signed native number. Oneof membership, reserved ranges and RPC streaming flags fit attributes; imported definitions are named roots with references. No HTTP status or parameter location is required for either adapter. Neither parser ships in #124.

| Neutral fact | OpenAPI classification | GraphQL design | Protobuf design |
|---|---|---|---|
| Operation removal | Breaking, including deprecated operations | Breaking | Source breaking |
| Argument added/required/type | Existing request-direction rules | Required with no default or changed input type: breaking; optional: non-breaking | Request fields classified by wire/source rules |
| Field type/required/nullability | Existing directional operation schema rules | Output nullable: breaking; output non-null: non-breaking; input non-null without default: breaking | Wire type compatibility, oneof and edition presence rules |
| Enum value added/removed | Existing directional operation rules | Output addition: review; removal: breaking | New number: non-breaking; number reused: wire breaking; removal: source breaking |
| Default/deprecation | Defaults: existing schema rule; named deprecation: review | Default: review; deprecation: adapter-specific advisory | Native option and generated source impacts |
| Wire identity | Parameter location change: breaking | Native coordinate changes use source identity | Field/enum number reuse, JSON name change, package/streaming changes retain native rule and `wire`/`source`/`wire_json` impacts |

`Adapter::attribute_kind` maps native attribute differences; `Adapter::classify` owns verdict and impact dimensions. `contract::compare` compares adapter-created surfaces and delegates both decisions, with structured absence-versus-null facts. It refuses cross-format comparisons. Adapters can add higher-order facts and correspondence/rename detection. `contract::openapi::diff` is the complete initial adapter: it reuses the mature endpoint differ's schema resolution and directional severity table, and adds independent named-type/service facts and pagination. `OpenApiAdapter` provides conservative classification for the generic surface comparator; use `openapi::diff` for the complete OpenAPI policy.

## Shared and unused types

All named types are present, including unused types. An added type is non-breaking; a removed or changed type is review, because a declaration alone does not establish input/output direction or a client dependency. A shared schema can be both input and output, with opposite nullability or enum verdicts. V2 therefore retains one operation-specific fact per affected use, with the existing directional verdict, plus independent type facts with review severity. Type and use records are intentional separate facts and have distinct IDs. Counts include both. `operations_changed` counts surviving operation coordinates once and excludes independent type/service changes. `--fail-on-breaking` agrees with the actual v2 `summary.breaking` count.

## Pagination inference

OpenAPI pagination is inferred and always `review`. No schema hint alone receives a breaking pagination verdict. Underlying required-argument/schema facts may independently break. For a surviving operation (exact or moved), compare old/new evidence:

1. At least one query control named exactly `page`, `size`, `pageSize`, `page_size`, `pageToken`, `page_token`, `cursor`, `limit`, or `offset`. `cursor`, `pageToken` and `page_token` are token controls.
2. At least one 2xx response whose first ref-resolved schema is an object with a direct array property and a continuation field, either a direct property or one object level down (`response_metadata/next_cursor`, `links/next`; refs resolved). Names compare case-insensitively ignoring `_`, `-`, `@` and `.`:
   - specific names qualify alone: `nextPageToken`, `nextPage`, `nextCursor`, `nextPageCursor`, `nextToken`, `nextMarker`, `nextLink`, `nextUrl`, `nextPageUrl`, `nextPageLink`, `afterCursor`, `continuationToken`, `continuation`, `LastEvaluatedKey`, `@odata.nextLink` (and their snake/kebab spellings);
   - generic names `cursor`, `next`, `after`, `marker` qualify only as an opaque string (not `date`/`date-time`/`time`) corroborated by a token control or an end flag.
3. End-of-results flags are boolean fields at the same two levels named `has_more`, `has_next`, `has_next_page`, `has_more_results`, `more_results`, `end_of_stream`, `end_of_results`, `is_last`, `is_last_page`, `last_page`, `complete`, `is_complete`, `done`, `drained`, `truncated`, `is_truncated` (any spelling). They are supporting evidence only; a flag without a continuation establishes nothing.
4. Evidence appears, disappears, or changes: request control schema/default/limit, continuation paths, end flags or matching response status. Evidence is sorted; descriptive annotations and control order do not create pagination changes.

Evidence per response is `{status, continuation}` plus `generic_continuation` and `end_flags` only when present. Paths are the property name at the top level and `/`-joined pointer tokens one level down. Details carry `confidence` and `evidence_rule`, computed from the side that has evidence (the new side when both do).

**2.0.0 stability.** When a side satisfies the 2.0.0 rule (a top-level property whose `type` is the string `array`, plus a top-level property named exactly `nextPageToken`, `next_page_token`, `nextPage`, `next_page`, `nextCursor` or `next_cursor`), that side's evidence is exactly the 2.0.0 evidence: only those names, only the responses that rule matched, no `generic_continuation` or `end_flags`, `confidence: inferred` and `evidence_rule: list+request_controls+continuation`. End flags, nested and other names on such a response are ignored. The broader rules and the table below apply to a side only when none of its responses satisfies the 2.0.0 rule.

A pagination fact 2.0.0 produced keeps its details and `vc2_` ID when the 2.0.0 rule matches both sides, or matches one side and the broader rules find no evidence on the other (2.0.0's `null` side stays `null`). When the other side has evidence that only the broader rules see, that evidence replaces 2.0.0's `null`, so the fact and its ID change: `nextPageToken` renamed to `next_token`, or moved to `meta/nextPageToken`, was a removal in 2.0.0 and is now a continuation change. Suppression lists keyed on 2.0.0 `vc2_` IDs can fire again in that case. Facts where neither side matches the 2.0.0 rule are new detections 2.0.0 did not emit.

Rules for a side without 2.0.0 evidence:

| Evidence | `confidence` | `evidence_rule` |
|---|---|---|
| specific name | `inferred` | `list+request_controls+continuation` |
| specific name + end flag | `corroborated` | `…+continuation+end_flag` |
| generic name + token control | `inferred` | `…+generic_continuation+token_control` |
| generic name + end flag | `inferred` | `…+generic_continuation+end_flag` |
| generic name + token control + end flag | `corroborated` | `…+generic_continuation+token_control+end_flag` |

The confidence scale is `declared` (the contract language declares paging, as GraphQL connections do) > `corroborated` > `inferred`; OpenAPI never reaches `declared`. Response headers (`Link`), prose descriptions, isolated controls, continuations nested two levels down and bare arrays do not establish pagination. `before`/`after` show complete evidence, with null meaning no qualifying inference. This deliberately misses unusual conventions instead of making a certainty claim. Protobuf inference requires page_size/page_token input and next_page_token output with repeated result items. Adapters must justify any breaking verdict using explicit semantic evidence.

## JSON and public use

The v2 envelope has `schema_version`, `generator`, `old`, `new`, `summary`, `changes`, and optional `deltas`. Each side contains raw `file_sha256` and the full `contract` surface. Hashes refer to exact parsed bytes. Deltas retain the OpenAPI hygiene/render profile. The v2 schema round-trips through serde; no CLI feature is required.

```rust
let old = vimanam::parse_openapi("old.json")?;
let new = vimanam::parse_openapi("new.json")?;
let surface = vimanam::contract::openapi::adapt(&new);
let changes = vimanam::contract::openapi::diff(&old, &new)?;
let json = vimanam::contract::json::openapi(&old, &new, None, "old-hash", "new-hash")?;
```

`openapi::parse` and `parse_bytes` return surfaces directly. Generic consumers implement `Adapter` and use `compare` on their own contracts. Neutral model kinds support renames, nullability, enum values, defaults, pagination and wire identity without forcing new parsers to use the HTTP-shaped v1 `Change` model.

## Limits

OpenAPI parsing, ref expansion cap and operation comparison limits remain as documented for v1: first request/response media type, exact HTTP path matching (plus the unique move pairing above), composition compared by index, supported OpenAPI schema grammar rather than all JSON Schema. Independent type comparison retains local `$ref` identity and includes default/deprecation facts; descriptions/examples/titles/extensions are annotations. No automatic rename matching, GraphQL/protobuf parsing or focused multi-format Markdown/Skill rendering was part of #124; the protobuf and GraphQL adapters below add them. Large v2 surfaces add output volume by design; callers needing only changes can select that field after parsing.


## Protobuf adapter

The protobuf adapter now implements this surface for proto2/proto3 and complete
FileDescriptorSets. Fields match on containing message FQN and field number;
enum labels include their names to support aliases. Native locations retain FQN
and number, with adapter-owned wire/source/JSON impacts and scoped failure gates.
See [protobuf contracts](protobuf.md) for parser requirements, explicit Editions
rejection, native rule names, stable identities and committed import handling.

## GraphQL adapter

The GraphQL adapter implements this surface for SDL (single or merged files)
and introspection results. Root fields are operations under their root
service; types, fields, arguments, input fields and enum values use schema
coordinates. List-to-connection transitions (Relay `edges { node }` or a
`nodes` list, with `pageInfo`) are `breaking` pagination facts; connection
control changes are `review`. See
[GraphQL contracts](graphql.md).
