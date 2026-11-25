/-
# Reference semantics of the `$ref`-aware schema diff (#181)

An executable reference model of what `vimanam diff` is meant to report for
schemas that use `$ref`, written as the specification #181 lacks. It is an
oracle, not a proof development: `lake exe refdiff` evaluates it on spec pairs
and `scripts/formal/refdiff-harness.py` compares it with the Rust. The
definitions are `partial` (executable, not yet usable in proofs) and the
sanity checks are `#guard`s. formal/README.md, "Reference semantics of `$ref`
diffs", states the same rules in prose.

## The two views

* **Per path** (`perPath`): what fully inlining both sides and diffing the
  resolved trees by JSON pointer reports at one use, which is main's
  semantics, minus main's artifacts: cycle cuts are keyed by component *name*
  pair in lockstep (not by ref spelling, side by side), there is no 64-hop
  cap, and a cut contributes nothing (main compares the two literal `$ref`
  objects there).
* **Grouped** (`grouped`): the same changes regrouped by *owner*, the deepest
  component pair `(X_old, Y_new)` whose expansion produced the change (or the
  use site itself), with the pointer relative to the owner, the usage
  direction, and the set of uses that reach it. This is #181's intended
  output.

Both are computed from one local walk (`go`) that compares two schemas the
way `diff_values` does, except at a `$ref`:

1. **Both sides reference a declared component** (`X`, `Y`): a *hop*. The
   `$ref` keys are not compared. Every key that a sibling sets on either side
   (`mask`) is compared here, at the current owner, as the effective value:
   the sibling if that side has one, else the target's value (siblings win,
   as `utils::resolve_schema_value` merged them). Everything else belongs to
   the pair `(X, Y)`, compared once, at relative pointers.
2. **One side references a declared component, the other is an inline
   object**: the reference is expanded in place, one hop (target with
   siblings merged over it), and the walk continues at the same owner. Two
   sides can keep expanding each other forever (each facing the other's
   inline content); the walk is a function of the value pair it compares, so
   it stops where a pair it produced by expansion recurs deeper on the chain,
   and reports nothing there. A pair that recurs at the same pointer is an
   alias loop (`X = {"$ref": X, "type": "array"}`): its value is the fixpoint,
   the merged siblings without `$ref`.
3. **Anything else**, including a reference to an undeclared component and a
   reference facing a non-object or nothing, is compared as written.

A hop is a *state* `(X, Y, mask, neg)`. The pair's content is visible at a
state except under the masked top-level keys (overlay masking: a key a
sibling overrides at the hop comes from the sibling, not from the
component), and pairs nested in the content are reached under the same rule.
A nested hop at the pair's root (an alias, `X = {"$ref": Z}`) inherits the
outer mask. `neg` records that the hop sits under a `not` keyword in its
owner (or its owner was so reached); verdicts there are Review, as
`under_not` makes them for inline schemas.

## Abstractions and boundary

* JSON objects keep the order they were given, but the CLI front end
  (`OracleExport/RefDiffMain.lean`) reads them through `Lean.Json`, which
  sorts keys; only "first media type" depends on order.
* Component names come from `#/components/schemas/NAME` and
  `#/definitions/NAME` (decoded), in either kind of document, as in
  `resolve::reference_name`.
* Endpoints match on `(METHOD, path)` only (moves are not paired, as in
  v1 output). A use is a non-body parameter `(in, name)`, the request body
  (first media type), or a response status present on both sides.
* Not modelled: `$ref`s outside schema positions (e.g. under `default`),
  parameter and response `$ref`s, the `type: "a | b"` union string form,
  non-object component targets beyond "compare as written", and everything
  outside schema comparison (structural endpoint changes).
-/
import VimanamProofs.Severity

namespace Vimanam.RefDiff

open Vimanam.Severity (Pointer Kind Val Severity Node Loc locate underNot
  requestSeverity responseSeverity)

/-! ## JSON -/

/-- A JSON value; numbers keep their source text. -/
inductive J where
  | null
  | bool (b : Bool)
  | num (s : String)
  | str (s : String)
  | arr (xs : List J)
  | obj (kvs : List (String × J))
  deriving Inhabited, Repr

namespace J

/-- Structural equality; objects compare as maps (key order is ignored, as
`serde_json::Map` equality does). -/
partial def beq : J → J → Bool
  | .null, .null => true
  | .bool a, .bool b => a == b
  | .num a, .num b => a == b
  | .str a, .str b => a == b
  | .arr xs, .arr ys => xs.length == ys.length && (xs.zip ys).all fun (x, y) => beq x y
  | .obj a, .obj b =>
    a.length == b.length && a.all fun (k, v) =>
      match b.lookup k with
      | some w => beq v w
      | none => false
  | _, _ => false

instance : BEq J := ⟨beq⟩

/-- A JSON string literal. -/
def quote (s : String) : String :=
  let body := s.toList.foldl (fun acc c =>
    acc ++ match c with
      | '"' => "\\\""
      | '\\' => "\\\\"
      | '\n' => "\\n"
      | '\t' => "\\t"
      | '\r' => "\\r"
      | c => if c.toNat < 0x20 then
          "\\u00" ++ String.ofList [Nat.digitChar (c.toNat / 16), Nat.digitChar (c.toNat % 16)]
        else String.singleton c) ""
  "\"" ++ body ++ "\""

/-- Compact JSON text, as `serde_json`'s `to_string` writes it. -/
partial def render : J → String
  | .null => "null"
  | .bool b => if b then "true" else "false"
  | .num s => s
  | .str s => quote s
  | .arr xs => "[" ++ ",".intercalate (xs.map render) ++ "]"
  | .obj kvs => "{" ++ ",".intercalate (kvs.map fun (k, v) => quote k ++ ":" ++ render v) ++ "}"

def field? (j : J) (k : String) : Option J :=
  match j with
  | .obj kvs => kvs.lookup k
  | _ => none

def isObj : J → Bool
  | .obj _ => true
  | _ => false

def isNull : J → Bool
  | .null => true
  | _ => false

def keys : J → List String
  | .obj kvs => kvs.map (·.1)
  | _ => []

/-- What the severity table can observe of a value. -/
def val : J → Val
  | .bool true => .tru
  | .bool false => .fls
  | .null => .null
  | _ => .other

end J

/-- Sorted, without duplicates. -/
def sortDedup (xs : List String) : List String :=
  (xs.mergeSort (fun a b => decide (a ≤ b))).eraseDups

/-! ## Canonicalisation (`utils::canonicalize_schema_value`) -/

def annotationKeys : List String := ["description", "title", "example", "examples", "deprecated"]
def schemaMapKeys : List String := ["properties", "patternProperties", "definitions", "$defs"]
def schemaKeys : List String := ["items", "additionalProperties", "not"]
def schemaListKeys : List String := ["allOf", "oneOf", "anyOf"]

/-- A `type` array is a set: sorted by its JSON text, deduplicated. -/
def canonType : J → J
  | .arr xs =>
    let sorted := xs.mergeSort (fun a b => decide (a.render ≤ b.render))
    .arr (sorted.foldr (fun x acc => if acc.head?.any (· == x) then acc else x :: acc) [])
  | j => j

/-- Strips annotations (`description`, `title`, `example(s)`, `deprecated`,
`x-*`) from schema objects, recursing through the schema grammar. `required`
and `enum` stay unsorted: the model compares them as sets. Applied before
references are followed, which is equivalent to main's resolve-then-
canonicalise because stripping is per key at fixed grammar positions and
merging a reference keeps every key at its position. -/
partial def canon : J → J
  | .obj kvs =>
    .obj <| (kvs.filter fun (k, _) => !(k.startsWith "x-" || annotationKeys.contains k)).map
      fun (k, v) =>
        if schemaMapKeys.contains k then
          (k, match v with
            | .obj m => .obj (m.map fun (n, s) => (n, canon s))
            | v => v)
        else if schemaKeys.contains k then (k, canon v)
        else if schemaListKeys.contains k then
          (k, match v with
            | .arr xs => .arr (xs.map canon)
            | v => v)
        else if k == "type" then (k, canonType v)
        else (k, v)
  | j => j

/-! ## References -/

/-- Decodes one JSON pointer token (`~1` is `/`, then `~0` is `~`). -/
def decodeToken (t : String) : String := (t.replace "~1" "/").replace "~0" "~"

/-- Encodes one JSON pointer token. -/
def encodeToken (t : String) : String := (t.replace "~" "~0").replace "/" "~1"

/-- `p` with `pre` removed, if `p` starts with it. -/
def afterPrefix? (pre p : String) : Option String :=
  if p.startsWith pre then some (String.ofList (p.toList.drop pre.length)) else none

/-- The component a `$ref` names (`resolve::reference_name`): the two local
spellings are the same name. -/
def refName? (r : String) : Option String :=
  (afterPrefix? "#/components/schemas/" r <|> afterPrefix? "#/definitions/" r).map decodeToken

#guard refName? "#/definitions/a~01b" == some "a~1b"
#guard refName? "#/components/schemas/x~1y" == some "x/y"
#guard refName? "other.json#/X" == none

/-- One document's (canonicalised) components. -/
structure Side where
  comps : List (String × J)

def Side.target (s : Side) (name : String) : J := (s.comps.lookup name).getD .null

/-- `j` as a reference to a component this side declares: the name and the
siblings (every key but `$ref`). An undeclared or external reference is
`none`: it stays a literal object. -/
def Side.declRef? (s : Side) (j : J) : Option (String × List (String × J)) :=
  match j with
  | .obj kvs =>
    match kvs.lookup "$ref" with
    | some (.str r) =>
      match refName? r with
      | some n => if (s.comps.lookup n).isSome then some (n, kvs.filter (·.1 != "$ref")) else none
      | none => none
    | _ => none
  | _ => none

/-- An object without its `$ref` key. -/
def dropRef : J → J
  | .obj kvs => .obj (kvs.filter (·.1 != "$ref"))
  | j => j

/-- A target with siblings laid over it (siblings win). -/
def merge (t : J) (sibs : List (String × J)) : J :=
  if sibs.isEmpty then t else
  match t with
  | .obj tk => .obj ((tk.filter fun (k, _) => !(sibs.any (·.1 == k))) ++ sibs)
  | _ => .obj sibs

/-! ## Changes and the local walk -/

/-- A reported change. `retargeted` is the Review fact a reached pair of
differently named components gets. -/
inductive Ch where
  | added (v : J)
  | removed (v : J)
  | changed (a b : J)
  | retargeted (old new : String)
  deriving Inhabited, Repr

instance : BEq Ch where
  beq
    | .added a, .added b => a == b
    | .removed a, .removed b => a == b
    | .changed a b, .changed c d => a == c && b == d
    | .retargeted a b, .retargeted c d => a == c && b == d
    | _, _ => false

/-- The severity table's view of a change. -/
def Ch.kind : Ch → Kind
  | .added v => .added v.val
  | .removed v => .removed v.val
  | .changed a b => .changed a.val b.val
  | .retargeted _ _ => .changed .other .other

/-- A hop found by a local walk: where (in the walk's owner), the component
pair, and the keys siblings set there. -/
structure Hop where
  at_ : Pointer
  old : String
  new : String
  mask : List String
  deriving Inhabited, Repr

/-- What one local walk found. -/
structure Out where
  facts : Array (Pointer × Ch) := #[]
  hops : Array Hop := #[]

abbrev W := StateM Out

def emit (p : Pointer) (c : Ch) : W Unit :=
  modify fun o => { o with facts := o.facts.push (p, c) }

/-- `value::empty_counterpart`. -/
def emptyCounterpart (child : Node) (present : J) : Option J :=
  match child, present with
  | .set .requiredElement, .arr _ => some (.arr [])
  | .propertyMap, .obj _ => some (.obj [])
  | _, _ => none

/-- The pointer token of a `required`/`enum` member. -/
def setSegment : J → String
  | .str s => s
  | j => j.render

mutual

/-- Compares `o` and `n` at `p` (a `node` of the schema grammar), resolving
references by the three rules in the header. `chain` holds the value pairs
a one-sided expansion produced on the way here: the walk is a function of
the pair it compares, so meeting one again means the rest repeats
periodically (see `expand`; main's name-keyed cut instead compares a literal
`$ref` against an expansion). -/
partial def go (os ns : Side) (chain : List (Pointer × J × J)) (node : Node) (p : Pointer)
    (o n : J) : W Unit := do
  let ro := if node == .schema then os.declRef? o else none
  let rn := if node == .schema then ns.declRef? n else none
  match ro, rn with
  | some (x, so), some (y, sn) =>
    let mask := so.map (·.1) ++ (sn.map (·.1)).filter (fun k => !(so.any (·.1 == k)))
    let tO := os.target x
    let tN := ns.target y
    for k in mask do
      goOpt os ns chain (node.child k) (p ++ [k])
        (so.lookup k <|> tO.field? k) (sn.lookup k <|> tN.field? k)
    modify fun out => { out with hops := out.hops.push ⟨p, x, y, sortDedup mask⟩ }
  | some (x, so), none =>
    if n.isObj then
      let o' := merge (os.target x) so
      expand os ns chain node p o' n
    else plain os ns chain node p o n
  | none, some (y, sn) =>
    if o.isObj then
      let n' := merge (ns.target y) sn
      expand os ns chain node p o n'
    else plain os ns chain node p o n
  | none, none => plain os ns chain node p o n

/-- Continues after a one-sided expansion produced `(o, n)`. A pair met
again at the same pointer is an alias loop (`X = {"$ref": X, ...}`) that
expansion makes no progress on: its value is the fixpoint, the siblings
without the `$ref`. A pair met again deeper repeats periodically from there:
nothing more is reported. -/
partial def expand (os ns : Side) (chain : List (Pointer × J × J)) (node : Node) (p : Pointer)
    (o n : J) : W Unit :=
  match chain.find? fun (_, a, b) => a == o && b == n with
  | none => go os ns ((p, o, n) :: chain) node p o n
  | some (q, _, _) =>
    if q == p then
      plain os ns chain node p (if (os.declRef? o).isSome then dropRef o else o)
        (if (ns.declRef? n).isSome then dropRef n else n)
    else pure ()

/-- One key present on either side (`diff_nodes`' object case). -/
partial def goOpt (os ns : Side) (chain : List (Pointer × J × J)) (node : Node) (p : Pointer)
    (o n : Option J) : W Unit :=
  match o, n with
  | some a, some b => go os ns chain node p a b
  | some a, none =>
    match emptyCounterpart node a with
    | some e => go os ns chain node p a e
    | none => emit p (.removed a)
  | none, some b =>
    match emptyCounterpart node b with
    | some e => go os ns chain node p e b
    | none => emit p (.added b)
  | none, none => pure ()

/-- `diff_nodes` without references: objects key-wise, sets as sets, other
arrays by index, scalars by equality. -/
partial def plain (os ns : Side) (chain : List (Pointer × J × J)) (node : Node) (p : Pointer)
    (o n : J) : W Unit := do
  match o, n with
  | .obj ok, .obj nk =>
    for (k, ov) in ok do
      goOpt os ns chain (node.child k) (p ++ [k]) (some ov) (nk.lookup k)
    for (k, nv) in nk do
      if (ok.lookup k).isNone then
        goOpt os ns chain (node.child k) (p ++ [k]) none (some nv)
  | .arr xs, .arr ys =>
    match node with
    | .set _ =>
      for x in xs do
        if !ys.contains x then emit (p ++ [setSegment x]) (.removed x)
      for y in ys do
        if !xs.contains y then emit (p ++ [setSegment y]) (.added y)
    | _ =>
      let common := min xs.length ys.length
      for i in [0:common] do
        go os ns chain (node.child (toString i)) (p ++ [toString i]) xs[i]! ys[i]!
      for i in [common:xs.length] do
        emit (p ++ [toString i]) (.removed xs[i]!)
      for i in [common:ys.length] do
        emit (p ++ [toString i]) (.added ys[i]!)
  | _, _ => if o == n then pure () else emit p (.changed o n)

end

/-- `/…/required/name` → `/…/properties/name`. -/
def requiredMemberPointer (p : Pointer) : Pointer :=
  match p.reverse with
  | name :: _ :: rest => (name :: "properties" :: rest).reverse
  | _ => p

/-- `diff::value_changes`' rule: a `required` member removed together with
its property is not reported separately. Applied per comparison scope. -/
def suppress (fs : List (Pointer × Ch)) : List (Pointer × Ch) :=
  let removedProps := fs.filterMap fun (p, c) =>
    match c with
    | .removed _ => if locate p == .property then some p else none
    | _ => none
  fs.filter fun (p, c) =>
    match c with
    | .removed _ =>
      !(locate p == .requiredElement && removedProps.any (· == requiredMemberPointer p))
    | _ => true

/-- One local walk from the root of two schemas. -/
def walk (os ns : Side) (o n : J) : Out :=
  ((go os ns [] .schema [] o n).run {}).2

#guard
  let s : Side := ⟨[("A", .obj [("type", .str "string")])]⟩
  let t : Side := ⟨[("A", .obj [("type", .str "integer")])]⟩
  let r := .obj [("$ref", .str "#/components/schemas/A"), ("nullable", .bool true)]
  let out := walk s t r r
  -- The sibling is compared at the use (unchanged); the target via a hop.
  out.facts.size == 0 && out.hops.size == 1 && out.hops[0]!.mask == ["nullable"]

/-! ## Spec pairs: uses -/

inductive Dir where
  | request
  | response
  deriving DecidableEq, Repr, Inhabited

def Dir.name : Dir → String
  | .request => "request"
  | .response => "response"

/-- One schema use of a matched endpoint: `METHOD path` plus `param:IN:NAME`,
`body` or `response:STATUS`. -/
structure Use where
  id : String
  dir : Dir
  old : J
  new : J
  deriving Inhabited

def methods : List String := ["get", "put", "post", "delete", "options", "head", "patch", "trace"]

/-- An endpoint: method (upper case), path, operation, path-level parameters. -/
structure Endpoint where
  method : String
  path : String
  op : J
  pathParams : List J

def endpoints (doc : J) : List Endpoint :=
  match doc.field? "paths" with
  | some (.obj ps) =>
    ps.flatMap fun (path, item) =>
      let pp := match item.field? "parameters" with
        | some (.arr xs) => xs
        | _ => []
      methods.filterMap fun m =>
        (item.field? m).map fun op => ⟨m.toUpper, path, op, pp⟩
  | _ => []

def isOas2 (doc : J) : Bool := (doc.field? "swagger").isSome

/-- The document's components, canonicalised. -/
def side (doc : J) : Side :=
  let raw := if isOas2 doc then doc.field? "definitions"
    else (doc.field? "components").bind (·.field? "schemas")
  match raw with
  | some (.obj kvs) => ⟨kvs.map fun (k, v) => (k, canon v)⟩
  | _ => ⟨[]⟩

/-- Endpoint matching: exact `(METHOD, path)`. Move pairing (operationId,
`v<digits>` segments) only renames the use in Markdown and v2 output; the v1
records the harness compares never pair moves, so the model does not. -/
def matchEndpoints (old new : List Endpoint) : List (Endpoint × Endpoint) :=
  old.filterMap fun o => (new.find? fun n => n.method == o.method && n.path == o.path).map (o, ·)

/-- Operation parameters over path-level ones, by `(in, name)`. -/
def params (e : Endpoint) : List J :=
  let ops := match e.op.field? "parameters" with
    | some (.arr xs) => xs
    | _ => []
  let key (p : J) := (p.field? "in", p.field? "name")
  ops ++ e.pathParams.filter fun p => !(ops.any fun q => key q == key p)

/-- A non-body parameter's schema: OpenAPI 3 `schema`, or the Swagger 2
`type`/`format`/`items`/`enum` keys rebuilt into one. -/
def paramSchema (oas2 : Bool) (p : J) : J :=
  if oas2 then
    .obj (["type", "format", "items", "enum"].filterMap fun k => (p.field? k).map (k, ·))
  else (p.field? "schema").getD .null

/-- The first media type's schema of a `content` map. -/
def contentSchema (content : Option J) : J :=
  match content with
  | some (.obj ((_, media) :: _)) => (media.field? "schema").getD .null
  | _ => .null

def bodySchema (oas2 : Bool) (e : Endpoint) : J :=
  if oas2 then
    match (params e).find? (·.field? "in" == some (.str "body")) with
    | some p => (p.field? "schema").getD .null
    | none => .null
  else contentSchema ((e.op.field? "requestBody").bind (·.field? "content"))

def responseSchema (oas2 : Bool) (r : J) : J :=
  if oas2 then (r.field? "schema").getD .null else contentSchema (r.field? "content")

def strOf : Option J → String
  | some (.str s) => s
  | _ => ""

/-- Every schema use of every matched endpoint, canonicalised. -/
def uses (oldDoc newDoc : J) : List Use :=
  let (o2, n2) := (isOas2 oldDoc, isOas2 newDoc)
  (matchEndpoints (endpoints oldDoc) (endpoints newDoc)).flatMap fun (eo, en) =>
    let ep := s!"{en.method} {en.path}"
    let nonBody (oas2 : Bool) (e : Endpoint) :=
      let _ := oas2
      (params e).filter fun p => p.field? "in" != some (.str "body")
    let ps := (nonBody o2 eo).filterMap fun po =>
      (nonBody n2 en).find? (fun pn => pn.field? "in" == po.field? "in" && pn.field? "name" == po.field? "name")
        |>.map fun pn =>
          ({ id := s!"{ep} param:{strOf (po.field? "in")}:{strOf (po.field? "name")}"
             dir := .request
             old := canon (paramSchema o2 po)
             new := canon (paramSchema n2 pn) } : Use)
    let (bo, bn) := (bodySchema o2 eo, bodySchema n2 en)
    let body : List Use :=
      if bo.isNull || bn.isNull then []
      else [{ id := s!"{ep} body", dir := .request, old := canon bo, new := canon bn }]
    let resp : List Use :=
      match eo.op.field? "responses", en.op.field? "responses" with
      | some (.obj ro), some (.obj rn) =>
        ro.filterMap fun (status, r) =>
          (rn.lookup status).bind fun r' =>
            let (so, sn) := (responseSchema o2 r, responseSchema n2 r')
            if so.isNull && sn.isNull then none
            else some { id := s!"{ep} response:{status}", dir := .response,
                        old := canon so, new := canon sn }
      | _, _ => []
    ps.filter (fun u => !(u.old.isNull && u.new.isNull)) ++ body ++ resp

/-! ## Grouped view -/

/-- A hop state: the component pair, the keys siblings mask at the hop, and
whether the hop is under a `not`. -/
structure St where
  old : String
  new : String
  mask : List String
  neg : Bool
  deriving BEq, Inhabited, Repr

/-- Who a change belongs to. -/
inductive Owner where
  | use
  | pair (old new : String)
  deriving BEq, Inhabited, Repr

/-- One reported change, as seen from one use. -/
structure Rec where
  use : String
  dir : Dir
  owner : Owner
  ptr : Pointer
  ch : Ch
  neg : Bool
  deriving Inhabited

instance : BEq Rec where
  beq a b := a.use == b.use && a.dir == b.dir && a.owner == b.owner && a.ptr == b.ptr
    && a.ch == b.ch && a.neg == b.neg

/-- The verdict: Review under `not` and for retargets, otherwise the
direction's table on the owner-relative pointer. -/
def Rec.severity (r : Rec) : Severity :=
  match r.ch with
  | .retargeted _ _ => .review
  | c =>
    if r.neg then .review else
    match r.dir with
    | .request => requestSeverity r.ptr c.kind
    | .response => responseSeverity r.ptr c.kind

/-- Visible at a state: not under a masked top-level key. -/
def visible (mask : List String) (p : Pointer) : Bool :=
  match p with
  | [] => true
  | k :: _ => !mask.contains k

/-- A pair's content, compared once: the targets walked from their roots with
`required` suppression applied
over the pair's own comparison. -/
def pairContent (os ns : Side) (x y : String) : List (Pointer × Ch) × List Hop :=
  let out := walk os ns (os.target x) (ns.target y)
  (suppress out.facts.toList, out.hops.toList)

/-- The state a hop leads to from a state (or from the use: `mask = []`,
`neg = false`). A hop at the owner's root inherits the outer mask. -/
def next (mask : List String) (neg : Bool) (h : Hop) : St :=
  { old := h.old, new := h.new
    mask := if h.at_.isEmpty then sortDedup (mask ++ h.mask) else h.mask
    neg := neg || underNot h.at_ }

/-- Every state reachable from a use's own hops (breadth first, each state
once; there are finitely many). -/
partial def reach (os ns : Side) (todo : List St) (seen : List St) : List St :=
  match todo with
  | [] => seen.reverse
  | s :: rest =>
    if seen.contains s then reach os ns rest seen else
    let (_, hops) := pairContent os ns s.old s.new
    let nxt := (hops.filter (visible s.mask ·.at_)).map (next s.mask s.neg)
    reach os ns (rest ++ nxt) (s :: seen)

/-- Everything one use reaches, grouped by owner. -/
def groupedUse (os ns : Side) (u : Use) : List Rec :=
  let out := walk os ns u.old u.new
  let atUse := (suppress out.facts.toList).map fun (p, c) =>
    ({ use := u.id, dir := u.dir, owner := .use, ptr := p, ch := c, neg := false } : Rec)
  let states := reach os ns (out.hops.toList.map (next [] false)) []
  let fromPairs := states.flatMap fun s =>
    let (facts, _) := pairContent os ns s.old s.new
    let own := Owner.pair s.old s.new
    let vis := (facts.filter (visible s.mask ·.1)).map fun (p, c) =>
      ({ use := u.id, dir := u.dir, owner := own, ptr := p, ch := c, neg := s.neg } : Rec)
    if s.old == s.new then vis
    else { use := u.id, dir := u.dir, owner := own, ptr := [], ch := .retargeted s.old s.new,
           neg := s.neg } :: vis
  (atUse ++ fromPairs).eraseDups

/-- The grouped view of a spec pair. -/
def grouped (oldDoc newDoc : J) : List Rec :=
  let (os, ns) := (side oldDoc, side newDoc)
  (uses oldDoc newDoc).flatMap (groupedUse os ns)

/-! ## Per-path view -/

/-- Every pair's content, computed once per spec pair and looked up by name. -/
abbrev Contents := List ((String × String) × (List (Pointer × Ch) × List Hop))

/-- The contents of every pair reachable from `states`. Unsuppressed: the
per-path view applies `required` suppression once, over the whole tree. -/
def contents (os ns : Side) (states : List St) : Contents :=
  (states.map fun s => (s.old, s.new)).eraseDups.map fun (x, y) =>
    let out := walk os ns (os.target x) (ns.target y)
    ((x, y), (out.facts.toList, out.hops.toList))

/-- Per-path changes at one use: the lockstep unfolding, absolute pointers,
cut where a state repeats on its own path (beneath a repeat every change has
already been reported at a shallower position relative to the same pair).
`fuel` bounds the states visited and the changes collected; `none` when it
runs out (dense cycles have exponentially many paths, #152). -/
partial def unfold (cs : Contents) (pre : Pointer) (s : St) (stack : List St)
    (acc : Array (Pointer × Ch)) (fuel : Nat) : Option (Array (Pointer × Ch) × Nat) := do
  if stack.contains s then return (acc, fuel)
  let (facts, hops) := (cs.lookup (s.old, s.new)).getD ([], [])
  let mut acc := acc
  for (p, c) in facts do
    if visible s.mask p then acc := acc.push (pre ++ p, c)
  if fuel ≤ acc.size then none
  let mut fuel := fuel - 1
  for h in hops do
    if visible s.mask h.at_ then
      let (acc', fuel') ← unfold cs (pre ++ h.at_) (next s.mask s.neg h) (s :: stack) acc fuel
      acc := acc'
      fuel := fuel'
  return (acc, fuel)

/-- The per-path view of one use, with `required` suppression over the whole
resolved tree as `value_changes` applies it; `none` past the budget. -/
def perPathUse (os ns : Side) (budget : Nat) (u : Use) : Option (List (Pointer × Ch)) := do
  let out := walk os ns u.old u.new
  let cs := contents os ns (reach os ns (out.hops.toList.map (next [] false)) [])
  let mut acc := out.facts
  let mut fuel := budget
  for h in out.hops do
    let (acc', fuel') ← unfold cs h.at_ (next [] false h) [] acc fuel
    acc := acc'
    fuel := fuel'
  return suppress acc.toList |>.eraseDups

/-! ## Sanity checks -/

private def ref (n : String) : J := .obj [("$ref", .str ("#/components/schemas/" ++ n))]
private def obj1 (k : String) (v : J) : J := .obj [(k, v)]
private def props (kvs : List (String × J)) : J := .obj [("type", .str "object"), ("properties", .obj kvs)]
private def oas3 (comps : List (String × J)) (resp : J) : J :=
  .obj [("openapi", .str "3.0.0"),
    ("paths", obj1 "/a" (obj1 "get" (obj1 "responses" (obj1 "200"
      (obj1 "content" (obj1 "application/json" (obj1 "schema" resp))))))),
    ("components", obj1 "schemas" (.obj comps))]

-- A change inside a component reached twice is one grouped record, at the
-- pair, relative pointer.
#guard
  let r := props [("a", ref "T"), ("b", ref "T")]
  let g := grouped (oas3 [("T", props [("x", obj1 "type" (.str "string"))])] r)
    (oas3 [("T", props [("x", obj1 "type" (.str "integer"))])] r)
  g.length == 1 && g[0]!.owner == .pair "T" "T" && g[0]!.ptr == ["properties", "x", "type"]
    && g[0]!.severity == .breaking

-- A self-reference terminates and reports once.
#guard
  let t (ty : String) := props [("self", ref "T"), ("x", obj1 "type" (.str ty))]
  let g := grouped (oas3 [("T", t "string")] (ref "T")) (oas3 [("T", t "boolean")] (ref "T"))
  g.length == 1

-- A sibling overriding `type` masks the component's `type` change at that use.
#guard
  let pinned := .obj [("$ref", .str "#/definitions/T"), ("type", .str "object")]
  let g := grouped (oas3 [("T", obj1 "type" (.str "object"))] pinned)
    (oas3 [("T", obj1 "type" (.str "array"))] pinned)
  g.isEmpty

-- A retarget to an identical component is one Review fact.
#guard
  let same := props [("x", obj1 "type" (.str "string"))]
  let g := grouped (oas3 [("P", same), ("D", same)] (ref "P")) (oas3 [("P", same), ("D", same)] (ref "D"))
  g.length == 1 && g[0]!.severity == .review

-- Under `not`, a component's enum narrowing is Review, as it is inline.
#guard
  let r := obj1 "not" (ref "E")
  let g := grouped (oas3 [("E", obj1 "enum" (.arr [.str "a", .str "b"]))] r)
    (oas3 [("E", obj1 "enum" (.arr [.str "a"]))] r)
  g.length == 1 && g[0]!.neg && g[0]!.severity == .review

end Vimanam.RefDiff
