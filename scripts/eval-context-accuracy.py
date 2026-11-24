#!/usr/bin/env python3
"""Deterministic task-accuracy evaluation comparing four context strategies
on the same LLM (claude-haiku-5-5, zero tools, isolated system prompt):

  1. no_spec        — memorization control (model sees only the question)
  2. summary        — vimanam --detail summary --no-report
  3. raw_spec       — raw OpenAPI YAML / JSON, raw .proto + transitive imports, or raw GraphQL SDL
  4. focused_slice  — vimanam --operation / --schema targeted Markdown slice

Every task is graded programmatically on strict JSON output (no LLM-as-judge).
Records exact API input/output tokens, tiktoken o200k_base tokens, chars/4
estimate, API latency, USD cost, and per-assertion pass/fail.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import time
from pathlib import Path

import tiktoken

REPO = Path(__file__).resolve().parent.parent
ENC = tiktoken.get_encoding("o200k_base")

SYSTEM_PROMPT = (
    "You are an API contract assistant. Answer strictly from the provided API context. "
    "Return ONLY a single valid JSON object matching the exact schema requested in the prompt, "
    "with no markdown code fences or prose outside the JSON."
)


def count_bpe(text: str) -> int:
    return len(ENC.encode(text, disallowed_special=()))


def extract_json(raw: str) -> dict | None:
    text = raw.strip()
    if text.startswith("```"):
        text = re.sub(r"^```[a-zA-Z]*\n", "", text)
        text = re.sub(r"\n```$", "", text.strip())
    try:
        obj = json.loads(text)
        return obj if isinstance(obj, dict) else None
    except Exception:
        m = re.search(r"\{.*\}", text, re.S)
        if m:
            try:
                obj = json.loads(m.group(0))
                return obj if isinstance(obj, dict) else None
            except Exception:
                return None
        return None


def norm_set(xs) -> set[str]:
    if not isinstance(xs, list):
        return set()
    return {str(x).strip() for x in xs}


def call_claude(model: str, prompt: str, timeout: int = 120) -> dict:
    t0 = time.perf_counter()
    p = subprocess.run(
        [
            "claude",
            "-p",
            "--model", model,
            "--no-session-persistence",
            "--tools", "",
            "--strict-mcp-config",
            "--setting-sources", "",
            "--system-prompt", SYSTEM_PROMPT,
            "--output-format", "json",
        ],
        input=prompt,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    wall_ms = round((time.perf_counter() - t0) * 1000)
    if p.returncode != 0:
        return {
            "error": (p.stderr or p.stdout).strip()[:400],
            "wall_ms": wall_ms,
        }
    try:
        data = json.loads(p.stdout)
    except Exception:
        return {"error": f"non-json cli output: {p.stdout[:300]}", "wall_ms": wall_ms}
    if data.get("is_error"):
        return {
            "error": str(data.get("result", "api error"))[:400],
            "wall_ms": wall_ms,
        }
    usage = data.get("usage", {})
    model_used = next(iter(data.get("modelUsage", {}).keys()), model)
    return {
        "model": model_used,
        "result": data.get("result", ""),
        "duration_api_ms": data.get("duration_api_ms"),
        "wall_ms": wall_ms,
        "cost_usd": round(data.get("total_cost_usd", 0.0), 6),
        "api_input_tokens": usage.get("input_tokens", 0)
        + usage.get("cache_creation_input_tokens", 0)
        + usage.get("cache_read_input_tokens", 0),
        "api_output_tokens": usage.get("output_tokens", 0),
    }


def vimanam_out(v22: str, args: list[str]) -> str:
    p = subprocess.run([v22, *args], capture_output=True, text=True, check=True)
    return p.stdout


def build_tasks(v22: str, corpus: Path) -> list[dict]:
    openai_yaml = corpus / "openai.yaml"
    stripe_json = corpus / "stripe.json"
    octo_gql = corpus / "octokit-graphql/schema.graphql"
    spacex_gql = REPO / "tests/fixtures/graphql/spacex-land-64a7fa9.graphql"
    health_proto = REPO / "tests/fixtures/protobuf/health-v1.20.0.proto"
    otel_root = corpus / "otel-proto"
    otel_trace = otel_root / "opentelemetry/proto/collector/trace/v1/trace_service.proto"
    otel_metrics = otel_root / "opentelemetry/proto/collector/metrics/v1/metrics_service.proto"

    # Concatenate transitive raw .proto files for OpenTelemetry tasks so raw_spec has full truth
    def concat_protos(paths: list[Path]) -> str:
        return "\n\n".join(f"// === {p.name} ===\n{p.read_text()}" for p in paths)

    otel_trace_raw = concat_protos([
        otel_trace,
        otel_root / "opentelemetry/proto/trace/v1/trace.proto",
        otel_root / "opentelemetry/proto/resource/v1/resource.proto",
        otel_root / "opentelemetry/proto/common/v1/common.proto",
    ])
    otel_metrics_raw = concat_protos([
        otel_metrics,
        otel_root / "opentelemetry/proto/metrics/v1/metrics.proto",
        otel_root / "opentelemetry/proto/resource/v1/resource.proto",
        otel_root / "opentelemetry/proto/common/v1/common.proto",
    ])

    openai_summary = vimanam_out(v22, [str(openai_yaml), "--detail", "summary", "--no-report"])
    stripe_summary = vimanam_out(v22, [str(stripe_json), "--detail", "summary", "--no-report"])
    health_summary = vimanam_out(v22, [str(health_proto), "--detail", "summary", "--no-report"])
    otel_trace_summary = vimanam_out(v22, [str(otel_trace), "-I", str(otel_root), "--detail", "summary", "--no-report"])
    otel_metrics_summary = vimanam_out(v22, [str(otel_metrics), "-I", str(otel_root), "--detail", "summary", "--no-report"])
    spacex_summary = vimanam_out(v22, [str(spacex_gql), "--detail", "summary", "--no-report"])
    octo_summary = vimanam_out(v22, [str(octo_gql), "--detail", "summary", "--no-report"])

    return [
        # 1. OpenAI POST /embeddings
        {
            "id": "oa_embeddings",
            "format": "OpenAPI YAML",
            "spec": "OpenAI OpenAPI 3.0 (1.3 MB YAML)",
            "question": (
                'For the operation that creates an embedding vector (`POST /embeddings`), return JSON:\n'
                '{"operation_id": string, "required_fields": [string], "encoding_format_enum": [string], '
                '"dimensions_type": string, "response_schema": string}'
            ),
            "contexts": {
                "no_spec": "",
                "summary": openai_summary,
                "raw_spec": openai_yaml.read_text(),
                "focused_slice": vimanam_out(v22, [str(openai_yaml), "--operation", "POST /embeddings", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            },
            "grade": lambda d: {
                "operation_id": d.get("operation_id") == "createEmbedding",
                "required_fields": norm_set(d.get("required_fields")) == {"input", "model"},
                "encoding_format_enum": norm_set(d.get("encoding_format_enum")) == {"float", "base64"},
                "dimensions_type": str(d.get("dimensions_type", "")).lower() in {"integer", "int"},
                "response_schema": d.get("response_schema") == "CreateEmbeddingResponse",
            },
        },
        # 2. OpenAI POST /batches
        {
            "id": "oa_batches",
            "format": "OpenAPI YAML",
            "spec": "OpenAI OpenAPI 3.0 (1.3 MB YAML)",
            "question": (
                'For the operation that creates a batch (`POST /batches`), return JSON:\n'
                '{"operation_id": string, "required_fields": [string], "completion_window_value": string, '
                '"supported_endpoints": [string], "response_schema": string}'
            ),
            "contexts": {
                "no_spec": "",
                "summary": openai_summary,
                "raw_spec": openai_yaml.read_text(),
                "focused_slice": vimanam_out(v22, [str(openai_yaml), "--operation", "POST /batches", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            },
            "grade": lambda d: {
                "operation_id": d.get("operation_id") == "createBatch",
                "required_fields": norm_set(d.get("required_fields")) == {"input_file_id", "endpoint", "completion_window"},
                "completion_window_value": d.get("completion_window_value") == "24h",
                "supported_endpoints": norm_set(d.get("supported_endpoints")) == {"/v1/responses", "/v1/chat/completions", "/v1/embeddings", "/v1/completions"},
                "response_schema": d.get("response_schema") == "Batch",
            },
        },
        # 3. OpenAI POST /files
        {
            "id": "oa_files",
            "format": "OpenAPI YAML",
            "spec": "OpenAI OpenAPI 3.0 (1.3 MB YAML)",
            "question": (
                'For the operation that uploads a file (`POST /files`), return JSON:\n'
                '{"operation_id": string, "required_fields": [string], "purpose_enum": [string], '
                '"response_schema": string}'
            ),
            "contexts": {
                "no_spec": "",
                "summary": openai_summary,
                "raw_spec": openai_yaml.read_text(),
                "focused_slice": vimanam_out(v22, [str(openai_yaml), "--operation", "POST /files", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            },
            "grade": lambda d: {
                "operation_id": d.get("operation_id") == "createFile",
                "required_fields": norm_set(d.get("required_fields")) == {"file", "purpose"},
                "purpose_enum": norm_set(d.get("purpose_enum")) == {"assistants", "batch", "fine-tune", "vision", "user_data", "evals"},
                "response_schema": d.get("response_schema") == "OpenAIFile",
            },
        },
        # 4. Stripe POST /v1/customers (7.9 MB raw exceeds 1M context window!)
        {
            "id": "oa_stripe_customers",
            "format": "OpenAPI JSON",
            "spec": "Stripe OpenAPI 3.0 (7.9 MB JSON)",
            "question": (
                'For creating a customer (`POST /v1/customers`), return JSON:\n'
                '{"operation_id": string, "tax_exempt_enum": [string], "invoice_prefix_max_len": integer, '
                '"response_schema": string}'
            ),
            "contexts": {
                "no_spec": "",
                "summary": stripe_summary,
                "raw_spec": "__OVER_CONTEXT_WINDOW_7.9MB__",
                "focused_slice": vimanam_out(v22, [str(stripe_json), "--operation", "POST /v1/customers", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            },
            "raw_file_for_tokens": stripe_json,
            "grade": lambda d: {
                "operation_id": d.get("operation_id") == "PostCustomers",
                "tax_exempt_enum": norm_set(d.get("tax_exempt_enum")) == {"none", "exempt", "reverse"},
                "invoice_prefix_max_len": d.get("invoice_prefix_max_len") == 12,
                "response_schema": d.get("response_schema") == "customer",
            },
        },
        # 5. Stripe POST /v1/payment_intents (7.9 MB raw exceeds 1M context window!)
        {
            "id": "oa_stripe_payment_intents",
            "format": "OpenAPI JSON",
            "spec": "Stripe OpenAPI 3.0 (7.9 MB JSON)",
            "question": (
                'For creating a PaymentIntent (`POST /v1/payment_intents`), return JSON:\n'
                '{"operation_id": string, "required_fields": [string], "amount_max_digits": integer, '
                '"currency_type": string}'
            ),
            "contexts": {
                "no_spec": "",
                "summary": stripe_summary,
                "raw_spec": "__OVER_CONTEXT_WINDOW_7.9MB__",
                "focused_slice": vimanam_out(v22, [str(stripe_json), "--operation", "POST /v1/payment_intents", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            },
            "raw_file_for_tokens": stripe_json,
            "grade": lambda d: {
                "operation_id": d.get("operation_id") == "PostPaymentIntents",
                "required_fields": norm_set(d.get("required_fields")) == {"amount", "currency"},
                "amount_max_digits": d.get("amount_max_digits") == 8,
                "currency_type": "string" in str(d.get("currency_type", "")).lower(),
            },
        },
        # 6. gRPC Health.Watch (.proto)
        {
            "id": "pb_health_watch",
            "format": "Protobuf / gRPC",
            "spec": "grpc/grpc health-v1.20.0.proto",
            "question": (
                'For RPC `grpc.health.v1.Health.Watch`, return JSON:\n'
                '{"streaming_mode": string, "request_message": string, "response_message": string, '
                '"status_field_number": integer, "watch_only_enum_name": string, "watch_only_enum_number": integer}'
            ),
            "contexts": {
                "no_spec": "",
                "summary": health_summary,
                "raw_spec": health_proto.read_text(),
                "focused_slice": vimanam_out(v22, [str(health_proto), "--operation", "RPC grpc.health.v1.Health.Watch", "--detail", "full", "--include-schemas", "--no-report"]),
            },
            "grade": lambda d: {
                "streaming_mode": "server" in str(d.get("streaming_mode", "")).lower(),
                "request_message": str(d.get("request_message", "")).endswith("HealthCheckRequest"),
                "response_message": str(d.get("response_message", "")).endswith("HealthCheckResponse"),
                "status_field_number": d.get("status_field_number") == 1,
                "watch_only_enum_name": d.get("watch_only_enum_name") == "SERVICE_UNKNOWN",
                "watch_only_enum_number": d.get("watch_only_enum_number") == 3,
            },
        },
        # 7. OpenTelemetry TraceService.Export (.proto)
        {
            "id": "pb_otel_trace",
            "format": "Protobuf / gRPC",
            "spec": "OpenTelemetry trace_service.proto (+ imports)",
            "question": (
                'For RPC `opentelemetry.proto.collector.trace.v1.TraceService.Export`, return JSON:\n'
                '{"streaming_mode": string, "request_field_1_name": string, "request_field_1_json_name": string, '
                '"partial_success_field_1_name": string, "partial_success_field_1_type": string, '
                '"partial_success_field_2_name": string, "partial_success_field_2_number": integer}'
            ),
            "contexts": {
                "no_spec": "",
                "summary": otel_trace_summary,
                "raw_spec": otel_trace_raw,
                "focused_slice": vimanam_out(v22, [str(otel_trace), "-I", str(otel_root), "--operation", "RPC opentelemetry.proto.collector.trace.v1.TraceService.Export", "--detail", "full", "--include-schemas", "--schema-depth", "3", "--no-report"]),
            },
            "grade": lambda d: {
                "streaming_mode": "unary" in str(d.get("streaming_mode", "")).lower(),
                "request_field_1_name": d.get("request_field_1_name") == "resource_spans",
                "request_field_1_json_name": d.get("request_field_1_json_name") == "resourceSpans",
                "partial_success_field_1_name": d.get("partial_success_field_1_name") == "rejected_spans",
                "partial_success_field_1_type": d.get("partial_success_field_1_type") == "int64",
                "partial_success_field_2_name": d.get("partial_success_field_2_name") == "error_message",
                "partial_success_field_2_number": d.get("partial_success_field_2_number") == 2,
            },
        },
        # 8. OpenTelemetry MetricsService.Export (.proto)
        {
            "id": "pb_otel_metrics",
            "format": "Protobuf / gRPC",
            "spec": "OpenTelemetry metrics_service.proto (+ imports)",
            "question": (
                'For RPC `opentelemetry.proto.collector.metrics.v1.MetricsService.Export`, return JSON:\n'
                '{"request_field_1_name": string, "request_field_1_json_name": string, '
                '"partial_success_field_1_name": string, "partial_success_field_1_json_name": string, '
                '"partial_success_field_2_name": string}'
            ),
            "contexts": {
                "no_spec": "",
                "summary": otel_metrics_summary,
                "raw_spec": otel_metrics_raw,
                "focused_slice": vimanam_out(v22, [str(otel_metrics), "-I", str(otel_root), "--operation", "RPC opentelemetry.proto.collector.metrics.v1.MetricsService.Export", "--detail", "full", "--include-schemas", "--schema-depth", "3", "--no-report"]),
            },
            "grade": lambda d: {
                "request_field_1_name": d.get("request_field_1_name") == "resource_metrics",
                "request_field_1_json_name": d.get("request_field_1_json_name") == "resourceMetrics",
                "partial_success_field_1_name": d.get("partial_success_field_1_name") == "rejected_data_points",
                "partial_success_field_1_json_name": d.get("partial_success_field_1_json_name") == "rejectedDataPoints",
                "partial_success_field_2_name": d.get("partial_success_field_2_name") == "error_message",
            },
        },
        # 9. SpaceX Land Query.launchesPastResult + LaunchFind (GraphQL SDL)
        {
            "id": "gql_spacex_launches",
            "format": "GraphQL SDL",
            "spec": "SpaceX Land (12 KB SDL)",
            "question": (
                'For GraphQL field `Query.launchesPastResult` and its `LaunchFind` input object, return JSON:\n'
                '{"argument_names": [string], "return_type": string, "apoapsis_km_type": string, '
                '"fairings_recovery_attempt_type": string, "core_flight_type": string}'
            ),
            "contexts": {
                "no_spec": "",
                "summary": spacex_summary,
                "raw_spec": spacex_gql.read_text(),
                "focused_slice": vimanam_out(v22, [str(spacex_gql), "--operation", "QUERY Query.launchesPastResult", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"])
                + "\n"
                + vimanam_out(v22, [str(spacex_gql), "--schema", "LaunchFind", "--no-report"]),
            },
            "grade": lambda d: {
                "argument_names": norm_set(d.get("argument_names")) == {"find", "limit", "offset", "order", "sort"},
                "return_type": d.get("return_type") == "LaunchesPastResult",
                "apoapsis_km_type": d.get("apoapsis_km_type") == "Float",
                "fairings_recovery_attempt_type": d.get("fairings_recovery_attempt_type") == "String",
                "core_flight_type": d.get("core_flight_type") == "Int",
            },
        },
        # 10. GitHub GraphQL Mutation.createIssue + CreateIssueInput (1.2 MB SDL)
        {
            "id": "gql_github_create_issue",
            "format": "GraphQL SDL",
            "spec": "GitHub GraphQL API (1.2 MB SDL)",
            "question": (
                'For GraphQL mutation `Mutation.createIssue` and its `CreateIssueInput` input object, return JSON:\n'
                '{"argument_name": string, "return_type": string, "required_input_fields": [string], '
                '"issue_template_type": string, "parent_issue_id_type": string}'
            ),
            "contexts": {
                "no_spec": "",
                "summary": octo_summary,
                "raw_spec": octo_gql.read_text(),
                "focused_slice": vimanam_out(v22, [str(octo_gql), "--operation", "MUTATION Mutation.createIssue", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"])
                + "\n"
                + vimanam_out(v22, [str(octo_gql), "--schema", "CreateIssueInput", "--no-report"]),
            },
            "grade": lambda d: {
                "argument_name": d.get("argument_name") == "input",
                "return_type": d.get("return_type") == "CreateIssuePayload",
                "required_input_fields": norm_set(d.get("required_input_fields")) == {"repositoryId", "title"},
                "issue_template_type": d.get("issue_template_type") == "String",
                "parent_issue_id_type": d.get("parent_issue_id_type") == "ID",
            },
        },
    ]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--v22", required=True)
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--model", default="haiku")
    ap.add_argument("--output", required=True)
    args = ap.parse_args()

    tasks = build_tasks(str(Path(args.v22).resolve()), Path(args.corpus).resolve())
    conditions = ["no_spec", "summary", "raw_spec", "focused_slice"]

    summary_by_cond = {
        c: {
            "tasks_passed": 0,
            "tasks_total": len(tasks),
            "assertions_passed": 0,
            "assertions_total": 0,
            "context_overflow_tasks": 0,
            "total_bpe_o200k": 0,
            "total_api_input_tokens": 0,
            "total_duration_api_ms": 0,
            "total_cost_usd": 0.0,
        }
        for c in conditions
    }

    task_records = []
    model_id_seen = None

    for task in tasks:
        rec = {
            "id": task["id"],
            "format": task["format"],
            "spec": task["spec"],
            "conditions": {},
        }
        for cond in conditions:
            ctx = task["contexts"][cond]
            if ctx == "__OVER_CONTEXT_WINDOW_7.9MB__":
                raw_text = task["raw_file_for_tokens"].read_text()
                bpe = count_bpe(raw_text)
                n_assert = len(task["grade"]({}))
                summary_by_cond[cond]["assertions_total"] += n_assert
                summary_by_cond[cond]["context_overflow_tasks"] += 1
                summary_by_cond[cond]["total_bpe_o200k"] += bpe
                rec["conditions"][cond] = {
                    "status": "context_overflow",
                    "note": f"Raw 7.9 MB JSON ({bpe:,} o200k_base tokens) exceeds 1,000,000-token context window",
                    "context_bpe_o200k": bpe,
                    "context_chars_div_4": (len(raw_text) + 3) // 4,
                    "task_passed": False,
                    "assertions_passed": 0,
                    "assertions_total": n_assert,
                }
                print(f"  {task['id']:26s} {cond:14s} OVERFLOW ({bpe:,} BPE tokens)", flush=True)
                continue

            bpe = count_bpe(ctx) if ctx else 0
            est = ((len(ctx) + 3) // 4) if ctx else 0
            if ctx:
                prompt = f"<api_context>\n{ctx}\n</api_context>\n\n{task['question']}\n"
            else:
                prompt = f"No API specification context is provided.\n\n{task['question']}\n"

            res = call_claude(args.model, prompt)
            if "error" in res:
                n_assert = len(task["grade"]({}))
                summary_by_cond[cond]["assertions_total"] += n_assert
                rec["conditions"][cond] = {
                    "status": "error",
                    "error": res["error"],
                    "context_bpe_o200k": bpe,
                    "context_chars_div_4": est,
                    "task_passed": False,
                    "assertions_passed": 0,
                    "assertions_total": n_assert,
                }
                print(f"  {task['id']:26s} {cond:14s} ERROR: {res['error'][:80]}", flush=True)
                continue

            model_id_seen = res["model"]
            parsed = extract_json(res["result"]) or {}
            checks = task["grade"](parsed)
            passed_cnt = sum(1 for v in checks.values() if v)
            total_cnt = len(checks)
            all_ok = (passed_cnt == total_cnt)

            s = summary_by_cond[cond]
            s["tasks_passed"] += int(all_ok)
            s["assertions_passed"] += passed_cnt
            s["assertions_total"] += total_cnt
            s["total_bpe_o200k"] += bpe
            s["total_api_input_tokens"] += res["api_input_tokens"]
            s["total_duration_api_ms"] += res["duration_api_ms"] or 0
            s["total_cost_usd"] = round(s["total_cost_usd"] + res["cost_usd"], 6)

            rec["conditions"][cond] = {
                "status": "ok",
                "context_bpe_o200k": bpe,
                "context_chars_div_4": est,
                "api_input_tokens": res["api_input_tokens"],
                "api_output_tokens": res["api_output_tokens"],
                "duration_api_ms": res["duration_api_ms"],
                "cost_usd": res["cost_usd"],
                "task_passed": all_ok,
                "assertions_passed": passed_cnt,
                "assertions_total": total_cnt,
                "checks": checks,
            }
            print(
                f"  {task['id']:26s} {cond:14s} {passed_cnt}/{total_cnt} "
                f"({'PASS' if all_ok else 'FAIL'}) bpe={bpe:,} api_in={res['api_input_tokens']:,} "
                f"lat={res['duration_api_ms']}ms cost=${res['cost_usd']:.4f}",
                flush=True,
            )
        task_records.append(rec)

    # Also compute summary over the 8 tasks where raw_spec fits in context
    fitting_ids = {t["id"] for t in tasks if t["contexts"]["raw_spec"] != "__OVER_CONTEXT_WINDOW_7.9MB__"}
    fitting_summary = {}
    for cond in conditions:
        tp = ap_c = at_c = bpe_s = api_in_s = dur_s = 0
        cost_s = 0.0
        for rec in task_records:
            if rec["id"] not in fitting_ids:
                continue
            c = rec["conditions"][cond]
            tp += int(c["task_passed"])
            ap_c += c["assertions_passed"]
            at_c += c["assertions_total"]
            bpe_s += c.get("context_bpe_o200k", 0)
            api_in_s += c.get("api_input_tokens", 0)
            dur_s += c.get("duration_api_ms", 0) or 0
            cost_s += c.get("cost_usd", 0.0)
        fitting_summary[cond] = {
            "tasks_passed": tp,
            "tasks_total": len(fitting_ids),
            "assertions_passed": ap_c,
            "assertions_total": at_c,
            "total_bpe_o200k": bpe_s,
            "total_api_input_tokens": api_in_s,
            "total_duration_api_ms": dur_s,
            "total_cost_usd": round(cost_s, 6),
        }

    report = {
        "meta": {
            "model": model_id_seen or args.model,
            "vimanam_version": subprocess.check_output([args.v22, "--version"], text=True).strip(),
            "task_count": len(tasks),
            "fitting_task_count": len(fitting_ids),
            "grading": "deterministic programmatic JSON assertions (no LLM judge)",
            "isolation": "claude -p --tools '' --strict-mcp-config --setting-sources ''",
        },
        "summary_all_10_tasks": summary_by_cond,
        "summary_8_tasks_where_raw_fits": fitting_summary,
        "tasks": task_records,
    }
    out_path = Path(args.output)
    out_path.write_text(json.dumps(report, indent=2) + "\n")
    print(f"\nWrote {out_path}", flush=True)


if __name__ == "__main__":
    main()
