# Contributing to Vimanam

Thanks for your interest in improving Vimanam! This is a small project, so the
process is light. Bug reports, feature ideas, and pull requests are all welcome.

## Getting started

Vimanam is a standard Rust (edition 2021) CLI. You need a Rust toolchain; the
minimum supported version (MSRV) is **1.85**.

```bash
git clone https://github.com/noemaforge/vimanam.git
cd vimanam
cargo build
cargo run -- path/to/spec.json -o out.md
```

## Development loop

Before opening a pull request, run the same checks CI enforces:

```bash
cargo fmt --check                          # formatting
cargo clippy --all-targets -- -D warnings  # lint (warnings are errors)
cargo test                                 # integration tests (tests/cli.rs)
```

CI also builds against the MSRV (Rust 1.85) on every pull request, so avoid
language or dependency features newer than that unless you intend to raise the
MSRV deliberately (and call it out in the PR).

`cargo test` runs the integration suite in `tests/cli.rs` against the fixtures
in `tests/fixtures/`. Run a single test by name substring, e.g.
`cargo test group_by_path`.

## Conventions worth knowing

- **Deterministic output is a tested invariant.** `output_is_deterministic` in
  `tests/cli.rs` asserts byte-identical output across runs. Order-sensitive maps
  (`paths`, `responses`, `content`, examples) use `IndexMap` — don't swap them
  back to `HashMap`.
- **Unknown spec fields are preserved**, not rejected: model structs carry a
  `#[serde(flatten)] extensions` map so vendor (`x-*`) fields survive.
- **`$ref`s are resolved during parsing**, not at render time (see
  `resolve_*_ref` in `src/utils.rs`).
- **Keep the CLI surface honest** — don't add a flag that only partially works;
  the project has a history of removing placeholder flags.
- **New tracked Markdown or fixtures need a `.gitignore` exception.** The blanket
  `*.md` ignore allowlists specific files (`README.md`, `CONTRIBUTING.md`,
  `tests/fixtures/**`, `.github/**/*.md`, …); add yours if you create one.

## Pull requests

- Keep changes focused; one logical change per PR is easiest to review.
- Add or update tests for behavior changes.
- Update the `## [Unreleased]` section of [`CHANGELOG.md`](CHANGELOG.md)
  (the project follows [Keep a Changelog](https://keepachangelog.com/) and
  [Semantic Versioning](https://semver.org/)).
- Reference the issue you're addressing (e.g. "Closes #12").

## Reporting bugs / requesting features

Open an issue using the templates. For security issues, please follow
[`SECURITY.md`](SECURITY.md) instead of filing a public issue.

By contributing, you agree that your contributions are licensed under the
project's [Apache-2.0 license](LICENSE).
