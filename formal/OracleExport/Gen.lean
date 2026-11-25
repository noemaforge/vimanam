/-
# The differential oracle's input generator and vector writer

Not a model: the support code of `lake exe oracle` (see `OracleExport/Main.lean`
and formal/README.md, "Differential oracle"). Nothing under `VimanamProofs/`
imports it.

* `Gen` is a deterministic pseudo-random generator: SplitMix64 over a
  `UInt64` state, one fixed seed per model, so the vectors are a pure
  function of the Lean sources and regenerate byte for byte.
* `Vectors` is one model's vector file: metadata, then one JSON array of cases
  per modelled function, written one case per line, so a diff names the cases
  that changed.
-/
import Lean.Data.Json

namespace Vimanam.Oracle

open Lean

/-! ## SplitMix64 -/

/-- The generator: SplitMix64's state. -/
abbrev Gen := StateM UInt64

/-- The next 64 random bits (SplitMix64, Steele, Lea and Flood 2014). -/
def nextU64 : Gen UInt64 := do
  let s := (← get) + 0x9E3779B97F4A7C15
  set s
  let z := (s ^^^ (s >>> 30)) * 0xBF58476D1CE4E5B9
  let z := (z ^^^ (z >>> 27)) * 0x94D049BB133111EB
  return z ^^^ (z >>> 31)

/-- A number in `[0, n)`; `0` when `n = 0`. -/
def below (n : Nat) : Gen Nat := do
  if n = 0 then return 0
  return (← nextU64).toNat % n

/-- One element of a nonempty list. -/
def pick {α : Type} [Inhabited α] (xs : List α) : Gen α := do
  return xs.getD (← below xs.length) default

/-- `n` draws. -/
def many {α : Type} (n : Nat) (g : Gen α) : Gen (List α) := do
  let mut out := #[]
  for _ in [0:n] do
    out := out.push (← g)
  return out.toList

/-- Run a generator from a seed. -/
def run {α : Type} (seed : UInt64) (g : Gen α) : α := (g.run seed).1

/-! ## Vector files -/

/-- One modelled function's cases: the Lean name, the Rust it is compared
with, and the cases (`{"input": .., "output": ..}` objects). -/
structure Fn where
  lean : String
  rust : String
  cases : List Json

/-- One model's vector file. -/
structure Vectors where
  model : String
  seed : UInt64
  /-- How the model's abstractions were instantiated, and any restriction on
  the inputs; free text, for people. -/
  notes : List String
  fns : List Fn

/-- The file `formal/oracle/<model>.json`: a fixed key order, one case per
line, a trailing newline. -/
def Vectors.render (v : Vectors) : String := Id.run do
  let q (s : String) : String := (toJson s).compress
  let mut out := "{\n"
  out := out ++ s!"\"model\": {q v.model},\n"
  out := out ++ s!"\"lean\": {q s!"formal/VimanamProofs/{v.model}.lean"},\n"
  out := out ++ s!"\"generator\": {q "formal/OracleExport/Main.lean"},\n"
  out := out ++ s!"\"seed\": {v.seed.toNat},\n"
  out := out ++ "\"notes\": [\n"
  out := out ++ ",\n".intercalate (v.notes.map q) ++ "\n],\n"
  out := out ++ "\"functions\": [\n"
  let fns := v.fns.map fun f =>
    "{\"lean\": " ++ q f.lean ++ ", \"rust\": " ++ q f.rust ++ ", \"cases\": [\n" ++
      ",\n".intercalate (f.cases.map Json.compress) ++ "\n]}"
  out := out ++ ",\n".intercalate fns ++ "\n]\n}\n"
  return out

/-- A case object. -/
def case (input output : Json) : Json :=
  Json.mkObj [("input", input), ("output", output)]

end Vimanam.Oracle
