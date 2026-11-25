/-
# `lake exe oracle`: write the differential-oracle vectors

Evaluates Lean model definitions over generated inputs and writes one JSON
file per model to `formal/oracle/` (or to the directory given as the only
argument). The Rust test in `src/diff/formal_oracle.rs` replays the inputs
through the real Rust and asserts the same outputs. See formal/README.md,
"Differential oracle".
-/
import OracleExport.Gen
import OracleExport.Severity

open Vimanam.Oracle

def allVectors : List Vectors := [
  Severity.vectors
]

def main (args : List String) : IO UInt32 := do
  if args.length > 1 then
    IO.eprintln "usage: lake exe oracle [OUT_DIR]"
    return 2
  let dir : System.FilePath :=
    match args with
    | [d] => d
    | _ => "oracle"
  IO.FS.createDirAll dir
  for v in allVectors do
    let path := dir / s!"{v.model}.json"
    IO.FS.writeFile path v.render
    IO.println s!"wrote {path}"
  return 0
