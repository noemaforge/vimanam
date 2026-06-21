# Vimanam

Vimanam is an OpenAPI/Swagger JSON to Markdown documentation generator.

Vimanam stands for Aeroplane in Malayalam. Like an aeroplane, it can fly high and give you a 20,000 feet view of the APIs. It can fly low and give you a detailed view of the APIs. You can also run it along the ground to look deep into the API fields and descriptions.

It supports both OpenAPI 2.0 (Swagger) and OpenAPI 3.0 specifications.

Besides producing documentation for humans, Vimanam is built for **feeding API specs to LLMs**: a multi-megabyte enterprise spec doesn't fit in a context window, but a filtered, summary-level Markdown rendering of it does. See [Preparing API context for LLMs](#preparing-api-context-for-llms).

## Features

- Convert OpenAPI JSON files to Markdown documentation
- Supports both OpenAPI 2.0 (Swagger) and OpenAPI 3.0 specifications
- Group endpoints by service, HTTP method, or path, or list them flat
- Filter by service, path, or method
- Multiple detail levels (summary, basic, standard, full)
- Token-budget-aware output (`--max-tokens`): steps the detail level down until the rendering fits, and reports what was trimmed on stderr
- Schema expansion at `--detail full --include-schemas`: resolves `$ref`s (with cycle detection) and renders request/response schemas as nested field tables
- Example rendering at `--detail full --include-examples`: emits request/response examples as fenced JSON blocks, resolving `$ref`s into `components/examples`
- Server URL information extraction and documentation
- Authentication and security schemes documentation
- Proper content type detection for responses
- Sorting options for endpoints (alphabetical, path length)
- Clean anchor generation for better navigation
- Deterministic, byte-identical output across runs — friendly to diffs, caching, and LLM prompt caching

## Installation

### From crates.io (recommended)

```bash
cargo install vimanam
```

### Prebuilt binaries

Download the binary for your platform (Linux, macOS Intel/ARM64, Windows) from the
[latest release](https://github.com/nrynss/vimanam/releases/latest) — no Rust toolchain needed.

### From source

Requires [Rust](https://www.rust-lang.org/tools/install) 1.85.0 or later.

```bash
# Clone repository
git clone https://github.com/nrynss/vimanam.git
cd vimanam

# Run directly without building (development)
cargo run -- input.json -o output.md

# Build and run
cargo build --release
./target/release/vimanam input.json -o output.md

# Or install system-wide
cargo install --path .
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
```

## Options

```
Usage: vimanam [OPTIONS] <FILE>

Arguments:
  <FILE>  Path to the OpenAPI JSON file

Options:
  -o, --output <FILE>                      Output file path
      --method                             Group endpoints by HTTP method instead of by service
      --group-by <service|method|path>     Grouping method for endpoints
      --flat                               Generate a flat list without hierarchical structure
      --service-filter <SERVICE[,...]>     Include only specific services (comma-separated)
      --path-filter <PATTERN>              Filter endpoints by path pattern
      --method-filter <METHOD[,...]>       Filter by HTTP methods (comma-separated)
      --exclude-deprecated                 Hide deprecated endpoints
      --required-only                      Only show required parameters
      --detail <summary|basic|standard|full> Control amount of information [default: summary]
      --include-schemas                    Include request/response schemas
      --include-examples                   Include request/response examples
      --include-auth                       Show authentication requirements and server URLs
      --no-toc                             Skip table of contents
      --sort <alpha|path-length|none>      Sorting method [default: alpha]
      --max-tokens <N>                     Fit output to a token budget, stepping detail down as needed
  -h, --help                               Print help
```

## Preparing API context for LLMs

Large API specs are a poor fit for LLM context windows: a 3 MB swagger file is hundreds of thousands of tokens of JSON, most of it boilerplate. Vimanam's detail levels and filters act as a token-budget dial, letting you hand an LLM (or a coding agent) exactly the slice of the API it needs, as compact Markdown.

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

`--max-tokens` uses a chars/4 token estimate — close enough to choose a detail level, but treat it as approximate rather than an exact cap.

A workflow that works well with coding agents: generate the `--detail summary` map once and reference it from the project's agent instructions (e.g. `CLAUDE.md`); have the agent regenerate a `--service-filter ... --detail standard` slice on demand when a task involves specific endpoints.

Output is deterministic — the same spec and flags produce byte-identical Markdown — so generated context files diff cleanly in git and don't needlessly invalidate LLM prompt caches.

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

## License

Apache License 2.0
