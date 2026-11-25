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
import shutil
import subprocess
import sys
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FORMAL = ROOT / "formal"
CASES = FORMAL / "refdiff" / "cases"

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
        return {"openapi": "3.0.0", "info": {"title": "t", "version": "1"}, "paths": paths,
                "components": {"schemas": comps}}

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


def run_model(exe: Path, pairs: list[tuple[Path, Path]], work: Path) -> list[dict]:
    listing = work / "batch.tsv"
    outs = [work / f"model-{i}.json" for i in range(len(pairs))]
    listing.write_text("".join(f"{o}\t{n}\t{out}\n" for (o, n), out in zip(pairs, outs)))
    subprocess.run([str(exe), "--batch", str(listing)], check=True)
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


def model_grouped(m: dict) -> set[tuple]:
    """Records as tuples; `NEGATED` collects those reached under `not`."""
    out = set()
    for r in m["grouped"]:
        o = r["owner"]
        owner = "use" if o is None else f"{o['old']}->{o['new']}"
        c = r["change"]
        rec = (r["use"], owner, r["ptr"], norm_change(c["op"], c.get("before"), c.get("after")), r["severity"])
        out.add(rec)
        if r["neg"]:
            NEGATED.add(rec)
    return out


# Model records reached through a `not` (their verdict is Review for that reason).
NEGATED: set[tuple] = set()
# Rust records with `details.under_not` (#182 from fb7d637).
RUST_NEGATED: set[tuple] = set()


def model_per_path(m: dict) -> set[tuple]:
    out = set()
    for r in m["per_path"]:
        c = r["change"]
        out.add((r["use"], r["ptr"], norm_change(c["op"], c.get("before"), c.get("after")), r["severity"]))
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


def rust_records(rust: dict, grouped: bool) -> set[tuple]:
    out = set()
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
            # #182 (from 870e2ad) writes the retarget fact as a change of the
            # component names at `/$ref`; the model's record for it is
            # `retargeted X -> Y` at the pair root.
            if (comp is not None and old_comp != comp and pointer == "/$ref"
                    and sc["operation"] == "changed" and (before, after) == (old_comp, comp)):
                pointer, change = "", f"retargeted {old_comp} -> {comp}"
            out.add((use, owner, pointer, change, rec["severity"]))
            if d.get("under_not"):
                RUST_NEGATED.add((use, owner, pointer, change, rec["severity"]))
        else:
            out.add((use, sc["pointer"], change, rec["severity"]))
    return out


# ── classification ──────────────────────────────────────────────────────────


def classify_grouped(model: set[tuple], rust: set[tuple]) -> list[tuple[str, tuple | None, tuple | None]]:
    """Pairs up model-only and Rust-only records and names each difference."""
    only_m = sorted(model - rust)
    only_r = sorted(rust - model)
    found = []
    used_r: set[tuple] = set()
    for m in only_m:
        use, owner, ptr, change, sev = m
        if change.startswith("retargeted"):
            found.append(("retarget-fact-missing", m, None))
            continue
        # Same record, different verdict.
        same = [r for r in only_r if r not in used_r and r[:4] == m[:4]]
        if same:
            r = same[0]
            used_r.add(r)
            kind = "verdict-under-not" if m in NEGATED else "verdict"
            found.append((kind, m, r))
            continue
        # Reached both through a `not` and without: Rust has one record, with
        # the plain verdict, where the model has a Review one as well.
        if m in NEGATED and any(r[:4] == m[:4] for r in rust):
            found.append(("verdict-under-not-merged", m, next(r for r in rust if r[:4] == m[:4])))
            continue
        # Same change at the same use, reported by a different owner: one
        # pointer is the other with the owner's prefix.
        moved = [r for r in only_r if r not in used_r and r[0] == use and r[3] == change
                 and (r[2].endswith(ptr) or ptr.endswith(r[2]))]
        if moved:
            r = moved[0]
            used_r.add(r)
            if owner == "use" or r[1] == "use":
                kind = "attribution-use-vs-component"
            else:
                kind = "attribution-component-pair"
            found.append((kind, m, r))
            continue
        found.append(("missing-in-rust" + ("-masked-reach" if owner != "use" else "-at-use"), m, None))
    # Records both sides report, where only one side says "reached under
    # `not`". (A record reached both ways is negated on the model side and may
    # coincide with a plain Review record; only flag Rust-only negation.)
    for r in sorted(model & rust):
        if r in RUST_NEGATED and r not in NEGATED:
            found.append(("under-not-flag-extra-in-rust", r, r))
    for r in only_r:
        if r in used_r:
            continue
        found.append(("extra-in-rust" + ("-component" if r[1] != "use" else "-at-use"), None, r))
    return found


def has_cycle(doc: dict) -> bool:
    comps = doc.get("definitions") or doc.get("components", {}).get("schemas", {}) or {}

    def refs(v, acc):
        if isinstance(v, dict):
            r = v.get("$ref")
            if isinstance(r, str):
                for pre in REF_PREFIXES:
                    if r.startswith(pre):
                        acc.add(r[len(pre):].replace("~1", "/").replace("~0", "~"))
            for c in v.values():
                refs(c, acc)
        elif isinstance(v, list):
            for c in v:
                refs(c, acc)
        return acc

    graph = {n: refs(s, set()) for n, s in comps.items()}
    state: dict[str, int] = {}

    def dfs(n):
        state[n] = 1
        for m in graph.get(n, ()):
            if m in graph and (state.get(m) == 1 or (m not in state and dfs(m))):
                return True
        state[n] = 2
        return False

    return any(n not in state and dfs(n) for n in graph)


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
    args = ap.parse_args()

    out = Path(args.out).resolve()
    if out.exists():
        shutil.rmtree(out)
    (out / "work").mkdir(parents=True)
    exe = build_model()

    pairs: list[tuple[str, Path, Path]] = []
    for old in sorted(CASES.glob("*.old.json")):
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
        if pr is None or "error" in pr:
            errors.append((name, "pr", pr))
            continue
        for kind, mrec, rrec in classify_grouped(model_grouped(m), rust_records(pr, True)):
            by_class[kind].append((name, o, n, mrec, rrec))
        if mn is not None:
            if "error" in mn:
                errors.append((name, "main", mn))
                continue
            truncated = set(m["per_path_truncated"])
            mp = {r for r in model_per_path(m) if r[0] not in truncated}
            rp = {r for r in rust_records(mn, False) if r[0] not in truncated}
            if mp == rp:
                main_agree += 1
            else:
                cyc = has_cycle(json.loads(o.read_text())) or has_cycle(json.loads(n.read_text()))
                main_by_class["cyclic" if cyc else "acyclic"].append((name, o, n, sorted(mp - rp), sorted(rp - mp)))

    # Shrink one representative per class.
    repro_dir = out / "repro"
    repro_dir.mkdir()
    shrunk = {}
    tmp = out / "work" / "shrink"
    tmp.mkdir()
    for kind, items in sorted(by_class.items()):
        name, o, n, mrec, rrec = items[0]
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
            return any(k == kind for k, _, _ in classify_grouped(model_grouped(mm), rust_records(pr, True)))

        if not args.no_shrink:
            old, new = shrink(old, new, still, tmp)
        base = repro_dir / kind
        base.with_suffix(".old.json").write_text(json.dumps(old, indent=1) + "\n")
        base.with_suffix(".new.json").write_text(json.dumps(new, indent=1) + "\n")
        po, pn = base.with_suffix(".old.json"), base.with_suffix(".new.json")
        mm = json.loads(subprocess.run([str(exe), str(po), str(pn)], capture_output=True, text=True, check=True).stdout)
        pr = run_rust(args.pr, po, pn)
        diffs = [d for d in classify_grouped(model_grouped(mm), rust_records(pr, True)) if d[0] == kind]
        shrunk[kind] = (name, po, pn, diffs)

    write_report(out, args, pairs, by_class, main_by_class, errors, shrunk, main_agree)
    print(out / "report.md")
    return 0


def fmt(rec):
    if rec is None:
        return "—"
    if isinstance(rec, list):
        rec = tuple(rec)
    return "`" + " | ".join(str(x) for x in rec) + (" (under not)" if rec in NEGATED else "") + "`"


def write_report(out, args, pairs, by_class, main_by_class, errors, shrunk, main_agree=0):
    lines = ["# `$ref` diff: Lean reference model vs Rust", ""]
    lines += [f"- pairs: {len(pairs)} ({sum(1 for p in pairs if not p[0].startswith('seed'))} fixed cases "
              f"from formal/refdiff/cases, {args.seeds} generated seeds × 2 directions)",
              f"- grouped comparison: `{args.pr}`", f"- per-path comparison: `{args.main}`", ""]
    lines += ["## Grouped view vs `--rust` (v1 records)", "",
              "Record: `use | owner | pointer | change | severity` (owner `use` = the use site, "
              "`X->Y` = component pair).", "",
              "| Class | Records | Pairs | Repro |", "| --- | ---: | ---: | --- |"]
    for kind, items in sorted(by_class.items()):
        rep = shrunk.get(kind)
        lines.append(f"| `{kind}` | {len(items)} | {len({i[0] for i in items})} | "
                     f"{rep[1].name if rep else ''} |")
    lines.append("")
    for kind, items in sorted(by_class.items()):
        lines += [f"### `{kind}`", ""]
        name, po, pn, diffs = shrunk[kind]
        lines += [f"Shrunk from `{name}`: `{po}` / `{pn}`", ""]
        for _, m, r in diffs[:6]:
            lines += [f"- model: {fmt(m)}", f"  rust:  {fmt(r)}"]
        lines += ["", "First occurrences:", ""]
        for name, o, n, m, r in items[:5]:
            lines += [f"- `{name}`: model {fmt(m)}; rust {fmt(r)}"]
        lines.append("")
    if args.main:
        lines += ["## Per-path view vs `--per-path` (v1 records)", "",
                  f"- agree: {main_agree} pairs (uses past the model's per-path budget excluded)",
                  "- differ, by whether either document has a component reference cycle:", ""]
        for cls, items in sorted(main_by_class.items()):
            lines += [f"- {cls}: {len(items)} pairs differ"]
            for name, o, n, mo, ro in items[:5]:
                lines += [f"  - `{name}` (`{o}`): model-only {len(mo)}, main-only {len(ro)}; e.g. "
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
