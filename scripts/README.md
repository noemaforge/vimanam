# scripts

## `compare-baseline.sh`

Runs a released vimanam binary and a fresh build of the working tree over the same matrix of CLI invocations, then compares stdout, stderr, exit code and any files or directories written, byte for byte. Use it to prove a refactor changes no behaviour, or to see exactly which outputs an intentional change affects.

### Set up a baseline

Build the release you want to compare against into a fixed location, from a throwaway worktree so your checkout is untouched:

```bash
git worktree add /tmp/vimanam-v1.4.1 v1.4.1
cargo install --path /tmp/vimanam-v1.4.1 --root /tmp/vimanam-baseline --locked
git worktree remove /tmp/vimanam-v1.4.1
```

`/tmp/vimanam-baseline/bin/vimanam` is the default baseline path.

For the optional test-list check, also record the baseline's test names while the baseline commit is checked out:

```bash
cargo test -- --list 2>/dev/null | rg ': test$' | sort > /tmp/vimanam-tests-before.txt
```

### Run

```bash
scripts/compare-baseline.sh                      # baseline vs. a fresh release build
CHECK_TESTS=1 scripts/compare-baseline.sh        # also compare test names
VERBOSE=1 scripts/compare-baseline.sh            # print a short unified diff per difference
scripts/compare-baseline.sh OLD_BIN NEW_BIN      # compare two arbitrary binaries
```

| Variable | Effect |
|---|---|
| `SKIP_BUILD=1` | Don't run `cargo build`; use `NEW_BIN` or `target/release/vimanam`. |
| `KEEP=1` | Keep the temporary work directory, with every captured output and a manifest of each case's arguments. |
| `VERBOSE=1` | Print a truncated unified diff for each differing case. |
| `CHECK_TESTS=1` | Compare `cargo test -- --list` against `/tmp/vimanam-tests-before.txt`. |
| `ALLOW_SAME=1` | Allow the two binaries to be identical files (normally an error, since it would make every case pass). |

Requires `jq`, and `shasum` or `sha256sum`. Works with macOS bash 3.2 and Linux.

The script exits 0 when every case is identical and 1 otherwise, printing one line per differing case followed by `N cases, M differ`.

### What it covers

Inputs are every spec in `tests/fixtures/`, the extra fixtures in `scripts/compare-fixtures/`, and, when present, the large gitignored specs at the repo root (`swagger.json`, `openapi.json`, `openapiv2.swagger.json`). For each input, the matrix covers:

- conversion at every detail level and grouping mode, with schemas, inline schemas, examples, `--max-tokens` and `--stats`;
- `--schema-depth` cut-offs, `--schema` and `--schema-field` selections, including valid and invalid selectors;
- `--operation` and `--operation-id` selectors;
- `--split` and `--output-mode skill` directory trees;
- every `diff` output: Markdown and JSON, `--report`, `--fail-on-breaking`, `-o`, unchanged pairs and parse failures;
- shell completions.

`scripts/compare-fixtures/` exists because the test fixtures don't exercise everything the comparison needs, notably top-level `anyOf`/`oneOf`/`allOf`, nested compositions, `additionalProperties` refs, recursion through compositions, and a dangling `$ref`. The `diff_compositions_*` pair changes schemas inside composition members.

Error paths are compared like any other output: a case where both binaries fail with the same message and exit code passes.

### Test-list check

Moving tests between modules changes their paths (`diff::tests::x` becomes `diff::value::tests::x`), so names are compared after stripping everything up to the last `::`. They're compared as a multiset, so dropping one of two tests that share a name is still reported. The check reports missing and new names separately, and it fails the run if `cargo test -- --list` fails or lists nothing.

### Reading a failure

Rerun with `KEEP=1 VERBOSE=1`. The run prints the path of `manifest.txt` in its work directory; look up the failing case's name there to get its exact arguments. Captured outputs are in `baseline/` and `candidate/` next to the manifest, under the same case name. Absolute paths under each side's work directory are replaced with `<WORK>` in stderr before comparing, so a path difference alone isn't reported.

Before trusting a clean run on a new setup, check that the script can fail. A candidate that appends one line to every output should be reported as differing on nearly every case:

```bash
printf '#!/bin/sh\n/tmp/vimanam-baseline/bin/vimanam "$@"; rc=$?; echo x; exit $rc\n' > /tmp/vimanam-fake
chmod +x /tmp/vimanam-fake
SKIP_BUILD=1 scripts/compare-baseline.sh /tmp/vimanam-baseline/bin/vimanam /tmp/vimanam-fake
```
