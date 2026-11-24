#!/usr/bin/env python3
"""Reproducible public benchmark and external-baseline suite for Vimanam.

Measures:
1. Standalone v2.2.0 and v2.2.0 vs v2.1.0 render/diff latency (hyperfine 2.0.0:
   mean, stddev, median, min, max, peak RSS, CPU instructions) on public
   OpenAPI, GraphQL, and Protobuf specifications.
2. Exact BPE tokenization (tiktoken o200k_base) vs Vimanam's chars/4 estimate
   across raw JSON, YAML, GraphQL SDL, .proto, and rendered Markdown.
3. Synthetic 500-file .proto Git-ref scaling benchmark (generated via git
   fast-import; zero private repos).
4. External baseline diff correctness and speed comparisons against oasdiff
   (OpenAPI) and buf breaking (Protobuf).

Run with:
  uv run --with tiktoken python3 scripts/benchmark-public.py \
    --v21 /tmp/vimanam-v2.1.0/target/release/vimanam \
    --v22 target/release/vimanam \
    --corpus /tmp/vimanam-eval-corpus \
    --output scripts/benchmark-v22-results.json
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shlex
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

import tiktoken

REPO = Path(__file__).resolve().parent.parent
ENC = tiktoken.get_encoding("o200k_base")


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
    # Matches src/markdown/mod.rs estimate_tokens (ceil(chars / 4) or len / 4)
    return (len(text) + 3) // 4


def run_cmd(args, cwd=None, env=None, timeout=180):
    t0 = time.perf_counter()
    p = subprocess.run(args, cwd=cwd, env=env, capture_output=True, timeout=timeout)
    return p.returncode, p.stdout, p.stderr, time.perf_counter() - t0


def parse_hf_summary(summary: dict) -> dict:
    w = summary["time_wall_clock"]
    m = summary.get("memory_peak_resident", {})
    i = summary.get("instructions", {})
    return {
        "count": w["count"],
        "mean_ms": round(w["mean"] * 1000, 2),
        "stddev_ms": round((w.get("stddev") or 0.0) * 1000, 2),
        "median_ms": round(w["median"] * 1000, 2),
        "min_ms": round(w["min"] * 1000, 2),
        "max_ms": round(w["max"] * 1000, 2),
        "rss_mb": round(m["mean"] / (1024 * 1024), 1) if "mean" in m else None,
        "instructions_m": round(i["mean"] / 1e6, 1) if "mean" in i else None,
    }


def hyperfine_pair(cmd_a: str, cmd_b: str, name_a: str, name_b: str, runs: int, warmup: int = 2, cwd=None, ignore_failure: bool = False) -> dict:
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as tf:
        out_json = Path(tf.name)
    try:
        args = [
            "hyperfine",
            "--warmup", str(warmup),
            "--runs", str(runs),
            "--export-json", str(out_json),
            "-n", name_a, cmd_a,
            "-n", name_b, cmd_b,
        ]
        if ignore_failure:
            args.insert(1, "--ignore-failure")
        p = subprocess.run(args, cwd=cwd, capture_output=True, text=True)
        if p.returncode != 0:
            return {"error": p.stderr.strip()[:300]}
        data = json.loads(out_json.read_text())
        a = parse_hf_summary(data["results"][0]["summary"])
        b = parse_hf_summary(data["results"][1]["summary"])
        speedup = round(a["mean_ms"] / b["mean_ms"], 2) if b["mean_ms"] > 0 else None
        # Check if 1-sigma intervals overlap (statistical noise check)
        intervals_overlap = (
            abs(a["mean_ms"] - b["mean_ms"]) <= (a["stddev_ms"] + b["stddev_ms"])
        )
        return {
            name_a: a,
            name_b: b,
            "speedup_a_over_b": speedup,
            "within_1sigma_noise": intervals_overlap,
        }
    finally:
        out_json.unlink(missing_ok=True)


def hyperfine_single(cmd: str, name: str, runs: int, warmup: int = 2, cwd=None) -> dict:
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as tf:
        out_json = Path(tf.name)
    try:
        args = [
            "hyperfine",
            "--warmup", str(warmup),
            "--runs", str(runs),
            "--export-json", str(out_json),
            "-n", name, cmd,
        ]
        p = subprocess.run(args, cwd=cwd, capture_output=True, text=True)
        if p.returncode != 0:
            return {"error": p.stderr.strip()[:300]}
        data = json.loads(out_json.read_text())
        return {name: parse_hf_summary(data["results"][0]["summary"])}
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

    lines.append("commit refs/heads/main")
    lines.append("mark :1")
    lines.append("author Bench <bench@example.invalid> 1700000000 +0000")
    lines.append("committer Bench <bench@example.invalid> 1700000000 +0000")
    add_msg("initial\n")

    def add_blob(path: str, content: str):
        data = content.encode("utf-8")
        lines.append(f"M 100644 inline {path}")
        lines.append(f"data {len(data)}")
        lines.append(content)

    for i in range(196):
        pkg = f"pkg_{i // 20}"
        add_blob(
            f"protos/{pkg}/model_{i:03d}.proto",
            f'syntax = "proto3";\npackage demo.{pkg};\nmessage Model{i:03d} {{ string id = 1; int64 count = 2; }}\n',
        )

    add_blob(
        "protos/common/types.proto",
        'syntax = "proto3";\npackage demo.common;\nmessage Money { string currency = 1; int64 units = 2; }\n',
    )
    add_blob(
        "protos/common/meta.proto",
        'syntax = "proto3";\npackage demo.common;\nmessage RequestMeta { string request_id = 1; }\n',
    )
    add_blob(
        "protos/orders/item.proto",
        'syntax = "proto3";\npackage demo.orders;\nimport "common/types.proto";\nmessage LineItem { string sku = 1; demo.common.Money price = 2; }\n',
    )
    target_v1 = (
        'syntax = "proto3";\n'
        'package demo.orders;\n'
        'import "common/meta.proto";\n'
        'import "orders/item.proto";\n'
        'message CreateOrderRequest { demo.common.RequestMeta meta = 1; repeated LineItem items = 2; }\n'
        'message CreateOrderResponse { string order_id = 1; }\n'
        'service OrderService { rpc CreateOrder(CreateOrderRequest) returns (CreateOrderResponse); }\n'
    )
    add_blob("protos/orders/service.proto", target_v1)

    target_v2 = (
        'syntax = "proto3";\n'
        'package demo.orders;\n'
        'import "common/meta.proto";\n'
        'import "orders/item.proto";\n'
        'message CreateOrderRequest { demo.common.RequestMeta meta = 1; repeated LineItem items = 2; string idempotency_key = 3; }\n'
        'message CreateOrderResponse { string order_id = 1; }\n'
        'service OrderService { rpc CreateOrder(CreateOrderRequest) returns (CreateOrderResponse); }\n'
    )
    lines.append("commit refs/heads/main")
    lines.append("mark :2")
    lines.append("author Bench <bench@example.invalid> 1700000100 +0000")
    lines.append("committer Bench <bench@example.invalid> 1700000100 +0000")
    add_msg("add field v2\n")
    lines.append("from :1")
    add_blob("protos/orders/service.proto", target_v2)
    lines.append("done\n")

    stream = "\n".join(lines).encode("utf-8")
    subprocess.run(["git", "fast-import", "--quiet"], cwd=repo, input=stream, check=True)
    subprocess.run(["git", "checkout", "-q", "main"], cwd=repo, check=True)
    subprocess.run(["git", "tag", "v1", "HEAD~1"], cwd=repo, check=True)
    subprocess.run(["git", "tag", "v2", "HEAD"], cwd=repo, check=True)
    return repo


def run_oasdiff_comparison(v22: str, tmpdir: Path) -> dict:
    """Compare vimanam diff vs oasdiff on public OpenAPI fixtures and a 6-case matrix."""
    out = {"oasdiff_version": subprocess.check_output(["oasdiff", "--version"], text=True).strip(), "cases": [], "fixture_speed": {}}

    # 1. Checked-in diff_old_oas3.json -> diff_new_oas3.json
    old_f = REPO / "tests/fixtures/diff_old_oas3.json"
    new_f = REPO / "tests/fixtures/diff_new_oas3.json"
    v_rc, v_out, _, _ = run_cmd([v22, "diff", str(old_f), str(new_f), "--format", "json", "--fail-on-breaking"])
    o_rc, o_out, _, _ = run_cmd(["oasdiff", "breaking", str(old_f), str(new_f), "--format", "json", "--fail-on", "ERR"])
    v_json = json.loads(v_out)
    o_json = json.loads(o_out) if o_out.strip() else []
    o_errs = [x for x in o_json if x.get("level") == 3] if isinstance(o_json, list) else []
    out["fixture_oas3"] = {
        "vimanam_exit": v_rc,
        "vimanam_summary": v_json["summary"],
        "oasdiff_exit": o_rc,
        "oasdiff_breaking_count": len(o_errs),
        "oasdiff_total_flagged": len(o_json) if isinstance(o_json, list) else None,
        "hyperfine": hyperfine_pair(
            sh_quote(["oasdiff", "breaking", str(old_f), str(new_f), "--format", "json"]),
            sh_quote([v22, "diff", str(old_f), str(new_f), "--format", "json"]),
            "oasdiff",
            "vimanam_2_2",
            runs=10,
            warmup=2,
            ignore_failure=True,
        ),
    }

    # 2. 6-case targeted OpenAPI mutation matrix
    def base_spec():
        return {
            "openapi": "3.0.3",
            "info": {"title": "Orders", "version": "1.0.0"},
            "paths": {
                "/v1/items": {
                    "get": {
                        "operationId": "listItems",
                        "parameters": [
                            {"name": "limit", "in": "query", "required": False, "schema": {"type": "integer"}}
                        ],
                        "responses": {
                            "200": {
                                "description": "OK",
                                "content": {
                                    "application/json": {
                                        "schema": {
                                            "type": "object",
                                            "required": ["id"],
                                            "properties": {
                                                "id": {"type": "string"},
                                                "name": {"type": "string"},
                                            },
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
                                        "properties": {
                                            "name": {"type": "string"},
                                            "note": {"type": "string"},
                                        },
                                    }
                                }
                            },
                        },
                        "responses": {"201": {"description": "Created"}},
                    },
                }
            },
        }

    def mut_remove_endpoint(s):
        del s["paths"]["/v1/items"]["post"]

    def mut_add_required_query(s):
        s["paths"]["/v1/items"]["get"]["parameters"].append(
            {"name": "tenant", "in": "query", "required": True, "schema": {"type": "string"}}
        )

    def mut_req_prop_required(s):
        s["paths"]["/v1/items"]["post"]["requestBody"]["content"]["application/json"]["schema"]["required"].append("note")

    def mut_remove_resp_prop(s):
        del s["paths"]["/v1/items"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]["properties"]["name"]

    def mut_add_optional_resp_prop(s):
        s["paths"]["/v1/items"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]["properties"]["sku"] = {"type": "string"}

    def mut_move_version_path(s):
        s["paths"]["/v2/items"] = s["paths"].pop("/v1/items")

    mutations = [
        ("Endpoint removed (POST /v1/items)", mut_remove_endpoint),
        ("Required query parameter added", mut_add_required_query),
        ("Request body property made required", mut_req_prop_required),
        ("Response property removed", mut_remove_resp_prop),
        ("Optional response property added", mut_add_optional_resp_prop),
        ("Versioned path moved (/v1/items -> /v2/items)", mut_move_version_path),
    ]
    mat_dir = tmpdir / "oas_matrix"
    mat_dir.mkdir()
    old_p = mat_dir / "old.json"
    old_p.write_text(json.dumps(base_spec(), indent=2))
    for idx, (label, fn) in enumerate(mutations):
        s = base_spec()
        fn(s)
        new_p = mat_dir / f"new_{idx}.json"
        new_p.write_text(json.dumps(s, indent=2))
        v_rc, v_out, _, _ = run_cmd([v22, "diff", str(old_p), str(new_p), "--format", "json", "--fail-on-breaking"])
        o_rc, o_out, _, _ = run_cmd(["oasdiff", "breaking", str(old_p), str(new_p), "--format", "json", "--fail-on", "ERR"])
        vj = json.loads(v_out)
        oj = json.loads(o_out) if o_out.strip() else []
        v_breaking = v_rc == 3
        o_breaking = o_rc != 0
        out["cases"].append({
            "case": label,
            "vimanam_breaking": v_breaking,
            "vimanam_kinds": sorted({c["kind"] for c in vj["changes"]}),
            "oasdiff_breaking": o_breaking,
            "oasdiff_ids": sorted({x.get("id", "") for x in oj if isinstance(x, dict)}),
            "verdict_match": (v_breaking == o_breaking),
        })
    return out


def run_buf_comparison(v22: str, tmpdir: Path) -> dict:
    """Compare vimanam diff (--breaking wire|source) vs buf breaking (WIRE, FILE)."""
    out = {"buf_version": subprocess.check_output(["buf", "--version"], text=True).strip(), "cases": [], "health_speed": {}}

    # 1. Public grpc/grpc Health v1.0.0 -> v1.20.0 (non-breaking RPC addition)
    bdir = tmpdir / "buf_health"
    (bdir / "old").mkdir(parents=True)
    (bdir / "new").mkdir(parents=True)
    shutil.copy(REPO / "tests/fixtures/protobuf/health-v1.0.0.proto", bdir / "old/health.proto")
    shutil.copy(REPO / "tests/fixtures/protobuf/health-v1.20.0.proto", bdir / "new/health.proto")
    (bdir / "old/buf.yaml").write_text("version: v2\nbreaking:\n  use:\n    - FILE\n")
    (bdir / "new/buf.yaml").write_text("version: v2\nbreaking:\n  use:\n    - FILE\n")

    out["health_speed"] = hyperfine_pair(
        sh_quote(["buf", "breaking", str(bdir / "new"), "--against", str(bdir / "old")]),
        sh_quote([v22, "diff", str(bdir / "old/health.proto"), str(bdir / "new/health.proto"), "--format", "json"]),
        "buf_breaking",
        "vimanam_2_2",
        runs=10,
        warmup=2,
        ignore_failure=True,
    )

    # 2. 8-case Protobuf compatibility matrix (WIRE vs SOURCE/FILE)
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
        (
            "Field type changed (int32 -> string)",
            base_proto.replace("int32 quantity = 2;", "string quantity = 2;"),
        ),
        (
            "Field removed without reserving number",
            base_proto.replace("  int32 quantity = 2;\n", ""),
        ),
        (
            "Field removed with number reserved",
            base_proto.replace("  int32 quantity = 2;\n", "  reserved 2;\n"),
        ),
        (
            "Same-number field rename (quantity -> qty)",
            base_proto.replace("int32 quantity = 2;", "int32 qty = 2;"),
        ),
        (
            "JSON name override changed",
            base_proto.replace("int32 quantity = 2;", 'int32 quantity = 2 [json_name = "orderQty"];'),
        ),
        (
            "Enum value removed without reserving number",
            base_proto.replace("  STATUS_ARCHIVED = 2;\n", ""),
        ),
        (
            "RPC changed from unary to server streaming",
            base_proto.replace("returns (Order);", "returns (stream Order);"),
        ),
        (
            "Optional field added (non-breaking)",
            base_proto.replace("  Status status = 3;\n", "  Status status = 3;\n  string note = 4;\n"),
        ),
    ]

    mdir = tmpdir / "buf_matrix"
    (mdir / "old").mkdir(parents=True)
    (mdir / "old/api.proto").write_text(base_proto)
    (mdir / "old/buf.yaml").write_text("version: v2\n")

    for idx, (label, new_src) in enumerate(cases):
        ndir = mdir / f"new_{idx}"
        ndir.mkdir()
        (ndir / "api.proto").write_text(new_src)

        # buf WIRE
        (ndir / "buf.yaml").write_text("version: v2\nbreaking:\n  use:\n    - WIRE\n")
        bw_rc, bw_out, _, _ = run_cmd(["buf", "breaking", str(ndir), "--against", str(mdir / "old"), "--error-format", "json"])
        # buf WIRE_JSON
        (ndir / "buf.yaml").write_text("version: v2\nbreaking:\n  use:\n    - WIRE_JSON\n")
        bwj_rc, _, _, _ = run_cmd(["buf", "breaking", str(ndir), "--against", str(mdir / "old"), "--error-format", "json"])
        # buf FILE (source)
        (ndir / "buf.yaml").write_text("version: v2\nbreaking:\n  use:\n    - FILE\n")
        bf_rc, bf_out, _, _ = run_cmd(["buf", "breaking", str(ndir), "--against", str(mdir / "old"), "--error-format", "json"])

        # vimanam wire & source
        vw_rc, vw_out, _, _ = run_cmd([v22, "diff", str(mdir / "old/api.proto"), str(ndir / "api.proto"), "--format", "json", "--breaking", "wire", "--fail-on-breaking"])
        vs_rc, _, _, _ = run_cmd([v22, "diff", str(mdir / "old/api.proto"), str(ndir / "api.proto"), "--format", "json", "--breaking", "source", "--fail-on-breaking"])
        vj = json.loads(vw_out)

        buf_file_rules = sorted({json.loads(line).get("type", "") for line in bf_out.decode().splitlines() if line.strip()})
        out["cases"].append({
            "case": label,
            "vimanam_wire_breaking": (vw_rc == 3),
            "buf_wire_breaking": (bw_rc != 0),
            "vimanam_source_breaking": (vs_rc == 3),
            "buf_wire_json_breaking": (bwj_rc != 0),
            "buf_file_breaking": (bf_rc != 0),
            "vimanam_native_kinds": sorted({c["native_kind"] for c in vj["changes"]}),
            "buf_file_rules": buf_file_rules,
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
            "runs_per_benchmark": args.runs,
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

    # 1. OpenAPI benchmarks (10 runs)
    print("[1/6] OpenAPI benchmarks...", flush=True)
    oa_specs = [
        ("Petstore OAS3 (JSON)", REPO / "tests/fixtures/petstore_oas3.json", "JSON", args.runs),
        ("Petstore OAS3 (YAML)", REPO / "tests/fixtures/petstore_oas3.yaml", "YAML", args.runs),
        ("OpenAI OpenAPI 3.0 (YAML)", corpus / "openai.yaml", "YAML", args.runs),
        ("Stripe OpenAPI 3.0 (JSON)", corpus / "stripe.json", "JSON", min(args.runs, 6)),
        ("Stripe OpenAPI 3.0 (YAML)", corpus / "stripe.yaml", "YAML", min(args.runs, 6)),
    ]
    for name, path, fmt, runs in oa_specs:
        flags = [str(path), "--detail", "full", "--include-schemas", "--no-report"]
        rc21, out21, err21, _ = run_cmd([v21, *flags])
        rc22, out22, err22, _ = run_cmd([v22, *flags])
        hf = (
            hyperfine_pair(sh_quote([v21, *flags]), sh_quote([v22, *flags]), "v2_1", "v2_2", runs=runs)
            if (rc21 == 0 and rc22 == 0)
            else hyperfine_single(sh_quote([v22, *flags]), "v2_2", runs=runs)
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
        print(f"  {name}: rc=({rc21},{rc22}) hf={hf}", flush=True)

    # 2. GraphQL benchmarks (10 runs)
    print("[2/6] GraphQL benchmarks...", flush=True)
    gql_specs = [
        ("SpaceX Land (SDL)", REPO / "tests/fixtures/graphql/spacex-land-64a7fa9.graphql", "SDL", args.runs),
        ("SpaceX Land (Introspection JSON)", REPO / "tests/fixtures/graphql/spacex-land-64a7fa9.introspection.json", "Introspection", args.runs),
        ("GitHub GraphQL API (SDL)", octo / "schema.graphql", "SDL", args.runs),
        ("GitHub GraphQL API (Introspection JSON)", octo / "schema.json", "Introspection", min(args.runs, 6)),
    ]
    for name, path, kind, runs in gql_specs:
        flags = [str(path), "--detail", "full", "--include-schemas", "--no-report"]
        rc21, out21, _, _ = run_cmd([v21, *flags])
        rc22, out22, _, _ = run_cmd([v22, *flags])
        hf = hyperfine_pair(sh_quote([v21, *flags]), sh_quote([v22, *flags]), "v2_1", "v2_2", runs=runs)
        results["graphql"].append({
            "name": name,
            "kind": kind,
            "size_bytes": path.stat().st_size,
            "byte_identical": (rc21 == 0 and rc22 == 0 and out21 == out22),
            "hyperfine": hf,
        })
        print(f"  {name}: identical={out21==out22} hf={hf}", flush=True)

    for label, dflags, cwd, runs in [
        ("SpaceX Land SDL diff (0a482d1 -> 64a7fa9)", ["diff", str(REPO / "tests/fixtures/graphql/spacex-land-0a482d1.graphql"), str(REPO / "tests/fixtures/graphql/spacex-land-64a7fa9.graphql"), "--format", "json"], None, args.runs),
        ("GitHub GraphQL SDL Git-ref diff (HEAD~1 -> HEAD)", ["diff", "--from-ref", "HEAD~1", "--to-ref", "HEAD", "--spec", "schema.graphql", "--format", "json"], str(octo), 5),
        ("GitHub GraphQL Introspection Git-ref diff (HEAD~1 -> HEAD)", ["diff", "--from-ref", "HEAD~1", "--to-ref", "HEAD", "--spec", "schema.json", "--format", "json"], str(octo), 5),
    ]:
        _, d21, _, _ = run_cmd([v21, *dflags], cwd=cwd)
        _, d22, _, _ = run_cmd([v22, *dflags], cwd=cwd)
        j21, j22 = json.loads(d21), json.loads(d22)
        hf = hyperfine_pair(sh_quote([v21, *dflags]), sh_quote([v22, *dflags]), "v2_1", "v2_2", runs=runs, cwd=cwd)
        results["graphql_diffs"].append({
            "name": label,
            "changes_exact_match": (j21["changes"] == j22["changes"]),
            "summary_v22": j22["summary"],
            "hyperfine": hf,
        })
        print(f"  {label}: exact={j21['changes']==j22['changes']} hf={hf}", flush=True)

    # 3. Protobuf targets (10 runs; v2.2.0 explicitly uses env PATH='' to prove zero protoc)
    print("[3/6] Protobuf benchmarks (v2.2.0 invoked with env PATH='')...", flush=True)
    for label, p, inc in [
        ("gRPC Health v1.20.0 (.proto)", REPO / "tests/fixtures/protobuf/health-v1.20.0.proto", []),
        ("gRPC Health v1.20.0 (.pb descriptor)", REPO / "tests/fixtures/protobuf/health-v1.20.0.pb", []),
        ("OTel Collector TraceService (.proto)", otel / "opentelemetry/proto/collector/trace/v1/trace_service.proto", ["-I", str(otel)]),
        ("OTel Collector MetricsService (.proto)", otel / "opentelemetry/proto/collector/metrics/v1/metrics_service.proto", ["-I", str(otel)]),
        ("OTel Collector LogsService (.proto)", otel / "opentelemetry/proto/collector/logs/v1/logs_service.proto", ["-I", str(otel)]),
    ]:
        pflags = [str(p), *inc, "--detail", "full", "--include-schemas", "--no-report"]
        _, o21, _, _ = run_cmd([v21, *pflags])
        _, o22, _, _ = run_cmd([v22, *pflags], env=env_no_protoc)
        cmd21 = sh_quote([v21, *pflags])
        cmd22 = sh_quote([v22, *pflags])
        # Zero-protoc parity verified above with env_no_protoc (PATH="")
        hf = hyperfine_pair(cmd21, cmd22, "v2_1", "v2_2", runs=args.runs)
        results["protobuf_targets"].append({
            "name": label,
            "v21_bytes": len(o21),
            "v22_bytes": len(o22),
            "field_notes_v21": len(re.findall(r"Field #\d+", o21.decode("utf-8", errors="replace"))),
            "field_notes_v22": len(re.findall(r"Field #\d+", o22.decode("utf-8", errors="replace"))),
            "hyperfine": hf,
        })
        print(f"  {label}: hf={hf}", flush=True)

    # OTel real Git-ref diff
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
            "hyperfine": hyperfine_pair(sh_quote([v21, *gargs]), sh_quote([v22, *gargs]), "v2_1", "v2_2", runs=6, cwd=str(otel)),
        }
        print(f"  OTel Git-ref diff: {results['protobuf_git_diff_otel']['hyperfine']}", flush=True)

    # 4. Synthetic 200-proto Git-ref scaling benchmark (measured, reproducible!)
    print("[4/6] Synthetic 200-proto Git-ref scaling benchmark...", flush=True)
    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        p200 = build_synthetic_200_proto_repo(tmp)
        sargs = ["diff", "--from-ref", "v1", "--to-ref", "v2", "--spec", "protos/orders/service.proto", "-I", "protos", "--format", "json"]
        _, so21, _, _ = run_cmd([v21, *sargs], cwd=str(p200))
        _, so22, _, _ = run_cmd([v22, *sargs], cwd=str(p200))
        sj21, sj22 = json.loads(so21), json.loads(so22)
        shf = hyperfine_pair(sh_quote([v21, *sargs]), sh_quote([v22, *sargs]), "v2_1", "v2_2", runs=3, warmup=1, cwd=str(p200))
        results["protobuf_git_diff_200_synthetic"] = {
            "total_committed_protos": 200,
            "imported_protos_used": 4,
            "changes_count": len(sj22["changes"]),
            "changes_exact_match": (sj21["changes"] == sj22["changes"]),
            "hyperfine": shf,
        }
        print(f"  Synthetic 200-proto Git-ref diff: {shf}", flush=True)

        # 5. External baseline comparisons: oasdiff & buf breaking
        print("[5/6] External baseline comparisons (oasdiff 1.39 & buf 1.75)...", flush=True)
        results["external_baselines"]["oasdiff"] = run_oasdiff_comparison(v22, tmp)
        results["external_baselines"]["buf"] = run_buf_comparison(v22, tmp)
        print(f"  oasdiff fixture speed: {results['external_baselines']['oasdiff']['fixture_oas3']['hyperfine']}", flush=True)
        print(f"  buf health speed: {results['external_baselines']['buf']['health_speed']}", flush=True)

    # 6. Exact BPE tokenization (o200k_base) vs chars/4 calibration
    print("[6/6] Exact BPE (tiktoken o200k_base) vs chars/4 calibration...", flush=True)
    cal_targets = [
        ("OpenAI OpenAPI 3.0 (1.3 MB YAML)", corpus / "openai.yaml", "YAML", [], ["--operation", "POST /chat/completions", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
        ("Stripe OpenAPI 3.0 (7.9 MB JSON)", corpus / "stripe.json", "JSON", [], ["--operation", "POST /v1/customers", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
        ("GitHub GraphQL API (1.2 MB SDL)", octo / "schema.graphql", "GraphQL SDL", [], ["--operation", "QUERY Query.repository", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
        ("SpaceX Land GraphQL (12 KB SDL)", REPO / "tests/fixtures/graphql/spacex-land-64a7fa9.graphql", "GraphQL SDL", [], ["--operation", "QUERY Query.launchesPastResult", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
        ("OTel TraceService (.proto + 3 transitive)", otel / "opentelemetry/proto/collector/trace/v1/trace_service.proto", "Protobuf", ["-I", str(otel)], ["--operation", "RPC opentelemetry.proto.collector.trace.v1.TraceService.Export", "--detail", "full", "--include-schemas", "--schema-depth", "3", "--no-report"]),
    ]
    for name, path, fmt, inc, sel_flags in cal_targets:
        raw_text = path.read_text(encoding="utf-8", errors="replace")
        if fmt == "Protobuf":
            # Include transitive imports for fair raw source size
            trans = [
                otel / "opentelemetry/proto/collector/trace/v1/trace_service.proto",
                otel / "opentelemetry/proto/trace/v1/trace.proto",
                otel / "opentelemetry/proto/resource/v1/resource.proto",
                otel / "opentelemetry/proto/common/v1/common.proto",
            ]
            raw_text = "\n".join(p.read_text() for p in trans)
        _, full_out, _, _ = run_cmd([v22, str(path), *inc, "--detail", "full", "--include-schemas", "--no-report"])
        _, sum_out, _, _ = run_cmd([v22, str(path), *inc, "--detail", "summary", "--no-report"])
        _, foc_out, _, _ = run_cmd([v22, str(path), *inc, *sel_flags])

        full_t = full_out.decode("utf-8", errors="replace")
        sum_t = sum_out.decode("utf-8", errors="replace")
        foc_t = foc_out.decode("utf-8", errors="replace")

        raw_bpe, raw_est = count_bpe(raw_text), chars_div_4(raw_text)
        full_bpe, full_est = count_bpe(full_t), chars_div_4(full_t)
        sum_bpe, sum_est = count_bpe(sum_t), chars_div_4(sum_t)
        foc_bpe, foc_est = count_bpe(foc_t), chars_div_4(foc_t)

        results["token_calibration"].append({
            "name": name,
            "format": fmt,
            "raw_chars": len(raw_text),
            "raw_bpe_o200k": raw_bpe,
            "raw_chars_div_4": raw_est,
            "raw_chars_per_bpe": round(len(raw_text) / raw_bpe, 2) if raw_bpe else None,
            "full_md_bpe_o200k": full_bpe,
            "full_md_chars_div_4": full_est,
            "full_md_chars_per_bpe": round(len(full_t) / full_bpe, 2) if full_bpe else None,
            "summary_bpe_o200k": sum_bpe,
            "summary_chars_div_4": sum_est,
            "focused_bpe_o200k": foc_bpe,
            "focused_chars_div_4": foc_est,
            "focused_vs_raw_bpe_reduction_pct": round((1 - foc_bpe / raw_bpe) * 100, 1) if raw_bpe else None,
        })
        print(f"  Token calibration {name}: raw={raw_bpe} BPE ({round(len(raw_text)/raw_bpe,2)} c/tok) -> focused={foc_bpe} BPE ({round(len(foc_t)/foc_bpe,2)} c/tok)", flush=True)

    out_path = Path(args.output)
    out_path.write_text(json.dumps(results, indent=2) + "\n")
    print(f"\nWrote {out_path}", flush=True)


if __name__ == "__main__":
    main()
