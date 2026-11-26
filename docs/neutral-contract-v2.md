# Neutral contract and JSON v2

`diff --format json` defaults to contract 2. `--contract 1` selects the unchanged OpenAPI v1 serializer and `vc1_` identity construction; `--contract 2` makes the default explicit. Markdown remains the existing OpenAPI report. A v2 JSON exit gate uses v2 adapter verdicts; a v1 gate uses v1 verdicts. The package version is independent of the JSON schema version. These additions preserve `parse_openapi`, `diff::diff`, and `diff::json::to_json`.

## Surface

`contract::Contract` holds a source format, title, version and ordered `entities`. Each entity has a category, name, stable ID, parent, service memberships, references, native location and adapter attributes. A flat node forest with explicit edges allows shared and cyclic named types without copies. Operations may belong to several services. Arguments/results belong to operations; fields and enum values belong to their schema root or enclosing field. A reusable type is a root; inline schemas expose member nodes under the argument/result containing them. Native schema attributes retain constraints, defaults, deprecation, composition and `$ref` identity. `references` contains stable named type IDs, including unresolved local targets; the absence of a target is inspectable rather than silently discarded.

Entity IDs are `ve2_` plus SHA-256 of canonical JSON `{v:2, format, kind, coordinate}`. Canonical JSON sorts object keys recursively, leaves arrays ordered, and uses compact serde_json encoding. Coordinates are exact case-sensitive native identities, scoped by their container. OpenAPI uses escaped `/services/NAME`, `/paths/PATH/method`, `/types/NAME`, and child argument/result/property coordinates. HTTP operation identity remains method plus exact path, excluding Swagger `basePath`; changing operationId changes its name, not identity. OpenAPI 2 definitions and OpenAPI 3 components share `/types/NAME`. Parameter coordinates include location and name. Enum coordinates use canonical JSON of the actual value, so null and the string "null" differ. Composition branches use their array index. Descriptions, versions and unrelated declaration order do not enter entity IDs. A rename of an entity's identity is remove/add unless its adapter explicitly identifies it as a rename or move.

### OpenAPI operation moves

The CLI (Markdown and v2 JSON) pairs OpenAPI operations that moved path; `--exact-paths`, `--format json --contract 1` and the library defaults (`openapi::diff`, `json::openapi`, `diff::diff`) keep exact matching, and `diff_with`/`openapi_with` take `diff::EndpointMatching::Moves`. After exact `(method, path)` matching, leftover removed and added operations of the same method pair by `operationId`, then by path equality after replacing whole `v<digits>` segments (`/api/v1/items` ↔ `/api/v2/items`, not `/v1beta`). A key qualifies only when it is unique across the whole old document and the whole new document (not merely among the leftovers) and both holders are leftovers, so pairing is one-to-one. Ambiguity keeps `operation_removed`/`operation_added`: old `/v1/items` + `/v2/items` against new `/v2/items` + `/v3/items` is an exact `/v2/items` match plus a removal and an addition, never a v1 → v3 move.

A pair emits one `operation_moved` fact (native kind `endpoint_moved`, breaking, since callers of the old path break). Its subject is the operation entity at the **new** coordinate; details hold `old_method`, `old_path`, `matched_by` (`operation_id` or `version_segment`), `old_operation_id`/`new_operation_id` only when they differ, and `from_operation`, the old operation's entity ID. Inner facts compare the pair as one operation and take their subjects and native locations from the side that holds them, as in exact mode: removals (`argument_removed`, `result_removed`, `field_removed`, `enum_value_removed`) use old-side subjects and native locations, which resolve in `old.contract.entities`; additions and changed facts (type, requiredness, constraint, default, deprecation, pagination) use new-side ones, which resolve in `new.contract.entities` and are the identities a later comparison from the new spec reuses. IDs are hashed from these subjects and details as usual, so they are deterministic. `operations_changed` counts the moved operation once; it is neither added nor removed. The surfaces in `old`/`new` stay unaligned.

Changes carry `subject` (an entity ID), a neutral `kind`, `severity`, compatibility `impacts`, `native`, `native_kind`, lossless `details`, and, for a fact reported once at a shared type, `uses` (see below; omitted when empty). A schema change may attribute to the containing schema root; its pointer identifies the member. Native OpenAPI coordinates distinguish declared surface pointers (`resolved:false`) from pointers into normalized operation schemas (`resolved:true`), where a `$ref` met by an inline schema on the other side is expanded in place. The latter are not input-file pointers. GraphQL locations carry schema coordinates; protobuf locations carry fully qualified names and optional numbers. Format-specific information remains in attributes and details rather than contaminating neutral categories.

Change IDs are `vc2_` plus SHA-256 of canonical `{v:2, subject, kind, details}`. Severity, impact dimensions, `uses`, generator version, raw-file hashes and native display attribution are excluded. Details are factual, not prose. Composite pagination evidence is part of the fact. Identity construction is versioned independently from a package release. Exact v1 output and IDs remain available through the original serializer.

### OpenAPI media types

A request body or response with several media types is compared media type by media type, matched by media-type key (type, subtype and parameter names case-insensitive, whitespace around `;` ignored; parameter values and wildcards exact) (#154; see the `diff` module docs, "Media types"; a matched media type is named as the old side writes it). The surface is unchanged: the parser already makes one body argument per request media type (`requestBody` when there is one, `requestBody (TYPE)` when there are several), and a status's result holds its first schema. Facts map as follows:

- A request media type one side lacks is `argument_removed` (subject the old body argument for it, breaking) or `argument_added` (the new one, non-breaking), native kind `request_media_type_removed`/`_added`, details `media_type`. A single media type re-keyed (`application/json` → `application/xml`) is both, on the same subject: the argument is `requestBody` on both sides, since its name carries the media type only when there are several.
- A response media type one side lacks is `result_removed` (breaking) or `result_added` (non-breaking) on the status's result, native kind `response_media_type_removed`/`_added`, details `status` and `media_type`.
- A schema change of one media type keeps its kind and subject rules (the body argument of that media type; the status's result) and adds `media_type` to details. Component facts are unchanged: their `uses` name the body argument or result, so two media types of one response are one use.
- Deprecation facts of a body or response compared per media type name the media type in details; one already reported at the same place without a media type (from the surface's first schema) is not repeated.

When each side has exactly one media type and it is the same key, a side has none, or a side names none (Swagger 2), nothing changes: no `media_type` appears, so every ID of a single-media-type spec is the same as before #154. A body or response that goes from one media type to two is compared per media type from then on, so its schema facts gain `media_type` and new IDs. Pagination inference still reads each 2xx response's first schema. Swagger 2 `consumes`/`produces` are not compared.

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

All named types are present, including unused types. An added type is non-breaking; a removed or changed type is review, because a declaration alone does not establish input/output direction or a client dependency. These independent type facts (`native_kind` `surface_changed`) compare each type's own attributes, with nested `$ref`s as edges.

`$ref` targets are compared as edges (#181). An operation's parameters, request body and responses are compared locally: wherever both sides reference a component at the same position, the pair of target names is an edge and the comparison does not descend into it; where only one side references a component and the other is an inline object, that reference is expanded in place, so inline ↔ referenced moves stay use-site facts. Keys beside a `$ref` other than annotations (`description`, `title`, `example`, `examples`, `deprecated` and `x-*`, the keys canonicalization strips; `default` counts as semantic) override the target in resolution, so they form an overlay: each is compared at the reference with its effective value (the sibling's, else the target's), and a change in the target under an overlaid top-level key is not listed for a use that reaches the target only through that overlay. Edges under an overlaid key of the next component are likewise not followed; an alias passes its incoming overlay on to its target. Cycles are ordinary graph cycles. Each `(old component, new component)` pair an edge reaches (directly or through other changed components) is compared once, the same way. A shared schema can be both input and output, with opposite nullability or enum verdicts, so a change inside a component is reported once **per usage direction** that reaches it, with the existing directional verdict:

- `native_kind` `component_schema_changed`; `subject` the named type `/types/NAME` (for `field_removed` and `enum_value_removed` the old type, so it resolves in `old.contract`, otherwise the new one); `kind` from the schema pointer (`type_kind_changed` for the root `/type`);
- `details`: `component` (the new name), `old_component` only when a reference was retargeted to a differently named component, `direction` (`request` for parameters and request bodies, `response` for responses) and `schema_change` (the v1 shape, pointer relative to the component);
- `native`: no method/path, pointer `/types/NAME` plus the schema pointer, `resolved:false`;
- `uses`: entity IDs of every argument and result of that direction whose schema reaches the pair, in report order. They resolve in `new.contract`.

Edges only join components that their own documents declare, so every subject and every `old_component` resolves (the old type in `old.contract`, the new one in `new.contract`). A reference whose target one side does not declare is compared at the use site against the literal `{"$ref": ...}` resolution leaves there, as in 2.2: deleting an alias still referenced is a use-site change, never a retarget of the alias's target.

`not` reverses every compatibility direction, so a change written under a `not` keyword is review. Component pointers are relative to the component, so the reach carries it instead: a use that reaches a component through a `not` keyword (at the use or in any component on the way, through aliases, overlays and cycles) gets its own fact with `under_not: true` in `details`, which is review; uses reaching it plainly keep their own fact and verdict.

A component reached from both directions therefore yields two facts, and `--fail-on-breaking` fires when either is breaking. The ID hashes the subject, kind and details, never `uses`: a new operation that starts using a component, or a different operation reaching it first, leaves the fact's ID unchanged. Changes inside an operation's own inline schema remain operation-specific facts with argument/result subjects, unchanged pointers and unchanged IDs. A `deprecated` change inside a shared component is reported once, by the named type's independent fact; a `deprecated` beside a `$ref` at the use site is still the use's fact. Every reached pair is also compared once for `deprecated` keywords, which the type facts cannot see across a reference retargeted from `A` to a differently named `B`, or inside a reference expanded in place against an inline schema: one `deprecated_changed` fact per pointer, `native_kind` `schema_deprecation`, subject the new type, details `{component, old_component (only when the names differ), pointer, before, after}`, native pointer `/types/B/...`, and the reaching uses (sorted entity IDs) in `uses`. When the type's own fact already reports the same pointer, before and after, this fact is omitted.

**Retargets.** A reference retargeted to a differently named component is always a change, even when the two components have the same shape: generated clients and typed consumers depend on the name (`Customer` → `Account`). Each reached pair with different names reports, per direction, one review `constraint_changed` fact at pointer `/$ref` whose `schema_change` is `changed` from the old name to the new one, ahead of its structural differences. This holds for an operation's own reference and for one inside a component; the ID comes from the pair and the pointer, so it does not depend on where or how often the retarget occurs.

**Migration from 2.2.** Before 2.3.0 every `$ref` was inlined and a change inside a component was one `request_schema_changed`, `response_schema_changed` or `parameter_schema_changed` fact per affected use and reference path, with an argument/result subject and a pointer into the inlined schema (`resolved:true`). Those facts, and their `vc2_` IDs, are replaced by the per-direction `component_schema_changed` facts above, so suppression lists keyed on them need the new IDs. Facts about operation-local inline schemas, operations, arguments, results, pagination and independent types keep their IDs. Specs without schema `$ref`s produce byte-identical output. `--contract 1` keeps one record per affected use (see `diff::json`), with `component` in details and a component-relative pointer.

Type and use records are intentional separate facts and have distinct IDs. Counts include both. `operations_changed` counts surviving operation coordinates once, including the operations in a fact's `uses`, and excludes independent type/service changes. `--fail-on-breaking` agrees with the actual v2 `summary.breaking` count.

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

The v2 envelope has `schema_version`, `generator`, `old`, `new`, `summary`, `changes`, optional `deltas`, and optional `warnings` (omitted when empty: each `{code, spec, pointer, message}`; today only `unparsed_definition`, a `definitions` entry that is not a valid schema and was compared as written, #188, whose changes have native kind `unparsed_definition_changed` and are Breaking unless the rule table classifies them). A change at or beneath a one-sided `$ref` the comparison refused to expand (past its expansion budget or chain cap) likewise fails closed, with native kind `expansion_cut`; both carry `fail_closed` in `details`. Each side contains raw `file_sha256` and the full `contract` surface. Hashes refer to exact parsed bytes. Deltas retain the OpenAPI hygiene/render profile. The v2 schema round-trips through serde; no CLI feature is required.

### Omitting the surfaces (`--omit-surfaces`)

The two `contract` surfaces are most of a v2 document: on large specs, megabytes of unchanged entities around a few kilobytes of `changes`. `diff --format json --omit-surfaces` writes the same document without them:

```json
{
  "schema_version": 2,
  "generator": { "name": "vimanam", "version": "…" },
  "surfaces_omitted": true,
  "old": { "file_sha256": "…" },
  "new": { "file_sha256": "…" },
  "summary": { … },
  "changes": [ … ],
  "deltas": { … }
}
```

- Each side keeps only `file_sha256`. `surfaces_omitted` is always `true` and appears only in this shape, so the default document is byte-identical with or without this feature and a consumer can tell an intentionally surface-free document from a truncated one.
- `schema_version`, `generator`, `summary`, `changes` (every `vc2_` ID, `ve2_` subject and `uses` ID, native location, details) and `deltas` are exactly those of the default run. The summary is still counted over the full surfaces, and `--fail-on-breaking` / `--breaking` exit codes are unchanged.
- The surfaces are still built (the comparison and the `operations_changed` count walk them) but are dropped before serialisation, which is where the time and size go.
- What is no longer resolvable from the document: a `ve2_` ID's entity record — its kind, name, parent chain, attributes and references. Each change's `native` location (OpenAPI method/path/status/pointer, GraphQL schema coordinate, protobuf full name and number) and `native_kind` say where it is without a lookup, and entity IDs are content-derived from `{format, kind, coordinate}`, so a consumer that needs the records can rerun without the flag and join on the same IDs.
- Rejected with exit status 1 before any input is read or output created with Markdown (`--format markdown`, the default) or `--contract 1`: neither carries surfaces.

In the library, `JsonDiff::without_surfaces` turns a document into `JsonDiffWithoutSurfaces` (with `SideDigest` sides), which serialises to this shape and round-trips through serde.

```rust
let old = vimanam::parse_openapi("old.json")?;
let new = vimanam::parse_openapi("new.json")?;
let surface = vimanam::contract::openapi::adapt(&new);
let changes = vimanam::contract::openapi::diff(&old, &new)?;
let json = vimanam::contract::json::openapi(&old, &new, None, "old-hash", "new-hash")?;
let changes_only = json.without_surfaces(); // the --omit-surfaces shape
```

`openapi::parse` and `parse_bytes` return surfaces directly. Generic consumers implement `Adapter` and use `compare` on their own contracts. Neutral model kinds support renames, nullability, enum values, defaults, pagination and wire identity without forcing new parsers to use the HTTP-shaped v1 `Change` model.

## Limits

OpenAPI parsing, the cap on expanding a chain of one-sided references and operation comparison limits remain as documented for v1: exact media-type keys (above), exact HTTP path matching (plus the unique move pairing above), composition compared by index, supported OpenAPI schema grammar rather than all JSON Schema. Independent type comparison retains local `$ref` identity and includes default/deprecation facts; descriptions/examples/titles/extensions are annotations. No automatic rename matching, GraphQL/protobuf parsing or focused multi-format Markdown/Skill rendering was part of #124; the protobuf and GraphQL adapters below add them. Large v2 surfaces add output volume by design; callers needing only changes can select that field after parsing.


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
