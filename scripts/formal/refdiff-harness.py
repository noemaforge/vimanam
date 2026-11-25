#!/usr/bin/env python3
"""Differential harness: the Lean `$ref` diff reference model against the Rust.

Runs `lake exe refdiff` (formal/VimanamProofs/RefDiff.lean) and one or two
`vimanam` binaries on the same OpenAPI spec pairs, normalises both outputs to
the same records, and writes a report of every disagreement, classified, with
a shrunk repro per disagreement class.

    python3 -I scripts/formal/refdiff-harness.py \\
        --rust target/release/vimanam \\
        [--per-path path/to/aa6b1ab/target/release/vimanam] \\
        --seeds 300 --out SCRATCH_DIR

Inputs: the fixed cases in formal/refdiff/cases/ (repros from #182's review
rounds) and `--seeds` generated pairs (cycles, siblings over refs, retargets,
aliases, one-sided components, mixed ref spellings, refs under `not`).

Comparisons (see formal/README.md, "Reference semantics of `$ref` diffs"):

* grouped: the model's grouped records against the `--rust` binary's v1
  (main since #182 merged; `--pr` is an alias)
  records (`diff --format json --contract 1`: one record per (use, change),
  with `component`/`old_component` for ref-reached changes). Unit:
  (use, owner, owner-relative pointer, change, severity).
* per-path: the model's per-path records against an optional `--per-path`
  binary built from before #182 (aa6b1ab or earlier; `--main` is an alias),
  whose v1 records have absolute pointers and no owners. That Rust's own
  artifacts (spelling- and side-keyed cycle cuts, the 64-hop cap) show up
  here; they are classified, not shrunk.

Values are normalised before comparing: an object or array is compared only
as "an object"/"an array" (how much of it is expanded is an implementation
detail; the severity table only sees scalars), and a local `$ref` string is
compared by component name.

No third-party dependencies; standard library only.
"""

from __future__ import annotations

import argparse
import copy
import json
import os
import random
import re
import shutil
import subprocess
import sys
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FORMAL = ROOT / "formal"
CASES = FORMAL / "refdiff" / "cases"
ALLOWLIST = FORMAL / "refdiff" / "allowlist.json"

# ── generator ───────────────────────────────────────────────────────────────

NAMES = ["A", "B", "C", "Page", "Item", "a~1b", "x/y"]


def enc(name: str) -> str:
    return name.replace("~", "~0").replace("/", "~1")


class Gen:
    """Small spec pairs that stress `$ref` handling. Adapted from the #182
    reviewers' gen.py, without the 70-hop chain (main's 64-hop cap is a known
    artifact) and with semantic siblings, aliases and `not`."""

    def __init__(self, seed: int):
        self.r = random.Random(seed)
        self.oas2 = self.r.random() < 0.25
        # OpenAPI 3.1 `type` arrays draw from their own stream, so adding
        # them left every other seed's documents as they were.
        self.r31 = random.Random(seed * 7919 + 1)
        self.v31 = not self.oas2 and self.r31.random() < 0.15

    def ref(self, name: str, siblings: bool = True) -> dict:
        r = self.r
        base = "#/definitions/" if (self.oas2 or r.random() < 0.2) else "#/components/schemas/"
        if r.random() < 0.04:
            return {"$ref": "#/components/schemas/Missing"}
        out: dict = {"$ref": base + enc(name)}
        if siblings:
            if r.random() < 0.15:
                out["description"] = "sib"
            if r.random() < 0.12:
                out["type"] = r.choice(["object", "string", "array"])
            if r.random() < 0.1:
                out["nullable"] = r.choice([True, False])
            if r.random() < 0.06:
                out["properties"] = {"s": {"type": "string"}}
            if r.random() < 0.05:
                out["required"] = ["s"]
        return out

    def leaf(self) -> dict:
        r = self.r
        s: dict = {"type": r.choice(["string", "integer", "boolean"])}
        if r.random() < 0.25:
            s["enum"] = r.sample(["p", "q", "r", "s"], 2)
        if r.random() < 0.15:
            s["nullable"] = True
        if self.v31 and self.r31.random() < 0.4:
            s["type"] = [s["type"], "null"] if self.r31.random() < 0.7 else [s["type"]]
        return s

    def schema(self, depth: int) -> dict:
        r = self.r
        k = r.random()
        if depth > 2 or k < 0.2:
            return self.leaf()
        if k < 0.5:
            return self.ref(r.choice(NAMES))
        if k < 0.6:
            return {"type": "array", "items": self.schema(depth + 1)}
        if k < 0.65:
            return {"not": self.ref(r.choice(NAMES), siblings=False)}
        props = {
            p: self.schema(depth + 1)
            for p in r.sample(["data", "next", "id", "meta", "items", "type"], r.randint(1, 3))
        }
        s = {"type": "object", "properties": props}
        if r.random() < 0.35:
            s["required"] = r.sample(list(props), 1)
        if r.random() < 0.1:
            s["allOf"] = [self.ref(r.choice(NAMES))]
        return s

    def mutate(self, s, rate: float):
        r = self.r
        s = copy.deepcopy(s)

        def walk(v):
            if isinstance(v, dict):
                x = r.random()
                if x < rate and isinstance(v.get("type"), str):
                    v["type"] = r.choice(["string", "integer", "boolean", "object"])
                elif isinstance(v.get("type"), list) and self.r31.random() < rate:
                    t = v["type"]
                    v["type"] = [m for m in t if m != "null"] if "null" in t else t + ["null"]
                elif x < rate * 1.5 and "enum" in v:
                    v["enum"] = v["enum"][:1] if len(v["enum"]) > 1 else v["enum"] + ["z"]
                elif x < rate * 2 and "properties" in v and v["properties"]:
                    v["properties"].pop(r.choice(list(v["properties"])))
                elif x < rate * 2.5 and "$ref" in v:
                    if r.random() < 0.5:
                        v["$ref"] = v["$ref"].replace("components/schemas", "definitions")
                    else:  # retarget
                        v["$ref"] = "#/components/schemas/" + enc(r.choice(NAMES))
                elif x < rate * 3 and "$ref" in v:
                    v["nullable"] = not v.get("nullable", False)
                elif x < rate * 3.3 and "required" in v:
                    del v["required"]
                for c in list(v.values()):
                    walk(c)
            elif isinstance(v, list):
                for c in v:
                    walk(c)

        walk(s)
        return s

    def doc(self, comps, paths):
        if self.oas2:
            return {"swagger": "2.0", "info": {"title": "t", "version": "1"}, "paths": paths, "definitions": comps}
        return {"openapi": "3.1.0" if self.v31 else "3.0.0", "info": {"title": "t", "version": "1"},
                "paths": paths, "components": {"schemas": comps}}

    def op(self, i: int):
        r = self.r
        resp = self.schema(0) if r.random() < 0.5 else self.ref(r.choice(NAMES))
        req = self.schema(0) if r.random() < 0.6 else self.ref(r.choice(NAMES))
        if self.oas2:
            params = [{"name": "body", "in": "body", "schema": req}]
            return {"operationId": f"op{i}", "parameters": params,
                    "responses": {"200": {"description": "ok", "schema": resp}}}
        params = []
        if r.random() < 0.4:
            params.append({"name": "f", "in": "query", "schema": self.ref(r.choice(NAMES))})
        out = {"operationId": f"op{i}", "responses": {"200": {"description": "ok",
               "content": {"application/json": {"schema": resp}}}}}
        if params:
            out["parameters"] = params
        if r.random() < 0.7:
            out["requestBody"] = {"content": {"application/json": {"schema": req}}}
        return out

    def pair(self):
        r = self.r
        comps = {}
        for n in NAMES:
            if r.random() < 0.85:
                comps[n] = self.schema(0)
        if r.random() < 0.3 and len(comps) >= 2:  # alias
            a, b = r.sample(list(comps), 2)
            comps[a] = self.ref(b)
        paths = {f"/p{i}": {"post": self.op(i)} for i in range(r.randint(2, 4))}
        new_comps = {n: (self.mutate(s, 0.2) if r.random() < 0.4 else copy.deepcopy(s)) for n, s in comps.items()}
        if r.random() < 0.25 and new_comps:  # one-sided component
            del new_comps[r.choice(list(new_comps))]
        if r.random() < 0.15:
            new_comps["New"] = {"type": "string"}
        new_paths = {p: (self.mutate(v, 0.1) if r.random() < 0.3 else copy.deepcopy(v)) for p, v in paths.items()}
        if r.random() < 0.2:
            new_paths["/v2/p0"] = new_paths.pop("/p0")
        return self.doc(comps, paths), self.doc(new_comps, new_paths)


# ── running ─────────────────────────────────────────────────────────────────


def build_model() -> Path:
    subprocess.run(["lake", "build", "refdiff"], cwd=FORMAL, check=True, stdout=subprocess.DEVNULL)
    return FORMAL / ".lake" / "build" / "bin" / "refdiff"


# Seconds for one `refdiff --batch` run over every pair; a model that does not
# terminate fails the run here instead of hanging it.
MODEL_TIMEOUT = 300


def run_model(exe: Path, pairs: list[tuple[Path, Path]], work: Path) -> list[dict]:
    listing = work / "batch.tsv"
    outs = [work / f"model-{i}.json" for i in range(len(pairs))]
    listing.write_text("".join(f"{o}\t{n}\t{out}\n" for (o, n), out in zip(pairs, outs)))
    try:
        subprocess.run([str(exe), "--batch", str(listing)], check=True, timeout=MODEL_TIMEOUT)
    except subprocess.TimeoutExpired:
        sys.exit(f"error: the reference model did not finish {len(pairs)} pairs in {MODEL_TIMEOUT}s")
    return [json.loads(p.read_text()) for p in outs]


def run_rust(exe: str, old: Path, new: Path, timeout: float = 20) -> dict | None:
    try:
        proc = subprocess.run([exe, "diff", str(old), str(new), "--format", "json", "--contract", "1"],
                              capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return {"error": f"timeout after {timeout}s"}
    if proc.returncode not in (0, 3):
        return {"error": proc.stderr.strip()[:500]}
    return json.loads(proc.stdout)


# ── normalisation ───────────────────────────────────────────────────────────

REF_PREFIXES = ("#/components/schemas/", "#/definitions/")


def norm_value(v):
    if isinstance(v, dict):
        return "{object}"
    if isinstance(v, list):
        return "[array]"
    if isinstance(v, str):
        for pre in REF_PREFIXES:
            if v.startswith(pre):
                name = v[len(pre):].replace("~1", "/").replace("~0", "~")
                return f"ref:{name}"
    return json.dumps(v, sort_keys=True)


def norm_change(op: str, before, after) -> str:
    # v1 writes a schema that is absent (`null`) on one side as added/removed.
    if op == "changed" and before is None:
        op = "added"
    elif op == "changed" and after is None:
        op = "removed"
    if op == "added":
        return f"added {norm_value(after)}"
    if op == "removed":
        return f"removed {norm_value(before)}"
    if op == "retargeted":
        return f"retargeted {before} -> {after}"
    return f"changed {norm_value(before)} -> {norm_value(after)}"


def model_grouped(m: dict) -> tuple[Counter, set[tuple]]:
    """Records as a multiset of tuples, and those reached under `not`."""
    out: Counter = Counter()
    neg = set()
    for r in m["grouped"]:
        o = r["owner"]
        owner = "use" if o is None else f"{o['old']}->{o['new']}"
        c = r["change"]
        rec = (r["use"], owner, r["ptr"], norm_change(c["op"], c.get("before"), c.get("after")), r["severity"])
        out[rec] += 1
        if r["neg"]:
            neg.add(rec)
    return out, neg


def under(ptr: str, cut: str) -> bool:
    """Whether `ptr` is at or beneath `cut` (both JSON pointers)."""
    return ptr == cut or ptr.startswith(cut + "/")


def model_per_path(m: dict) -> Counter:
    out: Counter = Counter()
    for r in m["per_path"]:
        c = r["change"]
        out[(r["use"], r["ptr"], norm_change(c["op"], c.get("before"), c.get("after")), r["severity"])] += 1
    return out


def rust_use(rec: dict) -> str | None:
    ep = f"{rec['endpoint']['method']} {rec['endpoint']['path']}"
    d = rec["details"]
    kind = rec["kind"]
    if kind == "parameter_schema_changed":
        return f"{ep} param:{d.get('location')}:{d.get('name')}"
    if kind == "request_schema_changed":
        return f"{ep} body"
    if kind == "response_schema_changed":
        return f"{ep} response:{d.get('status')}"
    return None


def rust_records(rust: dict, grouped: bool) -> tuple[Counter, set[tuple]]:
    """Records as a multiset of tuples, and those Rust marks `under_not`."""
    out: Counter = Counter()
    neg = set()
    for rec in rust.get("changes", []):
        use = rust_use(rec)
        if use is None:
            continue
        d = rec["details"]
        sc = d["schema_change"]
        before = sc["before"].get("value") if sc["before"]["present"] else None
        after = sc["after"].get("value") if sc["after"]["present"] else None
        change = norm_change(sc["operation"], before, after)
        if grouped:
            comp = d.get("component")
            old_comp = d.get("old_component", comp)
            owner = "use" if comp is None else f"{old_comp}->{comp}"
            pointer = sc["pointer"]
            # #182 writes the retarget fact as a change of the component names
            # at `/$ref`; the model's record for it is `retargeted X -> Y` at
            # the pair root.
            if (comp is not None and old_comp != comp and pointer == "/$ref"
                    and sc["operation"] == "changed" and (before, after) == (old_comp, comp)):
                pointer, change = "", f"retargeted {old_comp} -> {comp}"
            t = (use, owner, pointer, change, rec["severity"])
            out[t] += 1
            if d.get("under_not"):
                neg.add(t)
        else:
            out[(use, sc["pointer"], change, rec["severity"])] += 1
    return out, neg


# ── classification ──────────────────────────────────────────────────────────


def suffix_of(longer: str, shorter: str) -> bool:
    """`shorter` is a non-empty pointer suffix of `longer` (an empty pointer
    would match everything)."""
    return shorter != "" and longer.endswith(shorter)


def classify_grouped(model: Counter, model_neg: set[tuple], rust: Counter,
                     rust_neg: set[tuple], q1_for=None) -> list[tuple[str, tuple | None, tuple | None]]:
    """Pairs up model-only and Rust-only records (as multisets) and names
    each difference."""
    only_m = sorted((model - rust).elements())
    only_r = sorted((rust - model).elements())
    found = []
    used_r: Counter = Counter()

    def free(r):
        return used_r[r] < (rust - model)[r]

    for m in only_m:
        use, owner, ptr, change, sev = m
        if change.startswith("retargeted"):
            found.append(("retarget-fact-missing", m, None))
            continue
        # Same record, different verdict.
        same = [r for r in only_r if free(r) and r[:4] == m[:4]]
        if same:
            r = same[0]
            used_r[r] += 1
            kind = "verdict-under-not" if m in model_neg else "verdict"
            found.append((kind, m, r))
            continue
        # Reached both through a `not` and without: Rust has one record, with
        # the plain verdict, where the model has a Review one as well.
        if m in model_neg and any(r[:4] == m[:4] for r in rust):
            found.append(("verdict-under-not-merged", m, next(r for r in rust if r[:4] == m[:4])))
            continue
        # Same change at the same use, reported by a different owner: one
        # pointer is the other with the owner's prefix.
        moved = [r for r in only_r if free(r) and r[0] == use and r[3] == change
                 and (suffix_of(r[2], ptr) or suffix_of(ptr, r[2]))]
        if moved:
            r = moved[0]
            used_r[r] += 1
            if owner == "use" or r[1] == "use":
                kind = "attribution-use-vs-component"
            else:
                kind = "attribution-component-pair"
            found.append((kind, m, r))
            continue
        found.append(("missing-in-rust" + ("-masked-reach" if owner != "use" else "-at-use"), m, None))
    # Records both sides report, where only one side says "reached under
    # `not`".
    for r in sorted(model & rust):
        if r in rust_neg and r not in model_neg:
            found.append(("under-not-flag-extra-in-rust", r, r))
        elif r in model_neg and r not in rust_neg:
            found.append(("under-not-flag-missing-in-rust", r, r))
    for r in only_r:
        if used_r[r] > 0:
            used_r[r] -= 1
            continue
        found.append(("extra-in-rust" + ("-component" if r[1] != "use" else "-at-use"), None, r))
    return name_cuts(found, rust, q1_for(found) if q1_for else (lambda rec: True))


def name_cuts(found: list, rust: Counter, is_q1) -> list:
    """Renames the disagreements a component-name cut of one-sided expansion
    explains (#187). #182's `expand_once` stops when the component is already on its
    side's expansion stack and compares the literal `{"$ref": X}` with the
    other side's inline schema (an added/removed `$ref` record at `P/$ref`);
    the model keeps expanding while the compared value pair is new. Every
    unmatched record of the same use and owner at or beneath `P` is then
    `known-rust-bug-#187`, except Review records at a `$ref` key for which
    the Q1 condition holds (`is_q1`), which keep their class."""
    cuts = {(r[0], r[1], r[2][: -len("/$ref")]) for r in rust
            if r[2].endswith("/$ref") and r[3].split(" ")[0] in ("added", "removed")}
    out = []
    for kind, m, r in found:
        rec = m if m is not None else r
        if (kind.startswith(("missing-in-rust", "extra-in-rust"))
                and not (rec[2].endswith("/$ref") and rec[4] == "review" and is_q1(rec))
                and any(u == rec[0] and o == rec[1] and under(rec[2], p) for u, o, p in cuts)):
            kind = "known-rust-bug-#187"
        out.append((kind, m, r))
    return out


# ── known gaps ──────────────────────────────────────────────────────────────

# `Node.child` (formal/VimanamProofs/Severity.lean): the schema grammar the
# model follows `$ref`s in.
def node_child(node: str, key: str) -> str:
    if node == "schema":
        if key in ("properties", "patternProperties"):
            return "pmap"
        if key in ("allOf", "oneOf", "anyOf"):
            return "slist"
        if key in ("items", "additionalProperties", "not"):
            return "schema"
        if key in ("required", "enum"):
            return "set"
        return "opaq"
    if node in ("pmap", "slist"):
        return "schema"
    return "opaq"


ANNOTATIONS = ("description", "title", "example", "examples", "deprecated")


def ref_name(r) -> str | None:
    if not isinstance(r, str):
        return None
    for pre in REF_PREFIXES:
        if r.startswith(pre):
            return r[len(pre):].replace("~1", "/").replace("~0", "~")
    return None


def encode(token: str) -> str:
    return token.replace("~", "~0").replace("/", "~1")


class Gaps:
    """Where #182 follows a component `$ref` the model does not (#186), per
    use, from the model's export of the use schemas and components.

    For each use and side, walking the use schema and every component it
    reaches (through any reference, as #182 does): `positions[owner]` are the
    owner-relative pointers of references outside the `diff_values` grammar
    (owner `use`, or a component name); `reached` is every component
    reachable from such a reference's target; `grammar` is every component
    the use reaches through grammar positions only (as the model does); `all`
    is every component the use reaches at all. A record is #186's when its
    owner pair has a side in `reached` but not in `grammar` (a component the
    use also reaches through the grammar is compared normally), or its
    pointer is at or beneath one of its owner's positions."""

    def __init__(self, m: dict):
        self.by_use: dict[str, dict] = {}
        comps = m["components"]
        for u in m["uses"]:
            sides = {}
            for side in ("old", "new"):
                sides[side] = self._side(u[side], comps[side])
            self.by_use[u["id"]] = sides

    @staticmethod
    def _side(schema, comps: dict) -> dict:
        positions: dict[str, set[str]] = defaultdict(set)
        targets: set[str] = set()
        seen_all: set[str] = set()
        seen_grammar: set[str] = set()
        # (owner, value, reached through grammar positions only)
        todo: list[tuple[str, object, bool]] = [("use", schema, True)]

        def walk(owner, v, node, ptr, grammar):
            if isinstance(v, dict):
                n = ref_name(v.get("$ref"))
                if n is not None and n in comps:
                    via_grammar = grammar and node == "schema"
                    if node != "schema":
                        positions[owner].add(ptr)
                        targets.add(n)
                    if via_grammar and n not in seen_grammar:
                        seen_grammar.add(n)
                        seen_all.add(n)
                        todo.append((n, comps[n], True))
                    elif n not in seen_all:
                        seen_all.add(n)
                        todo.append((n, comps[n], False))
                for k, c in v.items():
                    if k == "$ref":
                        continue
                    if node == "schema" and (k.startswith("x-") or k in ANNOTATIONS):
                        continue
                    walk(owner, c, node_child(node, k), f"{ptr}/{encode(k)}", grammar)
            elif isinstance(v, list):
                for i, c in enumerate(v):
                    walk(owner, c, node_child(node, str(i)), f"{ptr}/{i}", grammar)

        while todo:
            owner, v, grammar = todo.pop()
            walk(owner, v, "schema", "", grammar)
        reached: set[str] = set()
        stack = list(targets)
        while stack:
            n = stack.pop()
            if n in reached:
                continue
            reached.add(n)
            stack.extend(r for r in component_refs(comps[n]) if r in comps)
        return {"positions": positions, "reached": reached, "grammar": seen_grammar, "all": seen_all}

    def covers(self, rec) -> bool:
        sides = self.by_use.get(rec[0])
        if sides is None:
            return False
        if rec[1] == "use":
            owners = {"old": "use", "new": "use"}
        else:
            o, n = rec[1].split("->", 1) if "->" in rec[1] else (rec[1], rec[1])
            for side, name in (("old", o), ("new", n)):
                if name in sides[side]["reached"] and name not in sides[side]["grammar"]:
                    return True
            owners = {"old": o, "new": n}
        return any(under(rec[2], p) for side in ("old", "new")
                   for p in sides[side]["positions"].get(owners[side], ()))

    def any(self) -> bool:
        return any(sides[s]["positions"] for sides in self.by_use.values() for s in ("old", "new"))

    def reaches(self, use: str, side: str) -> set[str]:
        sides = self.by_use.get(use)
        return sides[side]["all"] if sides else set()


def component_refs(v):
    """Every component name `v` references, anywhere."""
    if isinstance(v, dict):
        r = ref_name(v.get("$ref"))
        if r is not None:
            yield r
        for c in v.values():
            yield from component_refs(c)
    elif isinstance(v, list):
        for c in v:
            yield from component_refs(c)


def alias_loops(comps: dict, pure: bool) -> set[str]:
    """Components from which a chain of aliases comes back to a component
    already on it: alias loops, and the aliases leading into them (Q1). With
    `pure`, an alias is a component that is a `$ref` and nothing else (the
    exported components are canonicalised, so annotation and `x-*` siblings
    are already gone); otherwise any component with a `$ref` key, whatever
    siblings it has (`X = {"$ref": X, "required": ["s"]}` is a loop too)."""
    out = set()
    for start in comps:
        seen = [start]
        cur = comps[start]
        while isinstance(cur, dict) and "$ref" in cur and (not pure or set(cur) == {"$ref"}):
            n = ref_name(cur["$ref"])
            if n is None or n not in comps:
                break
            if n in seen:
                out.update(seen)
                break
            seen.append(n)
            cur = comps[n]
    return out


# ── per-path cuts of the pre-#182 resolver ──────────────────────────────────


def main_cuts(schema, comps: dict, budget: int = 200_000) -> set[str] | None:
    """Where `DocResolver::resolve` as of aa6b1ab (full inlining) leaves a
    reference literal on one side: the same reference *string* already on the
    chain, or 64 hops. `None` past `budget` nodes."""
    cuts: set[str] = set()
    count = [0]

    def expand(v, depth, stack, ptr):
        count[0] += 1
        if count[0] > budget:
            raise OverflowError
        if isinstance(v, dict):
            r = v.get("$ref")
            if isinstance(r, str):
                if depth >= 64 or r in stack:
                    cuts.add(ptr)
                    return
                name = ref_name(r)
                if name is None or name not in comps:
                    return
                t = comps[name]
                merged = t if len(v) == 1 and isinstance(t, dict) else {
                    **(t if isinstance(t, dict) else {}), **{k: x for k, x in v.items() if k != "$ref"}}
                expand(merged, depth + 1, stack + [r], ptr)
                return
            for k, c in v.items():
                expand(c, depth, stack, ptr + "/" + k.replace("~", "~0").replace("/", "~1"))
        elif isinstance(v, list):
            for i, c in enumerate(v):
                expand(c, depth, stack, f"{ptr}/{i}")

    try:
        expand(schema, 0, [], "")
    except (OverflowError, RecursionError):
        return None
    return cuts


# ── shrinking ───────────────────────────────────────────────────────────────


def candidates(doc):
    """Single-step reductions: delete one object key or one array element."""
    def walk(v, path):
        if isinstance(v, dict):
            for k in list(v):
                if path == [] and k in ("openapi", "swagger", "info", "paths"):
                    continue
                if path in (["components"],) and k == "schemas":
                    continue
                yield path + [k]
            for k, c in v.items():
                yield from walk(c, path + [k])
        elif isinstance(v, list):
            for i in range(len(v)):
                yield path + [i]
            for i, c in enumerate(v):
                yield from walk(c, path + [i])
    yield from walk(doc, [])


def delete(doc, path):
    doc = copy.deepcopy(doc)
    cur = doc
    for p in path[:-1]:
        cur = cur[p]
    del cur[path[-1]]
    return doc


def shrink(old, new, still_fails, tmp: Path, budget: int = 400):
    """Greedy delta debugging over both documents (same path deleted on both
    sides first, then one side at a time)."""
    tries = 0
    changed = True
    while changed and tries < budget:
        changed = False
        for path in list(candidates(old)):
            if tries >= budget:
                break
            for o2, n2 in (
                (_try(delete, old, path), _try(delete, new, path)),
                (_try(delete, old, path), new),
                (old, _try(delete, new, path)),
            ):
                if o2 is None or n2 is None or (o2 == old and n2 == new):
                    continue
                tries += 1
                if still_fails(o2, n2):
                    old, new, changed = o2, n2, True
                    break
            if changed:
                break
        if not changed:
            for path in list(candidates(new)):
                if tries >= budget:
                    break
                n2 = _try(delete, new, path)
                if n2 is None or n2 == new:
                    continue
                tries += 1
                if still_fails(old, n2):
                    new, changed = n2, True
                    break
    return old, new


def _try(f, *a):
    try:
        return f(*a)
    except (KeyError, IndexError, TypeError):
        return None


# ── main ────────────────────────────────────────────────────────────────────


def load_allowlist() -> list[dict]:
    return json.loads(ALLOWLIST.read_text())["rules"]


REF_TOKEN = re.compile(r"ref:(\S+)")


def node_at(v, ptr: str):
    """The value at JSON pointer `ptr` in `v`, or `None`."""
    if ptr == "":
        return v
    for t in ptr[1:].split("/"):
        t = t.replace("~1", "/").replace("~0", "~")
        if isinstance(v, dict) and t in v:
            v = v[t]
        elif isinstance(v, list) and t.isdigit() and int(t) < len(v):
            v = v[int(t)]
        else:
            return None
    return v


def ref_values(rec) -> set[str]:
    """The component names a record's change values mention (`ref:NAME`)."""
    return set(REF_TOKEN.findall(rec[3]))


def q1_checker(gaps: "Gaps", m: dict):
    """The Q1 condition for a pair, given its disagreements: a Review record
    at a `$ref` key, where the use reaches (on either side) a component in
    an alias loop, or leading into one, that is named by
    * the record's `ref:` values, or those of its counterpart disagreement
      at the same use, owner and pointer (an alias may have siblings), or
    * the record's owner pair, when that component is a pure alias (its
      content is the loop, so a `$ref` record there is the artefact).
    A `ref:` value naming anything else, a loop the use does not reach, or
    an owner whose alias has content (a change in that content is real)
    does not qualify. Structurally, too, the record must sit on a reference
    to the loop: on one side, the node at the record's owner-relative
    pointer without `/$ref` (in the use schema, or the owner's component) is
    an object whose `$ref` names an alias-loop component."""
    comps = m["components"]
    uses = {u["id"]: u for u in m["uses"]}
    loose = alias_loops(comps["old"], False) | alias_loops(comps["new"], False)
    pure = alias_loops(comps["old"], True) | alias_loops(comps["new"], True)
    def factory(found):
        peers: dict[tuple, set[str]] = defaultdict(set)
        for _, mrec, rrec in found:
            for x in (mrec, rrec):
                if x is not None:
                    peers[x[:3]] |= ref_values(x)

        def on_loop_ref(rec) -> bool:
            if rec[0] not in uses:
                return False
            base = rec[2][: -len("/$ref")]
            owner = rec[1].split("->", 1) if rec[1] != "use" else None
            for i, side in enumerate(("old", "new")):
                root = uses[rec[0]][side] if owner is None else comps[side].get(owner[i])
                node = node_at(root, base)
                if isinstance(node, dict) and ref_name(node.get("$ref")) in loose:
                    return True
            return False

        def is_q1(rec) -> bool:
            if not (rec[2].endswith("/$ref") and rec[4] == "review"):
                return False
            reach = gaps.reaches(rec[0], "old") | gaps.reaches(rec[0], "new")
            owners = set(rec[1].split("->", 1)) if rec[1] != "use" else set()
            named = (ref_values(rec) | peers[rec[:3]]) & loose & reach or owners & pure & reach
            return bool(named) and on_loop_ref(rec)
        return is_q1
    return factory


def allowed(kind: str, mrec, rrec, rules: list[dict], q1: bool = False) -> str | None:
    """The allowlist rule a disagreement falls under, if any. A rule with
    `"requires": "alias-loop"` only covers a disagreement for which the
    harness found the Q1 condition (`q1`, from `q1_checker`): it checks the
    condition, it does not assume it."""
    rec = mrec if mrec is not None else rrec
    for rule in rules:
        if kind not in rule["classes"]:
            continue
        if "pointer" in rule and rec[2] != rule["pointer"]:
            continue
        if "pointer_suffix" in rule and not rec[2].endswith(rule["pointer_suffix"]):
            continue
        if "verdict" in rule and not all(x is None or x[4] == rule["verdict"] for x in (mrec, rrec)):
            continue
        if rule.get("requires") == "alias-loop" and not q1:
            continue
        return rule["name"]
    return None


def per_path_compare(m: dict, mn: dict) -> tuple[Counter, Counter]:
    """Model and pre-#182 records of one pair, without truncated uses and
    without records at or beneath a cut (the model's, or either side's in the
    old resolver), where the two semantics deliberately differ."""
    truncated = set(m["per_path_truncated"])
    cuts: dict[str, set[str]] = defaultdict(set)
    for c in m["per_path_cuts"]:
        cuts[c["use"]].add(c["ptr"])
    comps = m["components"]
    for u in m["uses"]:
        for side in ("old", "new"):
            mc = main_cuts(u[side], comps[side])
            if mc is None:
                truncated.add(u["id"])
            else:
                cuts[u["id"]] |= mc

    def keep(r):
        return r[0] not in truncated and not any(under(r[1], c) for c in cuts[r[0]])

    mp = Counter({r: k for r, k in model_per_path(m).items() if keep(r)})
    rp = Counter({r: k for r, k in rust_records(mn, False)[0].items() if keep(r)})
    return mp, rp


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--rust", "--pr", dest="pr", required=True,
                    help="vimanam binary with #181's edge diff, e.g. main (grouped comparison)")
    ap.add_argument("--per-path", "--main", dest="main",
                    help="vimanam binary from before #182, e.g. aa6b1ab (per-path comparison)")
    ap.add_argument("--seeds", type=int, default=300)
    ap.add_argument("--first-seed", type=int, default=1)
    ap.add_argument("--out", required=True, help="output directory (report.md, repros)")
    ap.add_argument("--no-shrink", action="store_true")
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--cases", type=Path, default=CASES,
                    help="fixed cases directory (default formal/refdiff/cases)")
    ap.add_argument("--ci", action="store_true",
                    help="no shrinking; exit 1 on a Rust error or timeout, or on a grouped "
                         "disagreement formal/refdiff/allowlist.json does not cover")
    args = ap.parse_args()
    if args.ci:
        args.no_shrink = True
    rules = load_allowlist()

    out = Path(args.out).resolve()
    if out.exists():
        shutil.rmtree(out)
    (out / "work").mkdir(parents=True)
    exe = build_model()

    pairs: list[tuple[str, Path, Path]] = []
    for old in sorted(args.cases.glob("*.old.json")):
        pairs.append((old.name[: -len(".old.json")], old, old.with_name(old.name.replace(".old.", ".new."))))
    gen_dir = out / "generated"
    gen_dir.mkdir()
    for seed in range(args.first_seed, args.first_seed + args.seeds):
        o, n = Gen(seed).pair()
        po, pn = gen_dir / f"seed{seed}.old.json", gen_dir / f"seed{seed}.new.json"
        po.write_text(json.dumps(o))
        pn.write_text(json.dumps(n))
        pairs.append((f"seed{seed}", po, pn))
        # Both directions: swapping old and new must also agree.
        pairs.append((f"seed{seed}-rev", pn, po))

    models = run_model(exe, [(o, n) for _, o, n in pairs], out / "work")
    with ThreadPoolExecutor(args.jobs) as pool:
        prs = list(pool.map(lambda p: run_rust(args.pr, p[1], p[2]), pairs))
        mains = list(pool.map(lambda p: run_rust(args.main, p[1], p[2]), pairs)) if args.main else [None] * len(pairs)

    by_class: dict[str, list] = defaultdict(list)
    main_by_class: dict[str, list] = defaultdict(list)
    main_agree = 0
    errors = []
    for (name, o, n), m, pr, mn in zip(pairs, models, prs, mains):
        gaps = Gaps(m)
        gap = gaps.any()
        q1_for = q1_checker(gaps, m)
        if pr is None or "error" in pr:
            errors.append((name, "rust", pr))
        else:
            mg, mneg = model_grouped(m)
            rg, rneg = rust_records(pr, True)
            found = classify_grouped(mg, mneg, rg, rneg, q1_for)
            is_q1 = q1_for(found)
            for kind, mrec, rrec in found:
                rec = mrec if mrec is not None else rrec
                # The model does not follow references outside the schema
                # grammar; #182 does, and rates what it finds there (#186).
                # A record both sides report (a verdict or flag mismatch) is
                # never put down to that.
                paired = kind.startswith(("verdict", "under-not-flag"))
                if not paired and gaps.covers(rec):
                    kind = "known-rust-bug-#186"
                by_class[kind].append((name, o, n, mrec, rrec, is_q1(rec)))
        if mn is not None:
            if "error" in mn:
                errors.append((name, "per-path", mn))
                continue
            mp, rp = per_path_compare(m, mn)
            if mp == rp:
                main_agree += 1
            else:
                cls = "refs outside the grammar (#186)" if gap else "differ"
                main_by_class[cls].append((name, o, n, sorted((mp - rp).elements()),
                                           sorted((rp - mp).elements())))

    unallowed = [(kind, i) for kind, items in sorted(by_class.items()) for i in items
                 if allowed(kind, i[3], i[4], rules, i[5]) is None]

    # Shrink one representative per class.
    repro_dir = out / "repro"
    repro_dir.mkdir()
    shrunk = {}
    tmp = out / "work" / "shrink"
    tmp.mkdir()
    for kind, items in sorted(by_class.items()):
        name, o, n, mrec, rrec, *_ = items[0]
        old, new = json.loads(o.read_text()), json.loads(n.read_text())
        counter = [0]

        def still(old2, new2, kind=kind):
            counter[0] += 1
            po, pn = tmp / f"o{counter[0]}.json", tmp / f"n{counter[0]}.json"
            po.write_text(json.dumps(old2))
            pn.write_text(json.dumps(new2))
            pr = run_rust(args.pr, po, pn)
            if pr is None or "error" in pr:
                return False
            mm = json.loads(subprocess.run([str(exe), str(po), str(pn)], capture_output=True,
                                           text=True, check=True).stdout)
            return any(k == kind for k, _, _ in classify_grouped(*model_grouped(mm), *rust_records(pr, True)))

        if not args.no_shrink and kind != "known-rust-bug-#186":
            old, new = shrink(old, new, still, tmp)
        base = repro_dir / kind
        base.with_suffix(".old.json").write_text(json.dumps(old, indent=1) + "\n")
        base.with_suffix(".new.json").write_text(json.dumps(new, indent=1) + "\n")
        po, pn = base.with_suffix(".old.json"), base.with_suffix(".new.json")
        mm = json.loads(subprocess.run([str(exe), str(po), str(pn)], capture_output=True, text=True, check=True).stdout)
        pr = run_rust(args.pr, po, pn)
        diffs = [] if pr is None or "error" in pr else [
            d for d in classify_grouped(*model_grouped(mm), *rust_records(pr, True))
            if d[0] == kind or kind == "known-rust-bug-#186"]
        shrunk[kind] = (name, po, pn, diffs)

    write_report(out, args, pairs, by_class, main_by_class, errors, shrunk, main_agree, rules, unallowed)
    print(out / "report.md")
    if args.ci:
        for kind, (name, o, n, mrec, rrec, *_) in unallowed[:20]:
            print(f"not allowlisted: {kind} in {name}: model {fmt(mrec)}; rust {fmt(rrec)}", file=sys.stderr)
        for n_, w, e in errors[:20]:
            print(f"error: {n_} ({w}): {e}", file=sys.stderr)
        if unallowed or errors:
            print(f"{len(unallowed)} disagreement(s) outside the allowlist, {len(errors)} error(s)",
                  file=sys.stderr)
            return 1
        print(f"{len(pairs)} pairs: every disagreement is allowlisted")
    return 0


def fmt(rec):
    if rec is None:
        return "—"
    return "`" + " | ".join(str(x) for x in rec) + "`"


def write_report(out, args, pairs, by_class, main_by_class, errors, shrunk, main_agree, rules, unallowed):
    lines = ["# `$ref` diff: Lean reference model vs Rust", ""]
    lines += [f"- pairs: {len(pairs)} ({sum(1 for p in pairs if not p[0].startswith('seed'))} fixed cases "
              f"from {args.cases}, {args.seeds} generated seeds × 2 directions)",
              f"- grouped comparison: `{args.pr}`", f"- per-path comparison: `{args.main}`", ""]
    lines += ["## Grouped view vs `--rust` (v1 records)", "",
              "Record: `use | owner | pointer | change | severity` (owner `use` = the use site, "
              "`X->Y` = component pair).", "",
              "| Class | Records | Pairs | Allowlisted | Repro |", "| --- | ---: | ---: | ---: | --- |"]
    for kind, items in sorted(by_class.items()):
        rep = shrunk.get(kind)
        ok = sum(1 for i in items if allowed(kind, i[3], i[4], rules, i[5]) is not None)
        lines.append(f"| `{kind}` | {len(items)} | {len({i[0] for i in items})} | {ok} | "
                     f"{rep[1].name if rep else ''} |")
    lines += ["", f"Outside the allowlist (`formal/refdiff/allowlist.json`): {len(unallowed)} records.", ""]
    for kind, items in sorted(by_class.items()):
        lines += [f"### `{kind}`", ""]
        name, po, pn, diffs = shrunk[kind]
        lines += [f"Shrunk from `{name}`: `{po}` / `{pn}`", ""]
        for _, m, r in diffs[:6]:
            lines += [f"- model: {fmt(m)}", f"  rust:  {fmt(r)}"]
        lines += ["", "First occurrences:", ""]
        for name, o, n, m, r, *_ in items[:5]:
            lines += [f"- `{name}`: model {fmt(m)}; rust {fmt(r)}"]
        lines.append("")
    if args.main:
        lines += ["## Per-path view vs `--per-path` (v1 records)", "",
                  f"- agree: {main_agree} pairs (uses past the model's per-path budget excluded, and "
                  "records at or beneath a cut, the model's or the old resolver's on either side)",
                  "- differ:", ""]
        for cls, items in sorted(main_by_class.items()):
            lines += [f"- {cls}: {len(items)} pairs differ"]
            for name, o, n, mo, ro in items[:5]:
                lines += [f"  - `{name}` (`{o}`): model-only {len(mo)}, pre-#182-only {len(ro)}; e.g. "
                          f"{fmt(mo[0]) if mo else '—'} / {fmt(ro[0]) if ro else '—'}"]
        lines.append("")
    if errors:
        lines += ["## Errors", ""] + [f"- `{n}` ({w}): {e}" for n, w, e in errors[:20]] + [""]
    (out / "report.md").write_text("\n".join(lines))
    (out / "disagreements.json").write_text(json.dumps(
        {k: [{"pair": i[0], "old": str(i[1]), "new": str(i[2]), "model": i[3], "rust": i[4]} for i in v]
         for k, v in by_class.items()}, indent=1))


if __name__ == "__main__":
    sys.exit(main())
