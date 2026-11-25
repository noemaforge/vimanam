/-
# `lake exe refdiff`: evaluate the `$ref` diff reference model on spec pairs

    lake exe refdiff OLD.json NEW.json          # model output on stdout
    lake exe refdiff --batch LIST               # LIST: lines "OLD<TAB>NEW<TAB>OUT"

Prints, per spec pair, the grouped view (`RefDiff.grouped`) and the per-path
view (`RefDiff.perPathUse`) as JSON. `scripts/formal/refdiff-harness.py`
normalises this and the Rust's `diff --format json --contract 1` output to the
same records and lists disagreements. See formal/README.md, "Reference
semantics of `$ref` diffs".

Not a model: input and output only. `Lean.Json` sorts object keys, which
the model only observes as "first media type".
-/
import Lean.Data.Json
import VimanamProofs.RefDiff

open Lean
open Vimanam.RefDiff
open Vimanam.Severity (Severity Pointer)

partial def ofJson : Json → J
  | .null => .null
  | .bool b => .bool b
  | .num n => .num n.toString
  | .str s => .str s
  | .arr xs => .arr (xs.toList.map ofJson)
  | .obj kvs => .obj (kvs.foldl (fun acc k v => acc ++ [(k, ofJson v)]) [])

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
    ("uses", .arr (us.map fun u => .obj [("id", .str u.id), ("dir", .str u.dir.name)])),
    ("grouped", .arr ((us.flatMap (groupedUse os ns)).map recJson)),
    ("per_path_truncated", .arr (perPath.filterMap fun (u, r) => if r.isNone then some (.str u.id) else none)),
    ("per_path", .arr (perPath.flatMap fun (u, r) => (r.getD []).map fun (p, c) => pathJson u p c))]

def load (path : String) : IO J := do
  match Json.parse (← IO.FS.readFile path) with
  | .ok j => return ofJson j
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
