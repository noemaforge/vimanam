/-
# Compatibility theorems for the protobuf `diff` rules

The guarantees of the rules encoded in `Protobuf.lean`, stated against a
specification written from protobuf's encoding and Vimanam's documented
policy (`docs/protobuf.md`, "Identity and compatibility"), not from the
rules. This file reads no Rust of its own (it has no entry in
`formal/models.toml`).

## The specification

On the binary wire a field is its number and its encoding; names exist only
in source and in JSON/text. So:

* `wireHazardMsg`: an evolution of `M` endangers existing binary peers when
  a field number keeps a field whose shape changes (type, label or oneof
  membership: Vimanam rates every type change Breaking, including
  usage-dependent compatible conversions, and the docs list label and oneof
  changes as wire rules), when a field number is vacated without being
  reserved (`FIELD_NO_DELETE_UNLESS_NUMBER_RESERVED`: it can be reused with
  another meaning), when a required field is removed, or when a field
  appears at a number the old `M` reserved or as a required field.
* `wireHazardEnum`: an evolution of `E` endangers binary peers when a number
  is vacated without being reserved, when a value appears at a number the
  old `E` reserved, or when a label present on both sides moves to another
  number that the old `E` used ("Existing enum label reassigned to another
  used number": an old peer decodes the label's new number as the label that
  number meant to it). A label moved to a fresh number is a value removed
  plus a value added, a hazard only when its old number is vacated without
  being reserved, as for a field.

`wireHazardMsg` and `wireHazardEnum` restate the policy rows of the docs in
terms of numbers and shapes, not encoding semantics derived independently:
the theorems below protect those rows against regressions in the rules.
* `sourceHazardMsg`, `sourceHazardEnum`: a name that existing source or JSON
  uses disappears or changes meaning: `M`, `E` or the oneof removed, a field
  name gone, a field renamed at its number or moved to another number, a
  field's shape changed, an enum label gone, a reserved name reused, a
  required field added to an existing `M`.

Neither side of the specification looks at verdicts or impacts.

## What is proved

For `M` the wire gate is exact: on a surviving `M`, `--breaking wire` fails
if and only if the evolution is a wire hazard (`wire_gate_iff_hazard`), so
renames and deprecations at a number, removals of optional fields whose
number is reserved, and optional additions at unreserved numbers are never
wire-breaking (`no_wire_of_kept_shapes`). For `E` the wire gate is sound
(`enum_wire_gate_sound`) and catches every vacated and every reused reserved
number (`enum_vacated_fails_wire`, `enum_reserved_reuse_fails_wire`), but
not every moved label: the finding #190 (`enum_label_moved_wire_gap`,
`enum_alias_moved_wire_gap`), which `wire_gap_is_label_move` shows is the
only wire hazard the gate misses on the fragment. Every source hazard fails
`--breaking source` (`source_hazard_fails_source`), and `--breaking all`
fails exactly when `wire` or `source` does (`fails_all_iff`). The rule table
theorems pin which pointers supply wire evidence, that renames are
source-only, and how additions, removals and the gate behave.
-/
import VimanamProofs.Protobuf

namespace Vimanam.Protobuf

/-! ## Valid inputs (what protoc accepts) -/

/-- Whether a list has no duplicates (`List.Nodup`, as a `Bool`). -/
def nodup {α : Type} [DecidableEq α] : List α → Bool
  | [] => true
  | x :: xs => !xs.contains x && nodup xs

/-- protoc's rules for `M` on the fragment: distinct numbers (in `1 ..
536870911`) and names, oneof members are optional, no field uses a reserved
number or name, reserved ranges are nonempty and do not overlap, and `d` is
never a field name. -/
def Msg.valid (m : Msg) : Bool :=
  nodup (m.fields.map (·.num)) && nodup (m.fields.map (·.name)) &&
  m.fields.all (fun f =>
    decide (0 < f.num) && decide (f.num ≤ 536870911) && f.name != .d &&
    (!f.inOneof || f.label == .optional) &&
    !msgReserved m.resNums f.num && !m.resNames.contains f.name) &&
  m.resNums.all (fun r => decide (r.1 < r.2)) &&
  nodup m.resNames &&
  (m.resNums.all fun r => m.resNums.all fun s =>
    r == s || decide (r.2 ≤ s.1) || decide (s.2 ≤ r.1))

/-- protoc's rules for `E` on the fragment: at least one value, distinct
labels, 32-bit numbers, no value uses a reserved number or label, reserved
ranges are nonempty and do not overlap, and `D` is never a label. -/
def Enum.valid (e : Enum) : Bool :=
  !e.values.isEmpty && nodup (e.values.map (·.label)) &&
  e.values.all (fun v =>
    decide (-2147483648 ≤ v.num) && decide (v.num ≤ 2147483647) && v.label != .D &&
    !enumReserved e.resNums v.num && !e.resNames.contains v.label) &&
  e.resNums.all (fun r => decide (r.1 ≤ r.2)) &&
  nodup e.resNames &&
  (e.resNums.all fun r => e.resNums.all fun s =>
    r == s || decide (r.2 < s.1) || decide (s.2 < r.1))

/-! ## The specification -/

/-- What the binary encoding of a field depends on, as Vimanam's policy
counts it. -/
def Field.shape (f : Field) : Ty × Label × Bool := (f.ty, f.label, f.inOneof)

/-- An evolution of a surviving `M` that endangers binary peers. -/
def wireHazardMsg (o g : Msg) : Bool :=
  o.fields.any (fun f =>
    match g.at f.num with
    | some h => f.shape != h.shape
    | none => !msgReserved g.resNums f.num || f.label == .required) ||
  g.fields.any (fun h =>
    (o.at h.num).isNone && (msgReserved o.resNums h.num || h.label == .required))

/-- The number of a label, if `E` has it. -/
def Enum.numOf (e : Enum) (l : ELabel) : Option Int :=
  (e.values.find? (·.label == l)).map (·.num)

/-- Some label of the old `E` is, in the new one, at another number that some
label of the old `E` used: an old peer decodes the label's new number as a
different label. (A move to a fresh number is a value removed plus a value
added, rated by the vacated-number rule, as for fields.) -/
def labelMoved (o g : Enum) : Bool :=
  o.values.any fun v => match g.numOf v.label with
    | some n => n != v.num && o.values.any (·.num == n)
    | none => false

/-- An evolution of a surviving `E` that endangers binary peers. -/
def wireHazardEnum (o g : Enum) : Bool :=
  o.values.any (fun v =>
    !g.values.any (·.num == v.num) && !enumReserved g.resNums v.num) ||
  g.values.any (fun w => enumReserved o.resNums w.num) ||
  labelMoved o g

/-- An evolution of `M` that breaks existing source or JSON. -/
def sourceHazardMsg : Option Msg → Option Msg → Bool
  | some _, none => true
  | some o, some g =>
    (o.hasOneof && !g.hasOneof) ||
    o.fields.any (fun f =>
      !g.fields.any (·.name == f.name) ||
      g.fields.any (fun h => h.name == f.name && (h.num != f.num || h.shape != f.shape))) ||
    g.fields.any (fun h =>
      o.resNames.contains h.name || ((o.at h.num).isNone && h.label == .required)) ||
    o.fields.any (fun f => match g.at f.num with
      | some h => h.name != f.name
      | none => false)
  | _, _ => false

/-- An evolution of `E` that breaks existing source or JSON. -/
def sourceHazardEnum : Option Enum → Option Enum → Bool
  | some _, none => true
  | some o, some g =>
    o.values.any (fun v => !g.values.any (·.label == v.label)) ||
    g.values.any (fun w => o.resNames.contains w.label)
  | _, _ => false

/-- The wire hazards of the whole fragment. -/
def wireHazard (om gm : Option Msg) (oe ge : Option Enum) : Bool :=
  (match om, gm with
   | some o, some g => wireHazardMsg o g
   | _, _ => false) ||
  (match oe, ge with
   | some o, some g => wireHazardEnum o g
   | _, _ => false)

/-- The source hazards of the whole fragment. -/
def sourceHazard (om gm : Option Msg) (oe ge : Option Enum) : Bool :=
  sourceHazardMsg om gm || sourceHazardEnum oe ge

/-! ## Finite checks over the rule table -/

/-- Every neutral `ChangeKind`. -/
def allKinds : List ChangeKind :=
  [.serviceAdded, .serviceRemoved, .operationAdded, .operationRemoved, .operationRenamed,
   .operationMoved, .typeAdded, .typeRemoved, .typeKindChanged, .argumentAdded,
   .argumentRemoved, .argumentRequiredChanged, .argumentTypeChanged, .fieldAdded,
   .fieldRemoved, .fieldTypeChanged, .fieldRequiredChanged, .nullabilityChanged,
   .enumValueAdded, .enumValueRemoved, .deprecatedChanged, .defaultChanged,
   .paginationChanged, .wireIdentityChanged, .resultAdded, .resultRemoved, .constraintChanged]

/-- Every pointer class. -/
def allPtrs : List Ptr :=
  [.deprecated, .type, .default, .jsonName, .oneof, .group, .clientStreaming,
   .serverStreaming, .cardinality, .presence, .input, .output, .syntax, .fileOptions, .other]

/-- `allKinds` lists every kind. -/
theorem mem_allKinds (k : ChangeKind) : k ∈ allKinds := by
  cases k <;> decide

/-- `allPtrs` lists every pointer class. -/
theorem mem_allPtrs (p : Ptr) : p ∈ allPtrs := by
  cases p <;> decide

/-- A property of every `classify` input follows from checking the finite
table. -/
theorem forall_of_check {P : Bool → ChangeKind → Ptr → Prop} [∀ r k p, Decidable (P r k p)]
    (h : [false, true].all (fun r => allKinds.all (fun k =>
      allPtrs.all (fun p => decide (P r k p)))) = true) :
    ∀ r k p, P r k p := by
  intro r k p
  have hr := List.all_eq_true.mp h r (by cases r <;> decide)
  have hk := List.all_eq_true.mp hr k (mem_allKinds k)
  exact of_decide_eq_true (List.all_eq_true.mp hk p (mem_allPtrs p))

/-! ## The rule table -/

/-- Whether a fact with these impacts is in the wire or the source scope. -/
def gated (i : List Impact) : Bool := inScope .wire i || inScope .source i

/-- Every Breaking verdict `classify` gives names a wire or source impact,
so `--breaking all` fails exactly when `wire` or `source` would. -/
theorem classify_breaking_gated :
    ∀ r k p, (classify r k p).1 = .breaking → gated (classify r k p).2 = true :=
  forall_of_check (P := fun r k p =>
    (classify r k p).1 = .breaking → gated (classify r k p).2 = true) (by decide)

/-- No Review or Non-breaking verdict carries a wire impact, and a
Non-breaking one carries none at all. -/
theorem classify_not_breaking_no_wire :
    ∀ r k p, (classify r k p).1 ≠ .breaking → (classify r k p).2.contains .wire = false ∧
      ((classify r k p).1 = .nonBreaking → (classify r k p).2 = []) :=
  forall_of_check (P := fun r k p => (classify r k p).1 ≠ .breaking →
    (classify r k p).2.contains .wire = false ∧
      ((classify r k p).1 = .nonBreaking → (classify r k p).2 = [])) (by decide)

/-- The pointers the docs' rule table names as wire rules: `FIELD_SAME_TYPE`,
`FIELD_SAME_ONEOF`, `FIELD_SAME_GROUP_ENCODING`, the RPC streaming rules,
`FIELD_SAME_CARDINALITY` and the RPC request/response types. -/
def documentedWirePtrs : List Ptr :=
  [.type, .oneof, .group, .clientStreaming, .serverStreaming, .cardinality, .input, .output]

/-- Which attribute changes supply wire evidence: the documented wire rules,
each Breaking, and, as current Rust behaviour with no documented row
(#192, open), `presence` and `syntax` under `PROTOBUF_SURFACE_CHANGED`. No
other attribute pointer gives a wire impact. This pins the behaviour; it does
not certify the `presence` and `syntax` rows. -/
theorem attribute_wire_evidence :
    ∀ r p, ((classify r (attributeKind p) p).2.contains .wire = true ↔
      p ∈ documentedWirePtrs ∨ p ∈ [.presence, .syntax]) ∧
    ((classify r (attributeKind p) p).2.contains .wire = true →
      (classify r (attributeKind p) p).1 = .breaking) ∧
    (p ∈ [.presence, .syntax] → rule (attributeKind p) p = "PROTOBUF_SURFACE_CHANGED") := by
  intro r p
  cases r <;> cases p <;> decide

/-- A same-number rename (the structural `name` change) and a JSON name
change break source and JSON only: Breaking, impacts `source, wire_json`
(`FIELD_SAME_NAME`, `FIELD_SAME_JSON_NAME`). -/
theorem rename_source_only (r : Bool) :
    structuralKind .name = some .wireIdentityChanged ∧
    classify r .wireIdentityChanged .other = (.breaking, [.source, .wireJson]) ∧
    rule .wireIdentityChanged .other = "FIELD_SAME_NAME" ∧
    classify r (attributeKind .jsonName) .jsonName = (.breaking, [.source, .wireJson]) ∧
    rule (attributeKind .jsonName) .jsonName = "FIELD_SAME_JSON_NAME" := by
  cases r <;> decide

/-- No structural difference other than the name is a fact: a renamed
symbol's native location changes with it, and parents, services and
references follow identity. -/
theorem structural_only_name (s : Structural) :
    (structuralKind s).isSome = true ↔ s = .name := by
  cases s <;> decide

/-- Every field type change is Breaking on the wire, in source and in JSON,
whatever the subject (`FIELD_SAME_TYPE`; conservative for wire-compatible
conversions). -/
theorem type_change_wire (r : Bool) (p : Ptr) :
    classify r .fieldTypeChanged p = (.breaking, [.wire, .source, .wireJson]) ∧
    rule .fieldTypeChanged p = "FIELD_SAME_TYPE" := by
  cases r <;> cases p <;> decide

/-- Changing whether an RPC streams breaks gRPC framing: Breaking on the
wire, in source and in JSON. -/
theorem streaming_breaks_wire (r : Bool) :
    classify r (attributeKind .clientStreaming) .clientStreaming =
      (.breaking, [.wire, .source, .wireJson]) ∧
    classify r (attributeKind .serverStreaming) .serverStreaming =
      (.breaking, [.wire, .source, .wireJson]) := by
  cases r <;> decide

/-- Additions are Non-breaking with no impacts, except a field added as
`required`, which is Breaking on the wire and in source (before `diff`'s
downgrade for a new message). -/
theorem additions (r : Bool) (p : Ptr) :
    (∀ k ∈ [ChangeKind.serviceAdded, .operationAdded, .typeAdded, .enumValueAdded],
      classify r k p = (.nonBreaking, [])) ∧
    classify r .fieldAdded p = if r then (.breaking, [.wire, .source]) else (.nonBreaking, []) := by
  cases r <;> cases p <;> decide

/-- Removals of protobuf symbols are Breaking in source only; wire evidence
for a removal comes from `diff`'s composite facts. -/
theorem removals_source (r : Bool) (p : Ptr) :
    ∀ k ∈ [ChangeKind.fieldRemoved, .enumValueRemoved, .serviceRemoved, .operationRemoved,
      .typeRemoved], classify r k p = (.breaking, [.source]) := by
  cases r <;> cases p <;> decide

/-- Deprecation and effective-default changes need review and never fail a
gate. -/
theorem deprecation_default_review (r : Bool) (p : Ptr) :
    classify r .deprecatedChanged p = (.review, [.source]) ∧
    classify r .defaultChanged p = (.review, [.source]) := by
  cases r <;> cases p <;> decide

/-! ## The gate -/

/-- Only Breaking facts fail a gate. -/
theorem gate_breaking_only (sc : Scope) (s : Severity) (i : List Impact) :
    breakingGate sc s i = true → s = .breaking := by
  cases s <;> simp [breakingGate]

/-- `--breaking all` fails whenever `wire` or `source` does. -/
theorem gate_scope_le_all (sc : Scope) (s : Severity) (i : List Impact) :
    breakingGate sc s i = true → breakingGate .all s i = true := by
  cases sc <;> cases s <;> simp [breakingGate, inScope]

/-- A gate never fails on a fact outside its scope. -/
theorem gate_outside_scope (sc : Scope) (s : Severity) (i : List Impact) :
    inScope sc i = false → breakingGate sc s i = false := by
  intro h; simp [breakingGate, h]

/-- On gated Breaking facts `--breaking all` is exactly `wire` or
`source`. -/
theorem gate_all_of_gated (s : Severity) (i : List Impact)
    (h : s = .breaking → gated i = true) :
    breakingGate .all s i = (breakingGate .wire s i || breakingGate .source s i) := by
  cases s <;> simp_all [breakingGate, inScope, gated]

/-! ## The composite diff: algebra of `fails` -/

/-- The gate over two lists of facts. -/
theorem fails_append (sc : Scope) (xs ys : List Fact) :
    fails sc (xs ++ ys) = (fails sc xs || fails sc ys) := by
  simp [fails, List.any_append]

/-- The gate over facts gathered per element. -/
theorem fails_flatMap {α : Type} (sc : Scope) (l : List α) (f : α → List Fact) :
    fails sc (l.flatMap f) = l.any (fun x => fails sc (f x)) := by
  simp [fails, List.any_flatMap]

/-- The gate over a conditional list of facts. -/
theorem fails_ite (sc : Scope) (c : Prop) [Decidable c] (xs ys : List Fact) :
    fails sc (if c then xs else ys) = if c then fails sc xs else fails sc ys := by
  split <;> rfl

/-- No facts never fail a gate. -/
@[simp]
theorem fails_nil (sc : Scope) : fails sc [] = false := rfl

/-- The gate over one more fact. -/
@[simp]
theorem fails_cons (sc : Scope) (x : Fact) (xs : List Fact) :
    fails sc (x :: xs) = (breakingGate sc x.sev x.impacts || fails sc xs) := rfl

/-- A field kept at its number fails the wire gate exactly when its shape changes (`FIELD_SAME_TYPE`, `FIELD_SAME_CARDINALITY`, presence, `FIELD_SAME_ONEOF`). -/
theorem wire_fieldPairFacts (f h : Field) :
    fails .wire (fieldPairFacts f h) = (f.shape != h.shape) := by
  obtain ⟨n, nm, ty, lb, oo, dp⟩ := f
  obtain ⟨n', nm', ty', lb', oo', dp'⟩ := h
  cases ty <;> cases ty' <;> cases lb <;> cases lb' <;> cases oo <;> cases oo' <;>
    simp [fieldPairFacts, fails_append, fails_ite, breakingGate, inScope, neutral,
      classify, attributeKind, Field.shape, Field.presence, Field.defaultKind]

/-- A field gone from a surviving `M` fails the wire gate exactly when its number is not reserved or it was required. -/
theorem wire_fieldGoneFacts (g : Msg) (f : Field) :
    fails .wire (fieldGoneFacts g f) = (!msgReserved g.resNums f.num || f.label == .required) := by
  obtain ⟨n, nm, ty, lb, oo, dp⟩ := f
  cases lb <;> cases h : msgReserved g.resNums n <;>
    simp [fieldGoneFacts, breakingGate, inScope, neutral, classify, h]

/-- `Msg.at` finds a field with that number. -/
theorem msg_at_some {m : Msg} {n : Nat} {f : Field} (h : m.at n = some f) :
    f ∈ m.fields ∧ f.num = n := by
  unfold Msg.at at h
  exact ⟨List.mem_of_find?_eq_some h, by simpa using List.find?_some h⟩

/-- A valid `M` reserves none of its own numbers. -/
theorem msg_valid_not_reserved {m : Msg} (hm : m.valid = true) {f : Field}
    (hf : f ∈ m.fields) : msgReserved m.resNums f.num = false := by
  simp only [Msg.valid, Bool.and_eq_true, List.all_eq_true] at hm
  have := hm.1.1.1.2 f hf
  simpa using this.1.2

/-- A field of the new `M` fails the wire gate exactly when it is new and required, or reuses a number the old `M` reserved. -/
theorem wire_added (o : Msg) (ho : o.valid = true) (h : Field) :
    ((match o.at h.num with
      | some _ => fails .wire []
      | none => fails .wire [neutral h.subject (h.label = .required) .other .fieldAdded .other]) ||
      fails .wire (fieldReservedFacts o h)) =
    ((o.at h.num).isNone && (msgReserved o.resNums h.num || h.label == .required)) := by
  cases hat : o.at h.num with
  | some f =>
    have ⟨hf, hn⟩ := msg_at_some hat
    have hr := msg_valid_not_reserved ho hf
    rw [hn] at hr
    simp [fieldReservedFacts, hr]
    split <;> simp [breakingGate, inScope]
  | none =>
    obtain ⟨n, nm, ty, lb, oo, dp⟩ := h
    cases lb <;> cases hr : msgReserved o.resNums n <;>
      simp [fieldReservedFacts, neutral, classify, breakingGate, inScope, hr] <;>
      split <;> simp [breakingGate, inScope] <;> decide

/-- **The wire gate is exact on `M`**: on a surviving `M` (the old one
valid), `--breaking wire` fails if and only if `wireHazardMsg` holds. -/
theorem wire_gate_iff_hazard (o g : Msg) (ho : o.valid = true) :
    fails .wire (msgPairFacts o g) = wireHazardMsg o g := by
  unfold msgPairFacts wireHazardMsg
  simp only [fails_append, fails_flatMap]
  have h1 : fails .wire (if o.hasOneof && !g.hasOneof then
      [neutral .oneof false .oneof .typeRemoved .other] else []) = false := by
    split <;> simp [neutral, classify, breakingGate, inScope]
  have h2 : fails .wire (if !o.hasOneof && g.hasOneof then
      [neutral .oneof false .other .typeAdded .other] else []) = false := by
    split <;> simp [neutral, classify, breakingGate, inScope]
  rw [h1, h2, Bool.false_or, Bool.false_or]
  congr 1
  · congr 1; funext f
    cases g.at f.num <;> simp [wire_fieldPairFacts, wire_fieldGoneFacts]
  · congr 1; funext h
    rw [← wire_added o ho h]
    cases o.at h.num <;> simp

/-- A key that has no duplicates identifies the element. -/
theorem nodup_map_inj {α β : Type} [DecidableEq β] {f : α → β} :
    ∀ {l : List α}, nodup (l.map f) = true → ∀ {x y}, x ∈ l → y ∈ l → f x = f y → x = y
  | [], _, _, _, hx, _, _ => by simp at hx
  | z :: l, h, x, y, hx, hy, e => by
    simp [nodup] at h
    simp at hx hy
    rcases hx with rfl | hx <;> rcases hy with rfl | hy
    · rfl
    · exact absurd e.symm (h.1 y hy)
    · exact absurd e (h.1 x hx)
    · exact nodup_map_inj h.2 hx hy e

/-- `==` on field names is decided equality. -/
@[simp]
theorem fname_beq_eq (x y : FName) : (x == y) = decide (x = y) := rfl

/-- A field kept at its number fails the source gate exactly when its name or shape changes. -/
theorem source_fieldPairFacts (f h : Field) :
    fails .source (fieldPairFacts f h) = (f.name != h.name || f.shape != h.shape) := by
  obtain ⟨n, nm, ty, lb, oo, dp⟩ := f
  obtain ⟨n', nm', ty', lb', oo', dp'⟩ := h
  cases ty <;> cases ty' <;> cases lb <;> cases lb' <;> cases oo <;> cases oo' <;>
    simp [fieldPairFacts, fails_append, fails_ite, breakingGate, inScope, neutral,
      classify, attributeKind, Field.shape, Field.presence, Field.defaultKind, bne]

/-- A field gone from a surviving `M` always fails the source gate (`FIELD_NO_DELETE`). -/
theorem source_fieldGoneFacts (g : Msg) (f : Field) : fails .source (fieldGoneFacts g f) = true := by
  simp [fieldGoneFacts, fails_append, breakingGate, inScope, neutral, classify]

/-- A field of the new `M` named as the old `M` reserved fails the source gate. -/
theorem source_fieldReservedFacts (o : Msg) (h : Field) (hr : o.resNames.contains h.name = true) :
    fails .source (fieldReservedFacts o h) = true := by
  simp only [fieldReservedFacts, hr, Bool.or_true, ite_true]
  split <;> simp [breakingGate, inScope]

/-- A valid `M` has distinct field names. -/
theorem msg_valid_names {m : Msg} (hm : m.valid = true) : nodup (m.fields.map (·.name)) = true := by
  simp only [Msg.valid, Bool.and_eq_true] at hm
  exact hm.1.1.1.1.2

/-- A valid `M` has distinct field numbers. -/
theorem msg_valid_nums {m : Msg} (hm : m.valid = true) : nodup (m.fields.map (·.num)) = true := by
  simp only [Msg.valid, Bool.and_eq_true] at hm
  exact hm.1.1.1.1.1

/-- An old field whose own facts fail the source gate. -/
theorem source_of_old (o g : Msg) (f : Field) (hf : f ∈ o.fields)
    (h : (match g.at f.num with
      | some h => fails .source (fieldPairFacts f h)
      | none => fails .source (fieldGoneFacts g f)) = true) :
    fails .source (msgPairFacts o g) = true := by
  unfold msgPairFacts
  simp only [fails_append, fails_flatMap, Bool.or_eq_true, List.any_eq_true]
  refine Or.inl (Or.inr ⟨f, hf, ?_⟩)
  revert h; cases g.at f.num <;> simp

/-- A new field whose own facts fail the source gate. -/
theorem source_of_new (o g : Msg) (h : Field) (hh : h ∈ g.fields)
    (hs : fails .source ((match o.at h.num with
      | some _ => []
      | none => [neutral h.subject (h.label = .required) .other .fieldAdded .other]) ++
      fieldReservedFacts o h) = true) :
    fails .source (msgPairFacts o g) = true := by
  unfold msgPairFacts
  simp only [fails_append, fails_flatMap, Bool.or_eq_true, List.any_eq_true]
  refine Or.inr ⟨h, hh, ?_⟩
  revert hs; cases o.at h.num <;> simp

/-- **Every source hazard of a surviving `M` fails `--breaking source`**
(the new `M` valid). -/
theorem source_hazard_msg (o g : Msg) (hg : g.valid = true)
    (hz : sourceHazardMsg (some o) (some g) = true) : fails .source (msgPairFacts o g) = true := by
  simp only [sourceHazardMsg, Bool.or_eq_true, List.any_eq_true, Bool.and_eq_true,
    Bool.not_eq_true'] at hz
  rcases hz with (((hone | ⟨f, hf, hgone | ⟨h, hh, hn, hd⟩⟩) | ⟨h, hh, hres | ⟨hnone, hreq⟩⟩) |
    ⟨f, hf, hren⟩)
  · unfold msgPairFacts
    simp only [fails_append, Bool.or_eq_true]
    refine Or.inl (Or.inl (Or.inl ?_))
    simp [hone, neutral, classify, breakingGate, inScope]
  · apply source_of_old o g f hf
    cases hat : g.at f.num with
    | none => simp [source_fieldGoneFacts]
    | some h =>
      have ⟨hh, _⟩ := msg_at_some hat
      have : h.name ≠ f.name := by
        intro e; simp only [List.any_eq_false] at hgone; exact hgone h hh (by simp [e])
      simp [source_fieldPairFacts, Ne.symm this]
  · apply source_of_old o g f hf
    cases hat : g.at f.num with
    | none => simp [source_fieldGoneFacts]
    | some h' =>
      have ⟨hh', hn'⟩ := msg_at_some hat
      simp only [source_fieldPairFacts]
      by_cases e : h'.name = f.name
      · have : h' = h := nodup_map_inj (msg_valid_names hg) hh' hh (by simp at hn; rw [e, hn])
        subst this
        simp at hn hd
        rcases hd with hd | hd
        · exact absurd hn' hd
        · simp [bne]; exact Or.inr (Ne.symm hd)
      · simp [Ne.symm e]
  · apply source_of_new o g h hh
    simp [fails_append, source_fieldReservedFacts o h hres]
  · apply source_of_new o g h hh
    simp at hnone hreq
    simp [hnone, hreq, neutral, classify, breakingGate, inScope]
  · apply source_of_old o g f hf
    cases hat : g.at f.num with
    | none => simp [source_fieldGoneFacts]
    | some h =>
      simp [hat] at hren
      simp [source_fieldPairFacts, bne]; exact Or.inl (Ne.symm hren)

/-- `==` on enum labels is decided equality. -/
@[simp]
theorem elabel_beq_eq (x y : ELabel) : (x == y) = decide (x = y) := rfl

/-- A value gone from a surviving `E` fails the wire gate exactly when its number is vacated and unreserved, or holds a label that had another number. -/
theorem wire_valueGoneFacts (o g : Enum) (v : EVal) :
    fails .wire (valueGoneFacts o g v) =
      ((!enumReserved g.resNums v.num && !g.values.any (·.num == v.num)) ||
       g.values.any (fun w => w.num == v.num &&
         o.values.any (fun p => p.label == w.label && p.num != w.num))) := by
  unfold valueGoneFacts
  simp only [fails_append, fails_ite]
  split <;> split <;> simp_all [neutral, classify, breakingGate, inScope]
  all_goals assumption

/-- A value of the new `E` fails the wire gate exactly when the old `E` reserved its number. -/
theorem wire_valueReservedFacts (o : Enum) (w : EVal) :
    fails .wire (valueReservedFacts o w) = enumReserved o.resNums w.num := by
  unfold valueReservedFacts
  simp only
  cases h : enumReserved o.resNums w.num <;> simp [breakingGate, inScope] <;>
    split <;> simp [breakingGate, inScope]

/-- A valid `E` has distinct labels. -/
theorem enum_valid_labels {e : Enum} (he : e.valid = true) :
    nodup (e.values.map (·.label)) = true := by
  simp only [Enum.valid, Bool.and_eq_true] at he
  exact he.1.1.1.1.2

/-- In a valid `E`, a label's number is its value's. -/
theorem enum_numOf_eq {e : Enum} (he : e.valid = true) {w : EVal} (hw : w ∈ e.values) :
    e.numOf w.label = some w.num := by
  unfold Enum.numOf
  cases hf : e.values.find? (·.label == w.label) with
  | none =>
    rw [List.find?_eq_none] at hf
    exact absurd (hf w hw) (by simp)
  | some x =>
    have hx := List.mem_of_find?_eq_some hf
    have hl := List.find?_some hf
    simp at hl
    have := nodup_map_inj (enum_valid_labels he) hx hw hl
    simp [this]

/-- **The enum wire gate is sound**: `--breaking wire` fails on `E` only
for a wire hazard. -/
theorem enum_wire_gate_sound (o g : Enum) (hg : g.valid = true)
    (h : fails .wire (enumPairFacts o g) = true) : wireHazardEnum o g = true := by
  unfold enumPairFacts at h
  simp only [fails_append, fails_flatMap, Bool.or_eq_true, List.any_eq_true] at h
  unfold wireHazardEnum labelMoved
  simp only [Bool.or_eq_true, List.any_eq_true]
  rcases h with ((halias | ⟨v, hv, hgone⟩) | ⟨w, hw, hnew⟩)
  · exfalso; revert halias; split <;> simp [neutral, classify, breakingGate, inScope]
  · split at hgone
    · simp at hgone
    rw [wire_valueGoneFacts] at hgone
    simp only [Bool.or_eq_true, Bool.and_eq_true, Bool.not_eq_true', List.any_eq_true] at hgone
    rcases hgone with ⟨hnr, hfree⟩ | ⟨w, hw, hwn, p, hp, hpl⟩
    · refine Or.inl (Or.inl ⟨v, hv, ?_⟩)
      simp only [Bool.and_eq_true, Bool.not_eq_true']
      exact ⟨hfree, hnr⟩
    · refine Or.inr ⟨p, hp, ?_⟩
      simp at hpl hwn
      rw [hpl.1, enum_numOf_eq hg hw]
      simp only [bne_iff_ne, ne_eq, Bool.and_eq_true, List.any_eq_true,
        beq_iff_eq]
      exact ⟨Ne.symm hpl.2, v, hv, hwn.symm⟩
  · rcases hnew with hadd | hres
    · exfalso; revert hadd; split <;> simp [neutral, classify, breakingGate, inScope]
    · rw [wire_valueReservedFacts] at hres
      exact Or.inl (Or.inr ⟨w, hw, hres⟩)

/-- A vacated, unreserved enum number fails `--breaking wire`
(`ENUM_VALUE_NO_DELETE_UNLESS_NUMBER_RESERVED`). -/
theorem enum_vacated_fails_wire (o g : Enum) (v : EVal) (hv : v ∈ o.values)
    (hfree : g.values.any (·.num == v.num) = false)
    (hr : enumReserved g.resNums v.num = false) : fails .wire (enumPairFacts o g) = true := by
  have hvg : v ∉ g.values := fun hm => by
    have := List.any_eq_false.mp hfree v hm
    simp at this
  unfold enumPairFacts
  simp only [fails_append, fails_flatMap, Bool.or_eq_true, List.any_eq_true]
  refine Or.inl (Or.inr ⟨v, hv, ?_⟩)
  simp [hvg, wire_valueGoneFacts, hr, hfree]

/-- A value at a number the old `E` reserved fails `--breaking wire`
(`RESERVED_IDENTITY_REUSED`). -/
theorem enum_reserved_reuse_fails_wire (o g : Enum) (w : EVal) (hw : w ∈ g.values)
    (hr : enumReserved o.resNums w.num = true) : fails .wire (enumPairFacts o g) = true := by
  unfold enumPairFacts
  simp only [fails_append, fails_flatMap, Bool.or_eq_true, List.any_eq_true]
  refine Or.inr ⟨w, hw, ?_⟩
  simp [wire_valueReservedFacts, hr]

/-- Finding (#190): a label moved onto a number another label keeps, with
its old number reserved, passes `--breaking wire` although it is a wire
hazard: `A` was sent as `1` and is now sent as `2`, which an old peer reads
as `B`. -/
def movedOld : Enum := { values := [⟨.A, 1⟩, ⟨.B, 2⟩], resNums := [], resNames := [] }
/-- The new side of `movedOld`. -/
def movedNew : Enum := { values := [⟨.B, 2⟩, ⟨.A, 2⟩], resNums := [(1, 1)], resNames := [] }

/-- Finding (#190), first repro: valid documents, a wire hazard, and no
fact with a wire impact. -/
theorem enum_label_moved_wire_gap :
    movedOld.valid = true ∧ movedNew.valid = true ∧ wireHazardEnum movedOld movedNew = true ∧
      fails .wire (enumPairFacts movedOld movedNew) = false := by
  decide

/-! ## The whole fragment -/

/-- The second shape of the gap, with aliases and no reservation: `B` moves
from `1` (which `A` keeps) to `2` (which `C` keeps). -/
def aliasOld : Enum :=
  { values := [⟨.A, 1⟩, ⟨.B, 1⟩, ⟨.C, 2⟩], resNums := [], resNames := [] }
/-- The new side of `aliasOld`. -/
def aliasNew : Enum :=
  { values := [⟨.A, 1⟩, ⟨.B, 2⟩, ⟨.C, 2⟩], resNums := [], resNames := [] }

/-- Finding (#190), second repro: the same with aliases and nothing
reserved. -/
theorem enum_alias_moved_wire_gap :
    aliasOld.valid = true ∧ aliasNew.valid = true ∧ wireHazardEnum aliasOld aliasNew = true ∧
      fails .wire (enumPairFacts aliasOld aliasNew) = false := by
  decide

/-- Changes a surviving `M` may make without a wire impact: renames and
deprecations at a number, removals of optional fields whose number is then
reserved, and optional fields added at numbers the old `M` did not reserve. -/
theorem no_wire_of_kept_shapes (o g : Msg) (ho : o.valid = true)
    (hold : ∀ f ∈ o.fields, match g.at f.num with
      | some h => f.shape = h.shape
      | none => f.label ≠ .required ∧ msgReserved g.resNums f.num = true)
    (hnew : ∀ h ∈ g.fields, (o.at h.num).isNone = true →
      h.label ≠ .required ∧ msgReserved o.resNums h.num = false) :
    fails .wire (msgPairFacts o g) = false := by
  rw [wire_gate_iff_hazard o g ho]
  unfold wireHazardMsg
  simp only [Bool.or_eq_false_iff, List.any_eq_false]
  constructor
  · intro f hf
    have := hold f hf
    revert this
    cases g.at f.num <;> simp_all
  · intro h hh
    cases hat : (o.at h.num).isNone
    · simp
    · have := hnew h hh hat
      simp_all

/-- Pins of the docs' rule-table rows the fragment reaches, on concrete
documents: a same-number rename and a reserved optional removal are
source-only, a required removal is wire-breaking even when its number is
reserved, a required field in a new message is Non-breaking, and an alias
removal is source-only. -/
theorem doc_rows :
    -- FIELD_SAME_NAME: a same-number rename is source/JSON only.
    (let o := { fields := [⟨1, .a, .int32, .optional, false, false⟩], resNums := [], resNames := [] : Msg }
     let g := { fields := [⟨1, .b, .int32, .optional, false, false⟩], resNums := [], resNames := [] : Msg }
     fails .wire (msgPairFacts o g) = false ∧ fails .source (msgPairFacts o g) = true) ∧
    -- An optional field removed with its number reserved: source only.
    (let o := { fields := [⟨1, .a, .int32, .optional, false, false⟩], resNums := [], resNames := [] : Msg }
     let g := { fields := [], resNums := [(1, 2)], resNames := [.a] : Msg }
     fails .wire (msgPairFacts o g) = false ∧ fails .source (msgPairFacts o g) = true) ∧
    -- FIELD_REQUIRED_NO_DELETE: wire even when the number is reserved.
    (let o := { fields := [⟨1, .a, .int32, .required, false, false⟩], resNums := [], resNames := [] : Msg }
     let g := { fields := [], resNums := [(1, 2)], resNames := [.a] : Msg }
     fails .wire (msgPairFacts o g) = true) ∧
    -- A required field in a new message has no prior readers.
    fails .all (msgFacts none
      (some { fields := [⟨1, .a, .int32, .required, false, false⟩], resNums := [], resNames := [] })) = false ∧
    -- An alias removed: source only, the number keeps its other label.
    (let o := { values := [⟨.A, 0⟩, ⟨.B, 0⟩], resNums := [], resNames := [] : Enum }
     let g := { values := [⟨.A, 0⟩], resNums := [], resNames := [] : Enum }
     fails .wire (enumPairFacts o g) = false ∧ fails .source (enumPairFacts o g) = true) := by
  decide

/-- The enum reserved-range test is inclusive at both ends, so the bounds of
the 32-bit range need no overflow care: `diff` widens before adding one. -/
theorem enumReserved_inclusive (rs : List (Int × Int)) (n : Int) :
    enumReserved rs n = rs.any fun r => decide (r.1 ≤ n) && decide (n ≤ r.2) := by
  unfold enumReserved
  congr 1; funext r
  simp [Int.lt_add_one_iff]

/-- **Finding (#190): the only wire hazards `--breaking wire` misses are
moved enum labels.** Whenever the fragment has a wire hazard and the wire gate
passes, both sides declare `E`, a label moved to another number, and no
number was vacated without reservation or reused from a reservation. -/
theorem wire_gap_is_label_move (om gm : Option Msg) (oe ge : Option Enum)
    (hom : om.all Msg.valid = true) (hge : ge.all Enum.valid = true)
    (h : wireHazard om gm oe ge = true) (hn : fails .wire (diff om gm oe ge) = false) :
    ∃ o g, oe = some o ∧ ge = some g ∧ labelMoved o g = true ∧
      o.values.all (fun v => g.values.any (·.num == v.num) || enumReserved g.resNums v.num) ∧
      g.values.all (fun w => !enumReserved o.resNums w.num) := by
  unfold diff at hn
  rw [fails_append, Bool.or_eq_false_iff] at hn
  unfold wireHazard at h
  rw [Bool.or_eq_true] at h
  rcases h with hm | he
  · exfalso
    match om, gm, hom, hm, hn with
    | some o, some g, hom, hm, hn =>
      simp only [Option.all_some] at hom
      simp only [msgFacts, wire_gate_iff_hazard o g hom] at hn
      simp [hm] at hn
  · match oe, ge, hge, he, hn with
    | some o, some g, hge, he, hn =>
      simp only [Option.all_some] at hge
      simp only [enumFacts] at hn
      refine ⟨o, g, rfl, rfl, ?_⟩
      unfold wireHazardEnum at he
      simp only [Bool.or_eq_true, List.any_eq_true] at he
      have hne : fails .wire (enumPairFacts o g) = false := hn.2
      have hvac : o.values.all (fun v => g.values.any (·.num == v.num) ||
          enumReserved g.resNums v.num) = true := by
        rw [List.all_eq_true]; intro v hv
        cases hf : g.values.any (·.num == v.num)
        · cases hr : enumReserved g.resNums v.num
          · have := enum_vacated_fails_wire o g v hv hf hr
            rw [hne] at this; contradiction
          · rfl
        · rfl
      have hres : g.values.all (fun w => !enumReserved o.resNums w.num) = true := by
        rw [List.all_eq_true]; intro w hw
        cases hr : enumReserved o.resNums w.num
        · rfl
        · have := enum_reserved_reuse_fails_wire o g w hw hr
          rw [hne] at this; contradiction
      rcases he with (⟨v, hv, hz⟩ | ⟨w, hw, hz⟩) | hz
      · exfalso
        have := List.all_eq_true.mp hvac v hv
        simp only [Bool.and_eq_true, Bool.not_eq_true'] at hz
        rw [hz.1, hz.2] at this; contradiction
      · exfalso
        have := List.all_eq_true.mp hres w hw
        rw [hz] at this; contradiction
      · exact ⟨hz, hvac, hres⟩

/-- **Every source hazard fails `--breaking source`.** -/
theorem source_hazard_fails_source (om gm : Option Msg) (oe ge : Option Enum)
    (hgm : gm.all Msg.valid = true) (h : sourceHazard om gm oe ge = true) :
    fails .source (diff om gm oe ge) = true := by
  unfold sourceHazard at h
  unfold diff
  rw [fails_append, Bool.or_eq_true]
  rw [Bool.or_eq_true] at h
  rcases h with hm | he
  · left
    match om, gm, hgm, hm with
    | some o, none, _, _ =>
      simp [msgFacts, neutral, classify, breakingGate, inScope]
    | some o, some g, hgm, hm =>
      simp only [Option.all_some] at hgm
      exact source_hazard_msg o g hgm hm
  · right
    match oe, ge, he with
    | some o, none, _ =>
      simp [enumFacts, neutral, classify, breakingGate, inScope]
    | some o, some g, he =>
      simp only [sourceHazardEnum, Bool.or_eq_true, List.any_eq_true] at he
      unfold enumFacts enumPairFacts
      simp only [fails_append, fails_flatMap, Bool.or_eq_true, List.any_eq_true]
      rcases he with ⟨v, hv, hgone⟩ | ⟨w, hw, hname⟩
      · refine Or.inl (Or.inr ⟨v, hv, ?_⟩)
        have hvg : v ∉ g.values := fun hm => by
          simp only [Bool.not_eq_true', List.any_eq_false] at hgone
          exact hgone v hm (by simp)
        simp [hvg, valueGoneFacts, fails_append, neutral, classify, breakingGate, inScope]
      · refine Or.inr ⟨w, hw, Or.inr ?_⟩
        unfold valueReservedFacts
        simp only [hname, Bool.or_true, ite_true]
        split <;> simp [breakingGate, inScope]

/-- A fact that is not Breaking, or names a wire or source impact. -/
def Fact.ok (f : Fact) : Bool := f.sev != .breaking || gated f.impacts

/-- Every fact from the neutral comparison is gated. -/
@[simp]
theorem neutral_ok (s : Subject) (r : Bool) (a : TypeAttr) (k : ChangeKind) (p : Ptr) :
    (neutral s r a k p).ok = true := by
  have := classify_breaking_gated r k p
  unfold Fact.ok neutral
  cases h : (classify r k p).1 <;> simp_all

/-- `all` over a conditional list. -/
theorem all_ite (c : Prop) [Decidable c] (l : List Fact) :
    (if c then l else []).all Fact.ok = (if c then l.all Fact.ok else true) := by
  split <;> rfl

/-- The facts of a kept field are gated. -/
theorem fieldPairFacts_ok (f h : Field) : (fieldPairFacts f h).all Fact.ok = true := by
  simp only [fieldPairFacts, List.all_append, all_ite, List.all_cons, List.all_nil, neutral_ok,
    Bool.and_self, ite_self]

/-- The facts of a gone field are gated. -/
theorem fieldGoneFacts_ok (g : Msg) (f : Field) : (fieldGoneFacts g f).all Fact.ok = true := by
  unfold fieldGoneFacts
  simp only [List.all_append, all_ite, List.all_cons, List.all_nil, neutral_ok]
  split <;> split <;> simp [Fact.ok, gated, inScope]

/-- Reserved-identity facts of a field are gated. -/
theorem fieldReservedFacts_ok (o : Msg) (h : Field) :
    (fieldReservedFacts o h).all Fact.ok = true := by
  unfold fieldReservedFacts
  simp only
  split
  · cases msgReserved o.resNums h.num <;> simp [Fact.ok, gated, inScope]
  · rfl

/-- The facts of a gone value are gated. -/
theorem valueGoneFacts_ok (o g : Enum) (v : EVal) : (valueGoneFacts o g v).all Fact.ok = true := by
  unfold valueGoneFacts
  simp only [List.all_append, all_ite, List.all_cons, List.all_nil, neutral_ok]
  split <;> split <;> simp [Fact.ok, gated, inScope]

/-- Reserved-identity facts of a value are gated. -/
theorem valueReservedFacts_ok (o : Enum) (w : EVal) :
    (valueReservedFacts o w).all Fact.ok = true := by
  unfold valueReservedFacts
  simp only
  split
  · cases enumReserved o.resNums w.num <;> simp [Fact.ok, gated, inScope]
  · rfl

/-- Every fact `diff` reports on the fragment is Non-breaking, Review, or
names a wire or source impact. -/
theorem diff_ok (om gm : Option Msg) (oe ge : Option Enum) :
    (diff om gm oe ge).all Fact.ok = true := by
  have hm : (msgFacts om gm).all Fact.ok = true := by
    cases om <;> cases gm
    · rfl
    · simp only [msgFacts, List.all_append, all_ite, List.all_cons, List.all_nil, neutral_ok,
        Bool.and_self, ite_self, Bool.true_and, List.all_map]
      exact List.all_eq_true.mpr fun _ _ => by simp [Fact.ok]
    · simp only [msgFacts, List.all_append, all_ite, List.all_cons, List.all_nil, neutral_ok,
        Bool.and_self, ite_self, Bool.true_and, List.all_map]
      exact List.all_eq_true.mpr fun _ _ => by simp
    · rename_i o g
      simp only [msgFacts, msgPairFacts, List.all_append, all_ite, List.all_cons, List.all_nil,
        neutral_ok, Bool.and_self, ite_self, Bool.true_and, List.all_flatMap, Bool.and_eq_true]
      refine ⟨List.all_eq_true.mpr fun f _ => ?_, List.all_eq_true.mpr fun h _ => ?_⟩
      · cases g.at f.num <;> simp [fieldPairFacts_ok, fieldGoneFacts_ok]
      · cases o.at h.num <;> simp [fieldReservedFacts_ok]
  have he : (enumFacts oe ge).all Fact.ok = true := by
    cases oe <;> cases ge
    · rfl
    · simp only [enumFacts, List.all_append, List.all_cons, List.all_nil, neutral_ok,
        Bool.and_self, Bool.true_and, List.all_map]
      exact List.all_eq_true.mpr fun _ _ => by simp
    · simp only [enumFacts, List.all_append, List.all_cons, List.all_nil, neutral_ok,
        Bool.and_self, Bool.true_and, List.all_map]
      exact List.all_eq_true.mpr fun _ _ => by simp
    · rename_i o g
      simp only [enumFacts, enumPairFacts, List.all_append, all_ite, List.all_cons, List.all_nil,
        neutral_ok, Bool.and_self, ite_self, Bool.true_and, List.all_flatMap, Bool.and_eq_true]
      refine ⟨List.all_eq_true.mpr fun v _ => ?_, List.all_eq_true.mpr fun w _ => ?_⟩
      · split <;> simp [valueGoneFacts_ok]
      · simp [valueReservedFacts_ok]
  simp [diff, List.all_append, hm, he]

/-- `--breaking all` fails exactly when `--breaking wire` or `--breaking
source` does: every Breaking fact names a wire or source impact. -/
theorem fails_all_iff (om gm : Option Msg) (oe ge : Option Enum) :
    fails .all (diff om gm oe ge) =
      (fails .wire (diff om gm oe ge) || fails .source (diff om gm oe ge)) := by
  have h := diff_ok om gm oe ge
  generalize diff om gm oe ge = fs at h ⊢
  induction fs with
  | nil => rfl
  | cons x xs ih =>
    simp only [List.all_cons, Bool.and_eq_true] at h
    simp only [fails_cons, ih h.2]
    have hx : x.sev = .breaking → gated x.impacts = true := by
      intro hb; have := h.1; simp [Fact.ok, hb] at this; exact this
    rw [gate_all_of_gated x.sev x.impacts hx]
    cases breakingGate .wire x.sev x.impacts <;> cases breakingGate .source x.sev x.impacts <;>
      cases fails .wire xs <;> cases fails .source xs <;> rfl

end Vimanam.Protobuf
