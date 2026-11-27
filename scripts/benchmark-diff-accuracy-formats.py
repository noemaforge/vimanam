#!/usr/bin/env python3
"""Mechanical accuracy check of `vimanam diff` on real GraphQL and Protobuf history.

The OpenAPI counterpart is `benchmark-diff-accuracy.py`. This script does the same
for the two other formats, against the established tool for each:

`graphql` subcommand, consecutive octokit/graphql-schema commits that changed
`schema.json` (GitHub's introspection JSON), against `graphql-inspector diff`:

1. Ground truth. A structural diff of the two introspection documents gives every
   changed coordinate (`Type`, `Type.field`, `Type.field(arg:)`, `Enum.VALUE`),
   ignoring descriptions.
2. Grounding. Every finding from either tool must name a changed coordinate or a
   member of a changed type.
3. Coverage. Every changed coordinate should be reported by each tool.
4. Agreement. Breaking coordinates (`vimanam` severity `breaking`; inspector `✖`)
   are bucketed into both, vimanam-only and inspector-only.
5. Consistency. At each commit, `vimanam diff schema.graphql schema.json` compares
   the SDL and introspection files of the same commit. When upstream keeps them in
   sync it should report no changes; a non-empty result is either upstream drift or
   a disagreement between Vimanam's two readers, and is written out to check.

`protobuf` subcommand, consecutive open-telemetry/opentelemetry-proto release tags,
against `buf breaking`. Each tag is extracted with `git archive`, so a blobless
clone works. For every `.proto` file under `opentelemetry/proto` present on both
sides, `vimanam diff` runs with each side's own import root
(`--old-proto-path`/`--new-proto-path`). `buf breaking` runs once per pair with the
WIRE rules and once with the FILE rules. Per file, the check compares:

- wire: `vimanam` changes with the `wire` impact vs `buf` WIRE findings;
- source: `vimanam` breaking changes vs `buf` FILE findings.

Files added or removed between tags are listed separately, because `vimanam diff`
compares one file pair at a time.

Step 5 of the OpenAPI method, deciding who is right where the tools disagree, is
left to a person; `--evidence-dir` holds the material for it.

Standard library only. Needs Git, `graphql-inspector` and `buf` on PATH. Serial:

    python3 scripts/benchmark-diff-accuracy-formats.py graphql \\
        --binary target/release/vimanam --corpus /tmp/vimanam-eval-corpus \\
        --output scripts/benchmark-diff-accuracy-graphql-v23-results.json \\
        --evidence-dir /tmp/vimanam-accuracy-evidence/graphql
    python3 scripts/benchmark-diff-accuracy-formats.py protobuf \\
        --binary target/release/vimanam --corpus /tmp/vimanam-eval-corpus \\
        --output scripts/benchmark-diff-accuracy-protobuf-v23-results.json \\
        --evidence-dir /tmp/vimanam-accuracy-evidence/protobuf
"""

from __future__ import annotations

import argparse
import io
import json
import re
import subprocess
import sys
import tarfile
import tempfile
from collections import Counter
from pathlib import Path

OCTOKIT_SCHEMA_JSON_COMMITS = [
    "b64979a", "4b94ff4", "48b05fb", "fc3abe0", "d345d19", "8d8d6f1", "ee111b5", "503d8f1", "597478f",
]
OTEL_TAGS = ["v1.4.0", "v1.5.0", "v1.6.0", "v1.7.0", "v1.8.0", "v1.9.0", "v1.10.0", "v1.11.0", "v1.11.1"]


def run(cmd: list[str], cwd: Path | None = None) -> tuple[int, str, str]:
    p = subprocess.run(cmd, capture_output=True, text=True, cwd=cwd)
    return p.returncode, p.stdout, p.stderr


def vimanam_v2(binary: str, args: list[str]) -> dict:
    rc, out, err = run([binary, "diff", *args, "--format", "json", "--omit-surfaces"])
    if rc not in (0, 3):
        raise RuntimeError(f"vimanam diff {' '.join(args)} failed ({rc}): {err[:500]}")
    return json.loads(out)


# ---------------------------------------------------------------- GraphQL


def type_ref(t) -> str:
    if not isinstance(t, dict):
        return ""
    kind, name, inner = t.get("kind"), t.get("name"), t.get("ofType")
    if kind == "NON_NULL":
        return type_ref(inner) + "!"
    if kind == "LIST":
        return "[" + type_ref(inner) + "]"
    return name or ""


def gql_coordinates(doc) -> dict[str, str]:
    """Coordinate -> canonical description-free value, for every schema member."""
    schema = (doc.get("data") or doc)["__schema"]
    out: dict[str, str] = {}
    for t in schema.get("types") or []:
        name = t.get("name") or ""
        if name.startswith("__"):
            continue
        out[name] = json.dumps({
            "kind": t.get("kind"),
            "interfaces": sorted(type_ref(i) for i in t.get("interfaces") or []),
            "possibleTypes": sorted(type_ref(i) for i in t.get("possibleTypes") or []),
        }, sort_keys=True)
        for f in (t.get("fields") or []) + (t.get("inputFields") or []):
            fc = f"{name}.{f['name']}"
            out[fc] = json.dumps({
                "type": type_ref(f.get("type")),
                "default": f.get("defaultValue"),
                "deprecated": [f.get("isDeprecated"), f.get("deprecationReason")],
            }, sort_keys=True)
            for a in f.get("args") or []:
                out[f"{fc}({a['name']}:)"] = json.dumps({
                    "type": type_ref(a.get("type")),
                    "default": a.get("defaultValue"),
                    "deprecated": [a.get("isDeprecated"), a.get("deprecationReason")],
                }, sort_keys=True)
        for v in t.get("enumValues") or []:
            out[f"{name}.{v['name']}"] = json.dumps({"deprecated": [v.get("isDeprecated"), v.get("deprecationReason")]}, sort_keys=True)
    for d in schema.get("directives") or []:
        dc = f"@{d['name']}"
        out[dc] = json.dumps({"locations": sorted(d.get("locations") or []), "repeatable": d.get("isRepeatable")}, sort_keys=True)
        for a in d.get("args") or []:
            out[f"{dc}({a['name']}:)"] = json.dumps({"type": type_ref(a.get("type")), "default": a.get("defaultValue")}, sort_keys=True)
    return out


def base_coordinate(coord: str) -> str:
    """`Type.field(arg:)` -> `Type.field`; `Type` stays `Type`."""
    return coord.split("(", 1)[0]


def gql_ground_truth(old, new) -> dict:
    a, b = gql_coordinates(old), gql_coordinates(new)
    changed = {c for c in a.keys() | b.keys() if a.get(c) != b.get(c)}
    return {
        "old": set(a),
        "new": set(b),
        "changed": changed,
        "changed_bases": {base_coordinate(c) for c in changed},
        "changed_types": {c.split(".", 1)[0] for c in changed},
        "known": set(a) | set(b),
    }


MARKERS = (("✖", "breaking"), ("⚠", "dangerous"), ("✔", "safe"))
# graphql-inspector prints unquoted prose; each pattern maps one message shape to a
# schema coordinate. Lines that match none are counted as unparsed and kept as
# evidence, so a new message shape shows up instead of silently vanishing.
NAME = r"[A-Za-z_][A-Za-z0-9_]*"
INSPECTOR_PATTERNS = [
    (re.compile(rf"^Field ({NAME}) was (?:added to|removed from) (?:object|interface) type ({NAME})"), lambda m: f"{m[2]}.{m[1]}"),
    (re.compile(rf"^Input field ({NAME}) (?:of type \S+ )?was (?:added to|removed from) input object type ({NAME})"), lambda m: f"{m[2]}.{m[1]}"),
    (re.compile(rf"^Argument ({NAME}):.* (?:added to|removed from) field ({NAME})\.({NAME})"), lambda m: f"{m[2]}.{m[3]}({m[1]}:)"),
    (re.compile(rf"^Enum value ({NAME}) was (?:added to|removed from) enum ({NAME})"), lambda m: f"{m[2]}.{m[1]}"),
    (re.compile(rf"^Field ({NAME})\.({NAME}) "), lambda m: f"{m[1]}.{m[2]}"),
    (re.compile(rf"^Type ({NAME}) was (?:added|removed)"), lambda m: m[1]),
    (re.compile(rf"^Directive ({NAME}) was (?:added|removed)"), lambda m: f"@{m[1]}"),
]


def inspector_findings(old_p: Path, new_p: Path, known: set[str]) -> tuple[list[dict], int, str]:
    rc, out, err = run(["graphql-inspector", "diff", str(old_p), str(new_p)])
    text = out + err
    findings, unparsed = [], 0
    for line in text.splitlines():
        level = next((lvl for mark, lvl in MARKERS if mark in line), None)
        if level is None:
            continue
        message = line.split(next(mark for mark, _ in MARKERS if mark in line), 1)[1].strip()
        coord = next((build(m) for rx, build in INSPECTOR_PATTERNS if (m := rx.search(message))), None)
        if coord is None:
            unparsed += 1
        findings.append({"level": level, "coordinate": coord, "line": line.strip()})
    return findings, unparsed, text


def check_graphql_pair(binary: str, repo: Path, old_rev: str, new_rev: str, tmp: Path, evidence: Path | None) -> dict:
    old_p, new_p = tmp / f"{old_rev}.json", tmp / f"{new_rev}.json"
    for rev, dest in ((old_rev, old_p), (new_rev, new_p)):
        dest.write_bytes(subprocess.check_output(["git", "-C", str(repo), "show", f"{rev}:schema.json"]))
    gt = gql_ground_truth(json.loads(old_p.read_text()), json.loads(new_p.read_text()))

    vim = vimanam_v2(binary, [str(old_p), str(new_p)])["changes"]
    vim_rows = [{"severity": c.get("severity"), "coordinate": (c.get("native") or {}).get("coordinate"), "kind": c.get("native_kind")} for c in vim]
    ins, unparsed, ins_text = inspector_findings(old_p, new_p, gt["known"])

    def grounded(coord: str | None) -> bool:
        if not coord:
            return False
        b = base_coordinate(coord)
        return b in gt["changed_bases"] or b.split(".", 1)[0] in gt["changed_types"]

    vim_breaking = {base_coordinate(r["coordinate"]) for r in vim_rows if r["severity"] == "breaking" and r["coordinate"]}
    ins_breaking = {base_coordinate(r["coordinate"]) for r in ins if r["level"] == "breaking" and r["coordinate"]}
    vim_reported = {base_coordinate(r["coordinate"]) for r in vim_rows if r["coordinate"]}
    ins_reported = {base_coordinate(r["coordinate"]) for r in ins if r["coordinate"]}
    vim_reported_types = {c.split(".", 1)[0] for c in vim_reported}
    ins_reported_types = {c.split(".", 1)[0] for c in ins_reported}

    added_or_removed = {c for c in gt["changed"] if "." not in c and "(" not in c and (c not in gt["old"] or c not in gt["new"])}

    def covered(bases: set[str], types: set[str], c: str) -> bool:
        # A member of a type that was added or removed counts as covered when the
        # tool reports the type itself: both tools report a new type once rather
        # than listing every field of it.
        b = base_coordinate(c)
        owner = b.split(".", 1)[0] if not b.startswith("@") else b.split("(", 1)[0]
        return b in bases or ("." not in b and b in types) or (owner in added_or_removed and owner in bases)

    vim_only, ins_only = sorted(vim_breaking - ins_breaking), sorted(ins_breaking - vim_breaking)
    if evidence is not None:
        d = evidence / f"{old_rev}..{new_rev}"
        d.mkdir(parents=True, exist_ok=True)
        (d / "vimanam-only.json").write_text(json.dumps({c: [r for r in vim_rows if r["coordinate"] and base_coordinate(r["coordinate"]) == c] for c in vim_only}, indent=1))
        (d / "inspector-only.json").write_text(json.dumps({c: [r for r in ins if r["coordinate"] and base_coordinate(r["coordinate"]) == c] for c in ins_only}, indent=1))
        (d / "inspector-output.txt").write_text(ins_text)
        (d / "changed-coordinates.txt").write_text("\n".join(sorted(gt["changed"])) + "\n")
    return {
        "pair": f"{old_rev}..{new_rev}",
        "ground_truth": {"changed_coordinates": len(gt["changed"]), "changed_types": len(gt["changed_types"])},
        "vimanam": {
            "findings": len(vim_rows),
            "by_severity": dict(Counter(r["severity"] for r in vim_rows)),
            "breaking_coordinates": len(vim_breaking),
            "ungrounded_findings": len([r for r in vim_rows if not grounded(r["coordinate"])]),
            "changed_coordinates_not_reported": len([c for c in gt["changed"] if not covered(vim_reported, vim_reported_types, c)]),
        },
        "graphql_inspector": {
            "findings": len(ins),
            "by_level": dict(Counter(r["level"] for r in ins)),
            "unparsed_lines": unparsed,
            "breaking_coordinates": len(ins_breaking),
            "ungrounded_findings": len([r for r in ins if r["coordinate"] and not grounded(r["coordinate"])]),
            "changed_coordinates_not_reported": len([c for c in gt["changed"] if not covered(ins_reported, ins_reported_types, c)]),
        },
        "agreement": {
            "breaking_in_both": len(vim_breaking & ins_breaking),
            "breaking_vimanam_only": vim_only,
            "breaking_inspector_only": ins_only,
        },
    }


def check_graphql_consistency(binary: str, repo: Path, rev: str, tmp: Path, evidence: Path | None) -> dict:
    sdl, intro = tmp / f"{rev}.graphql", tmp / f"{rev}.introspection.json"
    sdl.write_bytes(subprocess.check_output(["git", "-C", str(repo), "show", f"{rev}:schema.graphql"]))
    intro.write_bytes(subprocess.check_output(["git", "-C", str(repo), "show", f"{rev}:schema.json"]))
    changes = vimanam_v2(binary, [str(sdl), str(intro)])["changes"]
    if evidence is not None and changes:
        d = evidence / "sdl-vs-introspection"
        d.mkdir(parents=True, exist_ok=True)
        (d / f"{rev}.json").write_text(json.dumps([
            {"severity": c.get("severity"), "coordinate": (c.get("native") or {}).get("coordinate"), "kind": c.get("native_kind"), "details": c.get("details")}
            for c in changes], indent=1))
    return {"commit": rev, "changes": len(changes), "by_severity": dict(Counter(c.get("severity") for c in changes))}


def graphql(args) -> dict:
    binary = str(Path(args.binary).resolve())
    repo = Path(args.corpus).resolve() / "octokit-graphql"
    evidence = Path(args.evidence_dir).resolve() if args.evidence_dir else None
    commits = OCTOKIT_SCHEMA_JSON_COMMITS
    results = {
        "environment": {
            "vimanam": subprocess.check_output([binary, "--version"], text=True).strip(),
            "graphql_inspector": run(["brew", "list", "--versions", "graphql-inspector"])[1].strip() or "graphql-inspector",
        },
        "pairs": [],
        "sdl_vs_introspection": [],
    }
    with tempfile.TemporaryDirectory(prefix="vimanam-accuracy-gql-") as td:
        tmp = Path(td)
        for old_rev, new_rev in zip(commits, commits[1:]):
            row = check_graphql_pair(binary, repo, old_rev, new_rev, tmp, evidence)
            results["pairs"].append(row)
            v, g, a = row["vimanam"], row["graphql_inspector"], row["agreement"]
            print(f"{row['pair']}: changed={row['ground_truth']['changed_coordinates']} breaking vimanam={v['breaking_coordinates']} "
                  f"inspector={g['breaking_coordinates']} both={a['breaking_in_both']} vimanam_only={len(a['breaking_vimanam_only'])} "
                  f"inspector_only={len(a['breaking_inspector_only'])} ungrounded vimanam={v['ungrounded_findings']} inspector={g['ungrounded_findings']} "
                  f"unreported vimanam={v['changed_coordinates_not_reported']} inspector={g['changed_coordinates_not_reported']} "
                  f"inspector_unparsed={g['unparsed_lines']}", flush=True)
        for rev in commits:
            row = check_graphql_consistency(binary, repo, rev, tmp, evidence)
            results["sdl_vs_introspection"].append(row)
            print(f"SDL vs introspection at {rev}: {row['changes']} changes {row['by_severity']}", flush=True)
    return results


# ---------------------------------------------------------------- Protobuf


def extract(repo: Path, ref: str, dest: Path) -> None:
    dest.mkdir(parents=True, exist_ok=True)
    data = subprocess.check_output(["git", "-C", str(repo), "archive", "--format=tar", ref, "opentelemetry"])
    with tarfile.open(fileobj=io.BytesIO(data)) as tar:
        tar.extractall(dest, filter="data")


def protos(root: Path) -> set[str]:
    return {str(p.relative_to(root)) for p in (root / "opentelemetry/proto").rglob("*.proto")}


BUF_V1 = '{{"version":"v1","breaking":{{"use":["{rule}"]}}}}'


def buf_findings(old_root: Path, new_root: Path, rule: str) -> list[dict]:
    rc, out, err = run(["buf", "breaking", str(new_root), "--against", str(old_root),
                        "--config", BUF_V1.format(rule=rule), "--against-config", BUF_V1.format(rule=rule),
                        "--error-format", "json"])
    if rc not in (0, 100):
        raise RuntimeError(f"buf breaking ({rule}) failed ({rc}): {(err or out)[:500]}")
    return [json.loads(line) for line in out.splitlines() if line.strip()]


def check_protobuf_pair(binary: str, repo: Path, old_tag: str, new_tag: str, tmp: Path, evidence: Path | None) -> dict:
    old_root, new_root = tmp / old_tag, tmp / new_tag
    extract(repo, old_tag, old_root)
    extract(repo, new_tag, new_root)
    old_files, new_files = protos(old_root), protos(new_root)
    shared = sorted(old_files & new_files)
    changed = [f for f in shared if (old_root / f).read_bytes() != (new_root / f).read_bytes()]

    per_file: dict[str, dict] = {}
    for f in shared:
        doc = vimanam_v2(binary, [str(old_root / f), str(new_root / f),
                                  "--old-proto-path", str(old_root), "--new-proto-path", str(new_root)])
        rows = [{"severity": c.get("severity"), "impacts": c.get("impacts"), "full_name": (c.get("native") or {}).get("full_name"),
                 "number": (c.get("native") or {}).get("number"), "kind": c.get("native_kind")} for c in doc["changes"]]
        per_file[f] = {
            "vimanam_findings": rows,
            "vimanam_wire_breaking": [r for r in rows if r["severity"] == "breaking" and "wire" in (r["impacts"] or [])],
            "vimanam_source_breaking": [r for r in rows if r["severity"] == "breaking"],
        }
    wire = buf_findings(old_root, new_root, "WIRE")
    file_rules = buf_findings(old_root, new_root, "FILE")

    # Compare the messages, enums and services touched, not files: `vimanam diff` of one file also reports the
    # definitions it imports, so the same change appears under several files. An
    # element key is the last two segments of a full name (`Message.field`,
    # `Enum.VALUE`, `Service.Rpc`), or one segment for a whole message, enum or
    # service; buf messages are parsed into the same shape.
    def vim_key(full_name: str | None, number) -> str | None:
        # The message, enum or service a change belongs to. Finer keys don't work:
        # buf names a renamed or renumbered field by its new name and number, and
        # vimanam by its old source identity, so the same change gets two names.
        if not full_name:
            return None
        parts = full_name.split(".")
        is_member = number is not None or (len(parts) > 1 and parts[-1][:1].islower())
        return parts[-2] if is_member and len(parts) > 1 else parts[-1]

    vim_wire = {vim_key(r["full_name"], r["number"]) for d in per_file.values() for r in d["vimanam_wire_breaking"]} - {None}
    vim_source = {vim_key(r["full_name"], r["number"]) for d in per_file.values() for r in d["vimanam_source_breaking"]} - {None}

    member = re.compile(r'on (?:message|enum|service) "([^"]+)"', re.I)
    whole = re.compile(r'(?:message|enum|service) "([^"]+)"', re.I)

    def buf_key(message: str) -> str | None:
        if m := member.search(message):
            return m[1].split(".")[-1]
        if m := whole.search(message):
            return m[1].split(".")[-1]
        return None

    def buf_keys(findings: list[dict]) -> tuple[set[str], list[str]]:
        keys, unparsed = set(), []
        for x in findings:
            k = buf_key(x.get("message") or "")
            (keys.add(k) if k else unparsed.append(x.get("message") or ""))
        return keys, unparsed

    buf_wire, wire_unparsed = buf_keys(wire)
    buf_file, file_unparsed = buf_keys(file_rules)

    if evidence is not None:
        d = evidence / f"{old_tag}..{new_tag}"
        d.mkdir(parents=True, exist_ok=True)
        (d / "elements.json").write_text(json.dumps({
            "wire_vimanam_only": sorted(vim_wire - buf_wire), "wire_buf_only": sorted(buf_wire - vim_wire),
            "source_vimanam_only": sorted(vim_source - buf_file), "source_buf_only": sorted(buf_file - vim_source),
            "buf_unparsed": wire_unparsed + file_unparsed,
        }, indent=1))
        (d / "buf.json").write_text(json.dumps({"WIRE": wire, "FILE": file_rules}, indent=1))
        (d / "vimanam-per-file.json").write_text(json.dumps({f: v for f, v in per_file.items() if v["vimanam_findings"]}, indent=1))
    return {
        "pair": f"{old_tag}..{new_tag}",
        "files": {"shared": len(shared), "changed": len(changed),
                  "added": sorted(new_files - old_files), "removed": sorted(old_files - new_files)},
        "vimanam": {"wire_breaking_elements": len(vim_wire), "source_breaking_elements": len(vim_source)},
        "buf": {
            "wire_breaking_elements": len(buf_wire), "file_breaking_elements": len(buf_file),
            "wire_rules": dict(Counter(x.get("type") for x in wire)),
            "file_rules": dict(Counter(x.get("type") for x in file_rules)),
            "unparsed_messages": len(wire_unparsed) + len(file_unparsed),
        },
        "agreement": {
            "wire_both": len(vim_wire & buf_wire),
            "wire_vimanam_only": sorted(vim_wire - buf_wire),
            "wire_buf_only": sorted(buf_wire - vim_wire),
            "source_both": len(vim_source & buf_file),
            "source_vimanam_only": sorted(vim_source - buf_file),
            "source_buf_only": sorted(buf_file - vim_source),
        },
    }


def protobuf(args) -> dict:
    binary = str(Path(args.binary).resolve())
    repo = Path(args.corpus).resolve() / "otel-proto"
    evidence = Path(args.evidence_dir).resolve() if args.evidence_dir else None
    results = {
        "environment": {
            "vimanam": subprocess.check_output([binary, "--version"], text=True).strip(),
            "buf": subprocess.check_output(["buf", "--version"], text=True).strip(),
            "comparison": "vimanam wire-impact breaking vs buf WIRE; vimanam breaking vs buf FILE; per .proto file",
        },
        "pairs": [],
    }
    with tempfile.TemporaryDirectory(prefix="vimanam-accuracy-pb-") as td:
        for old_tag, new_tag in zip(OTEL_TAGS, OTEL_TAGS[1:]):
            row = check_protobuf_pair(binary, repo, old_tag, new_tag, Path(td) / f"{old_tag}..{new_tag}", evidence)
            results["pairs"].append(row)
            v, b, a = row["vimanam"], row["buf"], row["agreement"]
            print(f"{row['pair']}: files changed={row['files']['changed']} added={len(row['files']['added'])} removed={len(row['files']['removed'])} "
                  f"wire: vimanam={v['wire_breaking_elements']} buf={b['wire_breaking_elements']} both={a['wire_both']} "
                  f"vimanam_only={len(a['wire_vimanam_only'])} buf_only={len(a['wire_buf_only'])} | "
                  f"source: vimanam={v['source_breaking_elements']} buf_file={b['file_breaking_elements']} both={a['source_both']} "
                  f"vimanam_only={len(a['source_vimanam_only'])} buf_only={len(a['source_buf_only'])} | buf_unparsed={b['unparsed_messages']}", flush=True)
    return results


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("format", choices=["graphql", "protobuf"])
    ap.add_argument("--binary", required=True, help="vimanam binary to check")
    ap.add_argument("--corpus", required=True, help="directory holding octokit-graphql/ and otel-proto/ clones")
    ap.add_argument("--output", required=True, help="JSON results file")
    ap.add_argument("--evidence-dir", help="write per-pair disagreement evidence here")
    args = ap.parse_args()
    results = graphql(args) if args.format == "graphql" else protobuf(args)
    Path(args.output).write_text(json.dumps(results, indent=1) + "\n")
    print(f"Wrote {args.output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
