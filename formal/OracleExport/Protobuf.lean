/-
# Oracle vectors for `Protobuf.lean` and `ProtobufCompat.lean`

Replayed by `src/contract/protobuf/formal_oracle.rs`.

* The rule table, exhaustively over its finite inputs: `attributeKind` on
  every pointer string below, `structuralKind` on every structural field,
  `classify` on every (neutral kind, pointer, `cardinality` attribute),
  `nativeKind` on every (neutral kind, pointer), with every removed type's
  `kind` attribute for `TypeRemoved`, and `breakingGate` on every (scope, severity, impact list).
* The composite diff: pairs of fragment documents (see `Protobuf.lean`), the
  fixed ones below and then generated ones (independent, or the old side
  mutated one to three times), each with the model's set of facts, the gate
  under each scope, the specification's hazards, and the known gap the
  failing guarantee is filed as, if any.
-/
import VimanamProofs.ProtobufCompat
import OracleExport.Gen

namespace Vimanam.Oracle.Protobuf

open Lean Vimanam.Protobuf Vimanam.Oracle

/-! ## Names, as the Rust spells them -/

def sevName : Severity → String
  | .breaking => "breaking"
  | .nonBreaking => "non_breaking"
  | .review => "review"

def impactName : Impact → String
  | .wire => "wire"
  | .source => "source"
  | .wireJson => "wire_json"

/-- `contract::ChangeKind`'s serde name. -/
def kindName : ChangeKind → String
  | .serviceAdded => "service_added"
  | .serviceRemoved => "service_removed"
  | .operationAdded => "operation_added"
  | .operationRemoved => "operation_removed"
  | .operationRenamed => "operation_renamed"
  | .operationMoved => "operation_moved"
  | .typeAdded => "type_added"
  | .typeRemoved => "type_removed"
  | .typeKindChanged => "type_kind_changed"
  | .argumentAdded => "argument_added"
  | .argumentRemoved => "argument_removed"
  | .argumentRequiredChanged => "argument_required_changed"
  | .argumentTypeChanged => "argument_type_changed"
  | .fieldAdded => "field_added"
  | .fieldRemoved => "field_removed"
  | .fieldTypeChanged => "field_type_changed"
  | .fieldRequiredChanged => "field_required_changed"
  | .nullabilityChanged => "nullability_changed"
  | .enumValueAdded => "enum_value_added"
  | .enumValueRemoved => "enum_value_removed"
  | .deprecatedChanged => "deprecated_changed"
  | .defaultChanged => "default_changed"
  | .paginationChanged => "pagination_changed"
  | .wireIdentityChanged => "wire_identity_changed"
  | .resultAdded => "result_added"
  | .resultRemoved => "result_removed"
  | .constraintChanged => "constraint_changed"

def allKinds : List ChangeKind :=
  [.serviceAdded, .serviceRemoved, .operationAdded, .operationRemoved, .operationRenamed,
   .operationMoved, .typeAdded, .typeRemoved, .typeKindChanged, .argumentAdded,
   .argumentRemoved, .argumentRequiredChanged, .argumentTypeChanged, .fieldAdded,
   .fieldRemoved, .fieldTypeChanged, .fieldRequiredChanged, .nullabilityChanged,
   .enumValueAdded, .enumValueRemoved, .deprecatedChanged, .defaultChanged,
   .paginationChanged, .wireIdentityChanged, .resultAdded, .resultRemoved, .constraintChanged]

/-- Pointer strings: every exact pointer the adapter compares with, the
prefixes alone and extended, lookalikes, and ordinary attribute pointers. -/
def ptrStrings : List String :=
  ["", "/", "/deprecated", "/type", "/default", "/default/kind", "/default/value", "/defaults",
   "/json_name", "/oneof", "/group", "/client_streaming", "/server_streaming",
   "/cardinality", "/presence", "/input", "/output", "/syntax",
   "/file_options", "/file_options/", "/file_options/java_package", "/options/deprecated",
   "/options/allowAlias", "/kind", "/package", "/extension", "/cardinality/x", "/type/x",
   "/oneofs", "deprecated", "/Deprecated"]

def structurals : List (String × Structural) :=
  [("name", .name), ("parent", .parent), ("services", .services),
   ("references", .references), ("native", .native)]

/-- A `cardinality` attribute: `"required"`, another label, or absent. -/
def cardinalities : List (Json × Bool) :=
  [(toJson "required", true), (toJson "optional", false), (toJson "repeated", false),
   (Json.null, false)]

/-- A removed type's `kind` attribute. -/
def typeAttrs : List (Json × TypeAttr) :=
  [(toJson "message", .message), (toJson "enum", .enum), (toJson "oneof", .oneof),
   (toJson "file", .other), (toJson "package", .other), (Json.null, .other)]

def scopeName : Scope → String
  | .wire => "wire"
  | .source => "source"
  | .all => "all"

def impactLists : List (List Impact) :=
  [[], [.wire], [.source], [.wireJson], [.wire, .source], [.source, .wireJson],
   [.wire, .wireJson], [.wire, .source, .wireJson]]

def impactsJson (i : List Impact) : Json := toJson (i.map impactName).toArray

def ruleTable : List Fn := [
  { lean := "Protobuf.Ptr.ofString, Protobuf.attributeKind",
    rust := "ProtobufAdapter::attribute_kind",
    cases := ptrStrings.map fun p =>
      case (toJson p) (toJson (kindName (attributeKind (Ptr.ofString p)))) },
  { lean := "Protobuf.structuralKind", rust := "ProtobufAdapter::structural_kind",
    cases := structurals.map fun (s, t) =>
      case (toJson s) (match structuralKind t with
        | some k => toJson (kindName k)
        | none => Json.null) },
  { lean := "Protobuf.classify", rust := "ProtobufAdapter::classify",
    cases := allKinds.flatMap fun k => ptrStrings.flatMap fun p =>
      cardinalities.map fun (cj, r) =>
        let (s, i) := classify r k (Ptr.ofString p)
        case (toJson #[toJson (kindName k), toJson p, cj])
          (toJson #[toJson (sevName s), impactsJson i]) },
  { lean := "Protobuf.nativeKind", rust := "protobuf::native_kind",
    -- `diff` reads the removed type's `kind` only for `TypeRemoved`.
    cases := allKinds.flatMap fun k => ptrStrings.flatMap fun p =>
      (if k = .typeRemoved then typeAttrs else [(Json.null, .other)]).map fun (aj, a) =>
        case (toJson #[toJson (kindName k), toJson p, aj])
          (toJson (nativeKind k (Ptr.ofString p) a)) },
  { lean := "Protobuf.breakingGate", rust := "protobuf::breaking_gate",
    cases := [Scope.wire, .source, .all].flatMap fun sc =>
      [Severity.breaking, .nonBreaking, .review].flatMap fun s =>
        impactLists.map fun i =>
          case (toJson #[toJson (scopeName sc), toJson (sevName s), impactsJson i])
            (toJson (breakingGate sc s i)) }
]

/-! ## Fragment documents -/

def fnameName : FName → String
  | .a => "a" | .b => "b" | .c => "c" | .d => "d"

def labelName : ELabel → String
  | .A => "A" | .B => "B" | .C => "C" | .D => "D"

def tyName : Ty → String
  | .int32 => "int32" | .string => "string"

def flabelName : Label → String
  | .optional => "optional" | .required => "required" | .repeated => "repeated"

def msgJson : Option Msg → Json
  | none => Json.null
  | some m => Json.mkObj [
      ("fields", toJson (m.fields.map fun f => toJson #[toJson f.num, toJson (fnameName f.name),
        toJson (tyName f.ty), toJson (flabelName f.label), toJson f.inOneof,
        toJson f.deprecated]).toArray),
      ("reserved_ranges", toJson (m.resNums.map fun r => toJson #[r.1, r.2]).toArray),
      ("reserved_names", toJson (m.resNames.map fnameName).toArray)]

def enumJson : Option Enum → Json
  | none => Json.null
  | some e => Json.mkObj [
      ("values", toJson (e.values.map fun v => toJson #[toJson (labelName v.label),
        toJson v.num]).toArray),
      ("reserved_ranges", toJson (e.resNums.map fun r => toJson #[r.1, r.2]).toArray),
      ("reserved_names", toJson (e.resNames.map labelName).toArray)]

def subjectName : Subject → String
  | .msg => "M"
  | .oneof => "M.o"
  | .field n k => s!"M.{fnameName n}#{k}"
  | .enum => "E"
  | .value l k => s!"{labelName l}#{k}"

def factJson (f : Fact) : Json :=
  toJson #[toJson (subjectName f.subject), toJson f.native, toJson (sevName f.sev),
    impactsJson f.impacts]

/-- Distinct facts, rendered, in sorted order. -/
def factSet (fs : List Fact) : List String :=
  let rendered := (fs.map fun f => (factJson f).compress).eraseDups
  rendered.mergeSort (fun a b => decide (a ≤ b))

/-- One fragment pair. -/
structure Pair where
  om : Option Msg
  gm : Option Msg
  oe : Option Enum
  ge : Option Enum

/-- The issue a moved enum label the wire gate misses is filed as
(`ProtobufCompat.enum_label_moved_wire_gap`). -/
def labelMovedIssue : Nat := 190

/-- The known gap a case falls in: the specification's wire hazard without
the wire gate failing, which `ProtobufCompat.wire_gap_is_label_move` shows
happens only for an enum label moved to another number. -/
def knownGap (p : Pair) : Option Nat :=
  if wireHazard p.om p.gm p.oe p.ge && !fails .wire (diff p.om p.gm p.oe p.ge) then
    some labelMovedIssue
  else none

def pairCase (p : Pair) : Json :=
  let fs := diff p.om p.gm p.oe p.ge
  case
    (Json.mkObj [
      ("old", Json.mkObj [("message", msgJson p.om), ("enum", enumJson p.oe)]),
      ("new", Json.mkObj [("message", msgJson p.gm), ("enum", enumJson p.ge)])])
    (Json.mkObj [
      ("facts", toJson ((factSet fs).map fun s => (Json.parse s).toOption.getD Json.null).toArray),
      ("fails", Json.mkObj [("wire", toJson (fails .wire fs)), ("source", toJson (fails .source fs)),
        ("all", toJson (fails .all fs))]),
      ("hazard", Json.mkObj [("wire", toJson (wireHazard p.om p.gm p.oe p.ge)),
        ("source", toJson (sourceHazard p.om p.gm p.oe p.ge))]),
      ("known_gap", match knownGap p with
        | some n => toJson n
        | none => Json.null)])

/-! ## Generated documents -/

instance : Inhabited FName := ⟨.a⟩
instance : Inhabited ELabel := ⟨.A⟩
instance : Inhabited Ty := ⟨.int32⟩
instance : Inhabited Label := ⟨.optional⟩

def coin : Gen Bool := do return (← below 2) == 0

def fieldNums : List Nat := [1, 2, 3, 536870911]

def genField (num : Nat) (name : FName) : Gen Field := do
  let label ← pick [Label.optional, .optional, .required, .repeated]
  let inOneof ← if label == .optional then coin else pure false
  return { num, name, ty := ← pick [Ty.int32, .string], label, inOneof,
           deprecated := (← below 4) == 0 }

/-- A shuffle (Fisher–Yates by repeated picks). -/
def shuffle {α : Type} [Inhabited α] (xs : List α) : Gen (List α) := do
  let mut rest := xs
  let mut out := #[]
  for _ in [0:xs.length] do
    let i ← below rest.length
    out := out.push (rest.getD i default)
    rest := rest.eraseIdx i
  return out.toList

/-- Free numbers a message may reserve, as candidate ranges. -/
def msgRangeCandidates : List (Nat × Nat) :=
  [(1, 2), (2, 3), (3, 4), (4, 5), (536870911, 536870912), (2, 4), (4, 100), (1, 4)]

def genMsgReserved (fields : List Field) : Gen (List (Nat × Nat) × List FName) := do
  let mut ranges : List (Nat × Nat) := []
  for r in msgRangeCandidates do
    if (← below 4) == 0 then
      let candidate := { fields := fields, resNums := r :: ranges, resNames := [] : Msg }
      if candidate.valid then ranges := r :: ranges
  let mut names : List FName := []
  for n in [FName.a, .b, .c, .d] do
    if (← below 3) == 0 && !(fields.any (·.name == n)) then names := names ++ [n]
  return (ranges.reverse, names)

def genMsg : Gen Msg := do
  let names ← shuffle [FName.a, .b, .c]
  let nums ← shuffle fieldNums
  let n ← below 4
  let mut fields := #[]
  for i in [0:n] do
    fields := fields.push (← genField (nums.getD i 1) (names.getD i .a))
  let (resNums, resNames) ← genMsgReserved fields.toList
  return { fields := fields.toList, resNums, resNames }

def enumNums : List Int := [-2147483648, -1, 0, 1, 2, 2147483647]

def enumRangeCandidates : List (Int × Int) :=
  [(-2147483648, -2147483648), (-1, -1), (0, 0), (1, 1), (2, 2), (2147483647, 2147483647),
   (3, 2147483647), (-2147483648, -2), (1, 2), (-5, -1)]

def genEnumReserved (values : List EVal) : Gen (List (Int × Int) × List ELabel) := do
  let mut ranges : List (Int × Int) := []
  for r in enumRangeCandidates do
    if (← below 4) == 0 then
      let candidate := { values := values, resNums := r :: ranges, resNames := [] : Enum }
      if candidate.valid then ranges := r :: ranges
  let mut names : List ELabel := []
  for l in [ELabel.A, .B, .C, .D] do
    if (← below 3) == 0 && !(values.any (·.label == l)) then names := names ++ [l]
  return (ranges.reverse, names)

def genEnum : Gen Enum := do
  let labels ← shuffle [ELabel.A, .B, .C]
  let n := 1 + (← below 3)
  let mut values := #[]
  for i in [0:n] do
    values := values.push { label := labels.getD i .A, num := ← pick enumNums : EVal }
  let (resNums, resNames) ← genEnumReserved values.toList
  return { values := values.toList, resNums, resNames }

def defaultField : Field :=
  { num := 1, name := .a, ty := .int32, label := .optional, inOneof := false, deprecated := false }

/-- One edit of a message: rename, retype, relabel, renumber, toggle oneof or
deprecation, remove (reserving or not), add, or change the reservations. -/
def mutateMsg (m : Msg) : Gen Msg := do
  let i ← below (max m.fields.length 1)
  let f := m.fields.getD i defaultField
  let set (g : Field) := m.fields.set i g
  match ← below 10 with
  | 0 => return { m with fields := set { f with name := ← pick [FName.a, .b, .c] } }
  | 1 => return { m with fields := set { f with ty := ← pick [Ty.int32, .string] } }
  | 2 =>
    let label ← pick [Label.optional, .required, .repeated]
    return { m with fields := set { f with label, inOneof := f.inOneof && label == .optional } }
  | 3 => return { m with fields := set { f with num := ← pick fieldNums } }
  | 4 => return { m with fields := set { f with inOneof := !f.inOneof && f.label == .optional } }
  | 5 => return { m with fields := set { f with deprecated := !f.deprecated } }
  | 6 => return { m with fields := m.fields.eraseIdx i }
  | 7 => return { m with fields := m.fields.eraseIdx i,
                         resNums := m.resNums ++ [(f.num, f.num + 1)],
                         resNames := if ← coin then m.resNames ++ [f.name] else m.resNames }
  | 8 => return { m with fields := m.fields ++ [← genField (← pick fieldNums) (← pick [FName.a, .b, .c])] }
  | _ =>
    let (resNums, resNames) ← genMsgReserved m.fields
    return { m with resNums, resNames }

/-- One edit of an enum: move a label, rename it, remove it (reserving or
not), add a label or an alias, or change the reservations. -/
def mutateEnum (e : Enum) : Gen Enum := do
  let i ← below (max e.values.length 1)
  let v := e.values.getD i { label := .A, num := 0 }
  match ← below 7 with
  | 0 => return { e with values := e.values.set i { v with num := ← pick enumNums } }
  | 1 => return { e with values := e.values.set i { v with label := ← pick [ELabel.A, .B, .C] } }
  | 2 => return { e with values := e.values.eraseIdx i }
  | 3 => return { e with values := e.values.eraseIdx i,
                         resNums := e.resNums ++ [(v.num, v.num)],
                         resNames := if ← coin then e.resNames ++ [v.label] else e.resNames }
  | 4 => return { e with values := e.values ++ [{ label := ← pick [ELabel.A, .B, .C],
                                                  num := ← pick enumNums }] }
  | 5 => return { e with values := e.values ++ [{ label := ← pick [ELabel.A, .B, .C],
                                                  num := v.num }] }
  | _ =>
    let (resNums, resNames) ← genEnumReserved e.values
    return { e with resNums, resNames }

/-- Up to `n` edits, keeping only edits that leave a valid document. -/
def edits {α : Type} (valid : α → Bool) (mutate : α → Gen α) (x : α) (n : Nat) : Gen α := do
  let mut cur := x
  for _ in [0:n] do
    let next ← mutate cur
    if valid next then cur := next
  return cur

def genOptMsg : Gen (Option Msg × Option Msg) := do
  match ← below 8 with
  | 0 => return (none, some (← genMsg))
  | 1 => return (some (← genMsg), none)
  | 2 => return (some (← genMsg), some (← genMsg))
  | 3 => return (none, none)
  | _ =>
    let o ← genMsg
    return (some o, some (← edits Msg.valid mutateMsg o (1 + (← below 3))))

def genOptEnum : Gen (Option Enum × Option Enum) := do
  match ← below 8 with
  | 0 => return (none, some (← genEnum))
  | 1 => return (some (← genEnum), none)
  | 2 => return (some (← genEnum), some (← genEnum))
  | 3 => return (none, none)
  | _ =>
    let o ← genEnum
    return (some o, some (← edits Enum.valid mutateEnum o (1 + (← below 3))))

/-! ## Fixed cases -/

def fld (num : Nat) (name : FName) (ty := Ty.int32) (label := Label.optional)
    (inOneof := false) (deprecated := false) : Field :=
  { num, name, ty, label, inOneof, deprecated }

def msg (fields : List Field) (resNums : List (Nat × Nat) := []) (resNames : List FName := []) :
    Msg := { fields, resNums, resNames }

def enm (values : List (ELabel × Int)) (resNums : List (Int × Int) := [])
    (resNames : List ELabel := []) : Enum :=
  { values := values.map fun (l, n) => { label := l, num := n }, resNums, resNames }

def onlyMsg (o g : Msg) : Pair := { om := some o, gm := some g, oe := none, ge := none }
def onlyEnum (o g : Enum) : Pair := { om := none, gm := none, oe := some o, ge := some g }

/-- One case per row of the docs' rule table that the fragment reaches, the
known-gap repros, and the reserved-range bounds. -/
def fixedPairs : List Pair := [
  -- FIELD_SAME_TYPE
  onlyMsg (msg [fld 1 .a]) (msg [fld 1 .a (ty := .string)]),
  -- FIELD_REQUIRED_NO_DELETE, reserved or not
  onlyMsg (msg [fld 1 .a (label := .required)]) (msg [] [(1, 2)] [.a]),
  onlyMsg (msg [fld 1 .a (label := .required)]) (msg []),
  -- FIELD_NO_DELETE_UNLESS_NUMBER_RESERVED, and a reserved removal
  onlyMsg (msg [fld 1 .a, fld 2 .b]) (msg [fld 1 .a]),
  onlyMsg (msg [fld 1 .a, fld 2 .b]) (msg [fld 1 .a] [(2, 3)] [.b]),
  -- RESERVED_IDENTITY_REUSED, by number and by name
  onlyMsg (msg [fld 1 .a] [(2, 3)]) (msg [fld 1 .a, fld 2 .b]),
  onlyMsg (msg [fld 1 .a] [] [.b]) (msg [fld 1 .a, fld 3 .b]),
  onlyMsg (msg [fld 1 .a] [(536870911, 536870912)]) (msg [fld 1 .a, fld 536870911 .b]),
  -- FIELD_SAME_ONEOF, FIELD_SAME_NAME / FIELD_SAME_JSON_NAME, a swap
  onlyMsg (msg [fld 1 .a]) (msg [fld 1 .a (inOneof := true)]),
  onlyMsg (msg [fld 1 .a]) (msg [fld 1 .b]),
  onlyMsg (msg [fld 1 .a, fld 2 .b]) (msg [fld 1 .b, fld 2 .a]),
  -- FIELD_SAME_CARDINALITY, presence and defaults
  onlyMsg (msg [fld 1 .a]) (msg [fld 1 .a (label := .repeated)]),
  -- required added to an existing message, and with a new one
  onlyMsg (msg [fld 1 .a]) (msg [fld 1 .a, fld 2 .b (label := .required)]),
  { om := none, gm := some (msg [fld 1 .a (label := .required)]), oe := none, ge := none },
  -- ENUM_VALUE_NO_DELETE_UNLESS_NUMBER_RESERVED, reserved, alias removal
  onlyEnum (enm [(.A, 0), (.B, 1)]) (enm [(.A, 0)]),
  onlyEnum (enm [(.A, 0), (.B, 1)]) (enm [(.A, 0)] [(1, 1)]),
  onlyEnum (enm [(.A, 0), (.B, 0)]) (enm [(.A, 0)]),
  -- ENUM_VALUE_NUMBER_REUSED: a swap, and a relabel onto a freed number
  onlyEnum (enm [(.A, 1), (.B, 2)]) (enm [(.A, 2), (.B, 1)]),
  onlyEnum (enm [(.A, 1), (.B, 2)]) (enm [(.A, 2)]),
  -- reserved enum numbers at the 32-bit bounds (the range end is inclusive)
  onlyEnum (enm [(.A, 0)] [(2147483647, 2147483647)]) (enm [(.A, 0), (.B, 2147483647)]),
  onlyEnum (enm [(.A, 0)] [(-2147483648, -2)]) (enm [(.A, 0), (.B, -2147483648)]),
  onlyEnum (enm [(.A, 0), (.B, 2147483647)]) (enm [(.A, 0)] [(3, 2147483647)]),
  -- Known gap: a label moved onto a number another label keeps
  onlyEnum (enm [(.A, 1), (.B, 2)]) (enm [(.B, 2), (.A, 2)] [(1, 1)]),
  onlyEnum (enm [(.A, 1), (.B, 1), (.C, 2)]) (enm [(.A, 1), (.B, 2), (.C, 2)]),
  -- MESSAGE_NO_DELETE / ONEOF_NO_DELETE / ENUM_NO_DELETE
  { om := some (msg [fld 1 .a (inOneof := true)]), gm := none,
    oe := some (enm [(.A, 0)]), ge := none }
]

def seed : UInt64 := 0x9B0F0B0F

def vectors : Vectors :=
  run seed do
    let mut pairs := #[]
    for _ in [0:600] do
      let (om, gm) ← genOptMsg
      let (oe, ge) ← genOptEnum
      pairs := pairs.push { om, gm, oe, ge : Pair }
    for p in fixedPairs ++ pairs.toList do
      if !(p.om.all Msg.valid && p.gm.all Msg.valid && p.oe.all Enum.valid &&
          p.ge.all Enum.valid) then
        panic! "an oracle document is not valid protobuf"
      -- `wire_gap_is_label_move`, checked on the cases themselves.
      if (knownGap p).isSome && !(match p.oe, p.ge with
          | some o, some g => labelMoved o g
          | _, _ => false) then
        panic! "a known gap that is not a moved enum label"
    return {
      model := "Protobuf"
      seed := seed
      notes := [
        "rule table: attribute_kind and the other functions on every listed pointer string; classify inputs [kind, pointer, cardinality attribute]; native_kind inputs [kind, pointer, removed type's kind attribute] (null: absent; type_removed only has the others); breaking_gate inputs [scope, severity, impacts].",
        "pairs: one proto2 file a.proto, package pkg, declaring message M and enum E when not null. A field is [number, name, type, label, in oneof M.o, deprecated], with json_name equal to its name; message reserved ranges are [start, end) and enum reserved ranges [start, end] (inclusive), as descriptors store them. allow_alias is set exactly when two labels share a number.",
        "facts: the distinct [subject, native_kind, severity, impacts] records of protobuf::diff, sorted; subject is the native full_name without 'pkg.', then '#number' when there is one.",
        "fails: the --fail-on-breaking verdict under each --breaking scope. hazard: ProtobufCompat's wire and source hazards. known_gap: the issue a wire hazard the wire gate misses is filed as."
      ]
      fns := ruleTable ++ [
        { lean := "Protobuf.diff, Protobuf.fails, ProtobufCompat.wireHazard, ProtobufCompat.sourceHazard",
          rust := "protobuf::diff, protobuf::breaking_gate",
          cases := (fixedPairs ++ pairs.toList).map pairCase }
      ]
    }

end Vimanam.Oracle.Protobuf
