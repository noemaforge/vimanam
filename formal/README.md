# Formal model of the `diff` severity rule table

A small Lean 4 development that models the rule table behind `vimanam diff`
(`diff::severity` in `src/diff/mod.rs`) and proves its direction guarantees:
what a change to a request or response schema does to existing clients. The
rule table is where Vimanam's costliest `diff` bugs were (#169: 62 Breaking
findings on a real OpenAI diff, 4 of them real), and it is small and
table-shaped, so it is the first thing worth modelling.

Lean only: no Mathlib, no Lake dependencies, toolchain pinned in
`lean-toolchain`. `lake build --wfail` must pass with no `sorry`, and every
theorem must rest on Lean's standard axioms alone.

```bash
cd formal
lake build --wfail                     # the proofs
lake exe oracle                        # regenerate oracle/*.json
../scripts/formal/check-axiom-audit.sh # every theorem audited, standard axioms only
../scripts/formal/check-oracle.sh      # committed vectors match the models
```

```bash
cargo test --lib formal_oracle         # replay the vectors through the Rust
python3 -I scripts/formal/check-model-drift.py
```

## Layout

| Path | What it is |
| --- | --- |
| `VimanamProofs/Severity.lean` | The table encoding only: `severity`, the request and response schema tables, `became`, and the pointer grammar (`Node`, `locate`, `under_not`). Re-reading the Rust touches this file. |
| `VimanamProofs/Direction.lean` | The specification (`effect`, `noop`) and the direction theorems over `Severity.lean`. Reads no Rust of its own. |
| `VimanamProofs/EnumReport.lean` | What `diff_values` reports for an `enum` keyword, and the direction theorems for the verdicts it then gets. |
| `VimanamProofs/History.lean` | The pre-#175 tables and reporting, and the findings they violate. Reads no current Rust. |
| `VimanamProofs/Audit.lean` | `#print axioms` for every theorem. |
| `OracleExport/` | `lake exe oracle`: writes `oracle/*.json` from the model definitions. |
| `oracle/` | The committed oracle vectors. |
| `models.toml` | Which Rust each model reads, with a fingerprint of it. |

## How the models are written

The definitions are the Rust, rule for rule and in the Rust's match order:
`requestRule` is the `match` of `request_schema_severity`, `Node.child` is
`Node::child`, and so on. Each model file opens with a header that says which
Rust it covers, how values are abstracted, and its boundary: what it does not
cover, so that silence is not read as a guarantee.

The guarantees are stated against a specification that does not come from
the table. `effect l k` says, from JSON Schema semantics alone, whether a
change of kind `k` at location `l` narrows or widens the set of instances the
schema admits: a `required` member added narrows; an `enum` member added
widens; a whole `enum` keyword added narrows (an absent `enum` admits every
value); `nullable` (default `false`) and `additionalProperties` (default
`true`) move with their effective value. `type` and properties have no
direction there. A table theorem then says, for example, that every change
`effect` calls a narrowing is Breaking on the request side.

`effect` gives a direction to those changes only. Every other change has
none, and the tables rate it Review (`type` and properties excepted: Breaking
or Non-breaking by convention), even where JSON Schema would call it a
narrowing or a widening: `maxLength`, `minLength`, `minimum`, `maximum`,
`exclusiveMinimum`, `exclusiveMaximum`, `multipleOf`, `pattern`, `format`,
`minItems`, `maxItems`, `uniqueItems`, `additionalProperties` between a
boolean and a schema, `items` shape, `allOf`/`oneOf`/`anyOf` variants,
`const`, `default`, and `type` (including adding `"null"` to a 3.1 `type`
array). So "every narrowing" in a theorem below means every narrowing
`effect` recognises; the others are left to a human as Review.

Every theorem states a property of named Rust behaviour, and its doc comment
says which in one sentence. Lemmas that only serve proofs (`forall_of_check`,
`locateGo_append`, ...) are used by a proof. The finite tables are proved by
`decide` over every location and change kind (`forall_of_check`), then lifted
to pointers through `locate` and `under_not`; nothing uses `native_decide`.

## What is proved

`Direction.lean`, over the table in `Severity.lean` (`diff::severity`, `request_schema_severity`,
`response_schema_severity`, `locate`, `under_not`):

| Theorem | Rust behaviour |
| --- | --- |
| `request_narrowing_breaking` | Requests are contravariant: every change `effect` calls a narrowing of what a client may send is Breaking, unless under `not`. |
| `request_widening_not_breaking` | No change `effect` calls a widening of what a client may send is Breaking. |
| `response_widening_not_nonBreaking` | Responses are covariant: no change `effect` calls a widening of what a server may return is Non-breaking (it is Breaking or Review). |
| `response_narrowing_not_breaking` | No change `effect` calls a narrowing of what a server may return is Breaking (the #169 rule). |
| `effect_swap` | Swapping old and new flips every change's direction (the specification is symmetric). |
| `request_swap` | A Breaking request change with a direction, or a property removal, is Non-breaking with old and new swapped; `type` and `additionalProperties` are the exceptions. |
| `response_swap` | A Breaking response change with a direction, or a property removal, is Non-breaking swapped. |
| `not_is_review` | Every change at or beneath a `not` keyword is Review in both tables. |
| `property_named_like_a_keyword` | A property called `type`, `enum`, `not`, `required`, ... is classified as a property, and does not count as `not` (the #45 review bug). |
| `enum_keyword_added` | A whole `enum` keyword appearing: request Breaking, response Non-breaking. |
| `enum_keyword_removed` | A whole `enum` keyword disappearing: request Non-breaking, response Review. |
| `request_review_only_when_undirected` | Every (location, change) pair with a direction has a Breaking or Non-breaking request verdict, except relaxing `additionalProperties` (Review). |
| `response_review_only_when_undirected` | The response table falls through to Review only under `not`, without a direction, on a widening, or on `additionalProperties` (no row). |
| `parameter_schema_is_request` | Table pin: parameter schemas use the request table. |
| `parameter_required_like_schema_required` | A parameter's `required` flag is rated like a `required` schema member. |
| `removal_breaking_addition_not` | Table pin: removing or moving an endpoint, removing a parameter or response code, or moving a parameter is Breaking; adding an endpoint, response code or optional parameter is Non-breaking. |
| `operation_id` | Table pin: only gaining an `operationId` is Non-breaking. |
| `request_noop_breaking`, `request_noop_breaking_witness` | Finding (open), see below. |
| `response_noop_breaking`, `response_noop_breaking_witness` | Finding (open), see below. |

`EnumReport.lean` (`diff_values` on an `enum` keyword, then `severity`):

| Theorem | Rust behaviour |
| --- | --- |
| `response_widening_flagged` | Whenever the new `enum` admits a value the old one did not, some reported response change is not Non-breaking, including deleting the whole keyword. |
| `request_narrowing_breaking` | Whenever the old `enum` admitted a value the new one does not, some reported request change is Breaking, including introducing the keyword. |
| `request_widening_not_breaking` | When the new `enum` admits everything the old one did, no reported request change is Breaking. |
| `member_added`, `member_removed` | The verdicts of one member added or removed, at any member. |

## Findings

Against earlier tables (`History.lean`), each the guarantee above that the
old Rust violates:

* `pre175_response_narrowing_breaking` (#169): the table at `bd051f8^` rated
  a response `enum` member removal, a narrowing, Breaking. Violates
  `response_narrowing_not_breaking`.
* `round1_enum_deletion_silent` (#175, review round 1): with the #169 fix
  alone, deleting a whole response `enum` was reported member by member, each
  now Non-breaking, while the response had widened to any value. Violates
  `EnumReport.response_widening_flagged`.
* `pre175_enum_introduction_silent`: before #175, introducing an `enum` on a
  request, a narrowing, was reported as member additions, each Non-breaking.
  Violates `EnumReport.request_narrowing_breaking`.

Against the current Rust (open, filed as #184, not fixed in this change).
Once #184 is fixed these should become guarantees, "no change `noop` calls a
no-op is Breaking", replacing the two finding theorems:

* `request_noop_breaking`: `became` reads a removed boolean as flipping and an
  added one as a change, ignoring the keyword's default. So the request table
  rates writing `nullable: false` (already the default) and dropping
  `additionalProperties: true` (absent already means `true`) as Breaking,
  though neither changes what the schema admits. Among the no-ops `noop`
  recognises (boolean keywords at their default), these are the only ones it
  rates Breaking. Other no-ops are outside `noop`: `type: "string"` to
  `type: ["string"]` is also rated Breaking (noted in #184). The first is asserted as Breaking by
  `severity_request_schema_breaking_rows` today.
* `response_noop_breaking`: for the same reason the response table rates
  dropping `nullable: false` Breaking; it is the only such no-op `noop`
  recognises.

## What the model does not cover

* How `diff_values` produces changes, beyond one `enum` keyword at the root
  (`EnumReport.lean`); `$ref` resolution, canonicalisation and the
  suppression of the companion `required` removal in `value_changes`.
* JSON pointer escaping (`~0`, `~1`): pointers are their decoded tokens.
* Values: the table sees only `true`, `false`, `null` and "anything else", so
  that is all the model distinguishes. In particular `type` changes have no
  direction in `effect`; adding `"null"` to an OpenAPI 3.1 `type` array
  widens the schema and is still rated Breaking on the request side
  (conservative, not unsafe).
* Response status codes: removing one is rated Breaking by convention
  (clients rely on documented statuses), which the model records but does not
  justify by variance.
* Endpoint matching and moves, the protobuf and GraphQL rule tables
  (`src/contract/`), the v2 JSON verdicts, and rendering.

## Keeping the models in step with the Rust

`models.toml` lists, for each model file except `Audit.lean`,
`Direction.lean` and `History.lean`, the Rust items it reads (`src/diff/mod.rs#severity`, ...) and
`read_digest`, a SHA-256 over those items with comments, `#[cfg(test)]` code
and whitespace normalised away.
`scripts/formal/check-model-drift.py` recomputes it; the `model-drift`
workflow runs it on every change to `src/diff/`, `formal/` or
`scripts/formal/` and annotates the changed Rust with a warning when a model
has drifted. When the model's `read_against` commit is in the clone and is
the state its digest was taken at (CI checks out full history for this), the
warning names only the refs that changed since it; otherwise it lists every
ref of the model and says why. To clear a warning, re-read the model against the Rust, update
it if the change matters, and run

```bash
python3 -I scripts/formal/check-model-drift.py --bump Severity
```

A bump is a claim that the model was re-read; editing the Lean alone does not
clear drift. A model file with no entry, or a ref that no longer resolves,
fails the check.

## Differential oracle

The drift check says the Rust changed; it does not say whether the two still
agree. The model definitions compute, so the oracle uses them as the
reference: `lake exe oracle` evaluates them over fixed and generated inputs
(SplitMix64, one fixed seed) and writes inputs and outputs to
`oracle/Severity.json`. `src/diff/formal_oracle.rs`, part of the default
`cargo test`, rebuilds each input as Rust values, calls the real
`diff::severity` (through `RequestSchemaChanged`, `ParameterSchemaChanged`,
`ResponseSchemaChanged` and every structural `ChangeKind`), `locate`,
`under_not` and `diff_values`, and fails with the function, case index and
input of every mismatch. Nothing in the comparison restates the table in
Rust. The test also fails when a file in `oracle/` has no replay, and when a
`ChangeKind` variant is rated by no case: its `variant` function matches
`ChangeKind` exhaustively with no wildcard, so a new variant does not compile
until the oracle lists it (and then fails until the Lean model covers it). If
`formal/oracle/` is absent (a packaged crate that excludes `formal/`), the
test skips with a message.

Covered: every fixed pointer (keyword lookalikes, `not` at several depths)
with every change kind in both tables, random pointers over a keyword
vocabulary, every structural change, and every pair of a small family of
`enum` keywords. The `lean` workflow regenerates the vectors and fails when
the committed ones are stale; commit regenerated vectors with the model
change that moved them.

## CI

`.github/workflows/lean.yml` runs when `formal/` or `scripts/formal/`
changes: it installs the pinned Lean release (checked by SHA-256), builds
with `--wfail`, rejects `sorry` and `admit`, runs the axiom audit
(`scripts/formal/check-axiom-audit.sh`: every theorem has a `#print axioms`
line, every answer names only `propext`, `Quot.sound`, `Classical.choice`)
and checks the oracle vectors. Actions are pinned to commit SHAs.

## After #182

noemaforge/vimanam#182 (#181) adds `ChangeKind::ComponentSchemaChanged`,
which `severity` dispatches by usage direction to the same two schema tables.
That changes `ChangeKind` and `severity`, so the drift check flags
`Severity.lean`. The re-read is: add the variant (with its direction) to
`Change` and `severity` in `Severity.lean`, extend the oracle's replay and
structural cases, regenerate the vectors, bump the digest, and add a
theorem that a component change gets the request or response verdict of its
direction. `Direction.lean` should not need to change, with one check:
component pointers are relative to the component, so the root rule
(`bodyDropped`, `p = []`) now fires at a component root, and
`response_swap`'s `p ≠ []` hypothesis must be re-checked there. The oracle
test's `variant` match will not compile until the new variant is added.
Expect a textual conflict next to `#[cfg(test)] mod formal_oracle;` in
`src/diff/mod.rs`, where #182 adds `mod components;`.

## Next candidates

Each is a property the current tests check by fuzzing only.

* `$ref` resolver equality (#180). `resolve::PairResolver::schemas_equal`
  skips resolving a pair it proves identical (equal roots whose transitive
  component closure is unchanged). Property: whenever it answers "equal",
  `DocResolver::resolve` of the two sides, then `canonicalize_schema_value`,
  gives equal values, so skipping the pair never hides a change. Needs a
  model of component maps, the ref-stack cycle guard and the depth cap of 64.
* Edge-diff completeness (#181). With refs diffed as edges, a change inside a
  shared component is reported once per (component pair, pointer, usage
  direction). Property: for every change the fully inlined diff reports at an
  operation, the edge diff reports the same change in the reached component
  with that operation among its uses, in the same direction, and vice versa;
  so the set of Breaking verdicts, and exit code 3, are unchanged.
