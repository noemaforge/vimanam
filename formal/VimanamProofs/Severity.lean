/-
# The `diff` severity rule table

Models `vimanam::diff::severity` (`src/diff/mod.rs`) and the two schema tables
it dispatches to, `request_schema_severity` (parameter and request-body
schemas: what the client sends) and `response_schema_severity` (what the
client receives), together with the schema-pointer grammar they classify
pointers with (`Node`, `locate`, `under_not` in `src/diff/value.rs`) and the
boolean-keyword helpers `became` and `at_default`.

The definitions are the Rust, rule for rule, in the Rust's match order, and
nothing else: the specification and the direction theorems live in
`Direction.lean`, so re-reading the table after a Rust change touches this
file only.

## Abstractions

* A JSON pointer is the list of its decoded reference tokens; the Rust's
  `split('/').skip(1)` and `decode_json_pointer_token` are not modelled (the
  oracle only generates tokens without `~` or `/`, and checks that).
* A JSON value is abstracted to what the table can observe: `true`, `false`,
  `null`, or anything else (`Val.other`). The table never looks deeper.
* A change kind is `Added v`, `Removed v` or `Changed old new`. The differ
  (`diff_values`) never reports `Changed a a`; theorems that need this take
  `Kind.Wf`.
* A `ComponentSchemaChanged` (#181) keeps its direction, `negated` flag and
  change; which component and which uses it reports are dropped.
* `ParameterAdded`, `EndpointRemoved` and the other structural changes keep
  only the fields `severity` reads (names, locations and statuses are
  dropped; an `operationId` is reduced to present or absent).

## Boundary

Not modelled, so not guaranteed here: how `diff_values` produces changes
(except the enum-keyword reporting in `EnumReport.lean`), `$ref` resolution
and canonicalisation, the suppression of the companion `required` removal in
`value_changes`, how `diff::components` finds shared components, their uses,
directions and `negated` reaches (`resolve::PairResolver::localize`),
endpoint matching and moves, the protobuf and GraphQL rule
tables (`src/contract/`), and the v2 JSON verdicts. `type` changes are taken
as having no known direction (the table rates them Breaking on both sides);
in particular adding `"null"` to an OpenAPI 3.1 `type` array widens the
schema, which the request table still rates Breaking (conservative, not a
direction error in the unsafe sense).
-/

namespace Vimanam.Severity

/-! ## The Rust's types -/

/-- `diff::Severity`. -/
inductive Severity
  | breaking
  | nonBreaking
  | review
  deriving DecidableEq, Repr

/-- A `serde_json::Value`, as far as the table can tell values apart. -/
inductive Val
  | tru
  | fls
  | null
  | other
  deriving DecidableEq, Repr

/-- The boolean a value is, if it is one (`Value::Bool`). -/
def Val.bool? : Val → Option Bool
  | .tru => some true
  | .fls => some false
  | _ => none

/-- `diff::ValueChangeKind`. -/
inductive Kind
  | added (v : Val)
  | removed (v : Val)
  | changed (old new : Val)
  deriving DecidableEq, Repr

/-- The kinds `diff_values` can emit: it skips equal values. -/
def Kind.Wf : Kind → Prop
  | .changed a b => a ≠ b
  | _ => True

instance (k : Kind) : Decidable k.Wf := by
  cases k <;> unfold Kind.Wf <;> infer_instance

/-- The same change seen from the other side: old and new swapped. -/
def Kind.swap : Kind → Kind
  | .added v => .removed v
  | .removed v => .added v
  | .changed a b => .changed b a

/-- `diff::value::Location`. -/
inductive Loc
  | type
  | requiredElement
  | enumElement
  | enum
  | property
  | additionalProperties
  | nullable
  | other
  deriving DecidableEq, Repr

/-- `diff::value::Node` (`Opaque` is `opaq`: `opaque` is a Lean keyword). -/
inductive Node
  | schema
  | propertyMap
  | schemaList
  | set (element : Loc)
  | opaq
  deriving DecidableEq, Repr

/-- A JSON pointer as its decoded reference tokens; `[]` is the root. -/
abbrev Pointer := List String

/-! ## The schema grammar (`src/diff/value.rs`) -/

/-- `Node::child`. -/
def Node.child : Node → String → Node
  | .schema, s =>
    match s with
    | "properties" | "patternProperties" => .propertyMap
    | "allOf" | "oneOf" | "anyOf" => .schemaList
    | "items" | "additionalProperties" | "not" => .schema
    | "required" => .set .requiredElement
    | "enum" => .set .enumElement
    | _ => .opaq
  | .propertyMap, _ => .schema
  | .schemaList, _ => .schema
  | .set _, _ => .opaq
  | .opaq, _ => .opaq

/-- `Node::locate`. -/
def Node.locate : Node → String → Loc
  | .schema, s =>
    match s with
    | "type" => .type
    | "nullable" => .nullable
    | "enum" => .enum
    | "additionalProperties" => .additionalProperties
    | _ => .other
  | .propertyMap, _ => .property
  | .set element, _ => element
  | .schemaList, _ => .other
  | .opaq, _ => .other

/-- The node a pointer reaches from `n`. -/
def walk (n : Node) : Pointer → Node
  | [] => n
  | s :: ss => walk (n.child s) ss

/-- The loop of `locate`: `loc` is the location of the last segment so far. -/
def locateGo (n : Node) (loc : Loc) : Pointer → Loc
  | [] => loc
  | s :: ss => locateGo (n.child s) (n.locate s) ss

/-- `diff::value::locate`. -/
def locate (p : Pointer) : Loc := locateGo .schema .other p

/-- The loop of `under_not`. -/
def underNotGo (n : Node) : Pointer → Bool
  | [] => false
  | s :: ss => (n == .schema && s == "not") || underNotGo (n.child s) ss

/-- `diff::value::under_not`. -/
def underNot (p : Pointer) : Bool := underNotGo .schema p

/-! ## The tables (`src/diff/mod.rs`) -/

/-- `became`: the change moves a boolean keyword whose absent value is `dflt`
to `flag` (#184). Added counts only when `flag` is not the default (writing
the default changes nothing); changed counts when the new value is `flag`;
removed counts only when the old value was not `flag` and the default is
(dropping the keyword leaves its default). -/
def became (k : Kind) (flag dflt : Bool) : Bool :=
  match k with
  | .added v => v.bool? == some flag && flag != dflt
  | .changed _ v => v.bool? == some flag
  | .removed v =>
    match v.bool? with
    | some was => was != flag && dflt == flag
    | none => false

/-- `at_default`: a boolean keyword written or dropped at its default value
`dflt`, which changes nothing (#184). -/
def atDefault (k : Kind) (dflt : Bool) : Bool :=
  match k with
  | .added v | .removed v => v.bool? == some dflt
  | .changed _ _ => false

/-- The match of `request_schema_severity`, after its `under_not` check. -/
def requestRule : Loc → Kind → Severity
  | .requiredElement, .added _ => .breaking
  | .requiredElement, .removed _ => .nonBreaking
  | .type, _ => .breaking
  | .property, .removed _ => .breaking
  | .property, .added _ => .nonBreaking
  | .enumElement, .removed _ => .breaking
  | .enumElement, .added _ => .nonBreaking
  | .enum, .added _ => .breaking
  | .enum, .removed _ => .nonBreaking
  | .additionalProperties, k =>
    if atDefault k true then .nonBreaking
    else if became k false true then .breaking
    else .review
  | .nullable, k =>
    if atDefault k false then .nonBreaking
    else if became k true false then .nonBreaking
    else if became k false false then .breaking
    else .review
  | _, _ => .review

/-- `request_schema_severity`. -/
def requestSeverity (p : Pointer) (k : Kind) : Severity :=
  if underNot p then .review else requestRule (locate p) k

/-- The match of `response_schema_severity`, after its root and `under_not`
checks. -/
def responseRule : Loc → Kind → Severity
  | .property, .removed _ => .breaking
  | .property, .added _ => .nonBreaking
  | .type, _ => .breaking
  | .enumElement, .removed _ => .nonBreaking
  | .enumElement, .added _ => .review
  | .enum, .removed _ => .review
  | .enum, .added _ => .nonBreaking
  | .requiredElement, .removed _ => .breaking
  | .requiredElement, .added _ => .nonBreaking
  | .additionalProperties, k => if atDefault k true then .nonBreaking else .review
  | .nullable, k =>
    if atDefault k false then .nonBreaking
    else if became k true false then .breaking
    else if became k false false then .nonBreaking
    else .review
  | _, _ => .review

/-- The root check of `response_schema_severity`: the response used to
document a body and no longer does. -/
def bodyDropped (p : Pointer) (k : Kind) : Bool :=
  match p, k with
  | [], .changed _ .null => true
  | _, _ => false

/-- `response_schema_severity`. -/
def responseSeverity (p : Pointer) (k : Kind) : Severity :=
  if bodyDropped p k then .breaking
  else if underNot p then .review
  else responseRule (locate p) k

/-- `diff::Direction`: whether a shared component's uses send it (request)
or receive it (response). -/
inductive Direction
  | request
  | response
  deriving DecidableEq, Repr

/-- `diff::ChangeKind`, keeping only what `severity` reads. For
`ComponentSchemaChanged` that is the direction, the `negated` flag and the
field-level change, whose pointer is relative to the component (`component`
and `uses` are dropped). -/
inductive Change
  | endpointAdded
  | endpointRemoved (wasDeprecated : Bool)
  | endpointMoved
  | parameterAdded (required : Bool)
  | parameterRemoved
  | parameterRequiredChanged (nowRequired : Bool)
  | parameterLocationChanged
  | parameterSchemaChanged (p : Pointer) (k : Kind)
  | responseAdded
  | responseRemoved
  | operationIdChanged (oldPresent newPresent : Bool)
  | deprecatedChanged (now : Bool)
  | requestSchemaChanged (p : Pointer) (k : Kind)
  | responseSchemaChanged (p : Pointer) (k : Kind)
  | componentSchemaChanged (direction : Direction) (negated : Bool) (p : Pointer) (k : Kind)
  deriving DecidableEq, Repr

/-- `diff::severity`. -/
def severity : Change → Severity
  | .endpointAdded => .nonBreaking
  | .endpointRemoved _ | .endpointMoved => .breaking
  | .parameterAdded required => if required then .breaking else .nonBreaking
  | .parameterRemoved => .breaking
  | .parameterRequiredChanged now => if now then .breaking else .nonBreaking
  | .parameterLocationChanged => .breaking
  | .parameterSchemaChanged p k | .requestSchemaChanged p k => requestSeverity p k
  | .responseAdded => .nonBreaking
  | .responseRemoved => .breaking
  | .operationIdChanged false true => .nonBreaking
  | .operationIdChanged _ _ => .breaking
  | .deprecatedChanged _ => .nonBreaking
  | .responseSchemaChanged p k => responseSeverity p k
  | .componentSchemaChanged _ true _ _ => .review
  | .componentSchemaChanged .request false p k => requestSeverity p k
  | .componentSchemaChanged .response false p k => responseSeverity p k

end Vimanam.Severity
