# Vimanam

[![CI](https://github.com/noemaforge/vimanam/actions/workflows/ci.yml/badge.svg)](https://github.com/noemaforge/vimanam/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/vimanam.svg)](https://crates.io/crates/vimanam)
[![License: Apache-2.0](https://img.shields.io/crates/l/vimanam.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.96-blue.svg)](Cargo.toml)

Vimanam is an OpenAPI/Swagger (JSON or YAML) to Markdown documentation generator.

Vimanam stands for Aeroplane in Malayalam. Like an aeroplane, it can fly high and give you a 20,000 feet view of the APIs. It can fly low and give you a detailed view of the APIs. You can also run it along the ground to look deep into the API fields and descriptions.

It supports both OpenAPI 2.0 (Swagger) and OpenAPI 3.0 specifications.

Besides producing documentation for humans, Vimanam is built for **feeding API specs to LLMs**: a multi-megabyte enterprise spec doesn't fit in a context window, but a filtered, summary-level Markdown rendering of it does. See [Preparing API context for LLMs](#preparing-api-context-for-llms).

## Features

- Convert OpenAPI JSON or YAML files to Markdown documentation (format detected by `.json`/`.yaml`/`.yml` extension, with automatic fallback)
- Supports both OpenAPI 2.0 (Swagger) and OpenAPI 3.0 specifications
- Group endpoints by service, HTTP method, or path, or list them flat
- Filter by service, path, or method
- Multiple detail levels (summary, basic, standard, full)
- Token-budget-aware output (`--max-tokens`): steps the detail level down until the rendering fits, and reports what was trimmed on stderr
- Linked directory output (`--split service|tag|endpoint`): a compact overview alongside requested detail pages and shared schema files, with an independent overview budget
- Agent navigation (`--output-mode skill`): a compact `SKILL.md`, service/operation/schema hubs and individual-file read costs
- Token-budget dry run (`--stats`): a per-service table of endpoint counts and estimated token sizes, for sizing slices before choosing filters
- Spec diffing (`vimanam diff old.json new.json`): compares two versions of a spec on *resolved* schemas — a change behind a shared `$ref` is reported on every endpoint that uses it — and classifies each change as breaking, non-breaking or needing review, with an exit code for CI (`--fail-on-breaking`)
- Spec hygiene report appended to every run (`--no-report` to skip): counts and lists operations missing a description, `operationId` or responses, deprecated and untagged operations, duplicate `operationId`s, and undescribed parameters
- Schema expansion at `--detail full --include-schemas`: renders request/response schemas as nested field tables. Shared component schemas are expanded once into a trailing "Schema Definitions" section and linked from each use site, keeping output compact when schemas are reused across endpoints; `--inline-schemas` instead expands every `$ref` inline at each use site (larger, fully self-contained, with cycle detection)
- Schema reads for oversized graphs: `--schema NAME` and `--schema-field NAME#POINTER` render one schema or subtree with its metadata, and `--schema-depth N` bounds expansion without dropping the selected fields
- Example rendering at `--detail full --include-examples`: emits request/response examples as fenced JSON blocks, resolving `$ref`s into `components/examples`
- Server URL information extraction and documentation
- Authentication and security schemes documentation
- Proper content type detection for responses
- Sorting options for endpoints (alphabetical, path length)
- Clean anchor generation for better navigation
- Deterministic, byte-identical output across runs — friendly to diffs, caching, and LLM prompt caching

## Installation

### Homebrew (macOS / Linux)

```bash
brew install noemaforge/tap/vimanam
```

No Rust toolchain required. Supports macOS (Apple Silicon & Intel) and x86_64 Linux.

### Install script (no toolchain)

```bash
# macOS / Linux
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/noemaforge/vimanam/releases/latest/download/vimanam-installer.sh | sh
```

```powershell
# Windows (PowerShell)
powershell -ExecutionPolicy ByPass -c "irm https://github.com/noemaforge/vimanam/releases/latest/download/vimanam-installer.ps1 | iex"
```

### From crates.io

```bash
cargo binstall vimanam   # prebuilt binary, no compile (needs cargo-binstall)
cargo install vimanam    # builds from source
```

### Prebuilt binaries

Download the archive for your platform (Linux, macOS Intel/ARM64, Windows) from the
[latest release](https://github.com/noemaforge/vimanam/releases/latest) — no Rust toolchain needed.
Archives are named `vimanam-<target-triple>.tar.xz` (`.zip` on Windows) and ship with a matching
`.sha256` checksum; each bundles the binary, `README.md`, `CHANGELOG.md`, and `LICENSE`. Extract it
and put the `vimanam` binary on your `PATH`.

### From source

Requires [Rust](https://www.rust-lang.org/tools/install) 1.96.0 or later.

```bash
# Clone repository
git clone https://github.com/noemaforge/vimanam.git
cd vimanam

# Run directly without building (development)
cargo run -- input.json -o output.md

# Build and run
cargo build --release
./target/release/vimanam input.json -o output.md

# Or install system-wide
cargo install --path .
```

### Development

```bash
cargo test
cargo fmt && cargo clippy
```

Integration tests live in `tests/cli/` and `tests/split.rs` and run against `tests/fixtures/`. `scripts/compare-baseline.sh` compares a fresh build with a released binary across the CLI matrix — stdout, stderr, exit code, and written files — so a refactor can be checked for byte-for-byte output parity. Setup and the case list are in [`scripts/README.md`](scripts/README.md).

### Shell completions

`vimanam completions <SHELL>` prints a completion script for `bash`, `zsh`, `fish`,
`powershell`, or `elvish`. Install it once and re-run after upgrading Vimanam:

```bash
# Bash (relies on the bash-completion v2 package, which stock macOS bash 3.2
# lacks; alternatively add `eval "$(vimanam completions bash)"` to ~/.bashrc)
mkdir -p ~/.local/share/bash-completion/completions
vimanam completions bash > ~/.local/share/bash-completion/completions/vimanam

# Zsh (make sure ~/.zfunc is on fpath before compinit runs in ~/.zshrc:
#   fpath+=~/.zfunc; autoload -Uz compinit; compinit)
mkdir -p ~/.zfunc
vimanam completions zsh > ~/.zfunc/_vimanam

# Fish
mkdir -p ~/.config/fish/completions
vimanam completions fish > ~/.config/fish/completions/vimanam.fish
```

```elvish
# Elvish: add this line to ~/.config/elvish/rc.elv
eval (vimanam completions elvish | slurp)
```

```powershell
# PowerShell: add this line to $PROFILE
vimanam completions powershell | Out-String | Invoke-Expression
```

## Usage

```bash
# Basic usage
vimanam input.json -o output.md

# Group by HTTP method
vimanam input.json --method -o output.md

# Group by path (one section per path, methods listed underneath)
vimanam input.json --group-by path -o output.md

# Generate summary only
vimanam input.json --detail summary -o output.md

# Fit the output to a token budget, stepping detail down as needed
vimanam input.json --detail full --max-tokens 8000 -o output.md

# Filter by specific services
vimanam input.json --service-filter Auth,Users -o output.md

# Filter by HTTP method
vimanam input.json --method-filter GET,POST -o output.md

# Show only paths containing a pattern
vimanam input.json --path-filter /api/v1 -o output.md

# Generate full details
vimanam input.json --detail full --include-schemas --include-examples -o output.md

# Include server and authentication information
vimanam input.json --include-auth -o output.md

# Print a shell completion script (see "Shell completions" above)
vimanam completions zsh

# Drop the spec hygiene report appended after the documentation
vimanam input.json --no-report -o output.md

# Size each service before choosing filters: endpoint counts and ~tokens per service
vimanam input.json --stats --detail standard

# Compare two versions of a spec; exit 3 if anything breaking changed
vimanam diff v1/openapi.json v2/openapi.json --report --fail-on-breaking
```

### Schema reads and expansion limits

Keep full split/Skill files available and request a smaller schema read when one file is too large:

```sh
vimanam spec.json --schema v1FindingSpec
vimanam spec.json --schema-field 'v1FindingSpec#/properties/finding_tags'
vimanam spec.json --schema-field 'v1FindingSpec#/properties/finding_tags/items/properties/tag'
vimanam spec.json --operation-id GetFinding --detail full --include-schemas --schema-depth 2
```

`--schema` and `--schema-field` are repeatable standalone reads. They always render full schema metadata and omit the endpoint hygiene report. Named schemas need not be reachable from an operation. Combining an exact operation selector adds a matched-operation context list; schema selection stays independent. Field reads retain ancestor types, descriptions, requiredness and enum values, pruning unrelated siblings and their references. A pointer crosses references automatically and selects schema subtrees using `/properties/NAME`, `/items`, `/additionalProperties`, or `/allOf/INDEX`, `/oneOf/INDEX`, `/anyOf/INDEX`. Escape property-name `~` as `~0` and `/` as `~1`; an empty pointer selects the complete schema. Invalid names/pointers fail before creating output. Both Swagger 2 definitions and OpenAPI 3 component schemas are supported.

`--schema-depth N` accepts 0 through 24, matching the recursion safety limit. A root has depth zero; each property, array item, composition variant, additional-properties schema and reference traversal adds one edge. At depth N, the row retains its type/reference, description and requiredness, but stops nested expansion and gives a full retrieval command. Depth zero keeps root metadata. Shared definitions inherit the shortest depth from the original roots, including across deferred definitions and split/Skill pages; limits never restart at every schema. Explicit field selectors protect their ancestor path and leaf metadata even below this depth; the limit then bounds expansion beyond that selection. Omissions are reported in Markdown and stderr.

Without standalone selectors, depth requires `--detail full --include-schemas`; it works with operation filters, `--stats` and split/Skill output. Selectors produce dedicated reads and conflict with split/Skill output and service-oriented `--stats`; use the commands in schema detail pages to read a field separately. Depth/field limits are opt-in; existing full rendering remains available when omitted.

For explicit schema reads, `--max-tokens` never drops selected metadata or lowers detail. If the selection exceeds the approximate characters/4 budget (including budget zero), Vimanam emits the complete requested read with an over-budget notice in Markdown and stderr. Narrow the field selector or explicitly set depth to reduce it. Other single-file output keeps its existing detail fallback, and split/Skill overview budgets affect only the overview.

## Spec hygiene report

Every run appends a short report after the documentation, separated by a horizontal rule, that flags common gaps in the spec: operations with no summary or description, no `operationId`, no documented responses, deprecated operations, operations with no tag (attributed to the default service), duplicate `operationId`s, and parameters without a description (a request body counts once per operation, however many media types it offers). It covers the same endpoints the documentation does, so `--service-filter`, `--path-filter`, `--method-filter` and `--exclude-deprecated` narrow the report too. Detail lists appear only for checks that found something.

```markdown
---

## Spec Hygiene Report

**6 endpoints** across **2 services**

| Check | Count |
|-------|------:|
| Missing description | 1 |
| Missing operationId | 1 |
| No responses documented | 1 |
| Deprecated | 2 |
| Untagged (no service tag) | 1 |
| Duplicate operationIds | 1 |
| Parameters without description | 3 |

### Missing description (1)
- `GET /health`

### Deprecated (2)
- `GET /ping`
- `DELETE /users/{id}`

### Duplicate operationIds (1)
- `getUser`
  - `DELETE /users/{id}`
  - `GET /users/{id}`

### Parameters without description (3)
- `GET /users` — `limit`
- `POST /users` — `requestBody`
- `DELETE /users/{id}` — `id`
```

Pass `--no-report` to omit it. The report is not counted against `--max-tokens` — the budget fits the documentation body only — so combine `--max-tokens` with `--no-report` when the whole output must stay within the budget.

### Linked pages with a compact overview

Keep complete API detail available on disk while loading only the relevant pages:

```bash
vimanam openapi.json --split endpoint -o ./api-docs \
  --detail full --include-schemas --include-examples --overview-max-tokens 2000
```

Start at `api-docs/index.md`. Its entries show method/path, operation ID and a brief description, and link to detail pages. The linked `api.md` retains the complete API description and global usage guidance outside the overview budget. Endpoint detail pages link to shared `schemas/*.md` files; each reachable schema is rendered once, including cyclic references. `--split service` produces one detail page per service, and `--split tag` uses the spec's tags (the parser's service names). Multi-tag operations appear under each selected service/tag. Endpoint splitting produces one page per operation. Filtering applies to the generated tree; omitted operations are disclosed in the overview with a command to retrieve the full tree.

`--detail` controls detail pages; the overview stays compact independently. `--overview-max-tokens` uses the approximate chars/4 estimate only for `index.md`. When operation entries do not fit, `index-all.md` retains complete navigation. Even a tiny budget preserves the navigation notice and can therefore exceed the estimate. Linked detail pages keep the requested detail level and schema tables. The hygiene report, when enabled, is a separate `report.md` page.

`--max-tokens` remains a single-document detail-fallback option and conflicts with `--split`; use `--overview-max-tokens` for a split overview. Split output uses shared schema links, so `--inline-schemas` also conflicts with `--split`. Schemas and examples still require `--detail full` and their respective inclusion flags. Each reduced-detail page tells the reader how to retrieve fuller detail into another directory.

Generated paths combine a readable slug with a hash of the operation, service name or schema reference. They stay stable when filters or unrelated operations change. Regeneration uses `.vimanam-manifest.json` to track owned files: it removes obsolete generated pages only when their contents are unchanged, preserves unrelated files, and refuses to overwrite edited generated pages or unmanaged collisions. Move edited pages aside or use a fresh output directory before regenerating. Symlink output paths and parents are rejected; use their resolved paths (for example `/private/tmp` instead of `/tmp` on macOS). Keep the manifest alongside the generated tree.

### Agent-navigable Skill tree

```bash
vimanam openapi.json --output-mode skill -o ./api-skill \
  --detail full --include-schemas --include-examples --include-auth \
  --overview-max-tokens 1600
```

Start with `api-skill/SKILL.md`, which has YAML frontmatter (`name`, `description`, API `version`) and explains how to choose reads. Follow a service hub to select an endpoint by method/path, operation ID and short description, then follow that endpoint's schema links as needed. `endpoints/index.md` lists all selected operations; `schemas/index.md` lists reachable shared schemas; `services/index.md` lists service hubs. Multi-tag operations share one endpoint file. `api.md` retains global API guidance.

Hub entries show the approximate cost of each linked file, using characters/4 rounded up from its final emitted contents. An endpoint estimate covers that endpoint file alone; reading linked schemas incurs their separate costs. Estimates help the agent choose what to load without loading the full referenced graph first.

`--overview-max-tokens` affects only `SKILL.md` (default estimate: 1600). When service entries do not fit, the root links to the complete `index.md` map; all detail files and directory hubs stay unchanged. Essential navigation may exceed an extremely small budget. Existing filters, sorting, detail levels and schema/example flags apply to detail files. Reduced-detail pages include retrieval commands, and omitted documentation is distinguished from content absent in the source spec. Schema tables still require `--detail full --include-schemas`. This profile uses the same stable paths and ownership protections as split output and conflicts with `--split`, `--max-tokens`, `--stats` and `--inline-schemas`.

## Options

```
Usage: vimanam [OPTIONS] <FILE>
       vimanam <COMMAND>

Commands:
  completions  Generate shell completions and print them to stdout
  diff         Compare two versions of a spec and report what changed, classified as breaking, non-breaking or needing review
  help         Print this message or the help of the given subcommand(s)

Arguments:
  <FILE>  Path to the OpenAPI JSON or YAML file

Options:
  -o, --output <FILE>                      Output file path, or directory for split/skill output
      --split <SPLIT>                      Write linked Markdown pages to the --output directory [service, tag, endpoint]
      --output-mode <OUTPUT_MODE>          Write an agent-navigable SKILL.md tree to the --output directory [skill]
      --overview-max-tokens <N>            Token budget for the split index or SKILL.md only; detail pages stay at the requested level
      --method                             Group endpoints by HTTP method instead of by service
      --group-by <service|method|path>     Grouping method for endpoints [default: service]
      --flat                               Generate a flat list without hierarchical structure
      --service-filter <SERVICE[,...]>     Include only specific services (comma-separated)
      --path-filter <PATTERN>              Filter endpoints by path pattern
      --method-filter <METHOD[,...]>       Filter by HTTP methods (comma-separated)
      --operation <METHOD PATH>            Render exactly this operation, as "METHOD /path/template" (repeatable; exact path match, Swagger 2 basePath excluded)
      --operation-id <ID>                  Render exactly the operation(s) with this operationId (repeatable, case-sensitive)
      --exclude-deprecated                 Hide deprecated endpoints
      --required-only                      Only show required parameters
      --detail <summary|basic|standard|full> Control amount of information [default: summary]
      --include-schemas                    Include request/response schemas
      --inline-schemas                     Fully inline every $ref schema instead of linking to a shared "Schema Definitions" section
      --schema-depth <N>                   Maximum schema traversal edges from a root (0..=24). Zero retains only roots
      --schema <NAME>                      Read a named schema directly (repeatable); full metadata, independent of reachability
      --schema-field <NAME#POINTER>        Read a schema subtree and its ancestors (repeatable; JSON-pointer escaping)
      --include-examples                   Include request/response examples
      --include-auth                       Show authentication requirements
      --toc                                Include the table of contents (the default; when both are given, the later of --toc/--no-toc wins)
      --no-toc                             Skip table of contents
      --sort <alpha|path-length|none>      Sorting method [default: alpha]
      --max-tokens <N>                     Fit single-file output to a token budget, stepping detail down (full → summary). The hygiene report is outside the budget
      --no-report                          Skip the spec hygiene report appended after the documentation
      --stats                              Dry run: per-service endpoint counts and estimated tokens (chars/4). TOTAL is one whole-document render, not the sum of the rows
  -h, --help                               Print help
  -V, --version                            Print version
```

```
Compare two versions of a spec and report what changed, classified as breaking, non-breaking or needing review

Usage: vimanam diff [OPTIONS] <OLD> <NEW>

Arguments:
  <OLD>  The older spec (JSON or YAML)
  <NEW>  The newer spec (JSON or YAML)

Options:
      --report            Append a Deltas section: spec hygiene counts for both specs and the estimated token size of each at --detail full --include-schemas
      --format <FORMAT>   Output format: a Markdown report or machine-readable JSON with stable change IDs [default: markdown] [possible values: markdown, json]
      --fail-on-breaking  Exit with status 3 when any breaking change is found, after writing the full report
  -o, --output <FILE>     Write the diff to FILE instead of stdout
  -h, --help              Print help (see more with '--help')
```

## Preparing API context for LLMs

Large API specs are a poor fit for LLM context windows: a 3 MB swagger file is hundreds of thousands of tokens of JSON, most of it boilerplate. Vimanam's detail levels and filters act as a token-budget dial, letting you hand an LLM (or a coding agent) exactly the slice of the API it needs, as compact Markdown.

For an agent that should choose its own reads, write a Skill tree instead of one file. `SKILL.md` is a compact map; service hubs, endpoint pages, and schema pages stay on disk at the requested detail, each with an approximate read cost. `--split` is the same idea as ordinary linked pages. When one schema is still too large, read it with `--schema` or `--schema-field`, or bound expansion with `--schema-depth`. Those reads keep field types, requiredness, descriptions, and enums. Single-file `--max-tokens` still steps detail down and can drop schema tables, so use it for a bounded overview, not for field research.

```bash
# Agent entry point: compact SKILL.md, full detail on the linked pages
vimanam openapi.json --output-mode skill -o ./api-skill \
  --detail full --include-schemas --include-examples

# Linked pages with a small index and full endpoint files
vimanam openapi.json --split endpoint -o ./api-docs \
  --detail full --include-schemas --overview-max-tokens 2000

# One field, or a depth-bounded operation, when a schema page is still too big
vimanam openapi.json --schema-field 'Finding#/properties/tags'
vimanam openapi.json --operation-id GetFinding --detail full \
  --include-schemas --schema-depth 2 --no-report
```

```bash
# 20,000-ft view: every service and operation name, usually <1% the size of the spec.
# Good as always-loaded context so the model knows what the API can do.
vimanam openapi.json --detail summary -o api-map.md

# Zoom into one service when the task touches it — parameters and responses
# included, everything else excluded
vimanam openapi.json --service-filter Findings --detail standard -o findings-api.md

# Slice by path or method instead
vimanam openapi.json --path-filter /v1/scans --detail standard -o scans-api.md
vimanam openapi.json --method-filter GET --detail basic -o read-api.md

# Or let Vimanam pick the detail level: ask for as much of a service as fits a
# token budget. It starts at --detail full and steps down until it fits,
# reporting any reduction on stderr.
vimanam openapi.json --service-filter Findings --detail full --max-tokens 8000 -o findings-api.md
```

`--max-tokens` uses a chars/4 token estimate — close enough to choose a detail level, but treat it as approximate rather than an exact cap. When the output is fed to a model, add `--no-report`: the spec hygiene report is useful to a human tidying the spec but is noise in an LLM prompt, and it is appended outside the token budget.

### Selecting exact operations

When a task concerns one or two operations, `--operation` and `--operation-id` render exactly those and nothing else:

```bash
vimanam openapi.json --operation "GET /users" --detail full --include-schemas --flat
vimanam openapi.json --operation "GET /users" --operation "DELETE /users/{id}" --detail standard
vimanam openapi.json --operation-id listUsers --detail full
```

Unlike `--path-filter`, which is a substring match (`--path-filter /users` also picks up `/users/{id}`, `/users/{id}/keys` and `/admin/users`), `--operation "<METHOD> <PATH>"` matches the spec's path template byte for byte: the method is case-insensitive, but `{id}` and `{userId}` are different templates and `/users/` is not `/users`. For Swagger 2 specs the path is the key under `paths`, without `basePath`. `--operation-id` matches operation IDs exactly (case-sensitive); if the spec reuses an ID, every operation carrying it is selected. Both flags are repeatable and can be combined — the selection is their union — and they are ANDed with the other filters, so `--operation "GET /users" --exclude-deprecated` renders nothing if that operation is deprecated (with a warning on stderr naming the filter). A value that matches no operation in the spec is an error (exit 1) rather than an empty document.

Under the default service grouping, services with no selected operation are left out, and an operation with several tags appears once under each of its services; add `--flat` to get each selected operation exactly once. The selection also scopes `--stats`, `--max-tokens` and the hygiene report.

The selectors round-trip with [`diff --format json`](#json-output): every change record's `endpoint.method` and `endpoint.path` can be passed back as `--operation "<method> <path>"` to render that operation's contract from the old spec (for `endpoint_removed`) or the new one:

```bash
vimanam diff v1.json v2.json --format json \
  | jq -r '.changes[] | select(.kind != "endpoint_removed") | "\(.endpoint.method) \(.endpoint.path)"' \
  | sort -u | while read -r op; do vimanam v2.json --operation "$op" --detail full --include-schemas --flat --no-report; done
```

Before choosing which slice to generate, `--stats` sizes the candidates without writing any Markdown: it prints one row per service with its visible endpoint count and the estimated token size of rendering that service alone, at whatever `--detail`, grouping and filter flags you pass, plus a TOTAL row for the whole document. A service left with no visible endpoint (for example, one whose only operations are dropped by `--exclude-deprecated`) is omitted from the table.

```bash
$ vimanam openapi.json --stats --detail standard
SERVICE    ENDPOINTS   ~TOKENS
Findings          42      6231
Scans             18      2890
Projects          31      4410
TOTAL             91     13102
```

Read it as a menu: a service that fits your budget can be pulled with `--service-filter <name>` at that detail level; one that does not can be re-measured at a lower `--detail` or handed to `--max-tokens`. Like `--max-tokens`, `~TOKENS` is a chars/4 estimate of the documentation body (the hygiene report is excluded). Each row is a separate render of that service alone, while `TOTAL` is a single render of the whole filtered document, so the rows do not necessarily add up to it: an operation tagged with several services is counted in each of their rows but once in the total, and the preamble and shared schema definitions are counted once per row.

A workflow that works well with coding agents: generate the `--detail summary` map once and reference it from the project's agent instructions (e.g. `CLAUDE.md`); have the agent regenerate a `--service-filter ... --detail standard` slice on demand when a task involves specific endpoints.

Output is deterministic — the same spec and flags produce byte-identical Markdown — so generated context files diff cleanly in git and don't needlessly invalidate LLM prompt caches.

## Comparing spec versions

`vimanam diff <OLD> <NEW>` compares two versions of a spec and prints a Markdown report of what changed, so a spec update can be reviewed (or gated in CI) before clients find out the hard way.

```bash
vimanam diff v1/openapi.json v2/openapi.json --report --fail-on-breaking
```

```markdown
# API Diff: Widgets API 1.0.0 → 1.1.0

**Summary:** 1 endpoint added, 1 removed, 4 changed; 4 breaking, 8 non-breaking, 1 to review

## Breaking changes (4)

| Change | Endpoint | Detail |
|--------|----------|--------|
| Parameter newly required | `GET /widgets/{id}` | `fields` (query) |
| Response removed | `GET /widgets/{id}` | 404 |
| operationId changed | `DELETE /widgets/{id}` | `Widgets_DeleteWidget` → `Widgets_RemoveWidget` |
| Endpoint removed | `GET /legacy` | was deprecated |

## Non-breaking changes (8)

| Change | Endpoint | Detail |
|--------|----------|--------|
| Response schema changed | `GET /widgets` | 200 `/properties/pricing` added |
| Response schema changed | `GET /widgets` | 200 `/properties/pricing` added to `required` |
| ... | | |

## Needs review (1)

| Change | Endpoint | Detail |
|--------|----------|--------|
| Request schema changed | `POST /widgets` | `/properties/weight/format` changed `float` → `double` |
```

**What is compared.** Endpoints are matched by method and path, parameters by name and location, responses by status code. Request and response bodies are compared as *resolved* schemas: every `$ref` is inlined first, so a change to a shared component schema shows up on every endpoint that references it — even when the operation object itself is byte-identical, which is the case a path-level or operation-level diff misses. Field-level differences are reported as JSON pointers into the resolved schema (`/properties/pricing`, `/items/type`). Descriptions, titles, examples and `x-*` extensions are ignored, and the order of `required` and `enum` members is irrelevant.

**Severity.** Every change is classified from the point of view of an existing client:

| Severity | Examples |
|----------|----------|
| Breaking | endpoint or response code removed; parameter removed, newly required or moved; `operationId` changed; a `type` changed anywhere; request: property removed, property added to `required`, `enum` member removed, `additionalProperties` set to `false`, `nullable` set to `false` (clients sending `null` now fail); response: schema removed entirely, property removed, property removed from `required`, `enum` member removed, `nullable` set to `true` |
| Non-breaking | endpoint or response code added; optional parameter added; parameter made optional; `deprecated` toggled; `operationId` added; request: property added, `required` member removed, `enum` member added, `nullable` set to `true`; response: property added, `required` member added, `nullable` set to `false` |
| Needs review | a response `enum` gaining a member (strictly typed clients reject values they do not know); anything else in a schema — `format`, `minimum`/`maximum`, `pattern`, `items` shape, `allOf`/`oneOf`/`anyOf` variants, other `additionalProperties` changes. Never trips `--fail-on-breaking` |

When a property is removed, the accompanying "removed from `required`" row for that property is not reported — the property going away is the change.

**Exit codes.** `0` — no breaking changes (or `--fail-on-breaking` not given); `1` — a spec failed to parse or the output could not be written; `2` — usage error; `3` — breaking changes found and `--fail-on-breaking` was given. The full report is always written before exiting.

`--report` appends a `## Deltas` section with the spec hygiene counts for both versions and the estimated token size of each at `--detail full --include-schemas`, so a spec update's documentation cost is visible alongside its API changes. `-o FILE` writes the report to a file (the subcommand has its own `-o`; the conversion flags do not apply to `diff`).

**Known limitations.** Only the first media type of a request body or response is compared. A path-template rename (`/pets/{id}` → `/pets/{petId}`) appears as a removal plus an addition. `allOf`/`oneOf`/`anyOf` lists are compared index-wise, so reordering variants is reported as changes. The model drops `null` from OpenAPI 3.1 type arrays (`type: ["string", "null"]` is folded to `string`), so a nullability change expressed as a type array is not detected — only `nullable: true`/`false` is.

### JSON output

`--format json` prints the same comparison as a single pretty-printed JSON document (2-space indentation, trailing newline) on stdout or in `-o FILE`, for CI tools, review bots and coding agents. Exit codes are identical across formats, and under `--fail-on-breaking` the complete document is written and flushed before exit status 3. Diagnostics go to stderr only, so stdout is always one parseable document. `-o FILE` and stdout carry byte-identical bytes.

```bash
vimanam diff old.json new.json --format json --report -o diff.json --fail-on-breaking
```

```json
{
  "schema_version": 1,
  "generator": { "name": "vimanam", "version": "1.4.0" },
  "old": { "title": "Widgets API", "version": "1.0.0", "file_sha256": "<64 lowercase hex>" },
  "new": { "title": "Widgets API", "version": "1.1.0", "file_sha256": "<64 lowercase hex>" },
  "summary": {
    "endpoints_added": 1, "endpoints_removed": 1, "endpoints_changed": 4,
    "breaking": 4, "non_breaking": 8, "review": 1
  },
  "changes": [
    {
      "id": "vc1_<64 lowercase hex>",
      "endpoint": { "method": "GET", "path": "/widgets/{id}" },
      "kind": "parameter_required_changed",
      "severity": "breaking",
      "details": { "name": "fields", "location": "query", "now_required": true }
    },
    {
      "id": "vc1_…",
      "endpoint": { "method": "GET", "path": "/widgets" },
      "kind": "response_schema_changed",
      "severity": "non_breaking",
      "details": {
        "status": "200",
        "schema_change": {
          "pointer": "/properties/pricing",
          "target": "property",
          "member": "pricing",
          "operation": "added",
          "before": { "present": false },
          "after": { "present": true, "value": { "type": "number" } }
        }
      }
    }
  ],
  "deltas": { "hygiene": [ { "check": "Missing description", "old": 3, "new": 1 } ],
              "tokens": { "old": 12345, "new": 12890, "estimate": "chars/4", "detail": "full+schemas" } }
}
```

**Field reference.** `schema_version` is `1`; bumping it (or the `vc1_` ID prefix below) signals a contract change. `generator.version` is the vimanam version that produced the document. `old`/`new` carry each spec's title, version and `file_sha256`. `summary` mirrors the Markdown summary line (its counts always agree); `changes` lists every change in the order the Markdown tables use before grouping by severity; `severity` is one of `breaking`, `non_breaking`, `review`. `deltas` is present **only** with `--report` — the key is omitted, never `null` — with hygiene rows in the same order as the Markdown `## Deltas` table and the same chars/4 token estimate the `--report` Markdown shows. Every `kind` maps one-to-one to an internal `ChangeKind` variant, and its `details` object carries exactly that variant's fields (e.g. `parameter_required_changed` → `name`, `location`, `now_required`; `response_schema_changed` → `status`, `schema_change`; `operation_id_changed` → `old`/`new`, which may be `null` because a spec may lack an `operationId`). `status` stays a string (`"200"`, `"default"`, `"4XX"`).

**Presence encoding.** `before`/`after` are each either `{ "present": false }` or `{ "present": true, "value": <json> }`. A present `value` may legitimately be JSON `null` (`default: null`, a `null` member of `enum`) — that is distinct from absence. One special case: "no schema at all" is encoded internally as a null at the root pointer `""`, so a root-level `null` side is reported as `{ "present": false }` with `operation` flipped to `added`/`removed` (a body appeared or disappeared) rather than a change to/from `null`.

**Pointers.** `schema_change.pointer` is an RFC 6901 pointer exactly as the differ emits it, and `target`/`member` classify it according to the schema grammar: `target` is one of `type`, `required_member`, `enum_member`, `property`, `additional_properties`, `nullable`, `other`, and `member` is the decoded last pointer segment for the targets that address a named member (`null` otherwise). A property *named* `type` is a `property`, not the `type` keyword. Pointers address the **resolved, canonicalised schema** — `$ref`s are inlined and `description`/`title`/`example(s)`/`deprecated`/`x-*` annotations are stripped before comparing — so a pointer is a location in that normalised form, not necessarily in your input file. `required`/`enum` members appear as `/required/<name>` and `/enum/<value>`.

**Change identity.** Every record's `id` is `"vc1_" + SHA-256` (lowercase hex) of a canonical JSON object `{ "v": 1, "endpoint": …, "kind": …, "details": … }` — the same `endpoint`, `kind` and `details` values the record emits, serialised with object keys sorted recursively and no whitespace. Severity, display strings, timestamps, file paths, `file_sha256` and the generator version are deliberately **not** part of the hash, so a future change to the severity rules cannot rewrite any ID; the `vc1_` prefix and the `"v": 1` field version the construction and must move together if it ever changes. Consequences you can rely on:

- the same pair of specs yields identical IDs on every run;
- reformatting a spec (whitespace, key order) changes `file_sha256` but no ID or record;
- an unrelated edit elsewhere in the spec leaves existing IDs unchanged;
- two different changes at the same pointer (`string` → `integer` vs `string` → `boolean`) get different IDs;
- a change behind a shared `$ref` produces one record per affected endpoint, each with its own ID, because the endpoint is part of the hash input;
- IDs are unique within a document.

**Known limits for IDs.** `allOf`/`oneOf`/`anyOf` members are compared by index, so reordering them changes pointers and therefore IDs, even when the set of variants is unchanged. A path-template rename is a removal plus an addition with two fresh IDs — there is no rename detection.

## Continuous integration

Generate your API docs in CI with the [vimanam GitHub Action](https://github.com/noemaforge/vimanam-action) — it downloads the matching prebuilt binary (no Rust toolchain on the runner), verifies its SHA256 checksum, and runs vimanam:

```yaml
- uses: noemaforge/vimanam-action@4599a14c84d9d7bce1ec34ed9f12f3036f06b518 # v1
  with:
    spec: openapi.json
    output: docs/api-map.md
    detail: summary

# Output is deterministic, so this fails CI when the committed docs drift from the spec:
- run: git diff --exit-code -- docs/api-map.md
```

To block a pull request that breaks API clients, diff the proposed spec against the one on the default branch; exit status 3 means breaking changes were found (see [Comparing spec versions](#comparing-spec-versions)):

```yaml
- run: git show origin/main:openapi.json > /tmp/openapi-main.json
- run: vimanam diff /tmp/openapi-main.json openapi.json --report --fail-on-breaking -o api-diff.md
```

Write the report with `-o` rather than piping it through `tee`: a pipeline would return `tee`'s exit status, not vimanam's, unless the shell runs with `pipefail`. `api-diff.md` is still written when the step fails, so a later step (`if: always()`) can upload it or post it as a pull-request comment.

Pin the action to a commit SHA, not a mutable tag — see the action's [Pinning](https://github.com/noemaforge/vimanam-action#pinning) notes. More patterns in [`examples/`](https://github.com/noemaforge/vimanam-action/tree/main/examples).

## Supported OpenAPI Versions

Vimanam supports:
- OpenAPI 2.0 (Swagger) documents using the `swagger` field
- OpenAPI 3.0+ documents using the `openapi` field

## Output Examples

The generated documentation includes:

### Server and Authentication Information
```markdown
## Server URLs
* https://api.example.com/v1
* https://dev-api.example.com/v1

## Authentication
* **apiKeyAuth**: API Key authentication (apiKey)
* **oauth2**: OAuth 2.0 authorization (oauth2)
```

### Endpoint Documentation
```markdown
### createUser {#createuser}
**Operation:** POST /users

**Description:** Create a new user account
**Operation ID:** `createUser`

#### Parameters
| Name | In | Required | Description |
|------|----|---------:|-------------|
| `body` | body | Yes | User information |

#### Responses
| Code | Type | Description |
|------|------|-------------|
| 201 | application/json | User created successfully |
| 400 | application/json | Invalid request |
```

## Roadmap

Shipped:

- **[v1.0.0](https://github.com/noemaforge/vimanam/milestone/1)** — first stable release: JSON and YAML input.
- **[v1.1.0](https://github.com/noemaforge/vimanam/milestone/2)** — token budgets, shell completions, the hygiene report, `--stats`, and `diff`.
- **[v1.2.0](https://github.com/noemaforge/vimanam/milestone/3)** — `diff --format json` with stable change IDs.
- **v1.3.0** — exact operation selection (`--operation`, `--operation-id`).
- **v1.4.0** — linked multi-file output (`--split`), the agent-navigable Skill tree (`--output-mode skill`), and schema/field reads with `--schema-depth`.

Still open:

- **[Packaging](https://github.com/noemaforge/vimanam/milestone/4)** — distribution channels (Scoop, winget, Chocolatey, AUR, native `.deb`/`.rpm`), shipped independently of code releases.

See the [open issues](https://github.com/noemaforge/vimanam/issues) for the full backlog.

## License

Apache License 2.0
