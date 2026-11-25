/-
# Findings against earlier rule tables

Not a model of current Rust (so it has no entry in `formal/models.toml`): the
rule tables and enum reporting as they were before #175, stated in the same
terms as `Severity.lean`, and the guarantees of that file that they violate.
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

The current Rust's counterparts are `Severity.response_narrowing_not_breaking`,
`EnumReport.response_widening_flagged` and
`EnumReport.request_narrowing_breaking`.
-/
import VimanamProofs.EnumReport

namespace Vimanam.History

open Vimanam.Severity Vimanam.EnumReport

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
    if became k true then .breaking
    else if became k false then .nonBreaking
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
  | l, k => requestRule l k

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
