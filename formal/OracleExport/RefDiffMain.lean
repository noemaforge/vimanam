/-
# `lake exe refdiff`: evaluate the `$ref` diff reference model on spec pairs

    lake exe refdiff OLD.json NEW.json          # model output on stdout
    lake exe refdiff --batch LIST               # LIST: lines "OLD<TAB>NEW<TAB>OUT"

Prints, per spec pair, the uses (with their deserialised, canonicalised
schemas), both documents' components, the grouped view (`RefDiff.grouped`)
and the per-path view (`RefDiff.perPathUse`, with its cut pointers) as JSON. `scripts/formal/refdiff-harness.py`
normalises this and the Rust's `diff --format json --contract 1` output to the
same records and lists disagreements. See formal/README.md, "Reference
semantics of `$ref` diffs".

Not a model: input and output only. Documents are read in spec order
(`Lean.Json` would sort object keys), as `serde_json` reads them with
`preserve_order`, so the model sees media types (#154) and every other
object in the order the spec writes them.
-/
import Lean.Data.Json
import VimanamProofs.RefDiff

open Lean
open Vimanam.RefDiff
open Vimanam.Severity (Severity Pointer)

namespace Ordered

open Std.Internal.Parsec Std.Internal.Parsec.String

/-! `Lean.Json.Parser`'s grammar, building `J` with object keys in document
order. Strings and numbers use `Lean.Json.Parser`'s own parsers, and a number
is the text `Lean.Json` renders for it, as before. A duplicate key is an
error, as there. -/

mutual
  partial def arrayCore (acc : Array J) : Parser (Array J) := do
    let hd ← anyCore
    let acc := acc.push hd
    let c ← any
    if c == ']' then ws; return acc
    else if c == ',' then ws; arrayCore acc
    else fail "unexpected character in array"

  partial def objectCore (acc : Array (String × J)) : Parser (Array (String × J)) := do
    let c ← peek!
    if c != '"' then fail "expected \""
    skip
    let k ← Lean.Json.Parser.str
    ws
    if acc.any (·.1 == k) then fail s!"duplicate object key {k.quote}"
    let c ← peek!
    if c != ':' then fail "expected :"
    skip; ws
    let v ← anyCore
    let acc := acc.push (k, v)
    let c ← any
    if c == '}' then ws; return acc
    else if c == ',' then ws; objectCore acc
    else fail "unexpected character in object"

  partial def anyCore : Parser J := do
    let c ← peek!
    if c == '[' then
      skip; ws
      if (← peek!) == ']' then skip; ws; return .arr []
      return .arr (← arrayCore #[]).toList
    else if c == '{' then
      skip; ws
      if (← peek!) == '}' then skip; ws; return .obj []
      return .obj (← objectCore #[]).toList
    else if c == '"' then
      skip
      let s ← Lean.Json.Parser.str
      ws
      return .str s
    else if c == 'f' then skipString "false"; ws; return .bool false
    else if c == 't' then skipString "true"; ws; return .bool true
    else if c == 'n' then skipString "null"; ws; return .null
    else if c == '-' || ('0' <= c && c <= '9') then
      let n ← Lean.Json.Parser.num
      ws
      return .num n.toString
    else fail "unexpected input"
end

def document : Parser J := do
  ws
  let j ← anyCore
  eof
  return j

def parse (s : String) : Except String J := Parser.run document s

end Ordered

def sevName : Severity → String
  | .breaking => "breaking"
  | .nonBreaking => "non_breaking"
  | .review => "review"

def ptrText (p : Pointer) : String := String.join (p.map fun t => "/" ++ encodeToken t)

def chJson : Ch → J
  | .added v => .obj [("op", .str "added"), ("after", v)]
  | .removed v => .obj [("op", .str "removed"), ("before", v)]
  | .changed a b => .obj [("op", .str "changed"), ("before", a), ("after", b)]
  | .retargeted o n => .obj [("op", .str "retargeted"), ("before", .str o), ("after", .str n)]

def ownerJson : Owner → J
  | .use => .null
  | .pair o n => .obj [("old", .str o), ("new", .str n)]

def recJson (r : Rec) : J :=
  .obj [("use", .str r.use), ("dir", .str r.dir.name), ("owner", ownerJson r.owner),
    ("ptr", .str (ptrText r.ptr)), ("change", chJson r.ch), ("neg", .bool r.neg),
    ("severity", .str (sevName r.severity))]

/-- Per-path records carry the use's direction table on the absolute pointer. -/
def pathJson (u : Use) (p : Pointer) (c : Ch) : J :=
  let r : Rec := { use := u.id, dir := u.dir, owner := .use, ptr := p, ch := c, neg := false }
  .obj [("use", .str u.id), ("ptr", .str (ptrText p)), ("change", chJson c),
    ("severity", .str (sevName r.severity))]

/-- Per-path changes per use are capped; a capped use is listed as truncated. -/
def budget : Nat := 20000

def evaluate (oldDoc newDoc : J) : J :=
  let (os, ns) := (side oldDoc, side newDoc)
  let us := uses oldDoc newDoc
  let perPath := us.map fun u => (u, perPathUse os ns budget u)
  .obj [
    ("uses", .arr (us.map fun u => .obj [("id", .str u.id), ("dir", .str u.dir.name),
      ("old", u.old), ("new", u.new)])),
    ("components", .obj [("old", .obj os.comps), ("new", .obj ns.comps)]),
    ("grouped", .arr ((us.flatMap (groupedUse os ns)).map recJson)),
    ("media", .arr ((mediaChanges oldDoc newDoc).map fun r =>
      .obj [("use", .str r.use), ("dir", .str r.dir.name), ("media_type", .str r.mediaType),
        ("op", .str (if r.added then "added" else "removed")),
        ("severity", .str (sevName r.severity))])),
    ("per_path_truncated", .arr (perPath.filterMap fun (u, r) => if r.isNone then some (.str u.id) else none)),
    ("per_path", .arr (perPath.flatMap fun (u, r) => ((r.map (·.1)).getD []).map fun (p, c) => pathJson u p c)),
  ("per_path_cuts", .arr (perPath.flatMap fun (u, r) => ((r.map (·.2)).getD []).map fun p =>
    .obj [("use", .str u.id), ("ptr", .str (ptrText p))]))]

def load (path : String) : IO J := do
  match Ordered.parse (← IO.FS.readFile path) with
  | .ok j => return j
  | .error e => throw (IO.userError s!"{path}: {e}")

def runOne (old new : String) : IO String := do
  return (evaluate (← load old) (← load new)).render

def main (args : List String) : IO UInt32 := do
  match args with
  | ["--batch", list] =>
    for line in (← IO.FS.lines list) do
      match line.splitOn "\t" with
      | [old, new, out] => IO.FS.writeFile out ((← runOne old new) ++ "\n")
      | [""] => pure ()
      | _ => throw (IO.userError s!"bad batch line: {line}")
    return 0
  | [old, new] =>
    IO.println (← runOne old new)
    return 0
  | _ =>
    IO.eprintln "usage: lake exe refdiff OLD NEW | lake exe refdiff --batch LIST"
    return 2
