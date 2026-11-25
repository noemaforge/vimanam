# scripts

## Release workflow maintenance

Use cargo-dist 0.32.0 and Python 3.11+ to regenerate the release workflow:

```bash
python3 scripts/generate-release.py
python3 scripts/generate-release.py --check
python3 -m unittest discover -s scripts -p 'test_*.py'
```

`generate-release.py` generates from the current tracked and unignored files in
an isolated temporary snapshot, so `--check` never edits the worktree. Action
SHA pins come from `[dist.github-action-commits]` in `dist-workspace.toml`.
Update those pins before regenerating when Dependabot changes release actions.
Do not use bare `dist generate`: it omits two intentional customizations.

- The plan job calls `require-green-ci.py` **before any dist commands**, with
  read access to Actions. It resolves the checked-out commit and requires the
  latest full `ci.yml` push run on `main` for that exact SHA to succeed. It checks
  the stable test, MSRV build, and dependency audit jobs in the latest attempt,
  rejecting missing/skipped jobs, failed/cancelled runs, PRs and scheduled audits.
  Queued/running or not-yet-visible CI is polled for up to ten minutes; API
  errors block the release. The tag must be `v` plus the package version.
  A failed plan blocks both builds and publishing. A parallel custom plan job
  is insufficient: cargo-dist's host job allows skipped build jobs.
- The reusable crates.io publisher runs after successful builds and hosting,
  including prereleases, while the Homebrew tap remains stable-only. Cargo's
  packaged-source verification build is retained; release never runs tests.

CI checks the generated output and packaging plan on main and on PRs changing
manifests, workflow files or release scripts. Ordinary source-only PRs skip that
step. Gate regression tests always run and use mocked read-only GitHub API
responses, without publishing or sleeping.

## `benchmark-gitrefs.py`

Measures the Git history engine through two release CLI binaries on generated
scratch repositories. Requires Python 3 and Git; the counting wrapper uses a
POSIX shell. No private specs or external repositories are used. The benchmark
creates committed JSON specs with 400 unrelated edits, a 20-step detectable
rename chain followed by a major later rewrite, and a merge requiring the
existing merge-aware fallback. Setup uses `git fast-import` outside the timers.

Build the pre-optimization baseline from the merged #113 tree, then the current
release binary. Keep the baseline binary outside the shared Cargo target directory:

```bash
git worktree add --detach /tmp/vimanam-gitrefs-before 190e3198d367abc25b6b66ad1e3af6a872e98cb5
cargo build --release --locked --manifest-path /tmp/vimanam-gitrefs-before/Cargo.toml
cp /tmp/vimanam-gitrefs-before/target/release/vimanam /tmp/vimanam-gitrefs-baseline
git worktree remove /tmp/vimanam-gitrefs-before
cargo build --release --locked
python3 scripts/benchmark-gitrefs.py /tmp/vimanam-gitrefs-baseline target/release/vimanam \
  --commits 400 --renames 20 --runs 5 --output /tmp/vimanam-gitrefs-results.json
```

Each scenario warms up each binary once, then records five whole-CLI wall times
with `time.perf_counter` and counts Git invocations through a PATH wrapper.
Timings include fixed CLI discovery/parsing/rendering costs and the wrapper's
overhead; these are end-to-end measurements, not isolated Git CPU timings.
Logging is disabled. The script rejects inconsistent subprocess counts and
compares parsed JSON between every run and binary, excluding only
`generator.version` for the package bump. It records every timed sample,
platform, Git version, binary versions and binary SHA256 hashes in JSON.
Adjust `--commits`, `--renames` and `--runs` to repeat at another scale.

The fast path covers backward ancestor comparisons whose complete interval is
linear and whose repository is not shallow. It uses
[`git log --follow`](https://git-scm.com/docs/git-log#Documentation/git-log.txt---follow)
to locate file-touch commits, then verifies each one using the engine's existing
full parent-edge rename rules. This retains the same 50% Git rename heuristic;
a move combined with a major rewrite in one commit can remain undetectable.
Once paths are identified, semantic API comparisons have no similarity threshold.
Forward and sibling comparisons, merges, shallow repositories, unexpected log
records and mapping mismatches keep the existing traversal. Merge fallback adds
two probe calls; neither merge traversal nor the older-common-commit search is
cached or optimized by this change. Small histories or specs edited in nearly
every commit may see little benefit or additional probe overhead.

The checked-in `benchmark-gitrefs-results.json` records the 2026-10-07 run on
macOS 26.7.1 ARM64 with Apple Git 2.54.0. The 1.6.0 baseline was built from
#113's reviewed head `de780d9fe86f83d1bcd61bb2ac0ecaaa75760ade`, whose tree
matches merged commit `190e3198d367abc25b6b66ad1e3af6a872e98cb5`. Both binaries
used `cargo build --release` and Rust 1.96.1; the candidate reports 1.7.0.

| Generated history | Baseline median | Optimized median | Git subprocesses before → after | Median speedup |
|---|---:|---:|---:|---:|
| 400 unrelated commits | 16.139s | 0.609s | 413 → 15 | 26.52× |
| 400 commits, 20 renames, later rewrite | 16.820s | 1.391s | 414 → 36 | 12.09× |
| 400 unrelated commits plus merge | 15.661s | 16.679s | 416 → 418 | 0.94× |

These counts include five fixed calls outside `materialize_follow`: two for
repository discovery and three for materializing the newer spec. The method's
counts are therefore 408 → 10, 409 → 31 and 411 → 413, respectively.
The merge scenario showed no measured benefit (6.5% slower here), with two
additional probes. Timings vary by machine and load; the raw file retains the
sample ranges, and these measurements are not a promise for other repositories.

## `benchmark-diff-refs.py`

The timing check for `vimanam diff` on large `$ref` graphs (#152, #181). It uses only the standard library, needs Git, and uses the same `--corpus` layout as `benchmark-public.py` (`openai-repo/`, `stripe-openapi/`). It extracts pinned OpenAI and Stripe revisions and runs each pair serially in Markdown, v1 JSON and v2 JSON: the OpenAI pair, a Stripe self-diff, the published Stripe benchmark pair, and the 15 consecutive release pairs of the last 16 `openapi/spec3.json` commits (`a402765` … `bcfd59f`), which only finish since `$ref` targets are compared as edges (#181). It reports the median and min–max wall time, peak RSS (`wait4`), and output bytes and SHA-256. With `--baseline BIN` it runs a second binary on the same inputs and exits 1 if any output differs where the baseline finished within `--baseline-timeout` (default `--timeout`); `--allow-different` reports differences without failing, for a baseline from before #181 on specs with `$ref`s. `--only stripe-releases` runs just the release pairs.

```bash
python3 scripts/benchmark-diff-refs.py --binary target/release/vimanam \
  --baseline /tmp/vimanam-main --corpus /tmp/vimanam-eval-corpus --runs 5
```

## `benchmark-public.py` and `eval-context-accuracy.py`

Reproducible evaluation scripts backing the [Benchmarks](https://noemaforge.github.io/vimanam/benchmarks/) section (`v2.2 standalone & LLM eval`, `v2.2 vs v2.1 upgrade`, and `v2.2 vs external tools`). Both use public open-source specifications (pinned SHAs for Stripe OpenAPI, OpenAI OpenAPI, GitHub GraphQL, SpaceX Land GraphQL, OpenTelemetry Proto, and gRPC Health) plus three checked-in synthetic multi-file Protobuf specs under `scripts/eval-fixtures/protos/`:

- `scripts/benchmark-public.py` → `scripts/benchmark-v22-results.json`: runs `hyperfine 2.0.0` (`median`, Q1–Q3 `IQR`, `min`, `max`, `mean`, `stddev`, peak RSS, CPU instructions, raw `samples_ms`, and disjoint-IQR checks), pins exact upstream commit SHAs (`PINNED_REVS`), compares `tiktoken` (`o200k_base`) BPE tokens against Vimanam's `chars / 4` estimate, benchmarks a generated 200-file `.proto` Git repo (`git fast-import`), compares against `oasdiff`, `buf breaking`, `graphql-inspector`, and `protoc-gen-doc`, and reconciles OpenAI `a01e367..373f26a` breaking findings.
- `scripts/eval-context-accuracy.py` → `scripts/eval-context-accuracy-results.json`: a serial (`--workers 1`), repeated-trial (`--trials 3`) evaluation with 11 tasks and 6 conditions, graded by deterministic programmatic JSON assertions on `claude-haiku-5-5` (zero tools, isolated system prompt). The conditions are `no_spec`, `summary`, `routed_e2e`, `fixed_slice`, `oracle_slice` and `raw_spec`.
  - **Cache protocol.** Every graded trial prepends a fresh nonce to each user prompt, so all graded trials are cold-cache calls and no trial reads another's cache. Right after trial 1 of each cell, its exact prompts are re-sent with the same nonce (`warm_replay`). The replay measures cost and token use only and is not graded. Each call records `uncached_input_tokens`, `cache_creation_input_tokens` and `cache_read_input_tokens`. Report cold against cold and warm against warm. Mixing the two inflated the earlier cost ratio from about 102× to 161×.
  - **`oracle_slice`** is the operation at `--schema-depth 2` plus `--schema` for exactly the named types that hold the answers. The build fails unless every task's `answer_markers` appear in it. `routed_e2e` vs `oracle_slice` isolates routing quality, and `oracle_slice` vs `raw_spec` isolates slice readability. `fixed_slice` (operation only, depth 2) remains a deliberately naive single-turn baseline.
  - **`answer_coverage`** is the fraction of answer markers present in each condition's context (per trial for `routed_e2e`). It separates "the answer was not retrieved" from "the answer was retrieved but misread".
  - **Tasks.** Three synthetic multi-file `.proto` fixtures (`scripts/eval-fixtures/protos/`) rule out memorization. `pb_otel_trace_export` is a real public spec whose answers sit beyond depth 2, so it exercises the follow-up path on non-synthetic input. It is memorization-prone: `no_spec` sometimes answers it fully from training data, so it shows the follow-up path runs, not that the context was needed. `--only-tasks a,b` runs a subset for smoke checks.
  - **Grading.** Each trial stores its parsed `answer`, so results can be re-graded without new API calls. `required_fields` accepts the `request.` / `body.` prefix that Vimanam renders on inline request bodies (#156). `routed_e2e` takes at most `MAX_SCHEMAS` (3) `--schema` names per turn, and both prompts state that limit.
  - Other recorded data: exact target routing (`route_target_matched`), follow-up use (`followup_rate`), Stripe `7.9 MB` raw overflow (`Prompt is too long`), SHA-256 of every spec file (`meta.corpus_sha256`), and Claude API tokens alongside `o200k_base` BPE and `chars / 4`.

### Corpus

Both scripts read public specs from `--corpus`. The LLM eval needs these exact revisions; check them against `meta.corpus_sha256` in the results file:

```bash
C=/tmp/vimanam-eval-corpus; mkdir -p "$C" && cd "$C"
git clone -q --filter=blob:none --no-checkout https://github.com/openai/openai-openapi.git openai-repo
git -C openai-repo show 498c71d:openapi.yaml > openai.yaml
git clone -q --filter=blob:none --no-checkout https://github.com/stripe/openapi.git stripe-openapi
git -C stripe-openapi checkout -q 5237fa0 -- openapi/spec3.json
git clone -q --filter=blob:none --no-checkout https://github.com/octokit/graphql-schema.git octokit-graphql
git -C octokit-graphql checkout -q 82ff2d4 -- schema.graphql
git clone -q --filter=blob:none --no-checkout https://github.com/open-telemetry/opentelemetry-proto.git otel-proto
git -C otel-proto checkout -q d31f7ac -- opentelemetry
```

```bash
# Re-run public performance, BPE calibration, and 4 external baselines
uv run --with tiktoken python3 scripts/benchmark-public.py \
  --v21 /tmp/vimanam-v2.1.0/target/release/vimanam \
  --v22 target/release/vimanam \
  --corpus /tmp/vimanam-eval-corpus \
  --runs 10 \
  --output scripts/benchmark-v22-results.json

# Re-run the serial 3-trial LLM evaluation (cold trials + warm replays; ~1 h, a few USD)
uv run --with tiktoken python3 scripts/eval-context-accuracy.py \
  --v22 target/release/vimanam \
  --corpus /tmp/vimanam-eval-corpus \
  --model haiku \
  --trials 3 \
  --workers 1 \
  --output scripts/eval-context-accuracy-results.json
```

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
