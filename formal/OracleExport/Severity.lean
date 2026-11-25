/-
# Oracle vectors for `Severity.lean` and `EnumReport.lean`

* `locate`, `underNot` against `diff::value::locate` / `under_not`, on fixed
  pointers (keyword lookalikes under `properties`, `not` at several depths)
  and random pointers over a small vocabulary of keywords and names.
* `requestSeverity`, `responseSeverity`, and `requestSeverity` again for
  parameters, against `diff::severity` on `RequestSchemaChanged`,
  `ResponseSchemaChanged` and `ParameterSchemaChanged`: every fixed pointer
  with every change kind (each `Val` in each position), then random pointers
  with random kinds (parameters: the random pairs only).
* `severity` on every structural `ChangeKind` (all flag combinations).
* `EnumReport.report` against `diff_values` on `{"enum": old}` vs
  `{"enum": new}` (an absent side is `{}`): the pointers and kinds in order,
  and each change's request and response verdict.

Values: `tru`, `fls`, `null` are JSON `true`, `false`, `null`; `other` is the
string `"x"`. Pointer tokens never contain `~` or `/`, so a pointer is
`"/" ++ tokens joined by "/"` (the Rust test checks this).
-/
import VimanamProofs.EnumReport
import OracleExport.Gen

namespace Vimanam.Oracle.Severity

open Lean Vimanam.Severity Vimanam.Oracle

def sevName : Severity → String
  | .breaking => "breaking"
  | .nonBreaking => "non_breaking"
  | .review => "review"

def locName : Loc → String
  | .type => "type"
  | .requiredElement => "required_element"
  | .enumElement => "enum_element"
  | .enum => "enum"
  | .property => "property"
  | .additionalProperties => "additional_properties"
  | .nullable => "nullable"
  | .other => "other"

def valName : Val → String
  | .tru => "true"
  | .fls => "false"
  | .null => "null"
  | .other => "other"

def kindJson : Kind → Json
  | .added v => toJson #["added", valName v]
  | .removed v => toJson #["removed", valName v]
  | .changed a b => toJson #["changed", valName a, valName b]

instance : Inhabited Val := ⟨.other⟩
instance : Inhabited Kind := ⟨.added .other⟩

/-- Pointers chosen for their boundaries. -/
def fixedPointers : List Pointer := [
  [], ["type"], ["items", "type"], ["properties", "a", "type"],
  ["properties", "type"], ["properties", "not"], ["properties", "enum"],
  ["properties", "required"], ["properties", "a"],
  ["required", "a"], ["required", "type"], ["enum", "x"], ["enum", "not"], ["enum"],
  ["items", "enum"], ["additionalProperties"], ["additionalProperties", "nullable"],
  ["nullable"], ["properties", "a", "nullable"], ["format"], ["allOf", "0"],
  ["allOf", "0", "required", "b"], ["items"], ["default", "type"],
  ["not"], ["not", "type"], ["not", "enum", "x"], ["items", "not", "nullable"],
  ["properties", "not", "type"], ["allOf", "not"], ["patternProperties", "^a", "type"]
]

def vocabulary : List String := [
  "properties", "patternProperties", "allOf", "oneOf", "anyOf", "items",
  "additionalProperties", "not", "required", "enum", "type", "nullable",
  "format", "default", "a", "0", "x"
]

def genPointer : Gen Pointer := do
  let n ← below 6
  many n (pick vocabulary)

def genKind : Gen Kind := do
  let v ← pick allVals
  let w ← pick allVals
  pick [.added v, .removed v, .changed v w]

def ptrJson (p : Pointer) : Json := toJson p.toArray

def locateCases (ps : List Pointer) : List Json :=
  ps.map fun p => case (ptrJson p) (toJson #[toJson (locName (locate p)), toJson (underNot p)])

def tableCases (f : Pointer → Kind → Severity) (pairs : List (Pointer × Kind)) : List Json :=
  pairs.map fun (p, k) => case (toJson #[ptrJson p, kindJson k]) (toJson (sevName (f p k)))

def structural : List Change :=
  [.endpointAdded, .endpointRemoved false, .endpointRemoved true, .endpointMoved,
   .parameterAdded false, .parameterAdded true, .parameterRemoved,
   .parameterRequiredChanged false, .parameterRequiredChanged true,
   .parameterLocationChanged, .responseAdded, .responseRemoved,
   .operationIdChanged false false, .operationIdChanged false true,
   .operationIdChanged true false, .operationIdChanged true true,
   .deprecatedChanged false, .deprecatedChanged true]

def changeJson : Change → Json
  | .endpointAdded => toJson #["endpoint_added"]
  | .endpointRemoved d => toJson #[toJson "endpoint_removed", toJson d]
  | .endpointMoved => toJson #["endpoint_moved"]
  | .parameterAdded r => toJson #[toJson "parameter_added", toJson r]
  | .parameterRemoved => toJson #["parameter_removed"]
  | .parameterRequiredChanged n => toJson #[toJson "parameter_required_changed", toJson n]
  | .parameterLocationChanged => toJson #["parameter_location_changed"]
  | .responseAdded => toJson #["response_added"]
  | .responseRemoved => toJson #["response_removed"]
  | .operationIdChanged o n => toJson #[toJson "operation_id_changed", toJson o, toJson n]
  | .deprecatedChanged n => toJson #[toJson "deprecated_changed", toJson n]
  | _ => toJson #["schema"]

def enumJson : EnumReport.Enum → Json
  | none => Json.null
  | some xs => toJson xs.toArray

def subsets : List (List Nat) :=
  [[], [0], [1], [0, 1], [1, 2], [0, 1, 2], [2, 0], [3]]

def enums : List EnumReport.Enum := none :: subsets.map some

def enumCases : List Json :=
  enums.flatMap fun o => enums.map fun n =>
    let out := (EnumReport.report o n).map fun (p, k) =>
      toJson #[toJson ("/" ++ "/".intercalate p), kindJson k,
        toJson (sevName (requestSeverity p k)), toJson (sevName (responseSeverity p k))]
    case (toJson #[enumJson o, enumJson n]) (toJson out.toArray)

def seed : UInt64 := 0x5EEDD1FF

def vectors : Vectors :=
  run seed do
    let randomPtrs ← many 300 genPointer
    let fixedPairs := fixedPointers.flatMap fun p => allKinds.map fun k => (p, k)
    let randomPairs ← many 300 (do return (← genPointer, ← genKind))
    let pairs := fixedPairs ++ randomPairs
    return {
      model := "Severity"
      seed := seed
      notes := [
        "Values: true, false, null as JSON; other is the string \"x\".",
        "A pointer is its tokens; none contains '~' or '/', so the Rust pointer is \"/\" + tokens joined by \"/\" (\"\" for []).",
        "request/response: every fixed pointer with every change kind, then 300 random (pointer, kind) pairs; parameter: the random pairs only (same Rust table as request).",
        "Changed with equal sides is included: diff_values never emits it, but severity must still agree.",
        "enum_report: the enum keyword of a root schema, old and new each absent (null) or a list of distinct integers; output is [pointer, kind, request verdict, response verdict] per reported change, in order."
      ]
      fns := [
        { lean := "Severity.locate, Severity.underNot", rust := "diff::value::locate, diff::value::under_not",
          cases := locateCases (fixedPointers ++ randomPtrs) },
        { lean := "Severity.requestSeverity", rust := "diff::severity(RequestSchemaChanged)",
          cases := tableCases requestSeverity pairs },
        { lean := "Severity.requestSeverity", rust := "diff::severity(ParameterSchemaChanged)",
          cases := tableCases (fun p k => severity (.parameterSchemaChanged p k)) randomPairs },
        { lean := "Severity.responseSeverity", rust := "diff::severity(ResponseSchemaChanged)",
          cases := tableCases responseSeverity pairs },
        { lean := "Severity.severity", rust := "diff::severity(structural ChangeKind)",
          cases := structural.map fun c => case (changeJson c) (toJson (sevName (severity c))) },
        { lean := "EnumReport.report", rust := "diff::value::diff_values on an enum keyword",
          cases := enumCases }
      ]
    }

end Vimanam.Oracle.Severity
