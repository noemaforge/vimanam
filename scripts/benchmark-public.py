#!/usr/bin/env python3
"""Reproducible public benchmark and external-baseline suite for Vimanam.

- Reports robust medians, min-max ranges, Q1-Q3 IQRs, disjoint-IQR checks, and
  full per-run sample lists (samples_ms).
- Pins exact public commit SHAs (with fallback only when a local clone omits them)
  so diff comparisons remain reproducible as upstream repos advance.
- Covers all 4 external baselines:
  * oasdiff 1.39.0 on OpenAI 4.9 MB (a01e367..373f26a) + full breaking-change
    reconciliation + bounded 15s probe on Stripe 7.9 MB (2f8d147..5237fa0).
  * buf breaking 1.75.0 on multi-file OpenTelemetry Proto Git-ref (40cb2e5..d31f7ac).
  * graphql-inspector 7.0.8 on SpaceX Land, GitHub 5.0 MB Introspection JSON
    (a6de3c1..597478f), and GitHub 1.2 MB SDL duplicate-field probe.
  * protoc-gen-doc 1.5.1 on gRPC Health and OpenTelemetry TraceService.
- Uses tiktoken (o200k_base) alongside chars/4; Claude API token calibration is
  measured directly in scripts/eval-context-accuracy.py.
"""

from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
import os
import platform
import re
import shlex
import shutil
import statistics
import subprocess
import tempfile
import time
from pathlib import Path

import tiktoken

REPO = Path(__file__).resolve().parent.parent
ENC = tiktoken.get_encoding("o200k_base")

# Pinned upstream SHAs so diff benchmarks reproduce exact revisions later
PINNED_REVS = {
    "openai_old": "a01e367",
    "openai_new": "373f26a",
    "stripe_old": "2f8d147",
    "stripe_new": "5237fa0",
    "otel_old": "40cb2e5",
    "otel_new": "d31f7ac",
    "octo_old": "a6de3c1",
    "octo_new": "597478f",
}


def resolve_rev(repo: Path, pinned: str, fallback: str) -> str:
    """Return pinned commit SHA when present in repo, else fall back to relative ref."""
    p = subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "--verify", "--quiet", f"{pinned}^{{commit}}"],
        capture_output=True,
        text=True,
    )
    ref = pinned if p.returncode == 0 else fallback
    return subprocess.check_output(["git", "-C", str(repo), "rev-parse", "--short", ref], text=True).strip()


def sh_quote(parts) -> str:
    return " ".join(shlex.quote(str(p)) for p in parts)


def file_sha256(path: str | Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def count_bpe(text: str) -> int:
    return len(ENC.encode(text, disallowed_special=()))


def chars_div_4(text: str) -> int:
    return (len(text) + 3) // 4


def run_cmd(args, cwd=None, env=None, timeout=180):
    t0 = time.perf_counter()
    p = subprocess.run(args, cwd=cwd, env=env, capture_output=True, timeout=timeout)
    return p.returncode, p.stdout, p.stderr, time.perf_counter() - t0


def quantiles_ms(samples_ms: list[float]) -> tuple[float, float]:
    s = sorted(samples_ms)
    if len(s) < 4:
        return s[0], s[-1]
    qs = statistics.quantiles(s, n=4)
    return round(qs[0], 2), round(qs[2], 2)


def parse_hf_result(result: dict) -> dict:
    summary = result["summary"]
    w = summary["time_wall_clock"]
    m = summary.get("memory_peak_resident", {})
    i = summary.get("instructions", {})
    samples = [round(x["time_wall_clock"]["value"] * 1000, 2) for x in result.get("measurements", [])]
    q1, q3 = quantiles_ms(samples) if samples else (round(w["min"] * 1000, 2), round(w["max"] * 1000, 2))
    return {
        "count": w["count"],
        "median_ms": round(w["median"] * 1000, 2),
        "q1_ms": q1,
        "q3_ms": q3,
        "min_ms": round(w["min"] * 1000, 2),
        "max_ms": round(w["max"] * 1000, 2),
        "mean_ms": round(w["mean"] * 1000, 2),
        "stddev_ms": round((w.get("stddev") or 0.0) * 1000, 2),
        "rss_mb": round(m["median"] / (1024 * 1024), 1) if "median" in m else None,
        "instructions_m": round(i["median"] / 1e6, 1) if "median" in i else None,
        "samples_ms": samples,
    }


def hyperfine_multi(commands: list[tuple[str, str]], runs: int, warmup: int = 2, cwd=None, ignore_failure: bool = False) -> dict:
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as tf:
        out_json = Path(tf.name)
    try:
        args = ["hyperfine", "--warmup", str(warmup), "--runs", str(runs), "--export-json", str(out_json)]
        if ignore_failure:
            args.insert(1, "--ignore-failure")
        for name, cmd in commands:
            args.extend(["-n", name, cmd])
        p = subprocess.run(args, cwd=cwd, capture_output=True, text=True)
        if p.returncode != 0:
            return {"error": p.stderr.strip()[:300]}
        data = json.loads(out_json.read_text())
        out = {}
        for idx, (name, _) in enumerate(commands):
            out[name] = parse_hf_result(data["results"][idx])
        if len(commands) == 2:
            a = out[commands[0][0]]
            b = out[commands[1][0]]
            out["median_speedup_a_over_b"] = round(a["median_ms"] / b["median_ms"], 2) if b["median_ms"] > 0 else None
            out["iqrs_disjoint"] = (a["q3_ms"] < b["q1_ms"]) or (b["q3_ms"] < a["q1_ms"])
            out["ranges_disjoint"] = (a["max_ms"] < b["min_ms"]) or (b["max_ms"] < a["min_ms"])
        return out
    finally:
        out_json.unlink(missing_ok=True)


def build_synthetic_200_proto_repo(tmpdir: Path) -> Path:
    """Create a reproducible 200-file .proto Git repo via git fast-import."""
    repo = tmpdir / "proto200"
    repo.mkdir()
    subprocess.run(["git", "init", "-q", "-b", "main"], cwd=repo, check=True)
    lines: list[str] = []

    def add_msg(msg: str):
        b = msg.encode("utf-8")
        lines.append(f"data {len(b)}\n{msg}")

    def add_blob(path: str, content: str):
        data = content.encode("utf-8")
        lines.append(f"M 100644 inline {path}")
        lines.append(f"data {len(data)}")
        lines.append(content)

    lines.extend([
        "commit refs/heads/main",
        "mark :1",
        "author Bench <bench@example.invalid> 1700000000 +0000",
        "committer Bench <bench@example.invalid> 1700000000 +0000",
    ])
    add_msg("initial\n")
    for i in range(196):
        pkg = f"pkg_{i // 20}"
        add_blob(
            f"protos/{pkg}/model_{i:03d}.proto",
            f'syntax = "proto3";\npackage demo.{pkg};\nmessage Model{i:03d} {{ string id = 1; int64 count = 2; }}\n',
        )
    add_blob("protos/common/types.proto", 'syntax = "proto3";\npackage demo.common;\nmessage Money { string currency = 1; int64 units = 2; }\n')
    add_blob("protos/common/meta.proto", 'syntax = "proto3";\npackage demo.common;\nmessage RequestMeta { string request_id = 1; }\n')
    add_blob("protos/orders/item.proto", 'syntax = "proto3";\npackage demo.orders;\nimport "common/types.proto";\nmessage LineItem { string sku = 1; demo.common.Money price = 2; }\n')
    target_v1 = (
        'syntax = "proto3";\npackage demo.orders;\nimport "common/meta.proto";\nimport "orders/item.proto";\n'
        'message CreateOrderRequest { demo.common.RequestMeta meta = 1; repeated LineItem items = 2; }\n'
        'message CreateOrderResponse { string order_id = 1; }\n'
        'service OrderService { rpc CreateOrder(CreateOrderRequest) returns (CreateOrderResponse); }\n'
    )
    add_blob("protos/orders/service.proto", target_v1)

    target_v2 = target_v1.replace("repeated LineItem items = 2;", "repeated LineItem items = 2; string idempotency_key = 3;")
    lines.extend([
        "commit refs/heads/main",
        "mark :2",
        "author Bench <bench@example.invalid> 1700000100 +0000",
        "committer Bench <bench@example.invalid> 1700000100 +0000",
    ])
    add_msg("add field v2\n")
    lines.append("from :1")
    add_blob("protos/orders/service.proto", target_v2)
    lines.append("done\n")

    subprocess.run(["git", "fast-import", "--quiet"], cwd=repo, input="\n".join(lines).encode("utf-8"), check=True)
    subprocess.run(["git", "checkout", "-q", "main"], cwd=repo, check=True)
    subprocess.run(["git", "tag", "v1", "HEAD~1"], cwd=repo, check=True)
    subprocess.run(["git", "tag", "v2", "HEAD"], cwd=repo, check=True)
    return repo


def run_oasdiff_comparison(v22: str, openai_repo: Path, stripe_repo: Path, tmpdir: Path) -> dict:
    """Compare vimanam diff vs oasdiff 1.39.0 at 4.9 MB OpenAI scale, 7.9 MB Stripe probe, and 6-case smoke matrix."""
    out: dict = {"oasdiff_version": subprocess.check_output(["oasdiff", "--version"], text=True).strip()}

    # 1. Real 4.9 MB OpenAI OpenAPI 3.0 diff (pinned a01e367..373f26a)
    oa_old = tmpdir / "openai_old.json"
    oa_new = tmpdir / "openai_new.json"
    oa_old_sha = resolve_rev(openai_repo, PINNED_REVS["openai_old"], "HEAD~5")
    oa_new_sha = resolve_rev(openai_repo, PINNED_REVS["openai_new"], "HEAD")
    oa_old.write_bytes(subprocess.check_output(["git", "-C", str(openai_repo), "show", f"{oa_old_sha}:openapi.json"]))
    oa_new.write_bytes(subprocess.check_output(["git", "-C", str(openai_repo), "show", f"{oa_new_sha}:openapi.json"]))

    vmd_rc, vmd_out, _, _ = run_cmd([v22, "diff", str(oa_old), str(oa_new)])
    vv1_rc, vv1_out, _, _ = run_cmd([v22, "diff", str(oa_old), str(oa_new), "--format", "json", "--contract", "1"])
    vv2_rc, vv2_out, _, _ = run_cmd([v22, "diff", str(oa_old), str(oa_new), "--format", "json", "--contract", "2"])
    ob_rc, ob_out, _, _ = run_cmd(["oasdiff", "breaking", str(oa_old), str(oa_new), "--format", "json"])
    oc_rc, oc_out, _, _ = run_cmd(["oasdiff", "changelog", str(oa_old), str(oa_new), "--format", "json"])
    vj1 = json.loads(vv1_out) if vv1_rc == 0 else None
    vj2 = json.loads(vv2_out) if vv2_rc == 0 else None
    oj_b = json.loads(ob_out) if (ob_rc == 0 and ob_out.strip()) else []
    oj_c = json.loads(oc_out) if (oc_rc == 0 and oc_out.strip()) else []

    reconciliation = {}
    if isinstance(vj2, dict):
        ents = {e["id"]: e for e in vj2["old"]["contract"]["entities"]}
        ents.update({e["id"]: e for e in vj2["new"]["contract"]["entities"]})
        v_brk = [c for c in vj2["changes"] if c["severity"] == "breaking"]
        reconciliation = {
            "vimanam_breaking_by_category": {
                f"{ents.get(c['subject'], {}).get('kind')}:{c['kind']}": n
                for (k, n) in Counter((ents.get(c["subject"], {}).get("kind"), c["kind"]) for c in v_brk).most_common()
                for c in [next(x for x in v_brk if (ents.get(x["subject"], {}).get("kind"), x["kind"]) == k)]
            },
            "oasdiff_breaking_by_id": dict(Counter(x.get("id", "") for x in oj_b if isinstance(x, dict)).most_common()),
            "oasdiff_changelog_by_level_and_id": {
                f"level_{lvl}:{cid}": n
                for ((lvl, cid), n) in Counter((x.get("level"), x.get("id", "")) for x in oj_c if isinstance(x, dict)).most_common()
            },
        }

    out["openai_4_9mb_diff"] = {
        "old_commit": oa_old_sha,
        "new_commit": oa_new_sha,
        "old_bytes": oa_old.stat().st_size,
        "new_bytes": oa_new.stat().st_size,
        "vimanam_md_bytes": len(vmd_out),
        "vimanam_v1_json_bytes": len(vv1_out),
        "vimanam_v2_json_bytes": len(vv2_out),
        "vimanam_v1_summary": vj1.get("summary") if isinstance(vj1, dict) else None,
        "vimanam_v2_summary": vj2.get("summary") if isinstance(vj2, dict) else None,
        "oasdiff_breaking_count": len(oj_b) if isinstance(oj_b, list) else None,
        "oasdiff_changelog_count": len(oj_c) if isinstance(oj_c, list) else None,
        "breaking_reconciliation": reconciliation,
        "hyperfine": hyperfine_multi(
            [
                ("oasdiff_breaking", sh_quote(["oasdiff", "breaking", str(oa_old), str(oa_new), "--format", "json"])),
                ("oasdiff_changelog", sh_quote(["oasdiff", "changelog", str(oa_old), str(oa_new), "--format", "json"])),
                ("vimanam_diff_md", sh_quote([v22, "diff", str(oa_old), str(oa_new)])),
                ("vimanam_diff_v1_json", sh_quote([v22, "diff", str(oa_old), str(oa_new), "--format", "json", "--contract", "1"])),
                ("vimanam_diff_v2_json", sh_quote([v22, "diff", str(oa_old), str(oa_new), "--format", "json", "--contract", "2"])),
            ],
            runs=5,
            warmup=1,
            ignore_failure=True,
        ),
    }

    # 2. Bounded 15-second probe on 7.9 MB Stripe OpenAPI diff (pinned 2f8d147..5237fa0)
    s_old = tmpdir / "stripe_old.json"
    s_new = tmpdir / "stripe_new.json"
    s_old_sha = resolve_rev(stripe_repo, PINNED_REVS["stripe_old"], "HEAD~5")
    s_new_sha = resolve_rev(stripe_repo, PINNED_REVS["stripe_new"], "HEAD")
    s_old.write_bytes(subprocess.check_output(["git", "-C", str(stripe_repo), "show", f"{s_old_sha}:openapi/spec3.json"]))
    s_new.write_bytes(subprocess.check_output(["git", "-C", str(stripe_repo), "show", f"{s_new_sha}:openapi/spec3.json"]))

    stripe_probe: dict = {
        "old_commit": s_old_sha,
        "new_commit": s_new_sha,
        "old_bytes": s_old.stat().st_size,
        "new_bytes": s_new.stat().st_size,
        "timeout_s": 15,
    }
    for key, cmd in [
        ("oasdiff_breaking", ["oasdiff", "breaking", str(s_old), str(s_new), "--format", "json"]),
        ("vimanam_diff_md", [v22, "diff", str(s_old), str(s_new)]),
        ("vimanam_diff_v2_json", [v22, "diff", str(s_old), str(s_new), "--format", "json", "--contract", "2"]),
    ]:
        t0 = time.perf_counter()
        try:
            p = subprocess.run(cmd, capture_output=True, timeout=15)
            stripe_probe[key] = {
                "status": "ok" if p.returncode == 0 else f"exit_{p.returncode}",
                "wall_ms": round((time.perf_counter() - t0) * 1000, 1),
                "stdout_bytes": len(p.stdout),
            }
        except subprocess.TimeoutExpired:
            stripe_probe[key] = {"status": "timeout_15s", "issue": "#152"}
    out["stripe_7_9mb_probe"] = stripe_probe

    # 2. 6-case OpenAPI smoke matrix
    def base_spec():
        return {
            "openapi": "3.0.3",
            "info": {"title": "Orders", "version": "1.0.0"},
            "paths": {
                "/v1/items": {
                    "get": {
                        "operationId": "listItems",
                        "parameters": [{"name": "limit", "in": "query", "required": False, "schema": {"type": "integer"}}],
                        "responses": {
                            "200": {
                                "description": "OK",
                                "content": {
                                    "application/json": {
                                        "schema": {
                                            "type": "object",
                                            "required": ["id"],
                                            "properties": {"id": {"type": "string"}, "name": {"type": "string"}},
                                        }
                                    }
                                },
                            }
                        },
                    },
                    "post": {
                        "operationId": "createItem",
                        "requestBody": {
                            "required": True,
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "required": ["name"],
                                        "properties": {"name": {"type": "string"}, "note": {"type": "string"}},
                                    }
                                }
                            },
                        },
                        "responses": {"201": {"description": "Created"}},
                    },
                }
            },
        }

    def m1(s): del s["paths"]["/v1/items"]["post"]
    def m2(s): s["paths"]["/v1/items"]["get"]["parameters"].append({"name": "tenant", "in": "query", "required": True, "schema": {"type": "string"}})
    def m3(s): s["paths"]["/v1/items"]["post"]["requestBody"]["content"]["application/json"]["schema"]["required"].append("note")
    def m4(s): del s["paths"]["/v1/items"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]["properties"]["name"]
    def m5(s): s["paths"]["/v1/items"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]["properties"]["sku"] = {"type": "string"}
    def m6(s): s["paths"]["/v2/items"] = s["paths"].pop("/v1/items")

    mat_dir = tmpdir / "oas_matrix"
    mat_dir.mkdir()
    old_p = mat_dir / "old.json"
    old_p.write_text(json.dumps(base_spec(), indent=2))
    out["smoke_matrix"] = []
    for idx, (label, fn) in enumerate([
        ("Endpoint removed (POST /v1/items)", m1),
        ("Required query parameter added", m2),
        ("Request body property made required", m3),
        ("Response property removed (optional)", m4),
        ("Optional response property added", m5),
        ("Versioned path moved (/v1/items -> /v2/items)", m6),
    ]):
        s = base_spec(); fn(s)
        new_p = mat_dir / f"new_{idx}.json"
        new_p.write_text(json.dumps(s, indent=2))
        v_rc, v_out, _, _ = run_cmd([v22, "diff", str(old_p), str(new_p), "--format", "json", "--fail-on-breaking"])
        o_rc, o_out, _, _ = run_cmd(["oasdiff", "breaking", str(old_p), str(new_p), "--format", "json", "--fail-on", "ERR"])
        vj = json.loads(v_out)
        oj = json.loads(o_out) if o_out.strip() else []
        out["smoke_matrix"].append({
            "case": label,
            "vimanam_breaking": (v_rc == 3),
            "vimanam_kinds": sorted({c["kind"] for c in vj["changes"]}),
            "oasdiff_breaking": (o_rc != 0),
            "oasdiff_ids": sorted({x.get("id", "") for x in oj if isinstance(x, dict)}),
            "verdict_match": ((v_rc == 3) == (o_rc != 0)),
        })
    return out


def run_buf_comparison(v22: str, otel_repo: Path, tmpdir: Path) -> dict:
    """Compare vimanam diff vs buf breaking 1.75.0 on real multi-file OTel Git-ref + 8-case smoke matrix."""
    out: dict = {"buf_version": subprocess.check_output(["buf", "--version"], text=True).strip()}

    # 1. Real multi-file OpenTelemetry Proto Git-ref comparison (pinned 40cb2e5..d31f7ac)
    # Using Git-ref mode isolates -I roots per commit accurately for both tools.
    old_sha = resolve_rev(otel_repo, PINNED_REVS["otel_old"], "HEAD~5")
    new_sha = resolve_rev(otel_repo, PINNED_REVS["otel_new"], "HEAD")
    out["otel_git_ref_comparison"] = {
        "old_commit": old_sha,
        "new_commit": new_sha,
        "scope_note": "buf breaking checks all 11 .proto files in the OTel module using go-git in-process; vimanam diff checks trace_service.proto plus its 3 transitive imports (4 files) via git subprocess blob resolution.",
        "hyperfine": hyperfine_multi(
            [
                ("buf_breaking_git", sh_quote(["buf", "breaking", f".git#ref={new_sha}", "--against", f".git#ref={old_sha}"])),
                ("vimanam_git_diff_trace", sh_quote([v22, "diff", "--from-ref", old_sha, "--to-ref", new_sha, "--spec", "opentelemetry/proto/collector/trace/v1/trace_service.proto", "-I", ".", "--format", "json"])),
            ],
            runs=6,
            warmup=1,
            cwd=str(otel_repo),
            ignore_failure=True,
        ),
    }

    # 2. 8-case Protobuf smoke matrix
    base_proto = '''syntax = "proto3";
package demo.v1;

enum Status {
  STATUS_UNSPECIFIED = 0;
  STATUS_ACTIVE = 1;
  STATUS_ARCHIVED = 2;
}

message Order {
  string id = 1;
  int32 quantity = 2;
  Status status = 3;
}

service OrderService {
  rpc GetOrder(Order) returns (Order);
}
'''
    cases = [
        ("Field type changed (int32 -> string)", base_proto.replace("int32 quantity = 2;", "string quantity = 2;")),
        ("Field removed without reserving number", base_proto.replace("  int32 quantity = 2;\n", "")),
        ("Field removed with number reserved", base_proto.replace("  int32 quantity = 2;\n", "  reserved 2;\n")),
        ("Same-number field rename (quantity -> qty)", base_proto.replace("int32 quantity = 2;", "int32 qty = 2;")),
        ("JSON name override changed", base_proto.replace("int32 quantity = 2;", 'int32 quantity = 2 [json_name = "orderQty"];')),
        ("Enum value removed without reserving number", base_proto.replace("  STATUS_ARCHIVED = 2;\n", "")),
        ("RPC changed from unary to server streaming", base_proto.replace("returns (Order);", "returns (stream Order);")),
        ("Optional field added (non-breaking)", base_proto.replace("  Status status = 3;\n", "  Status status = 3;\n  string note = 4;\n")),
    ]
    mdir = tmpdir / "buf_matrix"
    (mdir / "old").mkdir(parents=True)
    (mdir / "old/api.proto").write_text(base_proto)
    (mdir / "old/buf.yaml").write_text("version: v2\n")

    out["smoke_matrix"] = []
    for idx, (label, new_src) in enumerate(cases):
        ndir = mdir / f"new_{idx}"
        ndir.mkdir()
        (ndir / "api.proto").write_text(new_src)
        (ndir / "buf.yaml").write_text("version: v2\nbreaking:\n  use:\n    - WIRE\n")
        bw_rc, _, _, _ = run_cmd(["buf", "breaking", str(ndir), "--against", str(mdir / "old"), "--error-format", "json"])
        (ndir / "buf.yaml").write_text("version: v2\nbreaking:\n  use:\n    - WIRE_JSON\n")
        bwj_rc, _, _, _ = run_cmd(["buf", "breaking", str(ndir), "--against", str(mdir / "old"), "--error-format", "json"])
        (ndir / "buf.yaml").write_text("version: v2\nbreaking:\n  use:\n    - FILE\n")
        bf_rc, bf_out, _, _ = run_cmd(["buf", "breaking", str(ndir), "--against", str(mdir / "old"), "--error-format", "json"])

        vw_rc, vw_out, _, _ = run_cmd([v22, "diff", str(mdir / "old/api.proto"), str(ndir / "api.proto"), "--format", "json", "--breaking", "wire", "--fail-on-breaking"])
        vs_rc, _, _, _ = run_cmd([v22, "diff", str(mdir / "old/api.proto"), str(ndir / "api.proto"), "--format", "json", "--breaking", "source", "--fail-on-breaking"])
        vj = json.loads(vw_out)
        buf_rules = sorted({json.loads(line).get("type", "") for line in bf_out.decode().splitlines() if line.strip()})
        out["smoke_matrix"].append({
            "case": label,
            "vimanam_wire_breaking": (vw_rc == 3),
            "buf_wire_breaking": (bw_rc != 0),
            "vimanam_source_breaking": (vs_rc == 3),
            "buf_wire_json_breaking": (bwj_rc != 0),
            "buf_file_breaking": (bf_rc != 0),
            "vimanam_native_kinds": sorted({c["native_kind"] for c in vj["changes"]}),
            "buf_file_rules": buf_rules,
        })
    return out


def run_graphql_inspector_comparison(v22: str, octo_repo: Path, tmpdir: Path) -> dict:
    """Compare vimanam diff vs graphql-inspector on SpaceX Land, GitHub 1.2 MB SDL, and an 8-case matrix."""
    brew_ver = subprocess.run(["brew", "list", "--versions", "graphql-inspector"], capture_output=True, text=True).stdout.strip()
    out: dict = {"graphql_inspector_version": brew_ver or "graphql-inspector"}

    sp_old = REPO / "tests/fixtures/graphql/spacex-land-0a482d1.graphql"
    sp_new = REPO / "tests/fixtures/graphql/spacex-land-64a7fa9.graphql"
    v_rc, v_out, _, _ = run_cmd([v22, "diff", str(sp_old), str(sp_new), "--format", "json", "--fail-on-breaking"])
    g_rc, g_out, g_err, _ = run_cmd(["graphql-inspector", "diff", str(sp_old), str(sp_new)])
    vj = json.loads(v_out)
    g_text = (g_out + g_err).decode("utf-8", errors="replace")
    m_tot = re.search(r"Detected the following changes \((\d+)\)", g_text)
    m_brk = re.search(r"Detected (\d+) breaking changes?", g_text)
    out["spacex_land"] = {
        "vimanam_summary": vj["summary"],
        "vimanam_changes": len(vj["changes"]),
        "graphql_inspector_exit": g_rc,
        "graphql_inspector_total_changes": int(m_tot.group(1)) if m_tot else None,
        "graphql_inspector_breaking": int(m_brk.group(1)) if m_brk else 0,
        "hyperfine": hyperfine_multi(
            [
                ("graphql_inspector", sh_quote(["graphql-inspector", "diff", str(sp_old), str(sp_new)])),
                ("vimanam_2_2", sh_quote([v22, "diff", str(sp_old), str(sp_new), "--format", "json"])),
            ],
            runs=8,
            warmup=1,
            ignore_failure=True,
        ),
    }

    # Real GitHub GraphQL schema update commit (pinned a6de3c1..597478f)
    old_sha = resolve_rev(octo_repo, PINNED_REVS["octo_old"], "HEAD~1")
    new_sha = resolve_rev(octo_repo, PINNED_REVS["octo_new"], "HEAD")

    gh_old_json = tmpdir / "gh_old.json"
    gh_new_json = tmpdir / "gh_new.json"
    gh_old_json.write_bytes(subprocess.check_output(["git", "-C", str(octo_repo), "show", f"{old_sha}:schema.json"]))
    gh_new_json.write_bytes(subprocess.check_output(["git", "-C", str(octo_repo), "show", f"{new_sha}:schema.json"]))

    vmd_rc, vmd_out, _, _ = run_cmd([v22, "diff", str(gh_old_json), str(gh_new_json)])
    v_rc, v_out, _, _ = run_cmd([v22, "diff", str(gh_old_json), str(gh_new_json), "--format", "json"])
    g_rc, g_out, g_err, _ = run_cmd(["graphql-inspector", "diff", str(gh_old_json), str(gh_new_json)])
    vj = json.loads(v_out) if v_rc == 0 else None
    g_text = (g_out + g_err).decode("utf-8", errors="replace")
    m_tot = re.search(r"Detected the following changes \((\d+)\)", g_text)
    m_brk = re.search(r"Detected (\d+) breaking changes?", g_text)
    out["github_5_0mb_introspection_json"] = {
        "old_commit": old_sha,
        "new_commit": new_sha,
        "old_bytes": gh_old_json.stat().st_size,
        "new_bytes": gh_new_json.stat().st_size,
        "vimanam_md_bytes": len(vmd_out),
        "vimanam_json_bytes": len(v_out),
        "vimanam_summary": vj["summary"] if vj else None,
        "vimanam_changes": len(vj["changes"]) if vj else None,
        "graphql_inspector_exit": g_rc,
        "graphql_inspector_total_changes": int(m_tot.group(1)) if m_tot else 0,
        "graphql_inspector_breaking": int(m_brk.group(1)) if m_brk else 0,
        "hyperfine": hyperfine_multi(
            [
                ("graphql_inspector", sh_quote(["graphql-inspector", "diff", str(gh_old_json), str(gh_new_json)])),
                ("vimanam_2_2_md", sh_quote([v22, "diff", str(gh_old_json), str(gh_new_json)])),
                ("vimanam_2_2_json", sh_quote([v22, "diff", str(gh_old_json), str(gh_new_json), "--format", "json"])),
            ],
            runs=5,
            warmup=1,
            ignore_failure=True,
        ),
    }

    # Probe 1.2 MB GitHub SDL too (documents graphql-inspector duplicate-field rejection honestly)
    gh_old_sdl = tmpdir / "gh_old.graphql"
    gh_new_sdl = tmpdir / "gh_new.graphql"
    gh_old_sdl.write_bytes(subprocess.check_output(["git", "-C", str(octo_repo), "show", f"{old_sha}:schema.graphql"]))
    gh_new_sdl.write_bytes(subprocess.check_output(["git", "-C", str(octo_repo), "show", f"{new_sha}:schema.graphql"]))
    gs_rc, gs_out, gs_err, gs_ms = run_cmd(["graphql-inspector", "diff", str(gh_old_sdl), str(gh_new_sdl)])
    vs_rc, vs_out, _, vs_ms = run_cmd([v22, "diff", str(gh_old_sdl), str(gh_new_sdl)])
    vsj_rc, vsj_out, _, vsj_ms = run_cmd([v22, "diff", str(gh_old_sdl), str(gh_new_sdl), "--format", "json"])
    vsj = json.loads(vsj_out) if vsj_rc == 0 else None
    out["github_1_2mb_sdl_probe"] = {
        "old_commit": old_sha,
        "new_commit": new_sha,
        "old_bytes": gh_old_sdl.stat().st_size,
        "new_bytes": gh_new_sdl.stat().st_size,
        "graphql_inspector_exit": gs_rc,
        "graphql_inspector_error": (gs_err or gs_out).decode("utf-8", errors="replace").strip().splitlines()[0] if gs_rc != 0 else "",
        "vimanam_md_ms": round(vs_ms * 1000, 1),
        "vimanam_md_bytes": len(vs_out),
        "vimanam_json_ms": round(vsj_ms * 1000, 1),
        "vimanam_json_bytes": len(vsj_out),
        "vimanam_summary": vsj["summary"] if vsj else None,
        "vimanam_changes": len(vsj["changes"]) if vsj else None,
    }

    base_gql = """enum Role {
  ADMIN
  USER
}
input UserFilter {
  email: String
  limit: Int = 10
}
type User {
  id: ID!
  name: String!
  role: Role!
}
type Query {
  user(id: ID!, filter: UserFilter): User
  users(first: Int, after: String): [User!]!
}
"""
    gcases = [
        ("Output field removed (User.name)", base_gql.replace("  name: String!\n", "")),
        ("Output field made nullable (User.name)", base_gql.replace("name: String!", "name: String")),
        ("Required argument added (Query.user)", base_gql.replace("user(id: ID!, filter: UserFilter)", "user(id: ID!, tenant: ID!, filter: UserFilter)")),
        ("Optional argument added (Query.user)", base_gql.replace("user(id: ID!, filter: UserFilter)", "user(id: ID!, filter: UserFilter, verbose: Boolean)")),
        ("Input field made non-null without default", base_gql.replace("email: String", "email: String!")),
        ("Output enum value added (Role.AUDITOR)", base_gql.replace("  USER\n", "  USER\n  AUDITOR\n")),
        ("Input default value changed (limit: 10 -> 20)", base_gql.replace("limit: Int = 10", "limit: Int = 20")),
        (
            "List field converted to Relay connection",
            base_gql.replace("users(first: Int, after: String): [User!]!", "users(first: Int, after: String): UserConnection!")
            + "\ntype PageInfo { hasNextPage: Boolean! endCursor: String }\ntype UserEdge { node: User cursor: String! }\ntype UserConnection { edges: [UserEdge] pageInfo: PageInfo! }\n",
        ),
    ]
    gdir = tmpdir / "gql_matrix"
    gdir.mkdir()
    go = gdir / "old.graphql"
    go.write_text(base_gql)
    out["smoke_matrix"] = []
    for idx, (label, new_src) in enumerate(gcases):
        gn = gdir / f"new_{idx}.graphql"
        gn.write_text(new_src)
        v_rc, v_out, _, _ = run_cmd([v22, "diff", str(go), str(gn), "--format", "json", "--fail-on-breaking"])
        g_rc, g_out, g_err, _ = run_cmd(["graphql-inspector", "diff", str(go), str(gn)])
        vj = json.loads(v_out)
        g_txt = (g_out + g_err).decode("utf-8", errors="replace")
        g_brk = (g_rc != 0) or bool(re.search(r"detected\s+[1-9]\d*\s+breaking\s+change", g_txt, re.I))
        g_warn = ("dangerous" in g_txt.lower()) or ("⚠" in g_txt)
        v_sevs = sorted({c["severity"] for c in vj["changes"]})
        out["smoke_matrix"].append({
            "case": label,
            "vimanam_severities": v_sevs,
            "vimanam_breaking": (v_rc == 3),
            "graphql_inspector_breaking": g_brk,
            "graphql_inspector_dangerous": g_warn,
            "breaking_verdict_match": ((v_rc == 3) == g_brk),
        })
    return out


def run_protoc_gen_doc_comparison(v22: str, otel_repo: Path, tmpdir: Path) -> dict:
    """Compare vimanam pure-Rust .proto Markdown vs protoc + protoc-gen-doc 1.5.1."""
    out: dict = {
        "protoc_gen_doc_version": subprocess.check_output(["protoc-gen-doc", "--version"], text=True).strip(),
        "targets": [],
    }
    doc_out = tmpdir / "pgd_out"
    doc_out.mkdir()

    targets = [
        (
            "gRPC Health v1.20.0 (.proto)",
            REPO / "tests/fixtures/protobuf/health-v1.20.0.proto",
            [REPO / "tests/fixtures/protobuf"],
            [REPO / "tests/fixtures/protobuf/health-v1.20.0.proto"],
        ),
        (
            "OTel Collector TraceService (+ 3 transitive protos)",
            otel_repo / "opentelemetry/proto/collector/trace/v1/trace_service.proto",
            [otel_repo],
            [
                otel_repo / "opentelemetry/proto/collector/trace/v1/trace_service.proto",
                otel_repo / "opentelemetry/proto/trace/v1/trace.proto",
                otel_repo / "opentelemetry/proto/resource/v1/resource.proto",
                otel_repo / "opentelemetry/proto/common/v1/common.proto",
            ],
        ),
    ]
    for idx, (label, entry, inc_dirs, all_files) in enumerate(targets):
        pgd_md = doc_out / f"out_{idx}.md"
        pgd_cmd = [
            "protoc",
            *[a for d in inc_dirs for a in ("-I", str(d))],
            f"--doc_out={doc_out}",
            f"--doc_opt=markdown,{pgd_md.name}",
            *[str(f) for f in all_files],
        ]
        vim_md = doc_out / f"vim_{idx}.md"
        vim_cmd = [
            v22,
            str(entry),
            *[a for d in inc_dirs for a in ("-I", str(d))],
            "--detail", "full",
            "--include-schemas",
            "--no-report",
            "-o", str(vim_md),
        ]
        run_cmd(pgd_cmd)
        run_cmd(vim_cmd)
        pgd_txt = pgd_md.read_text(encoding="utf-8", errors="replace") if pgd_md.exists() else ""
        vim_txt = vim_md.read_text(encoding="utf-8", errors="replace") if vim_md.exists() else ""
        hf = hyperfine_multi(
            [
                ("protoc_gen_doc", sh_quote(pgd_cmd)),
                ("vimanam_2_2", sh_quote(vim_cmd)),
            ],
            runs=10,
            warmup=2,
        )
        out["targets"].append({
            "name": label,
            "protoc_gen_doc_bytes": len(pgd_txt.encode("utf-8")),
            "protoc_gen_doc_bpe_o200k": count_bpe(pgd_txt),
            "vimanam_bytes": len(vim_txt.encode("utf-8")),
            "vimanam_bpe_o200k": count_bpe(vim_txt),
            "hyperfine": hf,
        })
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--v21", required=True)
    ap.add_argument("--v22", required=True)
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--runs", type=int, default=10)
    ap.add_argument("--output", required=True)
    args = ap.parse_args()

    v21 = str(Path(args.v21).resolve())
    v22 = str(Path(args.v22).resolve())
    corpus = Path(args.corpus).resolve()
    stripe_repo = corpus / "stripe-openapi"
    openai_repo = corpus / "openai-repo"
    otel = corpus / "otel-proto"
    octo = corpus / "octokit-graphql"

    env_no_protoc = dict(os.environ)
    env_no_protoc["PATH"] = ""

    results: dict = {
        "environment": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "hyperfine": subprocess.check_output(["hyperfine", "--version"], text=True).strip(),
            "protoc": subprocess.check_output(["protoc", "--version"], text=True).strip(),
            "git": subprocess.check_output(["git", "--version"], text=True).strip(),
            "v21_version": subprocess.check_output([v21, "--version"], text=True).strip(),
            "v22_version": subprocess.check_output([v22, "--version"], text=True).strip(),
            "v21_sha256": file_sha256(v21),
            "v22_sha256": file_sha256(v22),
            "v21_binary_bytes": os.path.getsize(v21),
            "v22_binary_bytes": os.path.getsize(v22),
            "openai_openapi_commit": subprocess.check_output(["git", "-C", str(openai_repo), "rev-parse", "--short", "HEAD"], text=True).strip(),
            "stripe_openapi_commit": subprocess.check_output(["git", "-C", str(stripe_repo), "rev-parse", "--short", "HEAD"], text=True).strip(),
            "octokit_graphql_commit": subprocess.check_output(["git", "-C", str(octo), "rev-parse", "--short", "HEAD"], text=True).strip(),
            "otel_proto_commit": subprocess.check_output(["git", "-C", str(otel), "rev-parse", "--short", "HEAD"], text=True).strip(),
        },
        "openapi": [],
        "graphql": [],
        "graphql_diffs": [],
        "protobuf_targets": [],
        "protobuf_git_diff_otel": {},
        "protobuf_git_diff_200_synthetic": {},
        "token_calibration": [],
        "external_baselines": {},
    }

    # 1. OpenAPI benchmarks (10 runs including Stripe YAML for rock-solid median)
    print("[1/6] OpenAPI benchmarks (10 runs, median + IQR)...", flush=True)
    for name, path, fmt, runs in [
        ("Petstore OAS3 (JSON)", REPO / "tests/fixtures/petstore_oas3.json", "JSON", args.runs),
        ("Petstore OAS3 (YAML)", REPO / "tests/fixtures/petstore_oas3.yaml", "YAML", args.runs),
        ("OpenAI OpenAPI 3.0 (YAML)", corpus / "openai.yaml", "YAML", args.runs),
        ("Stripe OpenAPI 3.0 (JSON)", stripe_repo / "openapi/spec3.json", "JSON", args.runs),
        ("Stripe OpenAPI 3.0 (YAML)", stripe_repo / "openapi/spec3.yaml", "YAML", args.runs),
    ]:
        flags = [str(path), "--detail", "full", "--include-schemas", "--no-report"]
        rc21, out21, err21, _ = run_cmd([v21, *flags])
        rc22, out22, _, _ = run_cmd([v22, *flags])
        hf = (
            hyperfine_multi([("v2_1", sh_quote([v21, *flags])), ("v2_2", sh_quote([v22, *flags]))], runs=runs)
            if (rc21 == 0 and rc22 == 0)
            else hyperfine_multi([("v2_2", sh_quote([v22, *flags]))], runs=runs)
        )
        results["openapi"].append({
            "name": name,
            "format": fmt,
            "size_bytes": path.stat().st_size,
            "rc21": rc21,
            "rc22": rc22,
            "err21": err21.decode("utf-8", errors="replace").strip()[:220] if rc21 != 0 else "",
            "byte_identical": (rc21 == 0 and rc22 == 0 and out21 == out22),
            "hyperfine": hf,
        })
        print(f"  {name}: med_v21={hf.get('v2_1',{}).get('median_ms')} med_v22={hf['v2_2']['median_ms']} ratio={hf.get('median_speedup_a_over_b')} iqrs_disjoint={hf.get('iqrs_disjoint')}", flush=True)

    # 2. GraphQL benchmarks (10 runs)
    print("[2/6] GraphQL benchmarks...", flush=True)
    for name, path, kind, runs in [
        ("SpaceX Land (SDL)", REPO / "tests/fixtures/graphql/spacex-land-64a7fa9.graphql", "SDL", args.runs),
        ("SpaceX Land (Introspection JSON)", REPO / "tests/fixtures/graphql/spacex-land-64a7fa9.introspection.json", "Introspection", args.runs),
        ("GitHub GraphQL API (SDL)", octo / "schema.graphql", "SDL", args.runs),
        ("GitHub GraphQL API (Introspection JSON)", octo / "schema.json", "Introspection", args.runs),
    ]:
        flags = [str(path), "--detail", "full", "--include-schemas", "--no-report"]
        rc21, out21, _, _ = run_cmd([v21, *flags])
        rc22, out22, _, _ = run_cmd([v22, *flags])
        hf = hyperfine_multi([("v2_1", sh_quote([v21, *flags])), ("v2_2", sh_quote([v22, *flags]))], runs=runs)
        results["graphql"].append({
            "name": name,
            "kind": kind,
            "size_bytes": path.stat().st_size,
            "byte_identical": (rc21 == 0 and rc22 == 0 and out21 == out22),
            "hyperfine": hf,
        })
        print(f"  {name}: med_v21={hf['v2_1']['median_ms']} med_v22={hf['v2_2']['median_ms']} iqrs_disjoint={hf['iqrs_disjoint']}", flush=True)

    gql_old_sha = resolve_rev(octo, PINNED_REVS["octo_old"], "HEAD~1")
    gql_new_sha = resolve_rev(octo, PINNED_REVS["octo_new"], "HEAD")
    for label, dflags, cwd, runs in [
        ("SpaceX Land SDL diff (0a482d1 -> 64a7fa9)", ["diff", str(REPO / "tests/fixtures/graphql/spacex-land-0a482d1.graphql"), str(REPO / "tests/fixtures/graphql/spacex-land-64a7fa9.graphql"), "--format", "json"], None, args.runs),
        (f"GitHub GraphQL SDL Git-ref diff ({gql_old_sha} -> {gql_new_sha})", ["diff", "--from-ref", gql_old_sha, "--to-ref", gql_new_sha, "--spec", "schema.graphql", "--format", "json"], str(octo), 5),
        (f"GitHub GraphQL Introspection Git-ref diff ({gql_old_sha} -> {gql_new_sha})", ["diff", "--from-ref", gql_old_sha, "--to-ref", gql_new_sha, "--spec", "schema.json", "--format", "json"], str(octo), 5),
    ]:
        _, d21, _, _ = run_cmd([v21, *dflags], cwd=cwd)
        _, d22, _, _ = run_cmd([v22, *dflags], cwd=cwd)
        j21, j22 = json.loads(d21), json.loads(d22)
        hf = hyperfine_multi([("v2_1", sh_quote([v21, *dflags])), ("v2_2", sh_quote([v22, *dflags]))], runs=runs, cwd=cwd)
        results["graphql_diffs"].append({
            "name": label,
            "changes_exact_match": (j21["changes"] == j22["changes"]),
            "summary_v22": j22["summary"],
            "hyperfine": hf,
        })
        print(f"  {label}: med_v21={hf['v2_1']['median_ms']} med_v22={hf['v2_2']['median_ms']} iqrs_disjoint={hf['iqrs_disjoint']}", flush=True)

    # 3. Protobuf targets (10 runs; zero-protoc verified via env_no_protoc)
    print("[3/6] Protobuf benchmarks...", flush=True)
    for label, p, inc in [
        ("gRPC Health v1.20.0 (.proto)", REPO / "tests/fixtures/protobuf/health-v1.20.0.proto", []),
        ("gRPC Health v1.20.0 (.pb descriptor)", REPO / "tests/fixtures/protobuf/health-v1.20.0.pb", []),
        ("OTel Collector TraceService (.proto)", otel / "opentelemetry/proto/collector/trace/v1/trace_service.proto", ["-I", str(otel)]),
        ("OTel Collector MetricsService (.proto)", otel / "opentelemetry/proto/collector/metrics/v1/metrics_service.proto", ["-I", str(otel)]),
        ("OTel Collector LogsService (.proto)", otel / "opentelemetry/proto/collector/logs/v1/logs_service.proto", ["-I", str(otel)]),
    ]:
        pflags = [str(p), *inc, "--detail", "full", "--include-schemas", "--no-report"]
        _, o21, _, _ = run_cmd([v21, *pflags])
        rc22, o22, _, _ = run_cmd([v22, *pflags], env=env_no_protoc)
        assert rc22 == 0, f"v2.2 failed with PATH='': {label}"
        hf = hyperfine_multi([("v2_1", sh_quote([v21, *pflags])), ("v2_2", sh_quote([v22, *pflags]))], runs=args.runs)
        results["protobuf_targets"].append({
            "name": label,
            "v21_bytes": len(o21),
            "v22_bytes": len(o22),
            "field_notes_v21": len(re.findall(r"Field #\d+", o21.decode("utf-8", errors="replace"))),
            "field_notes_v22": len(re.findall(r"Field #\d+", o22.decode("utf-8", errors="replace"))),
            "hyperfine": hf,
        })
        print(f"  {label}: med_v21={hf['v2_1']['median_ms']} med_v22={hf['v2_2']['median_ms']} ratio={hf['median_speedup_a_over_b']}x", flush=True)

    otel_commit = results["environment"]["otel_proto_commit"]
    gargs = ["diff", "--from-ref", f"{otel_commit}~1", "--to-ref", otel_commit, "--spec", "opentelemetry/proto/trace/v1/trace.proto", "-I", ".", "--format", "json"]
    rc21, o21, _, _ = run_cmd([v21, *gargs], cwd=str(otel))
    rc22, o22, _, _ = run_cmd([v22, *gargs], cwd=str(otel))
    if rc21 == 0 and rc22 == 0:
        j21, j22 = json.loads(o21), json.loads(o22)
        results["protobuf_git_diff_otel"] = {
            "repo": "open-telemetry/opentelemetry-proto",
            "commit": otel_commit,
            "spec": "opentelemetry/proto/trace/v1/trace.proto",
            "changes_exact_match": (j21["changes"] == j22["changes"]),
            "changes_count": len(j22["changes"]),
            "hyperfine": hyperfine_multi([("v2_1", sh_quote([v21, *gargs])), ("v2_2", sh_quote([v22, *gargs]))], runs=6, cwd=str(otel)),
        }

    # 4. Synthetic 200-proto Git-ref scaling + 5. Large-scale external baselines
    print("[4/6] Synthetic 200-proto Git-ref scaling & [5/6] Real-scale oasdiff/buf baselines...", flush=True)
    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        p200 = build_synthetic_200_proto_repo(tmp)
        sargs = ["diff", "--from-ref", "v1", "--to-ref", "v2", "--spec", "protos/orders/service.proto", "-I", "protos", "--format", "json"]
        _, so21, _, _ = run_cmd([v21, *sargs], cwd=str(p200))
        _, so22, _, _ = run_cmd([v22, *sargs], cwd=str(p200))
        sj21, sj22 = json.loads(so21), json.loads(so22)
        results["protobuf_git_diff_200_synthetic"] = {
            "total_committed_protos": 200,
            "imported_protos_used": 4,
            "changes_count": len(sj22["changes"]),
            "changes_exact_match": (sj21["changes"] == sj22["changes"]),
            "hyperfine": hyperfine_multi([("v2_1", sh_quote([v21, *sargs])), ("v2_2", sh_quote([v22, *sargs]))], runs=3, warmup=1, cwd=str(p200)),
        }
        print(f"  200-proto Git-ref diff: {results['protobuf_git_diff_200_synthetic']['hyperfine']['median_speedup_a_over_b']}x", flush=True)

        results["external_baselines"]["oasdiff"] = run_oasdiff_comparison(v22, openai_repo, stripe_repo, tmp)
        print(f"  OpenAI 4.9 MB oasdiff vs vimanam: { {k: v['median_ms'] for k, v in results['external_baselines']['oasdiff']['openai_4_9mb_diff']['hyperfine'].items() if isinstance(v, dict)} }", flush=True)
        print(f"  Stripe 7.9 MB probe: { {k: results['external_baselines']['oasdiff']['stripe_7_9mb_probe'][k] for k in ('oasdiff_breaking', 'vimanam_diff_md', 'vimanam_diff_v2_json')} }", flush=True)

        results["external_baselines"]["buf"] = run_buf_comparison(v22, otel, tmp)
        print(f"  OTel multi-file buf vs vimanam: { {k: v['median_ms'] for k, v in results['external_baselines']['buf']['otel_git_ref_comparison']['hyperfine'].items() if isinstance(v, dict)} }", flush=True)

        results["external_baselines"]["graphql_inspector"] = run_graphql_inspector_comparison(v22, octo, tmp)
        print(f"  GitHub 5.0 MB Introspection graphql-inspector vs vimanam: { {k: v['median_ms'] for k, v in results['external_baselines']['graphql_inspector']['github_5_0mb_introspection_json']['hyperfine'].items() if isinstance(v, dict)} }", flush=True)

        results["external_baselines"]["protoc_gen_doc"] = run_protoc_gen_doc_comparison(v22, otel, tmp)
        print(f"  protoc-gen-doc vs vimanam: {[ (t['name'], t['hyperfine']['median_speedup_a_over_b']) for t in results['external_baselines']['protoc_gen_doc']['targets'] ]}", flush=True)

    # 6. o200k_base BPE vs chars/4 calibration
    print("[6/6] o200k_base BPE vs chars/4 calibration...", flush=True)
    for name, path, fmt, inc, sel_flags in [
        ("OpenAI OpenAPI 3.0 (1.3 MB YAML)", corpus / "openai.yaml", "YAML", [], ["--operation", "POST /chat/completions", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
        ("Stripe OpenAPI 3.0 (7.9 MB JSON)", stripe_repo / "openapi/spec3.json", "JSON", [], ["--operation", "POST /v1/customers", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
        ("GitHub GraphQL API (1.2 MB SDL)", octo / "schema.graphql", "GraphQL SDL", [], ["--operation", "QUERY Query.repository", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
        ("SpaceX Land GraphQL (12 KB SDL)", REPO / "tests/fixtures/graphql/spacex-land-64a7fa9.graphql", "GraphQL SDL", [], ["--operation", "QUERY Query.launchesPastResult", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
        ("OTel TraceService (.proto + 3 transitive)", otel / "opentelemetry/proto/collector/trace/v1/trace_service.proto", "Protobuf", ["-I", str(otel)], ["--operation", "RPC opentelemetry.proto.collector.trace.v1.TraceService.Export", "--detail", "full", "--include-schemas", "--schema-depth", "3", "--no-report"]),
    ]:
        raw_text = (
            "\n".join(p.read_text() for p in [
                otel / "opentelemetry/proto/collector/trace/v1/trace_service.proto",
                otel / "opentelemetry/proto/trace/v1/trace.proto",
                otel / "opentelemetry/proto/resource/v1/resource.proto",
                otel / "opentelemetry/proto/common/v1/common.proto",
            ])
            if fmt == "Protobuf"
            else path.read_text(encoding="utf-8", errors="replace")
        )
        _, full_out, _, _ = run_cmd([v22, str(path), *inc, "--detail", "full", "--include-schemas", "--no-report"])
        _, sum_out, _, _ = run_cmd([v22, str(path), *inc, "--detail", "summary", "--no-report"])
        _, foc_out, _, _ = run_cmd([v22, str(path), *inc, *sel_flags])

        full_t, sum_t, foc_t = full_out.decode("utf-8", "replace"), sum_out.decode("utf-8", "replace"), foc_out.decode("utf-8", "replace")
        raw_bpe, full_bpe, sum_bpe, foc_bpe = count_bpe(raw_text), count_bpe(full_t), count_bpe(sum_t), count_bpe(foc_t)
        results["token_calibration"].append({
            "name": name,
            "format": fmt,
            "raw_chars": len(raw_text),
            "raw_bpe_o200k": raw_bpe,
            "raw_chars_div_4": chars_div_4(raw_text),
            "raw_chars_per_o200k": round(len(raw_text) / raw_bpe, 2),
            "full_md_bpe_o200k": full_bpe,
            "full_md_chars_div_4": chars_div_4(full_t),
            "full_md_chars_per_o200k": round(len(full_t) / full_bpe, 2),
            "summary_bpe_o200k": sum_bpe,
            "focused_bpe_o200k": foc_bpe,
            "focused_vs_raw_o200k_reduction_pct": round((1 - foc_bpe / raw_bpe) * 100, 1),
        })

    Path(args.output).write_text(json.dumps(results, indent=2) + "\n")
    print(f"\nWrote {args.output}", flush=True)


if __name__ == "__main__":
    main()
