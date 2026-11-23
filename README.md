# Vimanam

[![CI](https://github.com/noemaforge/vimanam/actions/workflows/ci.yml/badge.svg)](https://github.com/noemaforge/vimanam/actions/workflows/ci.yml)
[![Docs](https://img.shields.io/badge/docs-GitHub%20Pages-047857)](https://noemaforge.github.io/vimanam/)
[![crates.io](https://img.shields.io/crates/v/vimanam.svg)](https://crates.io/crates/vimanam)
[![License: Apache-2.0](https://img.shields.io/crates/l/vimanam.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.96-blue.svg)](Cargo.toml)

Vimanam is a fast Rust CLI and library that converts **OpenAPI/Swagger** (JSON & YAML), **Protocol Buffers / gRPC** (`.proto` & binary descriptors), and **GraphQL** (SDL & introspection JSON) into token-budgeted Markdown, agent-navigable Skill trees, and deterministic breaking-change diffs.

*Vimanam* means airplane in Malayalam. Like an aircraft, it lets you fly at 20,000 feet (`--detail summary`), drop to one service or operation (`--operation`, `--schema-depth`), or inspect individual fields (`--schema-field`) without flooding an LLM context window.

Full documentation, guides, and public benchmarks live at **[noemaforge.github.io/vimanam](https://noemaforge.github.io/vimanam/)**.

## Install

```bash
# Homebrew (macOS and Linux)
brew install noemaforge/tap/vimanam

# Shell installer (Linux and macOS)
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/noemaforge/vimanam/releases/latest/download/vimanam-installer.sh | sh

# Cargo prebuilt or from source (MSRV 1.96)
cargo binstall vimanam
cargo install vimanam
```

Windows PowerShell installers, SHA-256 archives, shell completions (`bash`, `zsh`, `fish`, `elvish`, `nushell`, `powershell`), and `vimanam man` manual pages are documented in [Installation & completions](https://noemaforge.github.io/vimanam/installation/).

## Quickstart

### 1. OpenAPI / Swagger (JSON & pure-Rust YAML 1.2)

```bash
# Size services before rendering
vimanam openapi.yaml --stats --detail standard

# Render one operation with depth-bounded schemas for an LLM
vimanam openapi.yaml \
  --operation "POST /pets" \
  --detail full --include-schemas --schema-depth 2 --no-report

# Generate an agent-navigable SKILL.md directory tree
vimanam openapi.yaml \
  --output-mode skill --detail full --include-schemas -o ./api-skill
```

### 2. Protocol Buffers & gRPC (pure Rust — no `protoc` required)

```bash
# Compile .proto source directly with comments and Google well-known types
vimanam proto/service.proto -I proto --detail full --include-schemas --no-report

# Gate wire-breaking changes across Git commits (exit 3 on breaking changes)
vimanam diff --from-ref v1.0.0 --to-ref HEAD \
  --spec proto/service.proto -I proto --breaking wire --fail-on-breaking
```

### 3. GraphQL (single/multi-file SDL & introspection JSON)

```bash
# Merge an SDL directory and inspect one query
vimanam schema/ --operation "QUERY Query.user" --detail full --include-schemas --no-report

# Diff two schemas and emit token-budgeted context for affected root fields
vimanam diff old.graphql new.graphql --context --max-tokens 2000 --fail-on-breaking
```

## Supported formats

| Format | Inputs | Key capabilities |
|---|---|---|
| **OpenAPI / Swagger** | 2.0, 3.0, partial 3.1 (`.json`, `.yaml`, `.yml`) | Grouping, `$ref` linking, Spec Hygiene Report, moved-operation pairing, pagination inference |
| **Protocol Buffers / gRPC** | proto2/proto3 `.proto` (via `protox`), `.pb` / `FileDescriptorSet` | Zero-`protoc` compilation, `.proto` comments, wire vs source `--breaking` gates, in-memory Git-ref imports |
| **GraphQL** | `.graphql`/`.graphqls` files or directories (via `apollo-parser`), introspection JSON | `extend schema`, `@oneOf`, directional rules, Relay `edges`/`nodes` pagination, `diff --context` |

## Documentation

- **[Quickstart](https://noemaforge.github.io/vimanam/quickstart/)** — Install and run your first conversion, Skill tree, and diff
- **[Token budgets & selection](https://noemaforge.github.io/vimanam/llm-context/)** — `--stats`, `--costs`, `--operation`, `--schema`, `--schema-field`, `--schema-depth`, `--max-tokens`
- **[Split trees & Skill mode](https://noemaforge.github.io/vimanam/split-and-skills/)** — `--split`, `--output-mode skill`, `--overview-max-tokens`, and manifest protections
- **[OpenAPI & Swagger](https://noemaforge.github.io/vimanam/openapi/)** · **[Protocol Buffers & gRPC](https://noemaforge.github.io/vimanam/protobuf/)** · **[GraphQL](https://noemaforge.github.io/vimanam/graphql/)**
- **[Contract diff & Git refs](https://noemaforge.github.io/vimanam/diff/)** — Breaking-change CI gates (`exit 3`), rename-following Git diffs, and hygiene deltas
- **[Neutral JSON contract v2](https://noemaforge.github.io/vimanam/json-contracts/)** — Stable `ve2_`/`vc2_` SHA-256 IDs and `--contract 1` legacy mode
- **[CI pipelines & Rust library](https://noemaforge.github.io/vimanam/ci-and-library/)** — [`noemaforge/vimanam-action`](https://github.com/noemaforge/vimanam-action) and embedding `vimanam` with `default-features = false`
- **[CLI reference](https://noemaforge.github.io/vimanam/cli-reference/)** — Complete options, subcommands (`diff`, `completions`, `man`), and exit codes
- **[Benchmarks & v2.2 analysis](https://noemaforge.github.io/vimanam/benchmarks/)** — Task-accuracy eval, exact BPE tokenization, external baselines (`oasdiff`, `buf`), and `v2.2.0` vs `v2.1.0` data

## Release history & license

See [CHANGELOG.md](CHANGELOG.md) for release notes. Licensed under the [Apache License 2.0](LICENSE).
