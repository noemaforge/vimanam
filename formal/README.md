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
| `VimanamProofs/RefDiff.lean` | The `$ref`-aware schema diff reference model (#181): executable, no theorems. See "Reference semantics of `$ref` diffs". |
| `VimanamProofs/Audit.lean` | `#print axioms` for every theorem. |
| `OracleExport/` | `lake exe oracle`: writes `oracle/*.json` from the model definitions. `lake exe refdiff`: evaluates `RefDiff.lean` on spec pairs. |
| `refdiff/cases/`, `refdiff/expected/` | Fixed spec pairs for the `$ref` model (#182's review-round repros) and the model's pinned grouped output on them. |
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
none. The tables rate `type` changes and property additions/removals by
convention (a `patternProperties` entry counts as a property); everything
else falls through to Review (`Location::Other`, `_ => Review`), even where
JSON Schema would call it a narrowing or a widening. For example:
`maxLength`, `minLength`, `minimum`, `maximum`, `exclusiveMinimum`,
`exclusiveMaximum`, `multipleOf`, `pattern`, `format`, `minItems`,
`maxItems`, `uniqueItems`, `minProperties`, `maxProperties`, `readOnly`,
`writeOnly`, `prefixItems`, `contains`, `propertyNames`,
`dependentRequired`, `if`/`then`/`else`, `unevaluated*`,
`additionalProperties` between a boolean and a schema, `items` shape,
`allOf`/`oneOf`/`anyOf` variants, `const`, `default`, and `type` (including
adding `"null"` to a 3.1 `type` array). So "every narrowing" in a theorem below means every narrowing
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
| `component_follows_direction` | `ComponentSchemaChanged` (#181) with a plain reach uses the table of its direction, so every theorem above holds for component changes too. |
| `component_negated_review` | `ComponentSchemaChanged` reached through `not` (`negated`) is Review. |
| `component_matches_inline` | Reporting a component change once gives each occurrence's verdict: for a `$ref` in schema position at `pre` (not under `not`), the change at component pointer `p ≠ []` gets the verdict the inlined change at `pre ++ p` gets. |
| `component_negated_matches_inline` | A reach through `not` agrees with inlining: the inlined change under `not` is Review, as the `negated` component change is. |
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
* Shared components (#181): how `diff::components` and
  `resolve::PairResolver::localize` find components, their uses, directions
  and `negated` reaches. The model takes a `ComponentSchemaChanged` as given.
  The component's root (`p = []`) is excluded from
  `component_matches_inline` for two reasons. The response root rule fires
  there when the component's schema becomes `null`, but not at the inlined
  position. And `locate []` is `other` while the inlined position can be a
  keyword location: a request component change at `[]` to `false` is Review,
  but the same change inlined at `["additionalProperties"]` is Breaking; this
  cannot happen today, because the parser rejects boolean component schemas.
  `response_swap` keeps its `p ≠ []` hypothesis (it is about
  `response_schema_severity`, whichever change kind calls it).
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
has drifted, one annotation per changed file. The warning names only the refs
that changed since the state the digest was taken at: `read_against` when it
is that state, otherwise the newest of the last 100 commits touching the
model's files that is (this survives a squash merge, whose commit has the
bumped tree; CI checks out full history for it). When neither is found it
lists every ref and says why. `--bump` sets `read_against` to the merge-base
with `origin/main` when that has the bumped refs (it survives a squash
merge), else to `HEAD` when that does (commit the Rust first), else removes
it and says so. To clear a warning, re-read the model against the Rust, update
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
`ChangeKind` variant is rated by no case. One `variants!` macro invocation
declares both `ALL_VARIANTS` and `variant`, an exhaustive match with no
wildcard, so a name cannot be paired with the wrong arm: a new variant does
not compile until it is in the invocation, and the test then fails until some
case rates it. A name listed twice is an unreachable pattern, which
`clippy -D warnings` (CI) rejects. If
`formal/oracle/` is absent (a packaged crate that excludes `formal/`), the
test skips with a message, except under `CI`, where it fails.

Covered: every fixed pointer (keyword lookalikes, `not` at several depths)
with every change kind in both tables, random pointers over a keyword
vocabulary, every structural change, `ComponentSchemaChanged` in both
directions, negated and plain (the component root with every change kind,
then random pairs), and every pair of a small family of `enum` keywords. The `lean` workflow regenerates the vectors and fails when
the committed ones are stale; commit regenerated vectors with the model
change that moved them.

## CI

`.github/workflows/lean.yml` runs when `formal/` or `scripts/formal/`
changes: it installs the pinned Lean release (checked by SHA-256), builds
with `--wfail`, rejects `sorry` and `admit`, runs the axiom audit
(`scripts/formal/check-axiom-audit.sh`: every theorem has a `#print axioms`
line, every answer names only `propext`, `Quot.sound`, `Classical.choice`),
and checks the oracle vectors and that `refdiff/expected/` is the
reference model's current output on `refdiff/cases/`
(`scripts/formal/check-refdiff.sh`). Actions are pinned to commit SHAs.

## Re-reading after a rule change

When the drift check flags `Severity.lean`: re-read the flagged items, update
`Severity.lean` (and `Change`/`severity` for a new `ChangeKind` variant: the
oracle test's `variants!` list will not compile until the variant is there),
extend the exporter and the replay, regenerate the vectors, and bump the
digest. `Direction.lean` changes only when a guarantee changes. #182
(`ComponentSchemaChanged`) was re-read this way.

## Reference semantics of `$ref` diffs

`VimanamProofs/RefDiff.lean` is the specification of what `diff` reports for
schemas that use `$ref`, which #181 decides in outline only ("compare `$ref`
targets as edges"). It is an executable reference model used as an oracle,
not a proof development: the definitions are `partial`, the sanity checks are
`#guard`s, and proofs are deferred until the design settles. This section is
the same specification in prose.

### Inputs

* **Components** are the document's `components.schemas` (OpenAPI 3) or
  `definitions` (Swagger 2). A `$ref` names a component when it is
  `#/components/schemas/NAME` or `#/definitions/NAME`, in either kind of
  document, with the token decoded (`~1` is `/`, `~0` is `~`). The two
  spellings are the same name everywhere below. Any other `$ref`, or one
  naming a component the document does not declare, is a plain object.
* **Uses** are the schema positions of an endpoint present in both
  documents (matched on `METHOD path`): each non-body parameter matched on
  `(in, name)` (Swagger 2 parameters rebuilt from `type`, `format`, `items`,
  `enum`), the request body (first media type), and each response status
  present on both sides. Parameters and request bodies are the **request**
  direction, responses the **response** direction.
* Every schema is first **canonicalised** as `canonicalize_schema_value`
  does: annotations (`description`, `title`, `example`, `examples`,
  `deprecated`, `x-*`) are removed from schema objects, `type` arrays are
  sets, and `required`/`enum` are compared as sets. Annotation siblings of a
  `$ref` therefore vanish before anything below: they never mask.

### The comparison walk

Two schemas are compared as `diff_values` compares them (objects key by key,
`required`/`enum` as sets reported per member, a `properties` map or
`required` set missing on one side compared as empty, a missing `enum`
reported whole, other arrays by index, scalars by equality; a `required`
member removed together with its property is not reported, within one
comparison), with three rules at a schema position holding a `$ref`:

1. **Hop.** Both sides reference a declared component, `X` (old) and `Y`
   (new). The `$ref` keys are not compared. The *mask* is the set of keys a
   sibling sets on either side. Each masked key is compared here, as the
   effective value on each side: the sibling if that side has one, else the
   target's value at that key (siblings win, as resolution merges them).
   Every other key is the pair `(X, Y)`'s business: the hop leads to the
   state `(X, Y, mask, neg)`, where `neg` says the hop is under a `not`.
2. **One-sided expansion.** One side references a declared component and
   the other side is an inline object (including an undeclared `$ref`
   object): the reference is replaced by its target with its siblings merged
   over it, and the walk continues at the same position. Two sides can keep
   expanding against each other's inline content forever; the walk is a
   function of the value pair it compares, so where a pair produced by an
   expansion recurs deeper on the chain the rest is periodic and nothing more
   is reported, and where it recurs at the same position (an alias loop such
   as `X = {"$ref": X, "type": "array"}`) its value is the fixpoint: the
   merged siblings without `$ref`.
3. **As written.** Anything else (a reference facing a non-object or
   nothing, two undeclared references, a value present on one side only) is
   compared as written. A subtree present on one side only is one
   addition or removal; its contents are not expanded.

### Owners, states and reach

A change found by a walk belongs to its **owner**: the use site for the walk
from a use's root, or the pair `(X, Y)` for the walk that compares the two
targets from their roots. Each pair is compared once, whatever reaches it.

A state `(X, Y, mask, neg)` *sees* the pair's changes whose relative pointer
does not start with a masked key (overlay masking: under a masked key the
value comes from the sibling, and was compared at the hop), and reaches the
states of the hops the pair's walk found at positions it sees. A hop at the
pair's root (an alias, `X = {"$ref": Z, ...}`) inherits the mask; `neg`
accumulates. A use reaches the states of its own hops and everything
reachable from them; the set of states is finite, so cycles need no cut.

### Output

The **grouped view** is, for each use `u` and each state `s` it reaches:

* every change of the use's own walk: `(u, use site, pointer, change)`;
* every change of `s`'s pair that `s` sees: `(u, (X, Y), relative pointer,
  change, neg)`;
* if `X ≠ Y`, one retarget fact `(u, (X, Y), "", retargeted X → Y, neg)`,
  even when the two components are structurally identical.

Grouping these by everything but `u` gives #181's output: each change once
per (owner, pointer, change, direction, `neg`), with the uses that reach it.
A component change whose top-level key is masked on every path from a use is
not listed for that use.

**Verdicts**: a retarget is Review; a change reached under `not` (`neg`) is
Review, as `under_not` makes it for an inline schema; otherwise the
direction's table from `Severity.lean` (`requestSeverity` or
`responseSeverity`) on the owner-relative pointer.

The **per-path view** is the same walk unfolded per use with absolute
pointers, cutting where a state recurs on its own path: what fully inlining
both sides and diffing by JSON pointer reports, which is main's semantics
without main's artifacts (main cuts cycles by ref spelling, separately on
each side, compares the two literal `$ref` objects at a cut, and stops after
64 hops). Regrouping the per-path view by owner gives the grouped view,
except for retarget facts and for a `required` member whose property is
removed by a different owner (the per-path view suppresses it over the
whole tree, the grouped view within one owner's comparison).

### Running it against the Rust

```bash
cd formal && lake exe refdiff OLD.json NEW.json      # model output for one pair
scripts/formal/check-refdiff.sh                      # pinned output on refdiff/cases is current
python3 -I scripts/formal/refdiff-harness.py --pr PR_BIN --main MAIN_BIN --seeds 300 --out DIR
```

The harness generates spec pairs (cycles, siblings over references,
retargets, aliases, one-sided components, mixed spellings, references under
`not`; both directions of each), adds `refdiff/cases/`, and compares:

* the grouped view with `--pr`'s `diff --format json --contract 1` records
  (one per use and change, with `component`/`old_component`), record by
  record: `(use, owner, owner-relative pointer, change, verdict)`;
* the per-path view with `--main`'s v1 records.

Objects and arrays in changes compare only as "an object"/"an array" (how
far a side was expanded is not observable through the table) and `$ref`
strings compare by component name. Each disagreement is classified
(`verdict-under-not`, `retarget-fact-missing`, `attribution-*`,
`extra-in-rust-*`, `missing-in-rust-*`), and one pair per class is shrunk by
delta debugging into `DIR/repro/`. The report is `DIR/report.md`.

### Not covered

`$ref`s at non-schema positions (`default`, an `example` is stripped
anyway), parameter, response and request-body `$ref`s, external references,
the `"a | b"` type-union string, non-object component targets beyond
"compare as written", endpoint moves (v1 output never pairs them), key
order (`Lean.Json` sorts keys, so "first media type" is the first in sorted
order), and every structural change (endpoints, parameters, status codes).

## Next candidates

Each is a property the current tests check by fuzzing only.

* `$ref` resolver equality (#180). `resolve::PairResolver::schemas_equal`
  skips resolving a pair it proves identical (equal roots whose transitive
  component closure is unchanged). Property: whenever it answers "equal",
  `DocResolver::resolve` of the two sides, then `canonicalize_schema_value`,
  gives equal values, so skipping the pair never hides a change. Needs a
  model of component maps, the ref-stack cycle guard and the depth cap of 64.
* Edge-diff completeness (#181), now stated executably in `RefDiff.lean`
  (see "Reference semantics of `$ref` diffs"). With refs diffed as edges, a
  change inside a shared component is reported once per (component pair,
  pointer, usage direction). `component_matches_inline` already shows each
  such report gets every occurrence's inlined verdict. Property to prove:
  regrouping the per-path view by owner gives the grouped view (with the two
  exceptions stated there), in the same direction (and `negated` exactly
  when the reach passes through `not`), and a change's verdict in the
  grouped view equals its per-path verdict; so the set of Breaking verdicts,
  and exit code 3, are unchanged. Needs total (non-`partial`) definitions
  and a model of `diff::components`' reachability walk.
