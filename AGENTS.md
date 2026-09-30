# AGENTS.md

This file provides guidance to Codex (Codex.ai/code) when working with code in this repository.

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

Pipeline in `main.rs`: CLI args → `config::build_config` → `parser::parse_openapi` → `markdown::generate_markdown` → `report::write_report` (unless `--no-report`). Three paths short-circuit before the pipeline: the `completions <SHELL>` subcommand (`Commands` enum in `config.rs`, rendered with `clap_complete`), the `diff <OLD> <NEW>` subcommand (`Commands::Diff(DiffArgs)`, handled by `main::run_diff`), and `--stats`, which computes `stats::compute` and prints a table instead of Markdown. `run()` returns `Result<ExitCode>`; `main` maps errors to exit 1.

- `src/config.rs` — clap `Cli` struct and `*Arg` value enums, converted into the internal `DocConfig`. Grouping precedence in `build_config`: `--flat` > `--method` > `--group-by` > default (service). `input` is `Option<PathBuf>` marked `required = true` with `subcommand_negates_reqs` on the command, so `vimanam completions bash` needs no FILE while bare `vimanam` still errors; `args_conflicts_with_subcommands` rejects mixing conversion flags with a subcommand.
- `src/models.rs` — two layers of types: serde structs mirroring the OpenAPI spec (`OpenApiSpec`, `Operation`, `Parameter`, ...) and the spec-version-agnostic intermediate representation (`ApiDocumentation`, `Endpoint`, `Service`, `DocConfig` enums). The IR is what the generator consumes.
- `src/parser.rs` — deserializes the spec and flattens it into the IR. Handles both versions via serde alias (`swagger`/`openapi` field), derives "services" from tags (falling back to per-operation tags, then a default `"API"` service), merges path-level and operation-level parameters, represents an OpenAPI 3.0 `requestBody` as a synthetic `body` parameter, and resolves `$ref`s during extraction. On deserialize failure it re-parses as generic JSON to produce targeted error messages.
- `src/markdown/` — markdown generation, split into submodules behind the `generate_markdown` entry point in `mod.rs` (which also holds the `--max-tokens` budget logic that re-renders at progressively lower detail). `views.rs` has one `generate_*` function per grouping mode plus shared preamble/filter/sort helpers; `endpoint.rs` renders a single endpoint, branching on `DetailLevel`; `schema.rs` renders schema field tables; `examples.rs` renders example blocks. Writes through a generic `W: Write` (file or stdout).
  - Schema rendering is memoized per document via `schema::SchemaContext`: each body view (the four that render endpoints) creates one context, threads `&mut` it through `write_endpoint` → `write_schema_table`, then calls `render_schema_definitions` to emit the trailing `## Schema Definitions` section. By default a component `$ref` renders as a one-row link (`[Name](#schema-name)`) and is expanded once in that section; `--inline-schemas` flips the context to the old fully-inline expansion (with `ref_stack` cycle detection). Endpoint heading anchors come from `views::endpoint_anchor`, which scopes by service in the service view so a multi-tag operation rendered under several services keeps unique anchors.
- `src/report.rs` — Spec Hygiene Report: `analyze(&ApiDocumentation, &DocConfig) -> HygieneReport` (counts plus lists, data-only so a future TUI can reuse it) and `write_report`. Analyzes the same filtered endpoint set the body rendered (`markdown::visible_endpoints`); service count is services with at least one visible endpoint. `Endpoint.untagged` is set by the parser when the default-service fallback fired, which is the only way to tell an untagged operation from one tagged with the default name.
- `src/diff/` — `diff` subcommand: `diff(&old, &new) -> SpecDiff` (data-only `Vec<Change>`), `severity(&Change) -> Severity` (the whole breaking/non-breaking/review rule table lives in `mod.rs`, doc-commented and unit-tested row by row), `compute_deltas` for `--report`, and `write_diff` (in `render.rs`). The OpenAPI-agnostic JSON value differ (`diff_values`, `ValueChange`, `locate`) lives in `value.rs`; machine-readable `--format json` output lives in `json.rs`. Endpoints match on `(METHOD, path)`, parameters on `(name, in)`, responses on status code. Bodies are compared as resolved schemas: `utils::resolve_schema_value` inlines every `$ref` (ref-stack cycle guard, depth cap 24, unresolvable refs kept literal), `utils::canonicalize_schema_value` sorts `required`/`enum`, strips `description`/`title`/`example(s)`/`deprecated`/`x-*` from schema objects (not from property names) and sorts keys, then the generic `diff_values` yields JSON-pointer `ValueChange`s. OAS2 non-body parameter types are rebuilt from `type`/`format`/`items`/`enum` in `Parameter.extensions`.
- `src/stats.rs` — `--stats` dry-run: one row per visible service, `~TOKENS` = `markdown::estimate_tokens` of a real render narrowed to that service, `TOTAL` = one whole-document render (so rows need not sum to the total; multi-tag endpoints count once in TOTAL). Conflicts with `-o` and `--max-tokens`.
- `src/selection.rs` — diagnostics for `--operation`/`--operation-id` (the `OperationSelector` in `DocConfig`, applied as one more filter in `views::passes_filters`): `unmatched_selectors` checks every value against the whole spec before rendering (exit 1, with a deterministic edit-distance hint), and `filtered_out_warnings` names the filters (`views::removing_filters`) that removed a selected operation. `--operation` paths match `Endpoint::path` byte for byte — the same string `diff` reports, without Swagger 2 `basePath`.
- `src/utils.rs` — `$ref` resolution against `components`/`definitions`, server URL and security-scheme extraction, anchor-id cleaning, response content-type detection.

Unknown spec fields are preserved via `#[serde(flatten)] extensions` maps on most model structs rather than failing deserialization.

## Gotchas

- `src/markdown/schema_selection.rs` implements standalone `--schema NAME` and `--schema-field NAME#JSON_POINTER` reads, implying full metadata and suppressing endpoint hygiene. Repeatable pointers prune unrelated structural siblings while retaining ancestor metadata; pointers may cross refs and consumed-token recursive refs. Validate before creating any output. Selectors conflict with tree mode and service-oriented stats; exact operation selectors add context only. Explicit selections over `--max-tokens` stay intact with Markdown/stderr overage notices, even budget zero.
- `--schema-depth 0..=24` bounds property/items/composition/additionalProperties/ref edges from original roots, including linked definitions and split/Skill files. `SchemaContext::discover` relaxes minimum root depths transitively before rendering; split queues use minimum depths. Field-selector ancestor paths and leaf metadata survive smaller limits. All truncation has types/requiredness and source retrieval commands, never nonexistent fuller-file links. Without selectors, depth requires full detail with schemas. Default output is preserved; depth does not implicitly change full trees.

- `src/markdown/split.rs` renders `--split service|tag|endpoint -o DIR` trees separately from the single-file pipeline. `--overview-max-tokens` affects only `index.md`, with complete navigation retained in `index-all.md` when needed. Detail pages use the requested level and external `SchemaContext` links into shared `schemas/` pages. A hash manifest preflights all output paths, rejects symlinks and edited/unmanaged collisions, and cleans up only unchanged obsolete generated files. No directory-wide deletion. `--split` conflicts with `--max-tokens`, `--stats`, and `--inline-schemas`.

- `--output-mode skill -o DIR` uses `TreeLayout::Skill` over the endpoint split engine, adding YAML-frontmatter `SKILL.md` and service/endpoint/schema directory hubs. `apply_skill_profile` finalizes detail bytes before computing chars/4 read estimates in hubs, then measures hubs for root navigation (no circular cost annotations). `--overview-max-tokens` limits only `SKILL.md` (default 1600), with complete service navigation in `index.md`. One endpoint file is shared across multi-tag service hubs. Filters/detail flags and manifest protections are reused; skill mode conflicts with `--split`, `--max-tokens`, `--stats` and `--inline-schemas`.

- The hygiene report is appended by default to every conversion, outside the `--max-tokens` budget and separated from the body by exactly one blank line and `---` (`main::write_output` buffers the body to normalise trailing newlines). Tests asserting on body text usually don't need `--no-report`, but anything asserting `ends_with` or exact output does. `--stats` never emits the report.

- `diff --fail-on-breaking` exits 3 (not 1) when a breaking change exists, after writing the full report; "Needs review" changes never trip it. `diff` compares only the first media type of a request body or response (via `markdown::response_schema`), matches endpoints by exact path string (a path-template rename is remove+add), and its `-o` is its own flag — top-level conversion flags conflict with subcommands.
- `--include-schemas` and `--include-examples` only take effect at `--detail full`. `--inline-schemas` only changes how `--include-schemas` renders, so it does nothing on its own.
- Output determinism is a tested invariant (`output_is_deterministic` in `tests/cli/conversion.rs`): `paths`, `responses`, and `content` use `IndexMap` to preserve spec order. Don't swap them back to `HashMap`.
- The blanket `*.md` ignore in `.gitignore` has explicit exceptions for `README.md`, `AGENTS.md`, `CLAUDE.md`, `tests/fixtures/**`, `scripts/compare-fixtures/**`, and `scripts/README.md` — new tracked markdown or fixtures need an exception too.
