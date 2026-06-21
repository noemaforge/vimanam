# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[0.5.0]: https://github.com/nrynss/vimanam/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/nrynss/vimanam/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/nrynss/vimanam/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/nrynss/vimanam/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/nrynss/vimanam/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/nrynss/vimanam/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/nrynss/vimanam/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/nrynss/vimanam/releases/tag/v0.1.0
