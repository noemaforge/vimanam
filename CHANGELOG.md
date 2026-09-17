# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
