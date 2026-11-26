/-
# The protobuf `diff` rule table

Models the protobuf adapter of `vimanam diff` (`src/contract/protobuf.rs`):
the neutral comparison's classification (`ProtobufAdapter::attribute_kind`,
`structural_kind` and `classify`), the native rule names (`rule` and the
overrides in `diff`), the composite facts `protobuf::diff` adds after the
neutral comparison (removal without reservation, required-field removal,
enum number reuse, reserved-identity reuse, the required-field downgrade in a
new message), and the `--breaking wire|source|all` failure gate
(`protobuf::breaking_gate`).

The definitions are the Rust, rule for rule, in the Rust's match order, and
nothing else: the specification (which evolutions are wire or source
hazards) and the theorems live in `ProtobufCompat.lean`, so re-reading the
Rust after a change touches this file only.

## Abstractions

Two layers.

* **The rule table** (`attributeKind`, `structuralKind`, `classify`, `rule`,
  `nativeKind`, `breakingGate`) is total over its inputs: every neutral
  `ChangeKind`, every pointer class the Rust's string comparisons tell apart
  (`Ptr`, from `Ptr.ofString`), whether the subject entity's `cardinality`
  attribute is `"required"`, and the `kind` attribute of a removed type.
* **The composite diff** (`msgFacts`, `enumFacts`) is the whole of
  `protobuf::diff` on a fragment of protobuf: one proto2 file in package
  `pkg` declaring at most one message `M` and at most one enum `E`, on each
  side.
  * `M`'s fields are scalar (`int32` or `string`), `optional`, `required` or
    `repeated`, possibly members of the one oneof `M.o`, possibly
    `deprecated`; `json_name` is the field name (what protoc derives for
    these names). `M` may reserve number ranges (`[lo, hi)`, as descriptors
    store them) and names.
  * `E`'s values are labels at signed 32-bit numbers, aliases allowed
    (`allow_alias` is set exactly when two labels share a number, as protoc
    requires). `E` may reserve number ranges (inclusive `[lo, hi]`, as
    descriptors store them) and names.

  The output is the set of facts (subject, native rule, severity, impacts):
  the Rust may report the same record twice (a default's `kind` and `value`
  both change), which the set ignores.

## Boundary

Not modelled, so not guaranteed here: services and RPCs (streaming,
input/output and pagination facts are in the rule table only), messages and
enums as field types, groups, maps, extensions, nested types, more than one
message, enum or oneof, proto3 (presence and syntax changes), options other
than `deprecated` and `allow_alias`, explicit `json_name`s and defaults,
file and package changes and renamed files, the bundled-file filter, Editions
(rejected before diffing), and how descriptors are compiled or decoded.
-/

namespace Vimanam.Protobuf

/-! ## The rule table (`ProtobufAdapter`, `rule`) -/

/-- `contract::Severity`. -/
inductive Severity
  | breaking
  | nonBreaking
  | review
  deriving DecidableEq, Repr

/-- A compatibility dimension in `Change::impacts`. -/
inductive Impact
  | wire
  | source
  | wireJson
  deriving DecidableEq, Repr

/-- `contract::ChangeKind`, every variant. -/
inductive ChangeKind
  | serviceAdded
  | serviceRemoved
  | operationAdded
  | operationRemoved
  | operationRenamed
  | operationMoved
  | typeAdded
  | typeRemoved
  | typeKindChanged
  | argumentAdded
  | argumentRemoved
  | argumentRequiredChanged
  | argumentTypeChanged
  | fieldAdded
  | fieldRemoved
  | fieldTypeChanged
  | fieldRequiredChanged
  | nullabilityChanged
  | enumValueAdded
  | enumValueRemoved
  | deprecatedChanged
  | defaultChanged
  | paginationChanged
  | wireIdentityChanged
  | resultAdded
  | resultRemoved
  | constraintChanged
  deriving DecidableEq, Repr

/-- A change's `details["pointer"]` (`""` when absent), as far as the
adapter's string comparisons tell pointers apart. -/
inductive Ptr
  | deprecated
  | type
  | default
  | jsonName
  | oneof
  | group
  | clientStreaming
  | serverStreaming
  | cardinality
  | presence
  | input
  | output
  | syntax
  | fileOptions
  | other
  deriving DecidableEq, Repr

/-- The class of a pointer string: the exact pointers the adapter compares
with, then the `starts_with("/default")` arm of `attribute_kind` and the
`starts_with("/file_options/")` test of `diff`. No pointer is in two
classes: the exact pointers start with neither prefix. -/
def Ptr.ofString (p : String) : Ptr :=
  match p with
  | "/deprecated" => .deprecated
  | "/type" => .type
  | "/json_name" => .jsonName
  | "/oneof" => .oneof
  | "/group" => .group
  | "/client_streaming" => .clientStreaming
  | "/server_streaming" => .serverStreaming
  | "/cardinality" => .cardinality
  | "/presence" => .presence
  | "/input" => .input
  | "/output" => .output
  | "/syntax" => .syntax
  | _ =>
    if p.startsWith "/default" then .default
    else if p.startsWith "/file_options/" then .fileOptions
    else .other

/-- `ProtobufAdapter::attribute_kind`. -/
def attributeKind : Ptr → ChangeKind
  | .deprecated => .deprecatedChanged
  | .type => .fieldTypeChanged
  | .default => .defaultChanged
  | .jsonName | .oneof | .group | .clientStreaming | .serverStreaming => .wireIdentityChanged
  | _ => .constraintChanged

/-- The `field` argument of `structural_kind`: what `structural_values`
compares. -/
inductive Structural
  | name
  | parent
  | services
  | references
  | native
  deriving DecidableEq, Repr

/-- `ProtobufAdapter::structural_kind`. -/
def structuralKind : Structural → Option ChangeKind
  | .name => some .wireIdentityChanged
  | _ => none

/-- `ProtobufAdapter::classify`: `required` is whether the subject's
`cardinality` attribute is `"required"`. -/
def classify (required : Bool) (k : ChangeKind) (p : Ptr) : Severity × List Impact :=
  match k with
  | .fieldAdded => if required then (.breaking, [.wire, .source]) else (.nonBreaking, [])
  | .serviceAdded | .operationAdded | .typeAdded | .enumValueAdded => (.nonBreaking, [])
  | .deprecatedChanged | .defaultChanged => (.review, [.source])
  | .fieldTypeChanged => (.breaking, [.wire, .source, .wireJson])
  | .fieldRemoved | .enumValueRemoved => (.breaking, [.source])
  | .serviceRemoved | .operationRemoved | .typeRemoved => (.breaking, [.source])
  | .wireIdentityChanged =>
    match p with
    | .oneof | .group => (.breaking, [.wire, .source])
    | .clientStreaming | .serverStreaming => (.breaking, [.wire, .source, .wireJson])
    | _ => (.breaking, [.source, .wireJson])
  | _ =>
    match p with
    | .cardinality | .presence | .input | .output | .syntax => (.breaking, [.wire, .source])
    | _ => (.review, [])

/-- `rule`: the native rule name of a neutral fact. -/
def rule (k : ChangeKind) (p : Ptr) : String :=
  match k, p with
  | .fieldRemoved, _ => "FIELD_NO_DELETE"
  | .enumValueRemoved, _ => "ENUM_VALUE_NO_DELETE"
  | .typeRemoved, _ => "TYPE_NO_DELETE"
  | .serviceRemoved, _ => "SERVICE_NO_DELETE"
  | .operationRemoved, _ => "RPC_NO_DELETE"
  | .fieldTypeChanged, _ => "FIELD_SAME_TYPE"
  | .wireIdentityChanged, .jsonName => "FIELD_SAME_JSON_NAME"
  | .wireIdentityChanged, .oneof => "FIELD_SAME_ONEOF"
  | .wireIdentityChanged, .group => "FIELD_SAME_GROUP_ENCODING"
  | .wireIdentityChanged, .clientStreaming => "RPC_SAME_CLIENT_STREAMING"
  | .wireIdentityChanged, .serverStreaming => "RPC_SAME_SERVER_STREAMING"
  | .wireIdentityChanged, _ => "FIELD_SAME_NAME"
  | _, .cardinality => "FIELD_SAME_CARDINALITY"
  | _, .input => "RPC_SAME_REQUEST_TYPE"
  | _, .output => "RPC_SAME_RESPONSE_TYPE"
  | .deprecatedChanged, _ => "DEPRECATED_CHANGED"
  | _, _ => "PROTOBUF_SURFACE_CHANGED"

/-- The `kind` attribute of a removed type entity, as far as `diff` reads
it. -/
inductive TypeAttr
  | message
  | enum
  | oneof
  | other
  deriving DecidableEq, Repr

/-- The native kind `diff` gives a neutral fact: `FILE_OPTIONS_CHANGED` under
`/file_options/`, else `rule`; then a removed type by its `kind`. -/
def nativeKind (k : ChangeKind) (p : Ptr) (attr : TypeAttr) : String :=
  let base := if p = .fileOptions then "FILE_OPTIONS_CHANGED" else rule k p
  if k = .typeRemoved then
    match attr with
    | .message => "MESSAGE_NO_DELETE"
    | .enum => "ENUM_NO_DELETE"
    | .oneof => "ONEOF_NO_DELETE"
    | .other => "TYPE_NO_DELETE"
  else base

/-- `--breaking`. -/
inductive Scope
  | wire
  | source
  | all
  deriving DecidableEq, Repr

/-- Whether a fact's impacts are in the gate's scope. -/
def inScope : Scope → List Impact → Bool
  | .all, _ => true
  | .wire, i => i.contains .wire
  | .source, i => i.contains .source || i.contains .wireJson

/-- `protobuf::breaking_gate`: whether one fact makes `--fail-on-breaking`
exit 3 under the scope. -/
def breakingGate (sc : Scope) (s : Severity) (i : List Impact) : Bool :=
  decide (s = .breaking) && inScope sc i

/-! ## The composite diff (`protobuf::diff` on the fragment) -/

/-- A field name of `M` (`d` is only ever reserved). -/
inductive FName
  | a
  | b
  | c
  | d
  deriving DecidableEq, Repr

/-- An enum label of `E` (`D` is only ever reserved). -/
inductive ELabel
  | A
  | B
  | C
  | D
  deriving DecidableEq, Repr

/-- A scalar field type. -/
inductive Ty
  | int32
  | string
  deriving DecidableEq, Repr

/-- A proto2 field label. -/
inductive Label
  | optional
  | required
  | repeated
  deriving DecidableEq, Repr

/-- A field of `M`. -/
structure Field where
  num : Nat
  name : FName
  ty : Ty
  label : Label
  inOneof : Bool
  deprecated : Bool
  deriving DecidableEq, Repr

/-- The message `M`. `resNums` are descriptor reserved ranges, `[lo, hi)`. -/
structure Msg where
  fields : List Field
  resNums : List (Nat × Nat)
  resNames : List FName
  deriving DecidableEq, Repr

/-- A value of `E`. -/
structure EVal where
  label : ELabel
  num : Int
  deriving DecidableEq, Repr

/-- The enum `E`. `resNums` are descriptor reserved ranges, inclusive
`[lo, hi]`. -/
structure Enum where
  values : List EVal
  resNums : List (Int × Int)
  resNames : List ELabel
  deriving DecidableEq, Repr

/-- A fact's subject: its native location (`full_name` without `pkg.`, and
the number). -/
inductive Subject
  | msg
  | oneof
  | field (name : FName) (num : Nat)
  | enum
  | value (label : ELabel) (num : Int)
  deriving DecidableEq, Repr

/-- One reported change, as far as verdicts go. -/
structure Fact where
  subject : Subject
  native : String
  sev : Severity
  impacts : List Impact
  deriving DecidableEq, Repr

/-- A fact from the neutral comparison (`compare` then `classify`, `diff`'s
native kinds). -/
def neutral (s : Subject) (required : Bool) (attr : TypeAttr) (k : ChangeKind) (p : Ptr) :
    Fact :=
  { subject := s, native := nativeKind k p attr,
    sev := (classify required k p).1, impacts := (classify required k p).2 }

/-- The field number `n` of `M`, if declared. -/
def Msg.at (m : Msg) (n : Nat) : Option Field := m.fields.find? (·.num == n)

/-- Whether `M` declares the oneof (it has a member). -/
def Msg.hasOneof (m : Msg) : Bool := m.fields.any (·.inOneof)

/-- The message reserved-range test of `diff`: `lo <= n < hi`. -/
def msgReserved (rs : List (Nat × Nat)) (n : Nat) : Bool :=
  rs.any fun r => decide (r.1 ≤ n) && decide (n < r.2)

/-- `FieldDescriptor::supports_presence` in proto2: every non-repeated field. -/
def Field.presence (f : Field) : Bool := f.label != .repeated

/-- The `kind` of a field's effective default (`default_value`). -/
inductive DefaultKind
  | i32
  | string
  | list
  deriving DecidableEq, Repr

def Field.defaultKind (f : Field) : DefaultKind :=
  if f.label = .repeated then .list
  else match f.ty with
    | .int32 => .i32
    | .string => .string

/-- The subject of a field. -/
def Field.subject (f : Field) : Subject := .field f.name f.num

/-- The attribute and structural changes of the field entity `M#n`, present
on both sides (`compare`): `json_name` changes with the name. -/
def fieldPairFacts (o g : Field) : List Fact :=
  let s := o.subject
  let r := o.label = .required
  (if o.name ≠ g.name then
      [neutral s r .other .wireIdentityChanged .other,
       neutral s r .other (attributeKind .jsonName) .jsonName]
    else []) ++
  (if o.ty ≠ g.ty then [neutral s r .other (attributeKind .type) .type] else []) ++
  (if o.label ≠ g.label then
      [neutral s r .other (attributeKind .cardinality) .cardinality] else []) ++
  (if o.presence ≠ g.presence then
      [neutral s r .other (attributeKind .presence) .presence] else []) ++
  (if o.defaultKind ≠ g.defaultKind then
      [neutral s r .other (attributeKind .default) .default] else []) ++
  (if o.inOneof ≠ g.inOneof then [neutral s r .other (attributeKind .oneof) .oneof] else []) ++
  (if o.deprecated ≠ g.deprecated then
      [neutral s r .other (attributeKind .deprecated) .deprecated] else [])

/-- A field gone from a surviving `M`: `FieldRemoved`, then `diff`'s
`FIELD_NO_DELETE_UNLESS_NUMBER_RESERVED` (unless the new `M` reserves the
number; a field ID is its number, so no field of the new `M` reuses it) and
`FIELD_REQUIRED_NO_DELETE`. -/
def fieldGoneFacts (g : Msg) (f : Field) : List Fact :=
  [neutral f.subject (f.label = .required) .other .fieldRemoved .other] ++
  (if msgReserved g.resNums f.num then [] else
    [{ subject := f.subject, native := "FIELD_NO_DELETE_UNLESS_NUMBER_RESERVED",
       sev := .breaking, impacts := [.wire] }]) ++
  (if f.label = .required then
    [{ subject := f.subject, native := "FIELD_REQUIRED_NO_DELETE",
       sev := .breaking, impacts := [.wire, .source] }] else [])

/-- `RESERVED_IDENTITY_REUSED` for a field of the new `M` whose number or
name the old `M` reserves. -/
def fieldReservedFacts (o : Msg) (h : Field) : List Fact :=
  let numRes := msgReserved o.resNums h.num
  if numRes || o.resNames.contains h.name then
    [{ subject := h.subject, native := "RESERVED_IDENTITY_REUSED", sev := .breaking,
       impacts := if numRes then [.wire, .source] else [.source, .wireJson] }]
  else []

/-- The facts of `M` present on both sides. -/
def msgPairFacts (o g : Msg) : List Fact :=
  (if o.hasOneof && !g.hasOneof then
    [neutral .oneof false .oneof .typeRemoved .other] else []) ++
  (if !o.hasOneof && g.hasOneof then
    [neutral .oneof false .other .typeAdded .other] else []) ++
  o.fields.flatMap (fun f =>
    match g.at f.num with
    | some h => fieldPairFacts f h
    | none => fieldGoneFacts g f) ++
  g.fields.flatMap (fun h =>
    (match o.at h.num with
     | some _ => []
     | none => [neutral h.subject (h.label = .required) .other .fieldAdded .other]) ++
    fieldReservedFacts o h)

/-- `protobuf::diff`'s facts about `M`. A field added with a new `M` is
downgraded to Non-breaking with no impacts (`FieldAdded` whose parent is
new). -/
def msgFacts : Option Msg → Option Msg → List Fact
  | none, none => []
  | some o, none =>
    [neutral .msg false .message .typeRemoved .other] ++
    (if o.hasOneof then [neutral .oneof false .oneof .typeRemoved .other] else []) ++
    o.fields.map fun f => neutral f.subject (f.label = .required) .other .fieldRemoved .other
  | none, some g =>
    [neutral .msg false .other .typeAdded .other] ++
    (if g.hasOneof then [neutral .oneof false .other .typeAdded .other] else []) ++
    g.fields.map fun h =>
      { neutral h.subject (h.label = .required) .other .fieldAdded .other with
        sev := .nonBreaking, impacts := [] }
  | some o, some g => msgPairFacts o g

/-- The enum reserved-range test of `diff`: the range is stored as
`[lo, hi + 1]` in 64 bits, then `lo <= n < hi + 1`. -/
def enumReserved (rs : List (Int × Int)) (n : Int) : Bool :=
  rs.any fun r => decide (r.1 ≤ n) && decide (n < r.2 + 1)

/-- Whether two labels share a number: protoc requires `allow_alias` exactly
then. -/
def Enum.allowAlias (e : Enum) : Bool :=
  e.values.any fun v => e.values.any fun w => v.label != w.label && v.num == w.num

/-- The subject of a value. -/
def EVal.subject (v : EVal) : Subject := .value v.label v.num

/-- A value entity (`E#n:label`) gone from a surviving `E`:
`EnumValueRemoved`, then `diff`'s
`ENUM_VALUE_NO_DELETE_UNLESS_NUMBER_RESERVED` (unless the new `E` reserves
the number or another label keeps it) and `ENUM_VALUE_NUMBER_REUSED` (some
label of the new `E` at the number had another number in the old `E`). -/
def valueGoneFacts (o g : Enum) (v : EVal) : List Fact :=
  let reused := g.values.any (·.num == v.num)
  [neutral v.subject false .other .enumValueRemoved .other] ++
  (if !enumReserved g.resNums v.num && !reused then
    [{ subject := v.subject, native := "ENUM_VALUE_NO_DELETE_UNLESS_NUMBER_RESERVED",
       sev := .breaking, impacts := [.wire] }] else []) ++
  (if g.values.any (fun w => w.num == v.num &&
        o.values.any (fun p => p.label == w.label && p.num != w.num)) then
    [{ subject := v.subject, native := "ENUM_VALUE_NUMBER_REUSED",
       sev := .breaking, impacts := [.wire] }] else [])

/-- `RESERVED_IDENTITY_REUSED` for a value of the new `E` whose number or
label the old `E` reserves. -/
def valueReservedFacts (o : Enum) (w : EVal) : List Fact :=
  let numRes := enumReserved o.resNums w.num
  if numRes || o.resNames.contains w.label then
    [{ subject := w.subject, native := "RESERVED_IDENTITY_REUSED", sev := .breaking,
       impacts := if numRes then [.wire, .source] else [.source, .wireJson] }]
  else []

/-- The facts of `E` present on both sides; `allow_alias` is an option, so
its change is a Review `ConstraintChanged` on `E`. -/
def enumPairFacts (o g : Enum) : List Fact :=
  (if o.allowAlias != g.allowAlias then
    [neutral .enum false .other .constraintChanged .other] else []) ++
  o.values.flatMap (fun v => if g.values.contains v then [] else valueGoneFacts o g v) ++
  g.values.flatMap (fun w =>
    (if o.values.contains w then [] else
      [neutral w.subject false .other .enumValueAdded .other]) ++
    valueReservedFacts o w)

/-- `protobuf::diff`'s facts about `E`. -/
def enumFacts : Option Enum → Option Enum → List Fact
  | none, none => []
  | some o, none =>
    [neutral .enum false .enum .typeRemoved .other] ++
    o.values.map fun v => neutral v.subject false .other .enumValueRemoved .other
  | none, some g =>
    [neutral .enum false .other .typeAdded .other] ++
    g.values.map fun w => neutral w.subject false .other .enumValueAdded .other
  | some o, some g => enumPairFacts o g

/-- `protobuf::diff` on the fragment. -/
def diff (om gm : Option Msg) (oe ge : Option Enum) : List Fact :=
  msgFacts om gm ++ enumFacts oe ge

/-- The `--fail-on-breaking` verdict under a scope: some fact fails the
gate (`cli::run_diff`). -/
def fails (sc : Scope) (fs : List Fact) : Bool :=
  fs.any fun f => breakingGate sc f.sev f.impacts

end Vimanam.Protobuf
