# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.3] - 2026-06-15

### Added

- Schema expansion at `--detail full --include-schemas` (#5): the `Schema` model
  now captures `title`, `description`, `format`, `properties`, `items`,
  `required`, `allOf`/`oneOf`/`anyOf`, `enum`, `nullable`, and
  `additionalProperties`, and request/response schemas are rendered as nested
  field tables instead of a one-line type or reference name. `$ref`s are
  resolved against `components.schemas` (OpenAPI 3) and `definitions`
  (OpenAPI 2), with cycle detection and a depth guard so self-referential
  schemas terminate cleanly

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

[0.2.3]: https://github.com/nrynss/vimanam/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/nrynss/vimanam/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/nrynss/vimanam/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/nrynss/vimanam/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/nrynss/vimanam/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/nrynss/vimanam/releases/tag/v0.1.0
