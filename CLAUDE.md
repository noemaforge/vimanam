# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Vimanam is a Rust CLI that converts OpenAPI/Swagger JSON specs (both 2.0 and 3.0+) into Markdown documentation, with grouping, filtering, sorting, and detail-level options.

## Commands

```bash
cargo build                          # debug build
cargo build --release                # release build
cargo run -- <spec.json> -o out.md   # run during development
cargo test                           # integration tests in tests/cli.rs against tests/fixtures/*.json
cargo test optional_request_body     # run a single test by name substring
cargo fmt && cargo clippy            # format / lint
```

CI (`.github/workflows/ci.yml`) runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` on every push and pull request, plus an MSRV build on Rust 1.85. The release workflow (`.github/workflows/release.yml`) fires on a `v*` tag: it verifies the tag matches the `Cargo.toml` version, publishes to crates.io, and builds cross-platform binaries packaged as compressed archives with SHA256 checksums.

Real-world specs for manual testing may live at the repo root: `swagger.json` (small), `openapi.json` and `openapiv2.swagger.json` (~3 MB each). These and the root `*.md` sample outputs (`summary*.md`, `basicoutput.md`, `fulloutput.md`, etc.) are gitignored local artifacts — they won't exist in a fresh clone, and they're not docs to edit.

## Architecture

Pipeline in `main.rs`: CLI args → `config::build_config` → `parser::parse_openapi` → `markdown::generate_markdown`.

- `src/config.rs` — clap `Cli` struct and `*Arg` value enums, converted into the internal `DocConfig`. Grouping precedence in `build_config`: `--flat` > `--method` > `--group-by` > default (service).
- `src/models.rs` — two layers of types: serde structs mirroring the OpenAPI spec (`OpenApiSpec`, `Operation`, `Parameter`, ...) and the spec-version-agnostic intermediate representation (`ApiDocumentation`, `Endpoint`, `Service`, `DocConfig` enums). The IR is what the generator consumes.
- `src/parser.rs` — deserializes the spec and flattens it into the IR. Handles both versions via serde alias (`swagger`/`openapi` field), derives "services" from tags (falling back to per-operation tags, then a default `"API"` service), merges path-level and operation-level parameters, represents an OpenAPI 3.0 `requestBody` as a synthetic `body` parameter, and resolves `$ref`s during extraction. On deserialize failure it re-parses as generic JSON to produce targeted error messages.
- `src/markdown/` — markdown generation, split into submodules behind the `generate_markdown` entry point in `mod.rs` (which also holds the `--max-tokens` budget logic that re-renders at progressively lower detail). `views.rs` has one `generate_*` function per grouping mode plus shared preamble/filter/sort helpers; `endpoint.rs` renders a single endpoint, branching on `DetailLevel`; `schema.rs` renders schema field tables; `examples.rs` renders example blocks. Writes through a generic `W: Write` (file or stdout).
  - Schema rendering is memoized per document via `schema::SchemaContext`: each body view (the four that render endpoints) creates one context, threads `&mut` it through `write_endpoint` → `write_schema_table`, then calls `render_schema_definitions` to emit the trailing `## Schema Definitions` section. By default a component `$ref` renders as a one-row link (`[Name](#schema-name)`) and is expanded once in that section; `--inline-schemas` flips the context to the old fully-inline expansion (with `ref_stack` cycle detection). Endpoint heading anchors come from `views::endpoint_anchor`, which scopes by service in the service view so a multi-tag operation rendered under several services keeps unique anchors.
- `src/utils.rs` — `$ref` resolution against `components`/`definitions`, server URL and security-scheme extraction, anchor-id cleaning, response content-type detection.

Unknown spec fields are preserved via `#[serde(flatten)] extensions` maps on most model structs rather than failing deserialization.

## Gotchas

- `--include-schemas` and `--include-examples` only take effect at `--detail full`. `--inline-schemas` only changes how `--include-schemas` renders, so it does nothing on its own.
- Output determinism is a tested invariant (`output_is_deterministic` in `tests/cli.rs`): `paths`, `responses`, and `content` use `IndexMap` to preserve spec order. Don't swap them back to `HashMap`.
- The blanket `*.md` ignore in `.gitignore` has explicit exceptions for `README.md`, `CLAUDE.md`, and `tests/fixtures/**` — new tracked markdown or fixtures need an exception too.
