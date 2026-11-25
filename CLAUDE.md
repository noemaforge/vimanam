# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Vimanam is a Rust CLI that converts OpenAPI/Swagger JSON specs (both 2.0 and 3.0+) into Markdown documentation, with grouping, filtering, sorting, and detail-level options.

## Commands

```bash
cargo build                          # debug build
cargo build --release                # release build
cargo run -- <spec.json> -o out.md   # run during development
cargo run -- diff old.json new.json  # compare two spec versions
cargo test                           # integration tests in tests/cli/ against tests/fixtures/*.json
cargo test optional_request_body     # run a single test by name substring
cargo fmt && cargo clippy            # format / lint
scripts/compare-baseline.sh          # byte-for-byte output comparison against a baseline binary (see scripts/README.md)
```

CI (`.github/workflows/ci.yml`) runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` on every push and pull request, plus an MSRV build on Rust 1.96. The release workflow (`.github/workflows/release.yml`) fires on a `v*` tag: it verifies the tag matches the `Cargo.toml` version, publishes to crates.io, and builds cross-platform binaries packaged as compressed archives with SHA256 checksums.

Real-world specs for manual testing may live at the repo root: `swagger.json` (small), `openapi.json` and `openapiv2.swagger.json` (~3 MB each). These and the root `*.md` sample outputs (`summary*.md`, `basicoutput.md`, `fulloutput.md`, etc.) are gitignored local artifacts — they won't exist in a fresh clone, and they're not docs to edit.

## Architecture

Pipeline in `cli::run` (called by `main.rs`): CLI args → `config::build_config` → `parser::parse_openapi` → `markdown::generate_markdown` → `report::write_report` (unless `--no-report`). Five paths short-circuit before the pipeline: the `completions <SHELL>` subcommand (`Commands` enum in `src/cli/config.rs`, rendered with `clap_complete` / `clap_complete_nushell`), the `man [--out-dir DIR]` subcommand (`Commands::Man`, rendered with `clap_mangen`), the `diff <OLD> <NEW>` subcommand (`Commands::Diff(Box<DiffArgs>)`, handled by `cli::run_diff`), `--stats`, which computes `stats::compute` and prints a table instead of Markdown, and `--costs`, which computes `costs::compute` and prints token-cost breakdowns. `cli::run` returns whether breaking changes need exit 3; `main` maps errors to exit 1. Default feature `cli` enables `clap`, `clap_complete`, `clap_complete_nushell`, `clap_mangen`, and `env_logger`; `cargo test --no-default-features` covers the core library without CLI dependencies.

- `src/cli/config.rs` — clap `Cli` struct and `*Arg` value enums, converted into the internal `DocConfig`. Grouping precedence in `build_config`: `--flat` > `--method` > `--group-by` > default (service). `input` is `Option<PathBuf>` marked `required = true` with `subcommand_negates_reqs` on the command, so `vimanam completions bash` needs no FILE while bare `vimanam` still errors; `args_conflicts_with_subcommands` rejects mixing conversion flags with a subcommand.
- `src/models.rs` — two layers of types: serde structs mirroring the OpenAPI spec (`OpenApiSpec`, `Operation`, `Parameter`, ...) and the spec-version-agnostic intermediate representation (`ApiDocumentation`, `Endpoint`, `Service`, `DocConfig` enums). The IR is what the generator consumes.
- `src/parser.rs` — deserializes the spec and flattens it into the IR. Handles both versions via serde alias (`swagger`/`openapi` field), derives "services" from tags (falling back to per-operation tags, then a default `"API"` service), merges path-level and operation-level parameters, represents an OpenAPI 3.0 `requestBody` as a synthetic `body` parameter, and resolves `$ref`s during extraction. On deserialize failure it re-parses as generic JSON to produce targeted error messages.
- `src/markdown/` — markdown generation, split into submodules behind the `generate_markdown` entry point in `mod.rs` (which also holds the `--max-tokens` budget logic that re-renders at progressively lower detail). `views.rs` has one `generate_*` function per grouping mode plus shared preamble/filter/sort helpers; `endpoint.rs` renders a single endpoint, branching on `DetailLevel`; `schema.rs` renders schema field tables; `examples.rs` renders example blocks. Writes through a generic `W: Write` (file or stdout).
  - Schema rendering is memoized per document via `schema::SchemaContext`: each body view (the four that render endpoints) creates one context, threads `&mut` it through `write_endpoint` → `write_schema_table`, then calls `render_schema_definitions` to emit the trailing `## Schema Definitions` section. By default a component `$ref` renders as a one-row link (`[Name](#schema-name)`) and is expanded once in that section; `--inline-schemas` flips the context to the old fully-inline expansion (with `ref_stack` cycle detection). Endpoint heading anchors come from `views::endpoint_anchor`, which scopes by service in the service view so a multi-tag operation rendered under several services keeps unique anchors.
- `src/report.rs` — Spec Hygiene Report: `analyze(&ApiDocumentation, &DocConfig) -> HygieneReport` (counts plus lists, data-only so a future TUI can reuse it) and `write_report`. Analyzes the same filtered endpoint set the body rendered (`markdown::visible_endpoints`); service count is services with at least one visible endpoint. `Endpoint.untagged` is set by the parser when the default-service fallback fired, which is the only way to tell an untagged operation from one tagged with the default name.
- `src/diff/` — `diff` subcommand: `diff(&old, &new) -> SpecDiff` (data-only `Vec<Change>`), `severity(&Change) -> Severity` (the whole breaking/non-breaking/review rule table lives in `mod.rs`, doc-commented and unit-tested row by row), `compute_deltas` for `--report`, and `write_diff` (in `render.rs`). The OpenAPI-agnostic JSON value differ (`diff_values`, `ValueChange`, `locate`) lives in `value.rs`; machine-readable `--format json` output lives in `json.rs`. Endpoints match on `(METHOD, path)`; `diff_with(.., EndpointMatching::Moves)` then pairs leftovers whose key is unique across each whole document (same method; operationId, then whole `v<digits>` segments; linear, HashMap-indexed) as `EndpointMoved`, attributing inner changes to the new endpoint. Parameters match on `(name, in)`, responses on status code. `$ref` targets are edges (#181): `resolve::PairResolver::localize` walks each use's old/new schemas in parallel, records a `RefEdge` wherever both sides reference, at a schema position of the `value::Node` grammar, a component their own documents declare (dropping `$ref`; non-annotation siblings form an overlay: compared with effective values, recorded as the edge's `mask`, which `Components` uses to hide target changes and edges under those keys), expands a one-sided ref only against an inline object (name stack guard, depth cap 64), inlines both sides fully outside the grammar (`Role::Value`: `default`, `const`, `if`, `prefixItems`, `$defs`, ...; no edge, Review by `locate`, #186; `required`/`enum` members stay as written and a ref-holding member's change is reported inside it via `localize_with_members`), and leaves annotations and one-sided subtrees as written; a pair with different names always gets a Review `/$ref` retarget change; ref-free values come back unchanged, which is why ref-free output is byte-identical to 2.2. `diff::components::Components` then compares each reached pair once (breadth-first, skipping `(X, X)` when `PairResolver::is_stable`), finds reaching uses by reverse BFS, and emits `ChangeKind::ComponentSchemaChanged` once per (pair, change, direction) after the endpoint changes, ordered by component position. `values_equal` still skips provably identical use roots. v1 JSON expands each component change into one record per use (`json_records`); v2 maps it to a type-subject fact with unhashed `uses` (`openapi::component_fact`). Full resolution (`DocResolver::resolve`) is test-only now; pagination still resolves the top `PAGINATION_LEVELS` levels. `utils::canonicalize_schema_value` sorts `required`/`enum`, strips `description`/`title`/`example(s)`/`deprecated`/`x-*` from schema objects (not from property names) and sorts keys, then the generic `diff_values` yields JSON-pointer `ValueChange`s. OAS2 non-body parameter types are rebuilt from `type`/`format`/`items`/`enum` in `Parameter.extensions`.
- `src/stats.rs` — `--stats` dry-run: one row per visible service, `~TOKENS` = `markdown::estimate_tokens` of a real render narrowed to that service, `TOTAL` = one whole-document render (so rows need not sum to the total; multi-tag endpoints count once in TOTAL). Conflicts with `-o` and `--max-tokens`.
- `src/costs.rs` — `--costs` dry-run (#47): per-endpoint slice costs, per-schema definition/use/amplification rows, and hotspots. Conflicts with `-o`, `--max-tokens`, `--stats`, split/Skill output, and schema selection; `--inline-schemas` is honored.
- `src/selection.rs` — diagnostics for `--operation`/`--operation-id` (the `OperationSelector` in `DocConfig`, applied as one more filter in `views::passes_filters`): `unmatched_selectors` checks every value against the whole spec before rendering (exit 1, with a deterministic edit-distance hint), and `filtered_out_warnings` names the filters (`views::removing_filters`) that removed a selected operation. `--operation` paths match `Endpoint::path` byte for byte — the same string `diff` reports, without Swagger 2 `basePath`.
- `src/utils.rs` — `$ref` resolution against `components`/`definitions`, server URL and security-scheme extraction, anchor-id cleaning, response content-type detection.

Unknown spec fields are preserved via `#[serde(flatten)] extensions` maps on most model structs rather than failing deserialization.

## Gotchas

- `src/markdown/schema_selection.rs` implements standalone `--schema NAME` and `--schema-field NAME#JSON_POINTER` reads, implying full metadata and suppressing endpoint hygiene. Repeatable pointers prune unrelated structural siblings while retaining ancestor metadata; pointers may cross refs and consumed-token recursive refs. Validate before creating any output. Selectors conflict with tree mode and service-oriented stats; exact operation selectors add context only. Explicit selections over `--max-tokens` stay intact with Markdown/stderr overage notices, even budget zero.
- `--schema-depth 0..=24` bounds property/items/composition/additionalProperties/ref edges from original roots, including linked definitions and split/Skill files. Unset, `markdown::resolve_schema_depth` auto-bounds a selected slice (`--schema`/`--schema-field`, or an operation selector at full+schemas) at `schema::AUTO_SCHEMA_DEPTH` (2) when its roots reach more than `AUTO_SCHEMA_DEPTH_THRESHOLD` (200) components; `render_resolved` then adds a blockquote under the title, and `render`, `--stats` and `--costs` emit the stderr notice. Stats/costs resolve once and measure every row/slice with `render_resolved`, never re-deciding the depth per slice. Whole documents and split/Skill trees are never auto-bounded. Retrieval hints add `--schema-depth` whenever a depth is active, and stderr omissions are grouped per command by `summarize_omissions` (distinct operation+field counts, "across N operations"). Operation body tables use the synthetic `(request body)`/`(response body)` root labels with unprefixed child paths. `SchemaContext::discover` relaxes minimum root depths transitively before rendering; split queues use minimum depths. Field-selector ancestor paths and leaf metadata survive smaller limits. All truncation has types/requiredness and source retrieval commands, never nonexistent fuller-file links. Without selectors, depth requires full detail with schemas. Default output is preserved; depth does not implicitly change full trees.

- `src/markdown/split.rs` renders `--split service|tag|endpoint -o DIR` trees separately from the single-file pipeline. `--overview-max-tokens` affects only `index.md`, with complete navigation retained in `index-all.md` when needed. Detail pages use the requested level and external `SchemaContext` links into shared `schemas/` pages. A hash manifest preflights all output paths, rejects symlinks and edited/unmanaged collisions, and cleans up only unchanged obsolete generated files. No directory-wide deletion. `--split` conflicts with `--max-tokens`, `--stats`, and `--inline-schemas`.

- `--output-mode skill -o DIR` uses `TreeLayout::Skill` over the endpoint split engine, adding YAML-frontmatter `SKILL.md` and service/endpoint/schema directory hubs. `apply_skill_profile` finalizes detail bytes before computing chars/4 read estimates in hubs, then measures hubs for root navigation (no circular cost annotations). `--overview-max-tokens` limits only `SKILL.md` (default 1600), with complete service navigation in `index.md`. One endpoint file is shared across multi-tag service hubs. Filters/detail flags and manifest protections are reused; skill mode conflicts with `--split`, `--max-tokens`, `--stats` and `--inline-schemas`.

- The hygiene report is appended by default to every conversion, outside the `--max-tokens` budget and separated from the body by exactly one blank line and `---` (`main::write_output` buffers the body to normalise trailing newlines). Tests asserting on body text usually don't need `--no-report`, but anything asserting `ends_with` or exact output does. `--stats` never emits the report.

- `diff --fail-on-breaking` exits 3 (not 1) when a breaking change exists, after writing the full report; "Needs review" changes never trip it. `diff` compares only the first media type of a request body or response (via `markdown::response_schema`), matches endpoints by exact path string plus unique move pairing in Markdown/v2 JSON (`--exact-paths` disables it; `--contract 1` and the library `diff`/`openapi::diff` defaults never pair), and its `-o` is its own flag — top-level conversion flags conflict with subcommands.
- `--include-schemas` and `--include-examples` only take effect at `--detail full`. `--inline-schemas` only changes how `--include-schemas` renders, so it does nothing on its own.
- Output determinism is a tested invariant (`output_is_deterministic` in `tests/cli/conversion.rs`): `paths`, `responses`, and `content` use `IndexMap` to preserve spec order. Don't swap them back to `HashMap`. The models' flattened `extensions` `HashMap`s serialize through `models::sorted_extensions` (and `contract::openapi::extensions_value` for `json!`), because `preserve_order` would otherwise leak random key order into v2 JSON.
- The blanket `*.md` ignore in `.gitignore` has explicit exceptions for `README.md`, `AGENTS.md`, `CLAUDE.md`, `tests/fixtures/**`, `scripts/compare-fixtures/**`, and `scripts/README.md` — new tracked markdown or fixtures need an exception too.

## Neutral JSON contracts

`src/contract/` adds the neutral surface, stable `ve2_` entity / `vc2_` change IDs,
format-native coordinates and adapter-owned verdicts. `contract::openapi` adapts
and compares OpenAPI beside unchanged `parse_openapi` / `diff::diff` APIs.
`diff --format json` defaults to `--contract 2`; `--contract 1` selects the
unchanged v1 serializer/IDs/verdict. V2 adds independent named-type/service facts
and conservative review-only pagination inference (normalised names, one nested
level, generic names need a token control or end flag; a side the 2.0.0 rule
matches emits exactly 2.0.0's evidence/confidence/rule, so 2.0.0 IDs hold when
it matches both sides or the other side has no evidence, but change when only
broader rules match the other side — intentional, tested);
moved operations: `openapi::diff_from` aligns old paths, then
`restore_removal_sides` gives removals old-side subjects/natives
(additions/changes stay new-side); v2 JSON breaking exits use
v2 summary verdicts. Markdown remains the existing OpenAPI report. Preserve
explicit v1 fixture expectations. See `docs/neutral-contract-v2.md` for identity,
input/output direction, pagination evidence and future GraphQL/protobuf mappings.


## Protobuf adapter

`src/contract/protobuf.rs` compiles `.proto` sources in pure Rust with `protox`
(including Google well-known imports and `source_code_info`, no external
`protoc` required) and Rust-decodes complete binary FileDescriptorSets.
Proto2/proto3 only: reject Editions/unknown syntax before prost-reflect, which
cannot safely decode their features. Preserve original descriptor bytes and
custom options. Neutral field IDs use message FQN+number; enum label IDs add
names to preserve aliases. Safe same-number renames break source/JSON only;
reserved-identity reuse, type and RPC streaming changes supply wire evidence.
`diff` drops findings whose subject lives only in bundled `google/protobuf/*`
files (compiler-version skew), unless neither input has a user file. Source-retention applied options are stripped in `adapt`
(protoc drops them from descriptors). Enum reserved
bounds use wide integers.

`--breaking` scopes protobuf failure gates only; `--contract 1` rejects protobuf
before output creation. `cli::diff_inputs` resolves committed regular proto
files directly from Git blobs in memory via a custom `protox` `FileResolver`,
using only ref-relative confined import roots. Do not read imports from current
worktree during Git comparisons or extend OpenAPI candidate discovery
implicitly. `gitrefs::Repository` clones share one session:
resolved refs, `ls-tree` listings and rename edges are cached per commit, and
blobs stream through one `git cat-file --batch` child (raw bytes, no
textconv/filters); any batch failure disables it and re-reads with the
original `git show`, so error text is unchanged. Rename edges likewise
prefetch through `git diff-tree --stdin` in chunks (`EDGE_BATCH`), caching
nothing on unexpected output so `edge_changes` falls back per edge. Pass the CLI's `Repository`
along rather than re-discovering it. Renderer IR uses native RPC/result labels; existing budgets/schema
selectors/Skill manifest protection apply. Tests in `tests/protobuf.rs` and
`tests/protobuf_cli.rs` cover pure-Rust source compilation, descriptor parity
(CI installs a pinned checksum-verified `protoc` solely for the `protoc`
descriptor parity test), and Git-ref resolution. See `docs/protobuf.md` and
fixture provenance for limitations and native rules.

## GraphQL adapter

`src/contract/graphql.rs` parses SDL with apollo-parser (multi-file merge,
schema and type extensions applied after all definitions, duplicate/unknown
types rejected)
and introspection JSON into one `Document`; built-in scalars and `__` types are
excluded so both sources compare equal. Defaults are canonical GraphQL
literals. `diff` owns the directional rule table and native rule names;
`adapt` produces the v2 surface (root fields are operations, coordinates like
`Type.field(arg:)`). Connection pagination needs pageInfo plus edges/node or a
`nodes` list, with first/after or last/before; the matched shape is in details,
not evidence. Renderer IR uses `QUERY`/`MUTATION`/`SUBSCRIPTION` methods with
coordinate paths and a `result` response. Introspection JSON is sniffed from
bytes (`"__schema"`), so OpenAPI JSON detection is unchanged. `diff --context`
is GraphQL-only. Tests: `tests/graphql.rs`, `tests/graphql_cli.rs`; see
`docs/graphql.md` and fixture provenance.

## Formal model (Lean)

`formal/` is a Lean 4 model of the `diff` severity rule table (`severity`,
the request/response schema tables, `became`/`at_default` (keyword defaults,
#184), `locate`, `under_not`, and the
`diff_values` enum reporting) with direction theorems; see
`formal/README.md`. No Mathlib, pinned toolchain, `lake build --wfail`, axiom
audit (`scripts/formal/check-axiom-audit.sh`). `lake exe oracle` writes
`formal/oracle/*.json`, which `src/diff/formal_oracle.rs` replays through the
real Rust in `cargo test`: a rule-table change that disagrees with the model
fails there. Changing a modelled item in `src/diff/` makes
`scripts/formal/check-model-drift.py` warn until the model is re-read and
bumped (`--bump Severity`); change the Lean model and regenerate the vectors
together with a deliberate rule change.
