#!/usr/bin/env python3
"""Mechanical accuracy check of `vimanam diff` against `oasdiff` on real release pairs.

Speed says nothing about whether a verdict is right. This script compares the
breaking-change findings of `vimanam diff --contract 1` and `oasdiff` against a
ground truth computed from the specs themselves, and narrows what a person has
to judge to the operations where the two tools disagree.

For each consecutive pair of Stripe `openapi/spec3.json` releases:

1. Ground truth. A structural JSON diff of the two documents gives every
   changed JSON pointer. An operation counts as changed when its own subtree,
   its path-level parameters, or any component in its transitive `$ref`
   closure (old or new side) changed, or when it was added or removed.
2. Grounding (false-positive screen). Every finding from either tool must name
   a changed operation. A `vimanam` finding inside a shared component must also
   point at or under a changed location of that component.
3. Coverage (miss screen). Every changed operation should get at least one
   finding from each tool (`vimanam` at any severity, `oasdiff changelog` at any
   level). An uncovered operation is a candidate miss; some are legitimate,
   such as description-only edits that neither tool reports by design.
4. Agreement. Operations each tool rates breaking (`vimanam` severity
   `breaking`; `oasdiff breaking` at level ERR) are bucketed into both,
   vimanam-only and oasdiff-only. The findings behind each disagreement are
   written out for manual review.

Step 5, deciding which tool is right where they disagree, is left to a person.
The script writes the evidence for it under `--evidence-dir`.

Standard library only. Needs Git and `oasdiff` on PATH. Runs serially:

    python3 scripts/benchmark-diff-accuracy.py \\
        --binary target/release/vimanam \\
        --corpus /tmp/vimanam-eval-corpus \\
        --output scripts/benchmark-diff-accuracy-v23-results.json \\
        --evidence-dir /tmp/vimanam-accuracy-evidence
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path

STRIPE_RELEASES = [
    "a402765", "0aaf9a6", "da3eadb", "cfe95bf", "a54c388", "d771243", "6a1ea0a", "fbd0091",
    "f4ac6d9", "7061322", "d5d11f6", "af5309c", "30d3391", "c8faccb", "2bab976", "bcfd59f",
]
METHODS = {"get", "put", "post", "delete", "options", "head", "patch", "trace"}
OASDIFF_ERR = 3


def esc(token: str) -> str:
    return token.replace("~", "~0").replace("/", "~1")


def changed_pointers(old, new, ptr: str = "") -> list[str]:
    """Every pointer where the two JSON values differ, at the deepest level."""
    if type(old) is not type(new):
        return [ptr]
    if isinstance(old, dict):
        out: list[str] = []
        for key in old.keys() | new.keys():
            child = f"{ptr}/{esc(key)}"
            if key not in old or key not in new:
                out.append(child)
            else:
                out.extend(changed_pointers(old[key], new[key], child))
        return out
    if isinstance(old, list):
        if len(old) != len(new):
            return [ptr]
        out = []
        for i, (a, b) in enumerate(zip(old, new)):
            out.extend(changed_pointers(a, b, f"{ptr}/{i}"))
        return out
    return [] if old == new else [ptr]


def refs_in(value, found: set[str]) -> None:
    if isinstance(value, dict):
        ref = value.get("$ref")
        if isinstance(ref, str) and ref.startswith("#/components/"):
            found.add(ref[1:])
        for v in value.values():
            refs_in(v, found)
    elif isinstance(value, list):
        for v in value:
            refs_in(v, found)


def get_pointer(doc, ptr: str):
    cur = doc
    for raw in ptr.lstrip("/").split("/"):
        token = raw.replace("~1", "/").replace("~0", "~")
        if isinstance(cur, dict) and token in cur:
            cur = cur[token]
        else:
            return None
    return cur


def component_of(ptr: str) -> str | None:
    """`/components/<kind>/<name>` for a pointer inside a component, else None."""
    parts = ptr.split("/")
    if len(parts) >= 4 and parts[1] == "components":
        return "/".join(parts[:4])
    return None


def closure(doc, start: set[str]) -> set[str]:
    seen: set[str] = set()
    todo = list(start)
    while todo:
        comp = todo.pop()
        if comp in seen:
            continue
        seen.add(comp)
        refs: set[str] = set()
        refs_in(get_pointer(doc, comp), refs)
        todo.extend(component_of(r) or r for r in refs)
    return seen


def operations(doc) -> dict[tuple[str, str], dict]:
    ops = {}
    for path, item in (doc.get("paths") or {}).items():
        if not isinstance(item, dict):
            continue
        for method, op in item.items():
            if method in METHODS and isinstance(op, dict):
                ops[(method.upper(), path)] = {"op": op, "path_params": item.get("parameters")}
    return ops


def ground_truth(old, new) -> dict:
    changed = changed_pointers(old, new)
    changed_components = {c for c in map(component_of, changed) if c}
    old_ops, new_ops = operations(old), operations(new)
    changed_ops: dict[tuple[str, str], str] = {}
    for key in old_ops.keys() | new_ops.keys():
        method, path = key
        if key not in old_ops:
            changed_ops[key] = "added"
            continue
        if key not in new_ops:
            changed_ops[key] = "removed"
            continue
        own = f"/paths/{esc(path)}/{method.lower()}"
        shared = f"/paths/{esc(path)}/parameters"
        if any(p == own or p.startswith(own + "/") or p == shared or p.startswith(shared + "/") for p in changed):
            changed_ops[key] = "own"
            continue
        reach: set[str] = set()
        for side in (old_ops[key], new_ops[key]):
            refs: set[str] = set()
            refs_in(side, refs)
            reach |= {component_of(r) or r for r in refs}
        reach = closure(old, reach) | closure(new, reach)
        if reach & changed_components:
            changed_ops[key] = "component"
    return {"changed_pointers": changed, "changed_components": changed_components, "changed_ops": changed_ops}


def run(cmd: list[str]) -> tuple[int, str, str]:
    p = subprocess.run(cmd, capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def vimanam_findings(binary: str, old_p: Path, new_p: Path) -> list[dict]:
    rc, out, err = run([binary, "diff", str(old_p), str(new_p), "--format", "json", "--contract", "1"])
    if rc not in (0, 3):
        raise RuntimeError(f"vimanam diff failed ({rc}): {err[:500]}")
    return json.loads(out).get("changes", [])


def oasdiff_findings(kind: str, old_p: Path, new_p: Path) -> list[dict]:
    rc, out, err = run(["oasdiff", kind, str(old_p), str(new_p), "--format", "json"])
    if rc != 0:
        raise RuntimeError(f"oasdiff {kind} failed ({rc}): {err[:500]}")
    data = json.loads(out or "[]")
    return data if isinstance(data, list) else []


def op_key(method: str | None, path: str | None) -> tuple[str, str] | None:
    if not method or not path:
        return None
    return (method.upper(), path)


def grounded_component_pointer(change: dict, gt: dict) -> bool | None:
    """True or False for a component finding, None when not a component finding."""
    details = change.get("details") or {}
    comp = details.get("component")
    if not comp:
        return None
    base = f"/components/schemas/{esc(comp)}"
    ptr = details.get("pointer") or ""
    target = base + ptr
    return any(p == target or p.startswith(target + "/") or target.startswith(p + "/") for p in gt["changed_pointers"])


def check_pair(binary: str, repo: Path, old_rev: str, new_rev: str, tmp: Path, evidence: Path | None) -> dict:
    old_p, new_p = tmp / f"{old_rev}.json", tmp / f"{new_rev}.json"
    for rev, dest in ((old_rev, old_p), (new_rev, new_p)):
        dest.write_bytes(subprocess.check_output(["git", "-C", str(repo), "show", f"{rev}:openapi/spec3.json"]))
    old, new = json.loads(old_p.read_text()), json.loads(new_p.read_text())
    gt = ground_truth(old, new)
    changed_ops = gt["changed_ops"]

    vim = vimanam_findings(binary, old_p, new_p)
    oas_breaking = oasdiff_findings("breaking", old_p, new_p)
    oas_log = oasdiff_findings("changelog", old_p, new_p)

    vim_ops = {op_key(c["endpoint"]["method"], c["endpoint"]["path"]) for c in vim}
    vim_breaking_ops = {op_key(c["endpoint"]["method"], c["endpoint"]["path"]) for c in vim if c.get("severity") == "breaking"}
    oas_ops = {op_key(x.get("operation"), x.get("path")) for x in oas_log} - {None}
    oas_breaking_ops = {op_key(x.get("operation"), x.get("path")) for x in oas_breaking if x.get("level", 0) >= OASDIFF_ERR} - {None}

    component_checks = [grounded_component_pointer(c, gt) for c in vim]
    ungrounded_vim_ptrs = [c for c, ok in zip(vim, component_checks) if ok is False]

    both = vim_breaking_ops & oas_breaking_ops
    vim_only = vim_breaking_ops - oas_breaking_ops
    oas_only = oas_breaking_ops - vim_breaking_ops

    if evidence is not None:
        pair_dir = evidence / f"{old_rev}..{new_rev}"
        pair_dir.mkdir(parents=True, exist_ok=True)
        (pair_dir / "vimanam-only.json").write_text(json.dumps({
            f"{m} {p}": [c for c in vim if c.get("severity") == "breaking" and op_key(c["endpoint"]["method"], c["endpoint"]["path"]) == (m, p)]
            for m, p in sorted(vim_only)}, indent=1))
        (pair_dir / "oasdiff-only.json").write_text(json.dumps({
            f"{m} {p}": [x for x in oas_breaking if x.get("level", 0) >= OASDIFF_ERR and op_key(x.get("operation"), x.get("path")) == (m, p)]
            for m, p in sorted(oas_only)}, indent=1))
        (pair_dir / "ungrounded-vimanam.json").write_text(json.dumps(ungrounded_vim_ptrs, indent=1))
        (pair_dir / "changed-pointers.txt").write_text("\n".join(sorted(gt["changed_pointers"])) + "\n")

    def fmt(keys):
        return sorted(f"{m} {p}" for m, p in keys)

    return {
        "pair": f"{old_rev}..{new_rev}",
        "ground_truth": {
            "changed_pointers": len(gt["changed_pointers"]),
            "changed_components": len(gt["changed_components"]),
            "changed_operations": len(changed_ops),
            "changed_operations_by_reason": dict(Counter(changed_ops.values())),
        },
        "vimanam": {
            "findings": len(vim),
            "by_severity": dict(Counter(c.get("severity") for c in vim)),
            "breaking_operations": len(vim_breaking_ops),
            "findings_on_unchanged_operations": len([c for c in vim if op_key(c["endpoint"]["method"], c["endpoint"]["path"]) not in changed_ops]),
            "component_findings_at_unchanged_pointers": len(ungrounded_vim_ptrs),
            "changed_operations_not_reported": len(set(changed_ops) - vim_ops),
        },
        "oasdiff": {
            "breaking_findings": len(oas_breaking),
            "changelog_findings": len(oas_log),
            "by_level": dict(Counter(x.get("level") for x in oas_log)),
            "breaking_operations": len(oas_breaking_ops),
            "findings_on_unchanged_operations": len([x for x in oas_log if op_key(x.get("operation"), x.get("path")) not in changed_ops]),
            "changed_operations_not_reported": len(set(changed_ops) - oas_ops),
        },
        "agreement": {
            "breaking_in_both": len(both),
            "breaking_vimanam_only": fmt(vim_only),
            "breaking_oasdiff_only": fmt(oas_only),
        },
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--binary", required=True, help="vimanam binary to check")
    ap.add_argument("--corpus", required=True, help="directory holding the stripe-openapi clone")
    ap.add_argument("--output", required=True, help="JSON results file")
    ap.add_argument("--evidence-dir", help="write per-pair disagreement evidence here")
    ap.add_argument("--pairs", type=int, default=len(STRIPE_RELEASES) - 1, help="check only the first N pairs")
    args = ap.parse_args()

    binary = str(Path(args.binary).resolve())
    repo = Path(args.corpus).resolve() / "stripe-openapi"
    evidence = Path(args.evidence_dir).resolve() if args.evidence_dir else None
    pairs = list(zip(STRIPE_RELEASES, STRIPE_RELEASES[1:]))[: args.pairs]
    results = {
        "environment": {
            "vimanam": subprocess.check_output([binary, "--version"], text=True).strip(),
            "oasdiff": subprocess.check_output(["oasdiff", "--version"], text=True).strip(),
            "oasdiff_breaking_level": "ERR (level 3)",
        },
        "pairs": [],
    }
    with tempfile.TemporaryDirectory(prefix="vimanam-accuracy-") as td:
        for old_rev, new_rev in pairs:
            row = check_pair(binary, repo, old_rev, new_rev, Path(td), evidence)
            results["pairs"].append(row)
            a, v, o = row["agreement"], row["vimanam"], row["oasdiff"]
            print(
                f"{row['pair']}: changed_ops={row['ground_truth']['changed_operations']} "
                f"breaking vimanam={v['breaking_operations']} oasdiff={o['breaking_operations']} both={a['breaking_in_both']} "
                f"vimanam_only={len(a['breaking_vimanam_only'])} oasdiff_only={len(a['breaking_oasdiff_only'])} "
                f"ungrounded vimanam={v['findings_on_unchanged_operations']}+{v['component_findings_at_unchanged_pointers']} "
                f"oasdiff={o['findings_on_unchanged_operations']} "
                f"unreported vimanam={v['changed_operations_not_reported']} oasdiff={o['changed_operations_not_reported']}",
                flush=True,
            )
    Path(args.output).write_text(json.dumps(results, indent=1) + "\n")
    print(f"Wrote {args.output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
