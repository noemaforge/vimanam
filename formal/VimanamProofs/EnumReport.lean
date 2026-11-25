/-
# How the differ reports an `enum` keyword

Models what `diff_values` (`src/diff/value.rs`: `diff_nodes`, `diff_sets`,
`empty_counterpart`) reports for the `enum` keyword of one schema object, and
proves that the verdicts `severity` then gives follow the direction of the
change to the set of admitted values.

An `enum` is `none` (no keyword: every value is admitted) or `some xs` (only
the members of `xs`). The differ reports:

* gaining or losing the whole keyword as one change at `/enum` (#175: a
  missing `enum` is not an empty set, so `empty_counterpart` has no case for
  it);
* otherwise each member only in the old list as `Removed` at `/enum/<member>`,
  then each member only in the new list as `Added`, in list order.

Members are natural numbers (the pointer segment of a JSON number is its
decimal text, `set_element_segment`); the payload of each change is
`Val.other`, since the table never reads it.

## Boundary

One `enum` keyword at the schema root, with no other keyword changing. Nested
positions follow from `locate` and are not restated. Duplicate members are
outside the model (canonicalisation keeps them; the oracle generates none).
-/
import VimanamProofs.Direction

namespace Vimanam.EnumReport

open Vimanam.Severity

/-- An `enum` keyword: absent, or its member list. -/
abbrev Enum := Option (List Nat)

/-- Whether the enum admits value `x`. -/
def admits : Enum → Nat → Bool
  | none, _ => true
  | some xs, x => xs.contains x

/-- The pointer of one member. -/
def memberPtr (x : Nat) : Pointer := ["enum", toString x]

/-- What `diff_values` reports for `{"enum": old}` against `{"enum": new}`
(an absent side is the object without the key). -/
def report : Enum → Enum → List (Pointer × Kind)
  | none, none => []
  | some _, none => [(["enum"], .removed .other)]
  | none, some _ => [(["enum"], .added .other)]
  | some a, some b =>
    (a.filter (fun x => !b.contains x)).map (fun x => (memberPtr x, .removed .other)) ++
    (b.filter (fun x => !a.contains x)).map (fun x => (memberPtr x, .added .other))

theorem locate_member (x : Nat) : locate (memberPtr x) = .enumElement := rfl

theorem underNot_member (x : Nat) : underNot (memberPtr x) = false := rfl

theorem locate_enum : locate ["enum"] = .enum := rfl

/-- A member added: Non-breaking for requests, Review for responses. -/
theorem member_added (x : Nat) :
    requestSeverity (memberPtr x) (.added .other) = .nonBreaking ∧
    responseSeverity (memberPtr x) (.added .other) = .review := by
  have hb : bodyDropped (memberPtr x) (.added .other) = false := rfl
  simp only [requestSeverity, responseSeverity, hb, underNot_member, locate_member]
  exact ⟨rfl, rfl⟩

/-- A member removed: Breaking for requests, Non-breaking for responses. -/
theorem member_removed (x : Nat) :
    requestSeverity (memberPtr x) (.removed .other) = .breaking ∧
    responseSeverity (memberPtr x) (.removed .other) = .nonBreaking := by
  have hb : bodyDropped (memberPtr x) (.removed .other) = false := rfl
  simp only [requestSeverity, responseSeverity, hb, underNot_member, locate_member]
  exact ⟨rfl, rfl⟩

theorem underNot_enum : underNot ["enum"] = false := rfl

/-- Every reported change sits outside `not`, and none is the root. -/
theorem report_ptr (o n : Enum) (c : Pointer × Kind) (h : c ∈ report o n) :
    (c.1 = ["enum"] ∨ ∃ x, c.1 = memberPtr x) := by
  unfold report at h
  split at h
  · cases h
  · simp at h; exact Or.inl (by rw [h])
  · simp at h; exact Or.inl (by rw [h])
  · simp [List.mem_append, List.mem_map] at h
    rcases h with ⟨x, _, hx⟩ | ⟨x, _, hx⟩ <;> exact Or.inr ⟨x, by rw [← hx]⟩

/-- Response side (`diff_values` then `response_schema_severity`): when the new
`enum` admits a value the old one did not (the server may now return it), at
least one reported change is not Non-breaking, so the widening is never
silent. This covers deleting the whole keyword (the #175 round-1 finding). -/
theorem response_widening_flagged (o n : Enum) (x : Nat)
    (hw : admits o x = false ∧ admits n x = true) :
    ∃ c ∈ report o n, responseSeverity c.1 c.2 ≠ .nonBreaking := by
  match o, n with
  | none, _ => simp [admits] at hw
  | some a, none =>
    exact ⟨(["enum"], .removed .other), by simp [report], by decide⟩
  | some a, some b =>
    refine ⟨(memberPtr x, .added .other), ?_, by simp [(member_added x).2]⟩
    simp only [admits] at hw
    simp [report, List.mem_append, List.mem_map, List.mem_filter]
    exact ⟨x, ⟨List.contains_iff_mem.mp hw.2, fun h => by simp at hw; exact hw.1 h⟩, rfl⟩

/-- Request side (`diff_values` then `request_schema_severity`): when the old
`enum` admitted a value the new one does not (a client may have been sending
it), at least one reported change is Breaking. This covers introducing an
`enum` where there was none. -/
theorem request_narrowing_breaking (o n : Enum) (x : Nat)
    (hw : admits o x = true ∧ admits n x = false) :
    ∃ c ∈ report o n, requestSeverity c.1 c.2 = .breaking := by
  match o, n with
  | _, none => simp [admits] at hw
  | none, some b =>
    exact ⟨(["enum"], .added .other), by simp [report], by decide⟩
  | some a, some b =>
    refine ⟨(memberPtr x, .removed .other), ?_, (member_removed x).1⟩
    simp only [admits] at hw
    simp [report, List.mem_append, List.mem_map, List.mem_filter]
    exact ⟨x, ⟨List.contains_iff_mem.mp hw.1, fun h => by simp at hw; exact hw.2 h⟩, rfl⟩

/-- The converse, for requests: when the new `enum` admits everything the old
one did, no reported change is Breaking. -/
theorem request_widening_not_breaking (o n : Enum)
    (hw : ∀ x, admits o x = true → admits n x = true) :
    ∀ c ∈ report o n, requestSeverity c.1 c.2 ≠ .breaking := by
  intro c hc
  match o, n, hc with
  | none, none, hc => simp [report] at hc
  | some a, none, hc =>
    simp [report] at hc; rw [hc]; decide
  | none, some b, hc =>
    -- b admits everything: impossible for a finite list.
    have := hw (b.foldr (· + ·) 0 + 1) rfl
    simp [admits] at this
    have hle : ∀ ys : List Nat, ∀ y ∈ ys, y ≤ ys.foldr (· + ·) 0 := by
      intro ys; induction ys with
      | nil => simp
      | cons z zs ih =>
        intro y hy; simp at hy
        rcases hy with rfl | hy
        · simp
        · have := ih y hy; simp; omega
    have := hle b _ this; omega
  | some a, some b, hc =>
    simp [report, List.mem_append, List.mem_map, List.mem_filter] at hc
    rcases hc with ⟨x, ⟨hxa, hxb⟩, rfl⟩ | ⟨x, _, rfl⟩
    · have := hw x (by simp [admits, hxa]); simp [admits] at this
      exact absurd this (by simpa using hxb)
    · simp [(member_added x).1]

end Vimanam.EnumReport
