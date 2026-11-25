/-
# Direction theorems for the `diff` severity rule table

The guarantees of the table encoded in `Severity.lean`, stated against
`effect`: a specification, written from JSON Schema semantics rather than from
the table, of whether a schema change narrows or widens the set of instances
the schema admits. This file reads no Rust of its own (it has no entry in
`formal/models.toml`); when the Rust table changes, `Severity.lean` is re-read
and these proofs either still build or name the guarantee that broke.
-/
import VimanamProofs.Severity

namespace Vimanam.Severity
/-! ## The specification: which way a change moves the admitted instances -/

/-- A schema change narrows (admits fewer instances) or widens (admits more). -/
inductive Dir
  | narrows
  | widens
  deriving DecidableEq, Repr

/-- The other direction. -/
def Dir.flip : Dir → Dir
  | .narrows => .widens
  | .widens => .narrows

/-- A boolean keyword's value before the change; an absent keyword has its
default `dflt`. `none` when a side is not a boolean. -/
def flagBefore (dflt : Bool) : Kind → Option Bool
  | .added _ => some dflt
  | .removed v => v.bool?
  | .changed v _ => v.bool?

/-- A boolean keyword's value after the change (see `flagBefore`). -/
def flagAfter (dflt : Bool) : Kind → Option Bool
  | .added v => v.bool?
  | .removed _ => some dflt
  | .changed _ v => v.bool?

/-- The direction of a change to a boolean keyword whose `true` admits more
(`nullable`, `additionalProperties`): `none` when either side is not a
boolean or the effective value did not change. -/
def flagDir (dflt : Bool) (k : Kind) : Option Dir :=
  match flagBefore dflt k, flagAfter dflt k with
  | some a, some b => if a = b then none else if b then some .widens else some .narrows
  | _, _ => none

/-- The change leaves the keyword's effective value as it was: a no-op. -/
def flagNoop (dflt : Bool) (k : Kind) : Bool :=
  match flagBefore dflt k, flagAfter dflt k with
  | some a, some b => a == b
  | _, _ => false

/-- The JSON Schema meaning of a change at a location, independent of the
table. `required` members restrict, `enum` members allow values, a whole `enum`
keyword restricts (an absent `enum` admits every value), `nullable` defaults
to `false` (OpenAPI 3.0) and `additionalProperties` to `true` (JSON Schema).
`type`, properties and every other location have no direction here. -/
def effect : Loc → Kind → Option Dir
  | .requiredElement, .added _ => some .narrows
  | .requiredElement, .removed _ => some .widens
  | .enumElement, .added _ => some .widens
  | .enumElement, .removed _ => some .narrows
  | .enum, .added _ => some .narrows
  | .enum, .removed _ => some .widens
  | .nullable, k => flagDir false k
  | .additionalProperties, k => flagDir true k
  | _, _ => none

/-- A change the JSON Schema semantics say does nothing: a boolean keyword
written or dropped at the value it already had by default. -/
def noop : Loc → Kind → Bool
  | .nullable, k => flagNoop false k
  | .additionalProperties, k => flagNoop true k
  | _, _ => false

/-! ## Finite enumerations for the proofs -/

def allVals : List Val := [.tru, .fls, .null, .other]

def allKinds : List Kind :=
  allVals.map .added ++ allVals.map .removed ++
    allVals.flatMap fun a => allVals.map (.changed a)

def allLocs : List Loc :=
  [.type, .requiredElement, .enumElement, .enum, .property,
   .additionalProperties, .nullable, .other]

theorem mem_allKinds (k : Kind) : k ∈ allKinds := by
  cases k with
  | added v => cases v <;> decide
  | removed v => cases v <;> decide
  | changed a b => cases a <;> cases b <;> decide

theorem mem_allLocs (l : Loc) : l ∈ allLocs := by
  cases l <;> decide

/-- A property of every location and change kind follows from checking the
finite table. -/
theorem forall_of_check {P : Loc → Kind → Prop} [∀ l k, Decidable (P l k)]
    (h : allLocs.all (fun l => allKinds.all (fun k => decide (P l k))) = true) :
    ∀ l k, P l k := by
  intro l k
  have hl := List.all_eq_true.mp h l (mem_allLocs l)
  have hk := List.all_eq_true.mp hl k (mem_allKinds k)
  exact of_decide_eq_true hk

/-! ## The pointer grammar -/

theorem locateGo_append (n : Node) (loc : Loc) (pre : Pointer) (s : String) :
    locateGo n loc (pre ++ [s]) = (walk n pre).locate s := by
  induction pre generalizing n loc with
  | nil => rfl
  | cons t ts ih => exact ih _ _

theorem underNotGo_append (n : Node) (pre rest : Pointer) :
    underNotGo n (pre ++ rest) = (underNotGo n pre || underNotGo (walk n pre) rest) := by
  induction pre generalizing n with
  | nil => simp [underNotGo, walk]
  | cons t ts ih => simp [underNotGo, walk, ih, Bool.or_assoc]

theorem walk_append (n : Node) (pre rest : Pointer) :
    walk n (pre ++ rest) = walk (walk n pre) rest := by
  induction pre generalizing n with
  | nil => rfl
  | cons t ts ih => exact ih _

theorem locate_nil : locate [] = .other := rfl

/-- `locate` and `under_not` walk the schema grammar, so a *property* whose
name is a keyword (`type`, `enum`, `not`, `required`, ...) is classified as a
property, not as that keyword: the misclassification the #45 review found. -/
theorem property_named_like_a_keyword (pre : Pointer) (name : String)
    (h : walk .schema pre = .schema) :
    locate (pre ++ ["properties", name]) = .property ∧
    underNot (pre ++ ["properties", name]) = underNot pre := by
  constructor
  · have := locateGo_append .schema .other (pre ++ ["properties"]) name
    simp only [List.append_assoc, List.cons_append, List.nil_append] at this
    rw [locate, this, walk_append, h]
    rfl
  · rw [underNot, underNotGo_append, h, underNot]
    simp [underNotGo, Node.child]

/-- `under_not` is true for a `not` keyword in schema position and everything
beneath it. -/
theorem underNot_of_not (pre rest : Pointer) (h : walk .schema pre = .schema) :
    underNot (pre ++ "not" :: rest) = true := by
  rw [underNot, underNotGo_append, h]
  simp [underNotGo]

/-! ## Anything under `not` is Review -/

/-- Both `request_schema_severity` and `response_schema_severity` rate every
change at or beneath a `not` keyword as Review: `not` reverses every
direction, so the tables do not guess. -/
theorem not_is_review (pre rest : Pointer) (k : Kind) (h : walk .schema pre = .schema) :
    requestSeverity (pre ++ "not" :: rest) k = .review ∧
    responseSeverity (pre ++ "not" :: rest) k = .review := by
  have hn := underNot_of_not pre rest h
  have hne : pre ++ "not" :: rest ≠ [] := by simp
  refine ⟨by simp [requestSeverity, hn], ?_⟩
  have hb : bodyDropped (pre ++ "not" :: rest) k = false := by
    unfold bodyDropped; split <;> simp_all
  simp [responseSeverity, hn, hb]

/-! ## Direction: requests are contravariant, responses covariant -/

theorem requestRule_narrows :
    ∀ l k, k.Wf → effect l k = some .narrows → requestRule l k = .breaking :=
  forall_of_check (P := fun l k => k.Wf → effect l k = some .narrows → requestRule l k = .breaking)
    (by decide)

theorem requestRule_widens :
    ∀ l k, k.Wf → effect l k = some .widens → requestRule l k ≠ .breaking :=
  forall_of_check (P := fun l k => k.Wf → effect l k = some .widens → requestRule l k ≠ .breaking)
    (by decide)

theorem responseRule_widens :
    ∀ l k, k.Wf → effect l k = some .widens → responseRule l k ≠ .nonBreaking :=
  forall_of_check
    (P := fun l k => k.Wf → effect l k = some .widens → responseRule l k ≠ .nonBreaking)
    (by decide)

theorem responseRule_narrows :
    ∀ l k, k.Wf → effect l k = some .narrows → responseRule l k ≠ .breaking :=
  forall_of_check
    (P := fun l k => k.Wf → effect l k = some .narrows → responseRule l k ≠ .breaking)
    (by decide)

/-- The root rule of `response_schema_severity` only fires at the root, where
`locate` finds no keyword. -/
theorem effect_of_bodyDropped (p : Pointer) (k : Kind) (h : bodyDropped p k = true) :
    effect (locate p) k = none := by
  unfold bodyDropped at h
  split at h
  · rfl
  · cases h

/-- Requests are contravariant: `request_schema_severity` (and so `severity`
on `RequestSchemaChanged` and `ParameterSchemaChanged`) rates every change
that narrows what a client may send as Breaking, unless it is under `not`. -/
theorem request_narrowing_breaking (p : Pointer) (k : Kind) (hk : k.Wf)
    (hn : underNot p = false) (he : effect (locate p) k = some .narrows) :
    requestSeverity p k = .breaking := by
  simp [requestSeverity, hn, requestRule_narrows _ _ hk he]

/-- `request_schema_severity` never rates a change that widens what a client
may send as Breaking. -/
theorem request_widening_not_breaking (p : Pointer) (k : Kind) (hk : k.Wf)
    (he : effect (locate p) k = some .widens) :
    requestSeverity p k ≠ .breaking := by
  unfold requestSeverity
  split
  · simp
  · exact requestRule_widens _ _ hk he

/-- Responses are covariant: `response_schema_severity` never rates a change
that widens what a server may return as Non-breaking. It is Breaking or
Review, so a widening is never silently accepted. -/
theorem response_widening_not_nonBreaking (p : Pointer) (k : Kind) (hk : k.Wf)
    (he : effect (locate p) k = some .widens) :
    responseSeverity p k ≠ .nonBreaking := by
  unfold responseSeverity
  split
  · simp
  · split
    · simp
    · exact responseRule_widens _ _ hk he

/-- `response_schema_severity` never rates a change that narrows what a server
may return as Breaking (the #169 rule). -/
theorem response_narrowing_not_breaking (p : Pointer) (k : Kind) (hk : k.Wf)
    (he : effect (locate p) k = some .narrows) :
    responseSeverity p k ≠ .breaking := by
  unfold responseSeverity
  split
  · next hb => rw [effect_of_bodyDropped p k hb] at he; cases he
  · split
    · simp
    · exact responseRule_narrows _ _ hk he

/-! ## Swapping old and new -/

/-- Swapping old and new flips the direction of every change. -/
theorem effect_swap :
    ∀ l k, effect l k.swap = (effect l k).map Dir.flip :=
  forall_of_check (P := fun l k => effect l k.swap = (effect l k).map Dir.flip) (by decide)

theorem requestRule_swap :
    ∀ l k, k.Wf → (effect l k ≠ none ∨ l = .property) → l ≠ .additionalProperties →
      requestRule l k = .breaking → requestRule l k.swap = .nonBreaking :=
  forall_of_check
    (P := fun l k => k.Wf → (effect l k ≠ none ∨ l = .property) → l ≠ .additionalProperties →
      requestRule l k = .breaking → requestRule l k.swap = .nonBreaking)
    (by decide)

theorem responseRule_swap :
    ∀ l k, k.Wf → (effect l k ≠ none ∨ l = .property) →
      responseRule l k = .breaking → responseRule l k.swap = .nonBreaking :=
  forall_of_check
    (P := fun l k => k.Wf → (effect l k ≠ none ∨ l = .property) →
      responseRule l k = .breaking → responseRule l k.swap = .nonBreaking)
    (by decide)

/-- `request_schema_severity`: a Breaking request change with a direction (or
a property added or removed) becomes Non-breaking when old and new are
swapped. `type` changes (Breaking both ways) and `additionalProperties`
(relaxing it is Review, not Non-breaking) are the exceptions. -/
theorem request_swap (p : Pointer) (k : Kind) (hk : k.Wf)
    (hd : effect (locate p) k ≠ none ∨ locate p = .property)
    (ha : locate p ≠ .additionalProperties)
    (hb : requestSeverity p k = .breaking) : requestSeverity p k.swap = .nonBreaking := by
  unfold requestSeverity at *
  split at hb
  · cases hb
  · next hn => simp [hn, requestRule_swap _ _ hk hd ha hb]

/-- `response_schema_severity`: a Breaking response change with a direction
(or a property removal) becomes Non-breaking when old and new are swapped. -/
theorem response_swap (p : Pointer) (k : Kind) (hk : k.Wf)
    (hd : effect (locate p) k ≠ none ∨ locate p = .property)
    (hb : responseSeverity p k = .breaking) (hr : p ≠ []) :
    responseSeverity p k.swap = .nonBreaking := by
  have hd0 : ∀ k', bodyDropped p k' = false := by
    intro k'; unfold bodyDropped; split <;> simp_all
  unfold responseSeverity at *
  simp only [hd0, Bool.false_eq_true, ↓reduceIte] at *
  split at hb
  · cases hb
  · next hn => simp [hn, responseRule_swap _ _ hk hd hb]

/-! ## The whole `enum` keyword -/

/-- `request_schema_severity` and `response_schema_severity` on a whole `enum`
keyword appearing where there was none (#175): the schema now restricts its
values, Breaking for what a client sends, Non-breaking for what it receives. -/
theorem enum_keyword_added (p : Pointer) (v : Val) (hl : locate p = .enum)
    (hn : underNot p = false) :
    requestSeverity p (.added v) = .breaking ∧ responseSeverity p (.added v) = .nonBreaking := by
  have hr : bodyDropped p (.added v) = false := by unfold bodyDropped; split <;> simp_all
  simp [requestSeverity, responseSeverity, hn, hl, hr, requestRule, responseRule]

/-- The whole `enum` keyword disappearing (#175 review): the schema admits any
value. Non-breaking for what a client sends; Review, never Non-breaking, for
what it receives. -/
theorem enum_keyword_removed (p : Pointer) (v : Val) (hl : locate p = .enum)
    (hn : underNot p = false) :
    requestSeverity p (.removed v) = .nonBreaking ∧ responseSeverity p (.removed v) = .review := by
  have hr : bodyDropped p (.removed v) = false := by unfold bodyDropped; split <;> simp_all
  simp [requestSeverity, responseSeverity, hn, hl, hr, requestRule, responseRule]

/-! ## Every directed change gets a decisive verdict -/

theorem requestRule_review :
    ∀ l k, requestRule l k = .review →
      effect l k = none ∨ (l = .additionalProperties ∧ effect l k = some .widens) :=
  forall_of_check
    (P := fun l k => requestRule l k = .review →
      effect l k = none ∨ (l = .additionalProperties ∧ effect l k = some .widens))
    (by decide)

theorem responseRule_review :
    ∀ l k, responseRule l k = .review →
      effect l k = none ∨ effect l k = some .widens ∨ l = .additionalProperties :=
  forall_of_check
    (P := fun l k => responseRule l k = .review →
      effect l k = none ∨ effect l k = some .widens ∨ l = .additionalProperties)
    (by decide)

/-- `request_schema_severity` falls through to Review only under `not`, on a
change whose direction the semantics do not fix, or when
`additionalProperties` is relaxed: every other (location, change) pair with a
direction has a Breaking or Non-breaking verdict. -/
theorem request_review_only_when_undirected (p : Pointer) (k : Kind)
    (h : requestSeverity p k = .review) :
    underNot p = true ∨ effect (locate p) k = none ∨
      (locate p = .additionalProperties ∧ effect (locate p) k = some .widens) := by
  unfold requestSeverity at h
  split at h
  · next hn => exact Or.inl hn
  · exact Or.inr (requestRule_review _ _ h)

/-- `response_schema_severity` falls through to Review only under `not`, on a
change with no direction, on a widening (deliberately left to a human), or on
`additionalProperties`, which the response table has no row for. -/
theorem response_review_only_when_undirected (p : Pointer) (k : Kind)
    (h : responseSeverity p k = .review) :
    underNot p = true ∨ effect (locate p) k = none ∨ effect (locate p) k = some .widens ∨
      locate p = .additionalProperties := by
  unfold responseSeverity at h
  split at h
  · cases h
  · split at h
    · next hn => exact Or.inl hn
    · exact Or.inr (responseRule_review _ _ h)

/-! ## The structural rows -/

/-- `severity` dispatches a parameter schema change to the request table:
parameters are part of what the client sends. -/
theorem parameter_schema_is_request (p : Pointer) (k : Kind) :
    severity (.parameterSchemaChanged p k) = severity (.requestSchemaChanged p k) := rfl

/-- `severity` treats a parameter's `required` flag the way the request table
treats a `required` schema member: newly required (or added required) is
Breaking, made optional is Non-breaking. -/
theorem parameter_required_like_schema_required (now : Bool) (v : Val) :
    severity (.parameterRequiredChanged now) =
      requestRule .requiredElement (if now then .added v else .removed v) ∧
    severity (.parameterAdded true) = requestRule .requiredElement (.added v) := by
  cases now <;> simp [severity, requestRule]

/-- `severity`: removing anything a client calls, sends or handles (an
endpoint, a parameter, a response code) or moving it is Breaking; adding an
endpoint, a response code or an optional parameter is Non-breaking. -/
theorem removal_breaking_addition_not (d : Bool) :
    severity (.endpointRemoved d) = .breaking ∧ severity .endpointMoved = .breaking ∧
    severity .parameterRemoved = .breaking ∧ severity .parameterLocationChanged = .breaking ∧
    severity .responseRemoved = .breaking ∧
    severity .endpointAdded = .nonBreaking ∧ severity (.parameterAdded false) = .nonBreaking ∧
    severity .responseAdded = .nonBreaking := by
  simp [severity]

/-- `severity` on `OperationIdChanged`: only gaining an operationId where there
was none is Non-breaking; generated clients name methods after it, so any
rename or removal is Breaking. -/
theorem operation_id (o n : Bool) :
    severity (.operationIdChanged o n) = .nonBreaking ↔ (o = false ∧ n = true) := by
  cases o <;> cases n <;> simp [severity]

/-! ## Findings: no-ops the table rates Breaking -/

theorem requestRule_noop_breaking :
    ∀ l k, k.Wf → noop l k = true → requestRule l k = .breaking →
      (l = .nullable ∧ k = .added .fls) ∨ (l = .additionalProperties ∧ k = .removed .tru) :=
  forall_of_check
    (P := fun l k => k.Wf → noop l k = true → requestRule l k = .breaking →
      (l = .nullable ∧ k = .added .fls) ∨ (l = .additionalProperties ∧ k = .removed .tru))
    (by decide)

theorem responseRule_noop_breaking :
    ∀ l k, k.Wf → noop l k = true → responseRule l k = .breaking →
      l = .nullable ∧ k = .removed .fls :=
  forall_of_check
    (P := fun l k => k.Wf → noop l k = true → responseRule l k = .breaking →
      l = .nullable ∧ k = .removed .fls)
    (by decide)

/-- Finding (open): `became` reads a removed boolean as flipping to the
opposite value and an added one as a change from it, ignoring the keyword's
default. So `request_schema_severity` rates two no-ops Breaking: writing
`nullable: false` (already the default) and dropping
`additionalProperties: true` (absent already means `true`). These are the
only no-ops it rates Breaking. -/
theorem request_noop_breaking (p : Pointer) (k : Kind) (hk : k.Wf)
    (hnoop : noop (locate p) k = true) (h : requestSeverity p k = .breaking) :
    (locate p = .nullable ∧ k = .added .fls) ∨
      (locate p = .additionalProperties ∧ k = .removed .tru) := by
  unfold requestSeverity at h
  split at h
  · cases h
  · exact requestRule_noop_breaking _ _ hk hnoop h

/-- The two no-ops of `request_noop_breaking` do occur: both are rated
Breaking at a top-level keyword. -/
theorem request_noop_breaking_witness :
    requestSeverity ["nullable"] (.added .fls) = .breaking ∧ noop .nullable (.added .fls) ∧
    requestSeverity ["additionalProperties"] (.removed .tru) = .breaking ∧
      noop .additionalProperties (.removed .tru) := by
  decide

/-- Finding (open): for the same reason `response_schema_severity` rates
dropping `nullable: false` (a no-op) Breaking, and that is the only no-op it
rates Breaking away from the root. -/
theorem response_noop_breaking (p : Pointer) (k : Kind) (hk : k.Wf) (hr : p ≠ [])
    (hnoop : noop (locate p) k = true) (h : responseSeverity p k = .breaking) :
    locate p = .nullable ∧ k = .removed .fls := by
  have hd : bodyDropped p k = false := by unfold bodyDropped; split <;> simp_all
  unfold responseSeverity at h
  simp only [hd, Bool.false_eq_true, ↓reduceIte] at h
  split at h
  · cases h
  · exact responseRule_noop_breaking _ _ hk hnoop h

/-- The no-op of `response_noop_breaking` does occur. -/
theorem response_noop_breaking_witness :
    responseSeverity ["nullable"] (.removed .fls) = .breaking ∧ noop .nullable (.removed .fls) := by
  decide

end Vimanam.Severity
