# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Behavior changes

- **`--inline-schemas` renders are bounded at 2,000,000 schema rows**
  (#177). Whole-document `--detail full --include-schemas --inline-schemas`
  renders now finish on densely cyclic specs. Inline expansion re-expands
  every acyclic path through the component graph at each use site, so such
  specs grew exponentially and never finished. Without `--schema-depth`,
  vimanam now counts the inline rows first (without rendering) and, past
  2,000,000 rows, bounds expansion at `--schema-depth 2` with a stderr notice
  and a blockquote under the title. The count follows the view, so the
  default service view counts a multi-tag operation once per tag. This also
  covers `--max-tokens`, `--stats`, `--costs` and selected slices. The budget
  counts rows, not bytes: with short descriptions it is a few hundred MB of
  Markdown, and long descriptions scale that up. A 40-component adversarial
  spec that wrote 1.35 GB in 30 s without finishing now renders in about
  25 ms. Every render under the budget (all fixtures; the GitHub REST,
  protobuf-generated and petstore specs) is byte-identical.
  - **Changed output:** inline renders above the budget that used to finish,
    slowly (hundreds of MB), are now bounded as well. Pass `--schema-depth N`
    to choose a deeper bound.
  - **`--costs`:** its inline model expands every used schema separately (in
    linked mode too). It now measures at most 10,000 rows of each schema's
    expansion, at any `--schema-depth`, and marks a cut figure
    `inline-bounded`. INLINE/USE, INLINE TOTAL and AMPLIF. are then lower
    bounds, shown as `at least` in the hotspots. A dense 1,000-schema graph
    now reports in seconds.
  - **API:** no public library API change. The touched `--costs`/`--stats`
    types are crate-private, and `generate_markdown` applies the same bound.
- **`diff` compares `$ref` targets as edges** (#181). Each `(old component,
  new component)` pair is compared once, and a change inside a shared
  component is reported once per usage direction (request: parameters and
  request bodies; response: responses), with every operation schema that
  reaches it listed, instead of once per operation and reference path.
  Changes in an operation's own inline schemas keep their coordinates,
  pointers and IDs, and specs without schema `$ref`s produce byte-identical
  output. Every consecutive Stripe `spec3.json` release pair of the last 16
  commits now diffs in under 2 s in every output mode (all timed out before),
  and OpenAI `a01e367..373f26a` still reports the same 4 breaking findings.
  - **Verdicts.** Each direction keeps its own rule table, so a component
    shared by a request and a response yields two records, and
    `--fail-on-breaking` exits 3 when either is breaking. `not` subtrees stay
    Review and a whole `enum` keyword is still classified by direction.
  - **`not`.** A component reached through a `not` keyword (directly, via
    aliases, or inside another component) is reported for those uses as a
    separate Review record with `under_not: true` in JSON details, like any
    change under `not`; plain uses of the same component keep their verdict.
  - **Retargets.** A reference retargeted to a differently named component
    is a Review change at `/$ref` (old name → new name) even when the two
    have the same shape, followed by their structural differences; before,
    a same-shaped retarget reported nothing at an operation's root.
  - **Overlays.** Keys beside a `$ref` other than annotations (`default`
    included) are compared at the reference with their effective values, and
    a target change under such a key is not listed for that use, matching
    how resolution lets the sibling win. Changes in the target outside them
    are reported at the component, no longer at the use.
  - **Markdown.** A component change is one `Component schema changed` row
    (for example `` component `Widget` `` / `response /properties/pricing
    added (3 operations)`), and a new `## Affected operations` section lists
    each changed component's uses once.
  - **Contract v2 (`vc2_` IDs move for ref-reached changes).** A
    ref-reached change is one fact per direction with `native_kind`
    `component_schema_changed`: subject the named type (`/types/NAME`; the old
    type for a removal), `details` `component` (plus `old_component` for a
    retargeted reference), `direction` and `schema_change` with a pointer
    relative to the component, native pointer `/types/NAME/...` with
    `resolved: false`, and a new `uses` array of the reaching argument/result
    entity IDs. `uses` is context and not hashed, so the ID depends only on
    the component, direction and schema change, never on which operation
    reaches it first. These facts replace the per-use
    `request_schema_changed`/`response_schema_changed`/
    `parameter_schema_changed` facts whose IDs hashed an argument or result
    subject and a resolved pointer; suppression lists keyed on those IDs
    need the new IDs. A deprecation inside a shared component is now only the
    named type's own (unchanged) fact, not also one per use; a deprecation
    that only the component comparison sees (across a reference retargeted
    from `A` to `B`, or inside a reference expanded against an inline
    schema) is one `schema_deprecation` fact at the new type, with
    `old_component` when the names differ and the reaching uses. A reference to a
    component its document does not declare is compared at the use site, as
    before.
    `summary.operations_changed` counts the operations in `uses`.
  - **Contract v1 (`--contract 1`).** v1 adopts the component comparison but
    keeps its one-record-per-affected-endpoint shape: one record per use (the
    use's usual kind), with `component` (and `old_component`) in `details`
    and a component-relative `pointer`. Records that a change reached
    through several reference paths of one use collapse into one, so `vc1_`
    IDs of ref-reached records change. v1 severity counts now count records,
    so they can exceed the Markdown summary's.
  - **Library.** `diff::ChangeKind::ComponentSchemaChanged` (with
    `ComponentRef`, `Direction`, `SchemaUse`, `UseLocation`) is new;
    `SpecDiff::endpoints_changed` counts every use of a changed component; the
    neutral `contract::Change` has a new `uses` field (empty and omitted from
    JSON for every other fact), so code building `Change` literals must set
    it.
  - A value that exists on one side only (an added property, say) keeps
    nested `$ref`s as written instead of inlining them.

### Changed

- `diff` walks each (old, new) schema pair once, however many uses compare
  it (#196). Before, every use (each endpoint's request body and responses,
  and since #154 each media type) made its own walk with its own expansion
  budget, so on adversarial recursive specs the cost grew with the number
  of uses. A request and its response with the same schema, each media type
  of one body, and every endpoint sharing a body now share one walk; each use
  still gets its own records. The per-record work is cheaper too: pointer
  segments are decoded without allocating, Markdown classifies each change
  once, finding the fail-closed region over a change is linear in its
  pointer (it was quadratic), and v2 change IDs hash the canonical JSON as
  it is written. The v2 deprecation pass deduplicates each fact against
  the facts at its place instead of scanning every change reported so far,
  which was quadratic on cut walks: with `deprecated` added inside such a
  spec, `--format json` took 108 s and now takes 7.4 s (5.5 s without the
  deprecations). On #196's shape (9 and 10 mutually recursive components
  used by N endpoints), Markdown takes 1.1 s / 1.4 s / 5.2 s at N = 1 / 5 /
  20 (was 5.2 s / 26.3 s / 105 s) and `--format json` 0.9 s / 3.6 s /
  15.4 s (was 3.6 s / 18.6 s / 77 s); with three media types per body,
  N = 5 takes 3.8 s (was 77.5 s). Output is byte-identical. Memory still
  grows with the number of uses: their records are all reported. Uses that
  each compare a different pair still cost one budget per pair, and the
  memo keeps one extra copy of each compared pair's records for the whole
  diff: on adversarial specs whose walks are cut, 381 MB instead of 287 MB
  with five distinct pairs, and 494 MB instead of 420 MB (Markdown) or
  1,096 MB instead of 1,003 MB (v1 JSON) with one shared pair. Real specs
  measured (Stripe, OpenAI, an Endor Swagger 2 to OpenAPI 3 pair) use the
  same memory as before.
- Git-ref `diff` reads committed blobs through one long-lived
  `git cat-file --batch` child per repository session instead of one
  `git show` per blob, and caches resolved refs, tree listings and rename
  edges per commit (#164). On opentelemetry-proto (`40cb2e5..d31f7ac`,
  `trace_service.proto` and 3 imports) Git subprocesses drop from 39 to 10
  and the median wall time from 896 ms to 248 ms; the generated 200-proto
  repository drops from 40 to 11 subprocesses and 948 ms to 269 ms.
  Rename tracking diffs history edges in chunks through one
  `git diff-tree --stdin` per 256 edges instead of one `diff-tree` per edge:
  a 400-commit merge-fallback history drops from 418 subprocesses to 12
  (14.4 s to 0.46 s). Output and error messages are unchanged: blob bytes
  are raw (no textconv or filters), and any batch failure falls back to the
  original per-blob `git show` or per-edge `diff-tree`.

### Fixed

- `diff` compares every media type of a request body or response (#154).
  Before, only the first one was compared, so a removed second media type,
  or a breaking change in its schema, passed `--fail-on-breaking`. Media
  types match as RFC 9110 compares them: type, subtype and parameter names
  case-insensitively and without whitespace around `;` (so
  `Application/JSON` is a respelling and reports nothing), parameter values
  and wildcards exactly (so `application/json` to
  `application/json; charset=utf-8`, or `*/*` to `application/json`, is a
  removal plus an addition), and a matched media type is named as the old
  spec writes it: each
  media type both sides declare is compared as its own schema use, and one
  side lacking a media type is a new change kind. Removing a request media
  type (clients sending it fail) or a response media type (clients
  negotiating it with `Accept` fail) is Breaking; adding one is
  Non-breaking.
  - **Unchanged output.** When each side has exactly one media type and it
    is the same one (respellings included), a side has none (a response gains or loses its whole
    body), or a side is Swagger 2 (no media type per body or response;
    `consumes`/`produces` are still not compared), the body or response is
    compared by its first schema as before. On every consecutive Stripe
    release pair of the last 16 commits, the Endor OpenAPI 3 and Swagger 2
    specs (including 2 → 3) and every fixture pair, Markdown, v1 and v2
    output is byte-identical.
  - **Markdown.** New rows `Request media type removed`/`added` and
    `Response media type removed`/`added`; a schema row of a body or
    response compared per media type names it (`` 200 `application/xml`
    /properties/id/type ... ``), and `## Affected operations` lists each
    media-type use (`` response 200 `text/event-stream` ``).
  - **v1 JSON (`vc1_` IDs).** New kinds `request_media_type_added`,
    `request_media_type_removed`, `response_media_type_added` and
    `response_media_type_removed` (details `media_type`, plus `status`).
    A schema record of a body or response compared per media type has
    `media_type` in its details, so its ID differs from a single-media
    record's, and a component change reached through two media types is two
    records. On OpenAI `a01e367..373f26a`, two component records of
    `POST /completions` 200 become four (`application/json` and
    `text/event-stream`); nothing else changes.
  - **v2 JSON (`vc2_` IDs).** Surfaces are unchanged. A request media type
    is `argument_removed`/`argument_added` on its body argument, a response
    media type `result_removed`/`result_added` on the status's result, with
    `media_type` in details (native kinds as in v1; a single request media
    type re-keyed puts both facts on the same `requestBody` argument); a schema fact of one
    media type adds `media_type`. Component facts keep their IDs (two media
    types of one response are one use). Inline `deprecated` changes in a
    later media type are now reported (they were omitted).
  - **Library API break (#154).** `diff::ChangeKind::RequestSchemaChanged`
    and `ResponseSchemaChanged` (released) and `diff::UseLocation::RequestBody`
    and `Response` (new in this release) gained a `media_type:
    Option<String>` field, and `models::Parameter` (released) a
    `media_type: Option<String>` field (`#[serde(skip)]`, set on the
    parser's synthetic OpenAPI 3 body parameters), which breaks struct
    literals and field patterns without `..` of each. `ChangeKind` has four
    new variants (`RequestMediaTypeAdded`, `RequestMediaTypeRemoved`,
    `ResponseMediaTypeAdded`, `ResponseMediaTypeRemoved`; the enum is
    `#[non_exhaustive]`), as does `diff::json::JsonChangeKind`. The CLI is
    unaffected.
- `diff` no longer stops expanding a recursive component by its name when
  only one side writes it as a `$ref` (#187). Expansion now stops where the
  compared pair of old and new values recurs, as the Lean reference model
  specifies. Before, the walk compared a literal `$ref` with the other side's
  inline schema, which caused two problems:
  - changes beneath that point were hidden, so a Breaking change could pass
    `--fail-on-breaking`;
  - a recursive component unrolled inline (no change at all) reported a
    Breaking `/type` added and exited 3. On Stripe, inlining every response
    `$ref` two or three levels deep gave 45 and 754 false Breaking records;
    it now gives none.

  A pure alias loop (`C = {"$ref": C}`) facing an inline schema now compares
  as its siblings without `$ref`, instead of reporting a `$ref` removed.
  The work is bounded: past 64 one-sided expansions on one chain, or 4,096
  in one schema comparison, the reference is compared as written and fails
  closed: the `$ref` record there, and any change the rule table leaves to
  review beneath it, is Breaking (v1 details `fail_closed: "expansion_cut"`,
  v2 native kind `expansion_cut`), so a change hidden behind the cut still
  trips `--fail-on-breaking`. Two
  reference cycles facing each other no longer grow exponentially: 9 and 10
  components take 0.8 s.
- `diff` compares a `definitions` entry that is not a valid schema instead
  of dropping it (#188). Before, a change inside such an entry (one with
  `"required": "a"`, say) printed "No changes." and exited 0. The entry is
  now compared as written and fails closed: a change inside it that the
  rule table leaves to review is Breaking, whether the entry is compared
  with its counterpart or expanded against an inline schema (v1 details
  `fail_closed: "unparsed_definition"`, v2 native kind
  `unparsed_definition_changed`; Markdown rows say "fails closed");
  Breaking and Non-breaking verdicts stand.
  The report opens with a `> **Warning:**` line naming the entry by JSON
  pointer, the same goes to stderr, and v2 JSON has a new `warnings` array
  (omitted when empty). Markdown rendering still leaves the entry out.
  **Library API break (#187, #188).** The enum variants
  `diff::ChangeKind::ParameterSchemaChanged`,
  `diff::ChangeKind::RequestSchemaChanged` and
  `diff::ChangeKind::ResponseSchemaChanged` (released in earlier versions)
  gained a `fail_closed: Option<diff::FailClosed>` field. Downstream code
  that builds one of them as a struct literal must set it, and a pattern
  that lists their fields without `..` must add it or `..`. The CLI is
  unaffected. Also new: `diff::ChangeKind::ComponentSchemaChanged` (itself
  new in this release) has the same `fail_closed` field; the structs
  `ApiDocumentation` (field `unparsed_schemas`), `diff::SpecDiff` (field
  `unparsed_definitions`) and `contract::json::JsonDiff` (field
  `warnings`) gained a field each, which breaks struct literals of them;
  and the new types are `UnparsedSchema`, `diff::UnparsedDefinition`,
  `diff::FailClosed` and `contract::json::JsonWarning`.
- `diff` no longer rates removing a member from a **response** `enum` as
  breaking (#169): a server that stops returning a value cannot break a
  client, so the row is now non-breaking. Request-side enum removal stays
  breaking, so a narrowed enum on a component shared by a request and a
  response is still caught. The change is intentional in every output:
  Markdown, v2 JSON (`enum_value_removed` on a result) and `--contract 1`
  verdicts.
- `diff` reports an `enum` keyword present on only one side as one `/enum`
  change instead of per-member additions or removals, because a missing
  `enum` allows every value. Dropping a response `enum` is Review (the
  response widens); dropping a request `enum` is non-breaking (previously a
  false breaking); adding an `enum` where none existed is breaking on a
  request and non-breaking on a response. In v2/v1 JSON the change has
  target `other` at the `/enum` pointer. With both fixes, the OpenAI
  `a01e367..373f26a` diff drops from 62 breaking findings to 4, all genuine.
- `diff` rates any schema change at or beneath a `not` keyword as Review.
  `not` reverses every compatibility direction, so the ordinary request and
  response rules misclassified those changes.
- OpenAPI 3.1 `type` arrays keep `"null"` (#166): `["string", "null"]` now
  reads as `string` with `nullable: true`, so it renders `string | null` and
  `diff` classifies `"string"` ↔ `["string", "null"]` exactly like OAS 3.0
  `nullable` (breaking when a response gains null or a request loses it). A
  lone `["null"]` renders as `null`, duplicate members collapse, and
  reordered union members (`["string", "integer"]` vs
  `["integer", "string"]`) no longer produce a false `type` change.

## [2.2.0] - 2026-11-24

### Added

- Pure-Rust `.proto` source compilation via `protox` (#126): `.proto` files,
  Google well-known imports, `source_code_info`, and Git-ref `.proto` diffs no
  longer require an external `protoc` binary on `PATH`. Committed Git-ref
  `.proto` snapshots are resolved directly from Git blobs in memory (preserving
  import-root confinement, symlink/regular-file checks, and canonical shadow
  detection). Protobuf Editions remain explicitly rejected before output
  creation.
- `.proto` source comments surfaced in rendered Markdown documentation (#126):
  leading and trailing comments from `source_code_info` appear on services,
  RPCs, messages, scalar/repeated/singular-message/enum/`map` fields (alongside
  `Field #N` metadata across linked, `--inline-schemas`, `--split`, and
  `--output-mode skill` views), enums, and enum values, while neutral
  `ve2_`/`vc2_` IDs and diff facts remain comment-invariant.
- GraphQL `extend schema`, schema descriptions, repeatable directives, and
  `@oneOf` input objects (#125), powered by `apollo-parser`.
- CLI usability and packaging additions (#41, #33, #31):
  - `vimanam completions nushell` alongside bash, zsh, fish, elvish, and
    powershell.
  - `vimanam man [--out-dir DIR]` to emit roff manual pages (`vimanam(1)` on
    stdout, or `vimanam` and every subcommand page into `DIR`).
  - `clap::ValueHint` file/directory annotations on path flags and terminal-width
    `wrap_help`.

### Changed

- Replaced unmaintained `graphql-parser` with `apollo-parser` (#125), tightening
  GraphQL SDL and `canonical_literal` validation (rejecting braceless/empty
  `schema` blocks, conflicting `extend schema` overrides against implicit
  roots, malformed object fields / directive arguments, and duplicate
  input-object field or directive argument names) while preserving `ve2_`/`vc2_`
  identities and SDL/introspection parity.
- Replaced `serde_norway` (and its `unsafe-libyaml-norway` backend) with
  pure-Rust, unsafe-free `serde-saphyr` (#4, #78): enforces YAML 1.2 Core
  booleans (keeping `NO`/`yes`/`on`/`off`, leading-zero, and underscored
  scalars as strings in untyped JSON values), expands `<<:` merge keys, rejects
  duplicate mapping keys, and applies anti-DoS parsing budgets sized for
  multi-MB OpenAPI specs.
- Moved `tempfile` to `[dev-dependencies]` now that runtime Protobuf
  compilation no longer creates temporary directories.

### Security

- Updated `Cargo.lock` dependencies, resolving `RUSTSEC-2026-0486`
  (`rustix` `1.1.5` -> `1.1.6`) (#77).

## [2.1.0] - 2026-11-04

### Added

- GraphQL schema inputs (#125, #133; merged after the 2.0.0 tag, which
  cannot read GraphQL): SDL files or merged SDL directories and introspection
  JSON, mapped onto the neutral model with schema-coordinate
  identities. Directional breaking rules for removals, nullability, required
  arguments/input fields, type changes, union members, interfaces and type
  kinds; review for output enum additions and defaults; Relay connection
  pagination facts. Markdown report, neutral v2 JSON, `--fail-on-breaking`,
  `diff --context [--max-tokens N]` focused context, Git refs and native
  Markdown/Skill output. Public SpaceX Land schema fixtures at two versions.
  Backward-only (`last`/`before`) Relay connections are detected, and
  symlinked SDL files and directories are followed (loop-safe, each file
  merged once).
- OpenAPI `diff` pairs an operation moved to a new path (#137): after exact
  method+path matching, leftover operations of one method pair by an
  `operationId`, then by path equality ignoring whole `v<digits>` segments
  (`/v1/items` ↔ `/v2/items`), each key unique across the whole old and new
  spec; ambiguous keys stay removed plus added. A pair is a breaking
  `operation_moved` fact (Markdown "Endpoint moved") followed by the
  operation's inner changes, including `pagination_changed`, attributed to the
  new path; in v2 JSON, removal facts keep their old-side subjects and native
  locations. `diff --exact-paths` restores exact matching. `--contract 1`
  JSON is unchanged, and the library defaults (`diff::diff`,
  `contract::openapi::diff`, `contract::json::openapi`) stay exact; the new
  `diff_with`/`openapi_with` functions take `diff::EndpointMatching::Moves`.
- README table of input formats by release (#136).

### Changed

- `vimanam diff` Markdown and v2 JSON output (the defaults) change for specs
  where an operation moved path (#137): what 2.0.0 reported as an "Endpoint
  removed" plus an "Endpoint added" (`operation_removed` + `operation_added`)
  is now one breaking "Endpoint moved" (`operation_moved`) plus the inner
  changes, with different `vc2_` IDs and summary counts. Pass `--exact-paths`
  for the 2.0.0 output; `--format json --contract 1` is byte-identical.
- OpenAPI pagination inference (#138) matches continuation names
  case-insensitively ignoring `_`, `-`, `@` and `.`, with more specific names
  (`continuationToken`, `nextLink`, `@odata.nextLink`, `after_cursor`,
  `LastEvaluatedKey`, `next_url`, ...), continuations one object level down
  (`response_metadata.next_cursor`, `links.next`), and generic names (`cursor`,
  `next`, `after`) only when a token control or an end-of-results flag
  (`has_more`, `end_of_stream`, `is_last`, `drained`, ...) corroborates them.
  Details record the matched paths, `evidence_rule`, and a `corroborated`
  confidence when an end flag adds to sufficient evidence. On a side the
  2.0.0 rule (a top-level array plus one of its six top-level names) matches,
  the evidence, confidence and rule are exactly 2.0.0's and extra end flags or
  names are ignored. A pagination fact 2.0.0 emitted therefore keeps its
  details and `vc2_` ID when the 2.0.0 rule matches both sides, or one side
  while the broader rules find nothing on the other. It changes when the other
  side has evidence only the broader rules see: renaming `nextPageToken` to
  `next_token`, or moving it under `meta`, was a removal (`after: null`) in
  2.0.0 and is now a fact with the new side's evidence and a new `vc2_` ID, so
  a suppression list keyed on 2.0.0 IDs can fire again for such an operation.
  Otherwise the new rules only add detections 2.0.0 missed.
- GraphQL connections with `nodes` and `pageInfo` (no `edges`) are recognised
  as pagination (#139), with the same native kinds. Details name the matched
  `connection_fields` (`edges`, `nodes`, `edges+nodes`); a `nodes` connection
  is `declared` when `pageInfo` has `hasNextPage` and `endCursor`, else
  `inferred`. Because every GraphQL `pagination_changed` fact (Relay `edges`
  ones included) now carries `connection_fields`, its `vc2_` ID differs from
  builds of unreleased `main` after #133; no release contained GraphQL.

### Fixed

- `--group-by method` no longer drops non-HTTP operations: protobuf `RPC` and
  GraphQL root-field sections follow the HTTP methods, sorted by label.
  OpenAPI output is unchanged. (Merged with #133 after the 2.0.0 tag.)
- The changelog no longer lists GraphQL under 2.0.0 (#136).
- `vimanam diff --format json` (contract v2) is byte-deterministic:
  schema keywords and Swagger 2 parameter keywords kept in the models'
  `extensions` maps serialized in random `HashMap` order, so entity attributes
  and pagination details of the same inputs could differ in key order between
  runs. They are now sorted. `ve2_`/`vc2_` IDs were already stable (they hash
  canonical JSON); Markdown and `--contract 1` output are unchanged.

## [2.0.0] - 2026-10-14

GraphQL support (#125, #133) is not part of this release: it merged after the
`v2.0.0` tag and is listed under [Unreleased].

### Breaking

- `vimanam diff --format json` now emits JSON contract v2 by default, for
  OpenAPI inputs too. Scripts that parse the v1 envelope must pass
  `--contract 1`, which keeps the v1 shape, change IDs and verdicts. The
  library's v1 entry points (`diff::diff`, `diff::json::to_json`) are
  unchanged.

### Added

- Protocol Buffers/gRPC source and descriptor-set inputs (#126), with imported
  proto2/proto3 symbols, native field/enum identities, source/wire compatibility
  verdicts, scoped `--breaking` gates, neutral v2 JSON, Markdown/report and
  focused budget/Skill output. Source input requires protoc; Editions are
  explicitly rejected by the current descriptor adapter. Git imports use
  committed snapshots. Real public gRPC health release fixtures cover parity.

- Format-neutral contract and change model (#124), with an OpenAPI adapter,
  adapter-owned compatibility verdicts, stable entity/change IDs, native
  locations, and JSON contract v2. Includes independent named-type facts,
  operation service membership changes, and conservative pagination inference.

### Changed

- `vimanam diff --format json` now defaults to contract v2, including OpenAPI
  inputs. Select `--contract 1` for the legacy v1 envelope, change IDs, and
  verdicts. Existing OpenAPI library entry points and Markdown output remain
  available. GraphQL parsing is a separate follow-up.

### Fixed

- Neutral contract v2 classifies `deprecated` on single-schema keywords,
  attributes request-body schema changes to the first body argument, and
  sorts pagination continuation names so declaration order is not a change.

## [1.7.0] - 2026-10-07

### Added

- `vimanam diff --from-ref OLD --to-ref NEW` (#113) compares committed specs
  through the existing Markdown/JSON, deltas and breaking-change pipeline.
  Unique-candidate discovery at the newer ref and `--spec PATH` follow rename
  history; paired `--from-spec`/`--to-spec` paths bypass filename matching for
  moves combined with rewrites. Git input errors leave output files untouched.
- Core `gitrefs::Repository` API (#112) discovers Git worktrees, lists committed
  JSON/YAML OpenAPI candidates, and materializes exact spec bytes at arbitrary
  refs. Explicitly anchored rename tracking supports ancestor and sibling-branch
  comparisons, reporting missing or ambiguous lineage rather than guessing.
  Uses Git on PATH without new dependencies, checkout changes or CLI flags.

### Changed

- Backward Git-ref comparisons over complete linear history use a file-focused
  `git log --follow` fast path (#121), verifying file changes against the same
  parent-edge rename rules. Merge, sibling-branch, forward and shallow histories
  retain the merge-aware traversal. See `scripts/README.md` for reproducible
  wall-time and Git subprocess measurements on generated scratch repositories.

## [1.6.0] - 2026-10-07

### Added

- Rust library target (#117): parse OpenAPI files or bytes, inspect the normalized
  `ApiDocumentation` model, compute typed contract diffs and deltas, and render
  focused operation/schema context with the existing detail and token-budget options.
- JSON contract types support both `Serialize` and owned `Deserialize`, including
  the distinction between missing values and explicit JSON null. Stable change IDs
  and JSON `schema_version` remain unchanged. Growing change-kind, schema-target,
  and schema-operation enums are non-exhaustive for downstream consumers.
- Default `cli` feature contains command-line dependencies and the binary. Consumers
  can use `vimanam = { version = "1.6", default-features = false }` without `clap`,
  `clap_complete`, or `env_logger`. Rendering accepts a writer and optional notice
  callback; library code writes nothing directly to stdout or stderr.

### Changed

- The CLI delegates to the library. Generated Markdown, JSON contract data, and
  exit codes are preserved; JSON generator metadata reports the new package version.

## [1.5.0] - 2026-10-02

### Added

- `--costs`: a token-cost dry run (#47, #104). Prints per-endpoint slice costs with
  their share of a whole-document TOTAL, per-schema definition costs and
  use-site counts, a clearly-labeled inline-expansion model with an
  amplification ratio (cycle-guarded), and hotspot rankings. Estimates use the
  existing characters/4 heuristic over rendered output; rows overlap through
  the document frame and shared schemas, and the report says so. Honors the
  configured detail level, filters and schema mode (`--inline-schemas`
  included); conflicts with `-o`, `--max-tokens`, split/Skill output,
  `--stats` and schema selection.

## [1.4.2] - 2026-09-30

### Changed

- Internal layout only. The diff implementation lives in `src/diff/`, CLI tests
  live in `tests/cli/`, and schema composition rendering uses one shared loop.
  Generated output matches v1.4.1 byte for byte (#103).

## [1.4.1] - 2026-09-30

### Changed

- Refresh transitive crates in `Cargo.lock` to the latest versions compatible
  with Rust 1.96. Direct dependency requirements are unchanged, and generated
  Markdown is unchanged. This is the baseline for later refactors that must
  keep byte-for-byte output parity.

## [1.4.0] - 2026-09-30

### Added

- `--split service|tag|endpoint -o DIR` writes linked pages: a compact `index.md`,
  complete guidance in `api.md`, and shared `schemas/` pages. `--overview-max-tokens`
  budgets only the index; when entries do not fit, `index-all.md` keeps full
  navigation. Detail pages stay at the requested level. A manifest removes only
  unchanged generated files and refuses to overwrite edited or unmanaged paths (#67).
- `--output-mode skill -o DIR` writes an agent-navigable tree: `SKILL.md` with
  YAML frontmatter, service and schema hubs, and one shared endpoint file per
  operation. Hub entries show an approximate characters/4 read cost for that
  file alone. `--overview-max-tokens` limits only `SKILL.md` (default 1600);
  complete service navigation stays in `index.md` (#68).
- `--schema NAME` and `--schema-field NAME#JSON_POINTER` (both repeatable) read
  a named schema or a subtree with its ancestors, types, requiredness,
  descriptions, and enums. Pointers cross `$ref`s. Invalid selectors fail
  before any output file is created. These reads always use full schema
  metadata and omit the hygiene report (#101).
- `--schema-depth N` (0 through 24) bounds property, array, composition,
  additional-properties, and `$ref` expansion from the original roots, including
  deferred definitions and split/Skill pages. At the limit, rows keep their
  type and metadata, say what was omitted, and give a retrieval command. Links
  point only at schemas that were actually written. Explicit field selections
  stay intact under `--max-tokens`, including budget zero, with an over-budget
  notice instead of a lower-detail fallback (#101).

## [1.3.0] - 2026-09-26

### Added

- `--operation "<METHOD> <PATH>"` and `--operation-id <ID>` (both repeatable)
  render exactly the selected operations: the path must equal the spec's path
  template byte for byte (method case-insensitive, Swagger 2 `basePath`
  excluded), so `diff --format json` endpoints can be passed straight back. The
  selection is the union of both flags, ANDed with the other filters; a value
  matching no operation in the spec exits 1 (with a "did you mean" hint when one
  is close), one removed by another filter warns on stderr, and the service and
  summary views omit services the selection leaves empty (#99).

## [1.2.0] - 2026-09-26

### Added

- `vimanam diff --format json`: a machine-readable diff document (pretty-printed,
  trailing newline) with per-spec `file_sha256` over the raw input bytes, a
  one-to-one `kind`/`details` contract for every change, `target`/`member`
  classification and `before`/`after` presence encoding for schema changes, and
  a stable, content-derived `vc1_` change ID per record so CI tools and coding
  agents can track changes across runs (#98).

## [1.1.0] - 2026-09-17

### Added

- `--toc` flag as the explicit opposite of `--no-toc`; when both are given, the
  later one wins.
- A "no effect" warning when `--required-only` is combined with `--detail basic`
  or `--detail summary`, matching the existing `--include-schemas`/`--include-examples`
  warnings.
- `vimanam completions <SHELL>` subcommand printing bash/zsh/fish/PowerShell/Elvish
  completion scripts to stdout (#41).
- A spec hygiene report, appended after the documentation and separated by a
  horizontal rule, that counts and lists operations missing a description,
  `operationId` or documented responses, deprecated and untagged operations,
  duplicate `operationId`s, and parameters without a description. It covers the
  same endpoints the documentation does (filters narrow it too). On by default;
  `--no-report` disables it (#44).
- `--stats` dry run: prints a plain-text table of visible endpoint counts and
  estimated token sizes (chars/4) per service, plus a whole-document TOTAL,
  instead of Markdown, honouring `--detail`, grouping and filter flags. Sizes
  slices before choosing `--service-filter`/`--detail`/`--max-tokens`. Conflicts
  with `-o` and `--max-tokens`; the hygiene report is never included (#42).
- `vimanam diff <OLD> <NEW>` subcommand comparing two versions of a spec.
  Endpoints are matched by method and path, parameters by name and location,
  responses by status code; request and response bodies are compared as fully
  resolved schemas, so a change behind a shared `$ref` is reported on every
  endpoint that references it. Each change is classified as breaking,
  non-breaking or needing review; `--fail-on-breaking` exits 3 when a breaking
  change is found, `--report` appends hygiene-count and token-estimate deltas,
  and `-o` writes the report to a file (#45).

### Changed

- Output now ends with the spec hygiene report unless `--no-report` is given.
  The report is not counted against `--max-tokens`, so pass `--no-report` when
  the whole output must fit the budget (for example when feeding it to an LLM).

### Fixed

- Swagger 2.0 `schemes` is now respected when building the server URL from
  `host`/`basePath`; previously plain-HTTP specs were rendered with an assumed
  `https://` prefix.

## [1.0.1] - 2026-07-07

### Changed

- `Response` model now has a dedicated `$ref` field instead of relying on
  flattened extensions, for consistency with other ref-bearing types.
- MSRV documentation updated from 1.85 to 1.96 (actual MSRV as of 1.0.0).

This version was tagged and published as a GitHub release, but its crates.io
publish failed, so 1.0.1 never appeared on crates.io. Its code changes were
already on `main`; only the version bump was missing.

## [1.0.0] - 2026-06-30

### Added

- YAML input support: specs can now be supplied as `.yaml`/`.yml` in addition to
  `.json`, for both OpenAPI 3.x and Swagger 2.0. Format is detected by extension
  (case-insensitive) and falls back to the other parser for misnamed or
  extension-less files. YAML and JSON inputs produce byte-identical output (#4).

### Changed

- Adopted the Rust 2024 edition and raised the MSRV to 1.96.
- Replaced the deprecated `serde_yaml` dependency with the maintained `serde_norway`
  fork, which better handles YAML 1.1 quoting footguns (the "Norway problem").

### Security

- All GitHub Actions across CI and the dist release workflow are now pinned to full commit
  SHAs, and a `pin-check` CI job enforces it. dist does not SHA-pin its generated workflow, so
  `release.yml` is hand-pinned after generation; re-running `dist generate` reverts the pins
  (the `pin-check` job catches it).

## [0.6.0] - 2026-06-26

### Added

- Release automation via [`dist`](https://github.com/axodotdev/cargo-dist). Each `v*`
  tag now also publishes shell + PowerShell install scripts and a Homebrew formula
  (`brew install noemaforge/tap/vimanam`), and `cargo binstall vimanam` works against
  the released binaries — alongside the existing crates.io publish and prebuilt
  archives (#32, #25, #26).
- `aarch64-unknown-linux-gnu` (ARM64 Linux) release binaries.

### Changed

- Release archives are now `.tar.xz` named `vimanam-<target-triple>` (dist's
  convention) and additionally bundle `CHANGELOG.md`; previously `.tar.gz` named
  `vimanam-<version>-<target-triple>`. Checksums (`.sha256`) and a combined
  `sha256.sum` are still published.
- crates.io publishing moved from the old hand-rolled release workflow to a dedicated
  `publish-crate.yml`, since `dist` does not publish to crates.io.

## [0.5.1] - 2026-06-24

### Fixed

- Parameters given as a `$ref` (`#/components/parameters/...`) are now resolved
  during parsing; previously such specs failed to parse entirely because the
  bare `$ref` parameter carries no `name`/`in` (#48)
- Path items given as a `$ref` (`#/components/pathItems/...`) now contribute
  their operations instead of being silently dropped; an unresolvable path-item
  reference is logged and skipped (#50)
- OpenAPI 3.1 `type` arrays (e.g. `["string", "null"]`) now parse instead of
  failing; multiple non-null members render as a pipe-separated union such as
  `string | integer` (#51)
- Operations missing a `responses` object no longer abort the entire parse (#56)
- An operation-level parameter now overrides a path-level one of the same
  `(name, in)` instead of both rendering as duplicate rows (#54)
- Operations tagged with a value not in the declared `tags` list now get their
  own service section instead of being silently reassigned to the first service;
  service extraction is also `$ref`-aware so tags inside referenced path items
  are seen (#60)

### Changed

- Release artifacts are now compressed archives instead of bare binaries:
  `.tar.gz` for Linux/macOS and `.zip` for Windows, named
  `vimanam-<version>-<target-triple>` (cargo-binstall-friendly) and bundling the
  binary, `README.md`, and `LICENSE`. Each archive ships with a matching
  `.sha256` checksum, which downstream package managers need to verify
  downloads (#24)

## [0.5.0] - 2026-06-22

### Added

- `--include-examples` is now implemented: at `--detail full` it renders request
  and response examples as fenced JSON blocks, pulling from media-type `example`
  and `examples` and resolving `$ref`s into `components/examples`. It previously
  printed only a placeholder (#6)
- `--group-by path` groups endpoints by path, emitting one section per path with
  its methods listed underneath, in spec order (#8)
- `--max-tokens <N>` fits output to a token budget: it renders at the requested
  `--detail` level and, if the estimated token count (a chars/4 heuristic) is
  over budget, steps the detail level down (full → standard → basic → summary)
  until it fits, reporting any reduction on stderr (#7)

### Changed

- The `examples` maps on media types and `components.examples` switched from
  `HashMap` to `IndexMap`, so rendered examples preserve spec order and keep the
  output-determinism guarantee

### Fixed

- `--required-only` now also drops parameters whose `required` is unspecified,
  not only those explicitly marked `required: false`
- A `requestBody` given as a `$ref` (`#/components/requestBodies/...`) is now
  resolved during parsing; previously such specs failed to parse entirely
  because the referenced body carries no inline `content`

### Internal

- The ~1200-line `markdown.rs` was split into a `markdown/` module (`views`,
  `endpoint`, `schema`, `examples`) behind the unchanged `generate_markdown`
  entry point, and shared preamble, endpoint-filter, HTTP-method-list, and
  JSON-pointer helpers were de-duplicated. No behavior change.
- `Example` and `MediaType` gained `#[serde(flatten)]` extension maps, so
  unknown vendor (`x-*`) fields are preserved rather than dropped, matching the
  other model structs.

## [0.4.0] - 2026-06-16

### Fixed

- Fatal errors are now printed to stderr regardless of the `RUST_LOG` setting,
  instead of being silently swallowed when logging was not enabled (#14)
- `--method-filter` is now case-insensitive; methods are stored uppercase, so a
  lowercase value such as `--method-filter get` previously matched nothing and
  silently produced empty output (#13)
- `--service-filter` is now case-insensitive, for the same reason (#19)
- `clean_for_id` now collapses runs of 3+ consecutive separators into a single
  dash, so anchor IDs derived from inputs like `a///b` are clean (#15)
- The `## Authentication` section is now emitted in spec order and is
  deterministic across runs; `security_schemes` switched from `HashMap` to
  `IndexMap`, and `serde_json`'s `preserve_order` feature keeps OpenAPI 2.0
  `securityDefinitions` in declaration order rather than alphabetical (#16)
- The table of contents and body sections now share one endpoint ordering in
  every view, so TOC anchor links always point to the corresponding section in
  document order (#18)

### Changed

- Schema composition variant indices (`allOf`/`oneOf`/`anyOf`) are now 0-based
  (`allOf[0]`, `allOf[1]`, ...) to match JSON Pointer/jq conventions (#21)

### Performance

- `$ref` resolution no longer re-serializes the entire spec on every reference;
  the spec is serialized to JSON once per parse, making `$ref`-heavy large specs
  significantly faster (#17)

### Internal

- `--group-by` is no longer wrapped in a misleading `Option` (it always has a
  clap default), removing an unreachable fallback branch (#20)

## [0.3.0] - 2026-06-15

### Added

- Schema expansion at `--detail full --include-schemas` (#5): the `Schema` model
  now captures `title`, `description`, `format`, `properties`, `items`,
  `required`, `allOf`/`oneOf`/`anyOf`, `enum`, `nullable`, and
  `additionalProperties`, and request/response schemas are rendered as nested
  field tables instead of a one-line type or reference name. `$ref`s are
  resolved against `components.schemas` (OpenAPI 3) and `definitions`
  (OpenAPI 2), with cycle detection and a depth guard so self-referential
  schemas terminate cleanly

### Changed

- `--detail full --include-schemas` output format: request/response schemas now
  render as `| Field | Type | Required | Description |` tables instead of the
  previous single-line `// Schema type:` / `// Reference:` comment

## [0.2.2] - 2026-06-11

### Added

- `--version` / `-V` flag reporting the crate version (#3)
- crates.io publishing: registry metadata (keywords, categories) and an
  automated `cargo publish` job on release tags (#10)
- This changelog; release notes are now generated from it

## [0.2.1] - 2026-06-11

### Fixed

- Optional request bodies (no `required: true`) are now documented; they were
  previously dropped from the parameter table entirely, and the Required
  column now reflects the spec instead of always saying Yes
- Output is deterministic: `paths`, `responses`, and `content` preserve spec
  order via `IndexMap`, so identical inputs produce byte-identical Markdown

### Added

- Integration test suite (14 tests) with OpenAPI 2.0 and 3.0 fixtures,
  including determinism and request-body regression tests
- Working `--flat` grouping (previously a placeholder)
- CI workflow: fmt, clippy, and tests on stable, plus an MSRV (1.85) build
- README section on preparing API context for LLMs
- Doc comments across modules

### Changed

- Dependencies updated (clap 4.6, env_logger 0.11, indexmap 2.14, and others);
  `rust-version = "1.85"` declared
- Release workflow modernized: SHA-pinned actions, `dtolnay/rust-toolchain`,
  and `gh` CLI instead of deprecated/archived actions

### Removed

- Unimplemented flags: `--format`, `--template`, `--group-by path|tag`
- Unused dependencies `thiserror` and `path-clean`

## [0.2.0] - 2025-03-18

### Added

- OpenAPI 3.0 support (#1): `openapi` version field, `servers`, `components`,
  `requestBody`, and security schemes

## [0.1.1] - 2025-03-07

### Added

- macOS ARM64 release binaries

## [0.1.0] - 2025-03-07

### Added

- Initial release: OpenAPI 2.0 (Swagger) JSON to Markdown with grouping,
  filtering, sorting, and detail levels

[Unreleased]: https://github.com/noemaforge/vimanam/compare/v2.2.0...HEAD
[2.2.0]: https://github.com/noemaforge/vimanam/compare/v2.1.0...v2.2.0
[2.1.0]: https://github.com/noemaforge/vimanam/compare/v2.0.0...v2.1.0
[2.0.0]: https://github.com/noemaforge/vimanam/compare/v1.7.0...v2.0.0
[1.7.0]: https://github.com/noemaforge/vimanam/compare/v1.6.0...v1.7.0
[1.6.0]: https://github.com/noemaforge/vimanam/compare/v1.5.0...v1.6.0
[1.5.0]: https://github.com/noemaforge/vimanam/compare/v1.4.2...v1.5.0
[1.4.2]: https://github.com/noemaforge/vimanam/compare/v1.4.1...v1.4.2
[1.4.1]: https://github.com/noemaforge/vimanam/compare/v1.4.0...v1.4.1
[1.4.0]: https://github.com/noemaforge/vimanam/compare/v1.3.0...v1.4.0
[1.3.0]: https://github.com/noemaforge/vimanam/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/noemaforge/vimanam/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/noemaforge/vimanam/compare/v1.0.1...v1.1.0
[1.0.1]: https://github.com/noemaforge/vimanam/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/noemaforge/vimanam/compare/v0.6.0...v1.0.0
[0.6.0]: https://github.com/noemaforge/vimanam/compare/v0.5.1...v0.6.0
[0.5.1]: https://github.com/noemaforge/vimanam/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/noemaforge/vimanam/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/noemaforge/vimanam/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/noemaforge/vimanam/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/noemaforge/vimanam/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/noemaforge/vimanam/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/noemaforge/vimanam/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/noemaforge/vimanam/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/noemaforge/vimanam/releases/tag/v0.1.0
