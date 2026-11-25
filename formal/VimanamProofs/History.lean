/-
# Findings against earlier rule tables

Not a model of current Rust (so it has no entry in `formal/models.toml`): the
rule tables and enum reporting as they were before #175, stated in the same
terms as `Severity.lean`, and the guarantees of `Direction.lean` they violate.
Each theorem here is a real bug, kept as a finding:

* #169: `response_schema_severity` at `bd051f8^` rated a response `enum`
  member removal Breaking. Narrowing what a server returns cannot break a
  client; on a real OpenAI diff this produced most of 62 Breaking findings,
  of which 4 were real.
* #175 round 1: with the #169 fix alone, deleting a whole response `enum` was
  reported (by `empty_counterpart`, which then treated a missing `enum` as an
  empty set) as one member removal per old member, each now Non-breaking. The
  response had widened to any value and the gate stayed silent.
* Before #175 the same reporting made introducing an `enum` on a request,
  which narrows what a client may send, a list of member additions, each
  Non-breaking.
* #184: until the fix, `became` ignored a boolean keyword's default, so
  writing `nullable: false` or dropping `additionalProperties: true` in a
  request, and dropping `nullable: false` in a response, were Breaking
  though none of them changes anything. Found by this model before it was
  fixed; the fix changed the verdict of no-ops and nothing else.

The current Rust's counterparts are `Severity.response_narrowing_not_breaking`,
`EnumReport.response_widening_flagged`,
`EnumReport.request_narrowing_breaking` and `Severity.noop_never_breaking`.
-/
import VimanamProofs.EnumReport

namespace Vimanam.History

open Vimanam.Severity Vimanam.EnumReport

/-! ## Before #184 -/

/-- `became` before #184: a removed boolean read as flipping to the opposite
value and an added one as a change from it, whatever the keyword's default. -/
def becamePre184 (k : Kind) (flag : Bool) : Bool :=
  match k with
  | .added v | .changed _ v => v.bool? == some flag
  | .removed v =>
    match v.bool? with
    | some was => was != flag
    | none => false

/-- The match of `request_schema_severity` before #184 (`be2bd87`). -/
def requestRulePre184 : Loc → Kind → Severity
  | .additionalProperties, k => if becamePre184 k false then .breaking else .review
  | .nullable, k =>
    if becamePre184 k true then .nonBreaking
    else if becamePre184 k false then .breaking
    else .review
  | l, k => requestRule l k

/-- The match of `response_schema_severity` before #184 (`be2bd87`). -/
def responseRulePre184 : Loc → Kind → Severity
  | .additionalProperties, _ => .review
  | .nullable, k =>
    if becamePre184 k true then .breaking
    else if becamePre184 k false then .nonBreaking
    else .review
  | l, k => responseRule l k

/-- Finding #184: the pre-#184 request table rated two no-ops Breaking,
writing `nullable: false` and dropping `additionalProperties: true`; among
the no-ops `noop` recognises, only those. -/
theorem pre184_request_noop_breaking :
    ∀ l k, k.Wf → noop l k = true → requestRulePre184 l k = .breaking →
      (l = .nullable ∧ k = .added .fls) ∨ (l = .additionalProperties ∧ k = .removed .tru) :=
  forall_of_check
    (P := fun l k => k.Wf → noop l k = true → requestRulePre184 l k = .breaking →
      (l = .nullable ∧ k = .added .fls) ∨ (l = .additionalProperties ∧ k = .removed .tru))
    (by decide)

/-- Finding #184: the pre-#184 response table rated dropping `nullable: false`
Breaking, the only such no-op. -/
theorem pre184_response_noop_breaking :
    ∀ l k, k.Wf → noop l k = true → responseRulePre184 l k = .breaking →
      l = .nullable ∧ k = .removed .fls :=
  forall_of_check
    (P := fun l k => k.Wf → noop l k = true → responseRulePre184 l k = .breaking →
      l = .nullable ∧ k = .removed .fls)
    (by decide)

/-- The three no-ops did occur, each Breaking. -/
theorem pre184_witnesses :
    requestRulePre184 .nullable (.added .fls) = .breaking ∧
    requestRulePre184 .additionalProperties (.removed .tru) = .breaking ∧
    responseRulePre184 .nullable (.removed .fls) = .breaking := by
  decide

/-- The #184 fix changed the request table on no-ops only. -/
theorem fix184_request_only_noops :
    ∀ l k, k.Wf → requestRule l k ≠ requestRulePre184 l k → noop l k = true :=
  forall_of_check
    (P := fun l k => k.Wf → requestRule l k ≠ requestRulePre184 l k → noop l k = true)
    (by decide)

/-- The #184 fix changed the response table on no-ops only. -/
theorem fix184_response_only_noops :
    ∀ l k, k.Wf → responseRule l k ≠ responseRulePre184 l k → noop l k = true :=
  forall_of_check
    (P := fun l k => k.Wf → responseRule l k ≠ responseRulePre184 l k → noop l k = true)
    (by decide)

/-! ## Before #175 -/

/-- The match of `response_schema_severity` before #175 (`bd051f8^`): no
`Enum` location, no `under_not` check, and member removal Breaking. -/
def responseRulePre175 : Loc → Kind → Severity
  | .property, .removed _ => .breaking
  | .property, .added _ => .nonBreaking
  | .type, _ => .breaking
  | .enumElement, .removed _ => .breaking
  | .enumElement, .added _ => .review
  | .requiredElement, .removed _ => .breaking
  | .requiredElement, .added _ => .nonBreaking
  | .nullable, k =>
    if becamePre184 k true then .breaking
    else if becamePre184 k false then .nonBreaking
    else .review
  | _, _ => .review

/-- The same table after the #169 fix alone (the #175 branch at its first
review round): member removal Non-breaking, still no `Enum` location. -/
def responseRuleRound1 : Loc → Kind → Severity
  | .enumElement, .removed _ => .nonBreaking
  | l, k => responseRulePre175 l k

/-- The match of `request_schema_severity` before #175: as now, less the two
whole-`enum` rows (the location did not exist). -/
def requestRulePre175 : Loc → Kind → Severity
  | .enum, _ => .review
  | l, k => requestRulePre184 l k

/-- `diff_values` on an `enum` keyword before #175: `empty_counterpart`
compared a missing `enum` with an empty set, so gaining or losing the keyword
was reported member by member. -/
def reportPre175 : Enum → Enum → List (Pointer × Kind)
  | o, n => report (some (o.getD [])) (some (n.getD []))

/-- A finite list misses some number. -/
theorem outside (a : List Nat) : a.contains (a.foldr (· + ·) 0 + 1) = false := by
  have hle : ∀ y ∈ a, y ≤ a.foldr (· + ·) 0 := by
    induction a with
    | nil => simp
    | cons z zs ih =>
      intro y hy; simp at hy
      rcases hy with rfl | hy
      · simp
      · have := ih y hy; simp; omega
  cases h : a.contains (a.foldr (· + ·) 0 + 1)
  · rfl
  · have := hle _ (List.contains_iff_mem.mp h); omega

/-- Finding #169: the pre-#175 response table rates a narrowing (an `enum`
member removed from a response) Breaking, violating
`Severity.response_narrowing_not_breaking`. -/
theorem pre175_response_narrowing_breaking :
    effect .enumElement (.removed .other) = some .narrows ∧
    responseRulePre175 .enumElement (.removed .other) = .breaking := by
  decide

/-- Finding (#175 round 1): deleting a response `enum` widens the
response to any value, yet under the round-1 table every change the old
reporting produced for it was Non-breaking, violating
`EnumReport.response_widening_flagged`. -/
theorem round1_enum_deletion_silent (a : List Nat) :
    (∃ x, admits (some a) x = false ∧ admits none x = true) ∧
    ∀ c ∈ reportPre175 (some a) none, responseRuleRound1 (locate c.1) c.2 = .nonBreaking := by
  refine ⟨⟨_, outside a, rfl⟩, ?_⟩
  · intro c hc
    simp [reportPre175, report, List.mem_map, List.mem_filter] at hc
    obtain ⟨x, _, rfl⟩ := hc
    rfl

/-- Finding (before #175): introducing an `enum` on a request narrows what a
client may send, yet the old reporting and table rated every resulting change
Non-breaking, violating `EnumReport.request_narrowing_breaking`. -/
theorem pre175_enum_introduction_silent (b : List Nat) :
    (∃ x, admits none x = true ∧ admits (some b) x = false) ∧
    (∀ c ∈ reportPre175 none (some b), requestRulePre175 (locate c.1) c.2 = .nonBreaking) := by
  refine ⟨⟨_, rfl, outside b⟩, ?_⟩
  intro c hc
  simp [reportPre175, report, List.mem_map, List.mem_filter] at hc
  obtain ⟨x, _, rfl⟩ := hc
  rfl

end Vimanam.History
