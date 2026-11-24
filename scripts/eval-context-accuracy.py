#!/usr/bin/env python3
"""End-to-end routed and oracle task-accuracy evaluation on the same LLM
(claude-haiku-5-5, zero tools, isolated system prompt, repeated trials).

Conditions:
  1. no_spec      — memorization control (question only, N trials)
  2. summary      — vimanam --detail summary --no-report (N trials)
  3. routed_e2e   — real workflow: summary -> model selects vimanam operation/schema
                    flags -> harness runs vimanam -> optional 1 follow-up --schema
                    call when truncated or missing a referenced type -> answer (N trials)
  4. oracle_slice — single-turn harness-selected slice (N trials; separates routing
                    from slice readability)
  5. raw_spec     — raw contract bytes (N trials on small specs, 1 trial on >100k-token
                    specs; explicitly marked not_attempted on 7.9 MB Stripe JSON which
                    exceeds the 1,000,000-token context window)

Every task is graded deterministically on strict JSON assertions (no LLM judge).
Also records Claude API context tokens (api_input_tokens minus no_spec baseline)
alongside tiktoken o200k_base BPE tokens and chars/4 estimates.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import re
import statistics
import subprocess
import time
from pathlib import Path

import tiktoken

REPO = Path(__file__).resolve().parent.parent
ENC = tiktoken.get_encoding("o200k_base")

ANSWER_SYSTEM_PROMPT = (
    "You are an API contract assistant. Answer strictly from the provided API context. "
    "Return ONLY a single valid JSON object matching the exact schema requested in the prompt, "
    "with no markdown code fences or prose outside the JSON."
)

ROUTE_SYSTEM_PROMPT = (
    "You are an API contract navigator using the `vimanam` CLI. "
    "Given a `vimanam --detail summary` map and a question, choose which operation and/or "
    "named schemas to inspect next. Return ONLY a single valid JSON object with no fences or prose."
)

FOLLOWUP_OR_ANSWER_SYSTEM_PROMPT = (
    "You are an API contract assistant reading focused `vimanam` Markdown output. "
    "Answer strictly from the provided context. If any required field or nested schema was omitted "
    "(for example at a schema-depth cutoff or a referenced input/output type not expanded yet), "
    'you may request up to 3 named schemas by returning ONLY {"fetch_schemas": ["SchemaName", ...]}. '
    "Otherwise return ONLY the final JSON answer matching the exact requested schema, with no code fences."
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


def call_claude(model: str, prompt: str, system_prompt: str, timeout: int = 120) -> dict:
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
            "--system-prompt", system_prompt,
            "--output-format", "json",
        ],
        input=prompt,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    wall_ms = round((time.perf_counter() - t0) * 1000)
    if p.returncode != 0:
        return {"error": (p.stderr or p.stdout).strip()[:400], "wall_ms": wall_ms}
    try:
        data = json.loads(p.stdout)
    except Exception:
        return {"error": f"non-json cli output: {p.stdout[:300]}", "wall_ms": wall_ms}
    if data.get("is_error"):
        return {"error": str(data.get("result", "api error"))[:400], "wall_ms": wall_ms}
    usage = data.get("usage", {})
    model_used = next(iter(data.get("modelUsage", {}).keys()), model)
    return {
        "model": model_used,
        "result": data.get("result", ""),
        "duration_api_ms": data.get("duration_api_ms") or 0,
        "wall_ms": wall_ms,
        "cost_usd": round(data.get("total_cost_usd", 0.0), 6),
        "api_input_tokens": usage.get("input_tokens", 0)
        + usage.get("cache_creation_input_tokens", 0)
        + usage.get("cache_read_input_tokens", 0),
        "api_output_tokens": usage.get("output_tokens", 0),
    }


def vimanam_run(v22: str, args: list[str]) -> tuple[int, str, str]:
    p = subprocess.run([v22, *args], capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def vimanam_out(v22: str, args: list[str]) -> str:
    rc, out, err = vimanam_run(v22, args)
    if rc != 0:
        raise RuntimeError(f"vimanam failed ({rc}): {err.strip()}")
    return out


def build_tasks(v22: str, corpus: Path) -> list[dict]:
    openai_yaml = corpus / "openai.yaml"
    stripe_json = corpus / "stripe-openapi/openapi/spec3.json"
    octo_gql = corpus / "octokit-graphql/schema.graphql"
    spacex_gql = REPO / "tests/fixtures/graphql/spacex-land-64a7fa9.graphql"
    health_proto = REPO / "tests/fixtures/protobuf/health-v1.20.0.proto"
    otel_root = corpus / "otel-proto"
    otel_trace = otel_root / "opentelemetry/proto/collector/trace/v1/trace_service.proto"
    otel_metrics = otel_root / "opentelemetry/proto/collector/metrics/v1/metrics_service.proto"

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
        {
            "id": "oa_embeddings",
            "format": "OpenAPI YAML",
            "spec": "OpenAI OpenAPI 3.0 (1.3 MB YAML)",
            "spec_path": openai_yaml,
            "extra_args": [],
            "raw_large": True,
            "question": (
                'For the operation that creates an embedding vector (`POST /embeddings`), return JSON:\n'
                '{"operation_id": string, "required_fields": [string], "encoding_format_enum": [string], '
                '"dimensions_type": string, "response_schema": string}'
            ),
            "summary": openai_summary,
            "raw_spec": openai_yaml.read_text(),
            "oracle_slice": vimanam_out(v22, [str(openai_yaml), "--operation", "POST /embeddings", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            "grade": lambda d: {
                "operation_id": d.get("operation_id") == "createEmbedding",
                "required_fields": norm_set(d.get("required_fields")) == {"input", "model"},
                "encoding_format_enum": norm_set(d.get("encoding_format_enum")) == {"float", "base64"},
                "dimensions_type": str(d.get("dimensions_type", "")).lower() in {"integer", "int"},
                "response_schema": d.get("response_schema") == "CreateEmbeddingResponse",
            },
        },
        {
            "id": "oa_batches",
            "format": "OpenAPI YAML",
            "spec": "OpenAI OpenAPI 3.0 (1.3 MB YAML)",
            "spec_path": openai_yaml,
            "extra_args": [],
            "raw_large": True,
            "question": (
                'For the operation that creates a batch (`POST /batches`), return JSON:\n'
                '{"operation_id": string, "required_fields": [string], "completion_window_value": string, '
                '"supported_endpoints": [string], "response_schema": string}'
            ),
            "summary": openai_summary,
            "raw_spec": openai_yaml.read_text(),
            "oracle_slice": vimanam_out(v22, [str(openai_yaml), "--operation", "POST /batches", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            "grade": lambda d: {
                "operation_id": d.get("operation_id") == "createBatch",
                "required_fields": norm_set(d.get("required_fields")) == {"input_file_id", "endpoint", "completion_window"},
                "completion_window_value": d.get("completion_window_value") == "24h",
                "supported_endpoints": norm_set(d.get("supported_endpoints")) == {"/v1/responses", "/v1/chat/completions", "/v1/embeddings", "/v1/completions"},
                "response_schema": d.get("response_schema") == "Batch",
            },
        },
        {
            "id": "oa_files",
            "format": "OpenAPI YAML",
            "spec": "OpenAI OpenAPI 3.0 (1.3 MB YAML)",
            "spec_path": openai_yaml,
            "extra_args": [],
            "raw_large": True,
            "question": (
                'For the operation that uploads a file (`POST /files`), return JSON:\n'
                '{"operation_id": string, "required_fields": [string], "purpose_enum": [string], '
                '"response_schema": string}'
            ),
            "summary": openai_summary,
            "raw_spec": openai_yaml.read_text(),
            "oracle_slice": vimanam_out(v22, [str(openai_yaml), "--operation", "POST /files", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            "grade": lambda d: {
                "operation_id": d.get("operation_id") == "createFile",
                "required_fields": norm_set(d.get("required_fields")) == {"file", "purpose"},
                "purpose_enum": norm_set(d.get("purpose_enum")) == {"assistants", "batch", "fine-tune", "vision", "user_data", "evals"},
                "response_schema": d.get("response_schema") == "OpenAIFile",
            },
        },
        {
            "id": "oa_stripe_customers",
            "format": "OpenAPI JSON",
            "spec": "Stripe OpenAPI 3.0 (7.9 MB JSON)",
            "spec_path": stripe_json,
            "extra_args": [],
            "raw_large": True,
            "question": (
                'For creating a customer (`POST /v1/customers`), return JSON:\n'
                '{"operation_id": string, "tax_exempt_enum": [string], "invoice_prefix_max_len": integer, '
                '"response_schema": string}'
            ),
            "summary": stripe_summary,
            "raw_spec": None,
            "raw_file_for_tokens": stripe_json,
            "oracle_slice": vimanam_out(v22, [str(stripe_json), "--operation", "POST /v1/customers", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            "grade": lambda d: {
                "operation_id": d.get("operation_id") == "PostCustomers",
                "tax_exempt_enum": norm_set(d.get("tax_exempt_enum")) == {"none", "exempt", "reverse"},
                "invoice_prefix_max_len": d.get("invoice_prefix_max_len") == 12,
                "response_schema": d.get("response_schema") == "customer",
            },
        },
        {
            "id": "oa_stripe_payment_intents",
            "format": "OpenAPI JSON",
            "spec": "Stripe OpenAPI 3.0 (7.9 MB JSON)",
            "spec_path": stripe_json,
            "extra_args": [],
            "raw_large": True,
            "question": (
                'For creating a PaymentIntent (`POST /v1/payment_intents`), return JSON:\n'
                '{"operation_id": string, "required_fields": [string], "amount_max_digits": integer, '
                '"currency_type": string}'
            ),
            "summary": stripe_summary,
            "raw_spec": None,
            "raw_file_for_tokens": stripe_json,
            "oracle_slice": vimanam_out(v22, [str(stripe_json), "--operation", "POST /v1/payment_intents", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            "grade": lambda d: {
                "operation_id": d.get("operation_id") == "PostPaymentIntents",
                "required_fields": norm_set(d.get("required_fields")) == {"amount", "currency"},
                "amount_max_digits": d.get("amount_max_digits") == 8,
                "currency_type": "string" in str(d.get("currency_type", "")).lower(),
            },
        },
        {
            "id": "pb_health_watch",
            "format": "Protobuf / gRPC",
            "spec": "grpc/grpc health-v1.20.0.proto",
            "spec_path": health_proto,
            "extra_args": [],
            "raw_large": False,
            "question": (
                'For RPC `grpc.health.v1.Health.Watch`, return JSON:\n'
                '{"streaming_mode": string, "request_message": string, "response_message": string, '
                '"status_field_number": integer, "watch_only_enum_name": string, "watch_only_enum_number": integer}'
            ),
            "summary": health_summary,
            "raw_spec": health_proto.read_text(),
            "oracle_slice": vimanam_out(v22, [str(health_proto), "--operation", "RPC grpc.health.v1.Health.Watch", "--detail", "full", "--include-schemas", "--no-report"]),
            "grade": lambda d: {
                "streaming_mode": "server" in str(d.get("streaming_mode", "")).lower(),
                "request_message": str(d.get("request_message", "")).endswith("HealthCheckRequest"),
                "response_message": str(d.get("response_message", "")).endswith("HealthCheckResponse"),
                "status_field_number": d.get("status_field_number") == 1,
                "watch_only_enum_name": d.get("watch_only_enum_name") == "SERVICE_UNKNOWN",
                "watch_only_enum_number": d.get("watch_only_enum_number") == 3,
            },
        },
        {
            "id": "pb_otel_trace",
            "format": "Protobuf / gRPC",
            "spec": "OpenTelemetry trace_service.proto (+ imports)",
            "spec_path": otel_trace,
            "extra_args": ["-I", str(otel_root)],
            "raw_large": False,
            "question": (
                'For RPC `opentelemetry.proto.collector.trace.v1.TraceService.Export`, return JSON:\n'
                '{"streaming_mode": string, "request_field_1_name": string, "request_field_1_json_name": string, '
                '"partial_success_field_1_name": string, "partial_success_field_1_type": string, '
                '"partial_success_field_2_name": string, "partial_success_field_2_number": integer}'
            ),
            "summary": otel_trace_summary,
            "raw_spec": otel_trace_raw,
            "oracle_slice": vimanam_out(v22, [str(otel_trace), "-I", str(otel_root), "--operation", "RPC opentelemetry.proto.collector.trace.v1.TraceService.Export", "--detail", "full", "--include-schemas", "--schema-depth", "3", "--no-report"]),
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
        {
            "id": "pb_otel_metrics",
            "format": "Protobuf / gRPC",
            "spec": "OpenTelemetry metrics_service.proto (+ imports)",
            "spec_path": otel_metrics,
            "extra_args": ["-I", str(otel_root)],
            "raw_large": False,
            "question": (
                'For RPC `opentelemetry.proto.collector.metrics.v1.MetricsService.Export`, return JSON:\n'
                '{"request_field_1_name": string, "request_field_1_json_name": string, '
                '"partial_success_field_1_name": string, "partial_success_field_1_json_name": string, '
                '"partial_success_field_2_name": string}'
            ),
            "summary": otel_metrics_summary,
            "raw_spec": otel_metrics_raw,
            "oracle_slice": vimanam_out(v22, [str(otel_metrics), "-I", str(otel_root), "--operation", "RPC opentelemetry.proto.collector.metrics.v1.MetricsService.Export", "--detail", "full", "--include-schemas", "--schema-depth", "3", "--no-report"]),
            "grade": lambda d: {
                "request_field_1_name": d.get("request_field_1_name") == "resource_metrics",
                "request_field_1_json_name": d.get("request_field_1_json_name") == "resourceMetrics",
                "partial_success_field_1_name": d.get("partial_success_field_1_name") == "rejected_data_points",
                "partial_success_field_1_json_name": d.get("partial_success_field_1_json_name") == "rejectedDataPoints",
                "partial_success_field_2_name": d.get("partial_success_field_2_name") == "error_message",
            },
        },
        {
            "id": "gql_spacex_launches",
            "format": "GraphQL SDL",
            "spec": "SpaceX Land (12 KB SDL)",
            "spec_path": spacex_gql,
            "extra_args": [],
            "raw_large": False,
            "question": (
                'For GraphQL field `Query.launchesPastResult` and its `LaunchFind` input object, return JSON:\n'
                '{"argument_names": [string], "return_type": string, "apoapsis_km_type": string, '
                '"fairings_recovery_attempt_type": string, "core_flight_type": string}'
            ),
            "summary": spacex_summary,
            "raw_spec": spacex_gql.read_text(),
            "oracle_slice": (
                vimanam_out(v22, [str(spacex_gql), "--operation", "QUERY Query.launchesPastResult", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"])
                + "\n"
                + vimanam_out(v22, [str(spacex_gql), "--schema", "LaunchFind", "--no-report"])
            ),
            "grade": lambda d: {
                "argument_names": norm_set(d.get("argument_names")) == {"find", "limit", "offset", "order", "sort"},
                "return_type": d.get("return_type") == "LaunchesPastResult",
                "apoapsis_km_type": d.get("apoapsis_km_type") == "Float",
                "fairings_recovery_attempt_type": d.get("fairings_recovery_attempt_type") == "String",
                "core_flight_type": d.get("core_flight_type") == "Int",
            },
        },
        {
            "id": "gql_github_create_issue",
            "format": "GraphQL SDL",
            "spec": "GitHub GraphQL API (1.2 MB SDL)",
            "spec_path": octo_gql,
            "extra_args": [],
            "raw_large": True,
            "question": (
                'For GraphQL mutation `Mutation.createIssue` and its `CreateIssueInput` input object, return JSON:\n'
                '{"argument_name": string, "return_type": string, "required_input_fields": [string], '
                '"issue_template_type": string, "parent_issue_id_type": string}'
            ),
            "summary": octo_summary,
            "raw_spec": octo_gql.read_text(),
            "oracle_slice": (
                vimanam_out(v22, [str(octo_gql), "--operation", "MUTATION Mutation.createIssue", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"])
                + "\n"
                + vimanam_out(v22, [str(octo_gql), "--schema", "CreateIssueInput", "--no-report"])
            ),
            "grade": lambda d: {
                "argument_name": d.get("argument_name") == "input",
                "return_type": d.get("return_type") == "CreateIssuePayload",
                "required_input_fields": norm_set(d.get("required_input_fields")) == {"repositoryId", "title"},
                "issue_template_type": d.get("issue_template_type") == "String",
                "parent_issue_id_type": d.get("parent_issue_id_type") == "ID",
            },
        },
    ]


def run_single_turn_trial(model: str, task: dict, ctx: str) -> dict:
    if ctx:
        prompt = f"<api_context>\n{ctx}\n</api_context>\n\n{task['question']}\n"
    else:
        prompt = f"No API specification context is provided.\n\n{task['question']}\n"
    res = call_claude(model, prompt, ANSWER_SYSTEM_PROMPT)
    if "error" in res:
        n_assert = len(task["grade"]({}))
        return {
            "status": "error",
            "error": res["error"],
            "task_passed": False,
            "assertions_passed": 0,
            "assertions_total": n_assert,
            "api_input_tokens": 0,
            "api_output_tokens": 0,
            "duration_api_ms": 0,
            "cost_usd": 0.0,
        }
    parsed = extract_json(res["result"]) or {}
    checks = task["grade"](parsed)
    passed = sum(1 for v in checks.values() if v)
    total = len(checks)
    return {
        "status": "ok",
        "model": res["model"],
        "api_input_tokens": res["api_input_tokens"],
        "api_output_tokens": res["api_output_tokens"],
        "duration_api_ms": res["duration_api_ms"],
        "cost_usd": res["cost_usd"],
        "task_passed": (passed == total),
        "assertions_passed": passed,
        "assertions_total": total,
        "checks": checks,
    }


def run_routed_e2e_trial(v22: str, model: str, task: dict) -> dict:
    """Real end-to-end workflow with no oracle selection:
    Turn 1: summary -> model selects --operation / --operation-id / --schemas.
    Harness executes vimanam (fixed uniform default --schema-depth 2).
    Turn 2: model sees retrieved slice and either answers directly OR returns
            {"fetch_schemas": [...]} for one follow-up vimanam --schema call.
    Turn 3 (optional): model answers with retrieved slice + follow-up schemas.
    """
    spec_path = str(task["spec_path"])
    extra = list(task["extra_args"])
    total_in = 0
    total_out = 0
    total_ms = 0
    total_cost = 0.0
    total_ctx_bpe = count_bpe(task["summary"])
    turns = 0
    followup_used = False
    routed_cmd_ok = False
    model_seen = model

    # --- Turn 1: Routing from summary map ---
    route_prompt = (
        f"<vimanam_summary>\n{task['summary']}\n</vimanam_summary>\n\n"
        f"User task:\n{task['question']}\n\n"
        "Select the vimanam operation selector to fetch at full detail with schemas (depth 2). "
        "Return JSON with exact fields:\n"
        '{"operation_id": string or null, "operation": string or null, "schemas": [string]}\n'
        "Notes:\n"
        "- For OpenAPI, set `operation_id` to the operation name shown in the summary OR `operation` to `METHOD /path` (e.g. `POST /embeddings`).\n"
        "- For Protobuf/gRPC, set `operation_id` to the full RPC name (e.g. `grpc.health.v1.Health.Watch`).\n"
        "- For GraphQL, set `operation_id` to the root coordinate (e.g. `Mutation.createIssue`) and optionally list any named input schema mentioned in the question under `schemas`.\n"
    )
    r1 = call_claude(model, route_prompt, ROUTE_SYSTEM_PROMPT)
    turns += 1
    if "error" in r1:
        n_assert = len(task["grade"]({}))
        return {"status": "route_error", "error": r1["error"], "task_passed": False, "assertions_passed": 0, "assertions_total": n_assert, "turns": turns}
    model_seen = r1["model"]
    total_in += r1["api_input_tokens"]
    total_out += r1["api_output_tokens"]
    total_ms += r1["duration_api_ms"]
    total_cost += r1["cost_usd"]

    plan = extract_json(r1["result"]) or {}
    op_id = plan.get("operation_id")
    op_str = plan.get("operation")
    schemas = [str(s) for s in (plan.get("schemas") or []) if s][:3]

    # Execute vimanam with the model's chosen selector (uniform depth 2 across all tasks!)
    chunks: list[str] = []
    if op_id:
        rc, out, _ = vimanam_run(v22, [spec_path, *extra, "--operation-id", str(op_id), "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"])
        if rc == 0:
            chunks.append(out)
            routed_cmd_ok = True
    if not routed_cmd_ok and op_str:
        rc, out, _ = vimanam_run(v22, [spec_path, *extra, "--operation", str(op_str), "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"])
        if rc == 0:
            chunks.append(out)
            routed_cmd_ok = True
    for s in schemas:
        rc, out, _ = vimanam_run(v22, [spec_path, *extra, "--schema", s, "--no-report"])
        if rc == 0:
            chunks.append(out)

    retrieved = "\n\n".join(chunks)
    if not retrieved:
        # Routing failed completely; fall back to summary so model still produces a graded turn
        retrieved = task["summary"]
    total_ctx_bpe += count_bpe(retrieved)

    # --- Turn 2: Answer or request follow-up schemas ---
    t2_prompt = (
        f"<api_context>\n{retrieved}\n</api_context>\n\n"
        f"{task['question']}\n\n"
        'If any needed schema was omitted at depth 2 or not yet shown, return ONLY {"fetch_schemas": ["ExactSchemaName", ...]}. '
        "Otherwise return ONLY the final JSON object."
    )
    r2 = call_claude(model, t2_prompt, FOLLOWUP_OR_ANSWER_SYSTEM_PROMPT)
    turns += 1
    if "error" in r2:
        n_assert = len(task["grade"]({}))
        return {"status": "turn2_error", "error": r2["error"], "task_passed": False, "assertions_passed": 0, "assertions_total": n_assert, "turns": turns}
    total_in += r2["api_input_tokens"]
    total_out += r2["api_output_tokens"]
    total_ms += r2["duration_api_ms"]
    total_cost += r2["cost_usd"]

    parsed = extract_json(r2["result"]) or {}
    # Optional Turn 3 if the model explicitly requested follow-up schemas
    if isinstance(parsed.get("fetch_schemas"), list) and parsed["fetch_schemas"]:
        followup_used = True
        extra_chunks: list[str] = []
        for s in [str(x) for x in parsed["fetch_schemas"] if x][:3]:
            rc, out, _ = vimanam_run(v22, [spec_path, *extra, "--schema", s, "--no-report"])
            if rc == 0:
                extra_chunks.append(out)
        followup_text = "\n\n".join(extra_chunks)
        combined = retrieved + ("\n\n" + followup_text if followup_text else "")
        total_ctx_bpe += count_bpe(followup_text)
        t3_prompt = f"<api_context>\n{combined}\n</api_context>\n\n{task['question']}\n"
        r3 = call_claude(model, t3_prompt, ANSWER_SYSTEM_PROMPT)
        turns += 1
        if "error" not in r3:
            total_in += r3["api_input_tokens"]
            total_out += r3["api_output_tokens"]
            total_ms += r3["duration_api_ms"]
            total_cost += r3["cost_usd"]
            parsed = extract_json(r3["result"]) or {}
        else:
            parsed = {}

    checks = task["grade"](parsed)
    passed = sum(1 for v in checks.values() if v)
    total = len(checks)
    return {
        "status": "ok",
        "model": model_seen,
        "turns": turns,
        "routed_cmd_ok": routed_cmd_ok,
        "followup_used": followup_used,
        "route_choice": plan,
        "total_context_bpe_o200k": total_ctx_bpe,
        "api_input_tokens": total_in,
        "api_output_tokens": total_out,
        "duration_api_ms": total_ms,
        "cost_usd": round(total_cost, 6),
        "task_passed": (passed == total),
        "assertions_passed": passed,
        "assertions_total": total,
        "checks": checks,
    }


def aggregate_trials(trials: list[dict], context_bpe: int | None = None, context_chars: int | None = None) -> dict:
    n = len(trials)
    n_pass = sum(1 for t in trials if t.get("task_passed"))
    a_pass = [t["assertions_passed"] for t in trials]
    a_tot = trials[0]["assertions_total"]
    api_ins = [t.get("api_input_tokens", 0) for t in trials]
    durs = [t.get("duration_api_ms", 0) for t in trials]
    costs = [t.get("cost_usd", 0.0) for t in trials]
    out = {
        "status": "ok",
        "trials": n,
        "tasks_passed": n_pass,
        "task_pass_rate": round(n_pass / n, 3),
        "assertions_mean": round(statistics.mean(a_pass), 2),
        "assertions_min": min(a_pass),
        "assertions_max": max(a_pass),
        "assertions_total": a_tot,
        "api_input_tokens_median": round(statistics.median(api_ins)),
        "duration_api_ms_median": round(statistics.median(durs)),
        "cost_usd_mean": round(statistics.mean(costs), 6),
        "trial_details": trials,
    }
    if context_bpe is not None:
        out["context_bpe_o200k"] = context_bpe
    if context_chars is not None:
        out["context_chars"] = context_chars
        out["context_chars_div_4"] = (context_chars + 3) // 4
    if "turns" in trials[0]:
        out["turns_mean"] = round(statistics.mean(t["turns"] for t in trials), 2)
        out["followup_rate"] = round(sum(1 for t in trials if t.get("followup_used")) / n, 3)
        out["route_cmd_ok_rate"] = round(sum(1 for t in trials if t.get("routed_cmd_ok")) / n, 3)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--v22", required=True)
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--model", default="haiku")
    ap.add_argument("--trials", type=int, default=3)
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--output", required=True)
    args = ap.parse_args()

    v22 = str(Path(args.v22).resolve())
    corpus = Path(args.corpus).resolve()
    tasks = build_tasks(v22, corpus)
    conditions = ["no_spec", "summary", "routed_e2e", "oracle_slice", "raw_spec"]

    # Build flat list of trial jobs so we can execute with bounded parallelism
    jobs = []
    for t_idx, task in enumerate(tasks):
        for cond in conditions:
            if cond == "raw_spec" and task["raw_spec"] is None:
                continue
            n_trials = 1 if (cond == "raw_spec" and task["raw_large"]) else args.trials
            for trial_idx in range(n_trials):
                jobs.append((t_idx, cond, trial_idx))

    print(f"Running {len(jobs)} total trials across {len(tasks)} tasks and 5 conditions (workers={args.workers})...", flush=True)

    results_map: dict[tuple[int, str], list[dict]] = {}

    def exec_job(job):
        t_idx, cond, trial_idx = job
        task = tasks[t_idx]
        if cond == "no_spec":
            r = run_single_turn_trial(args.model, task, "")
        elif cond == "summary":
            r = run_single_turn_trial(args.model, task, task["summary"])
        elif cond == "oracle_slice":
            r = run_single_turn_trial(args.model, task, task["oracle_slice"])
        elif cond == "raw_spec":
            r = run_single_turn_trial(args.model, task, task["raw_spec"])
        elif cond == "routed_e2e":
            r = run_routed_e2e_trial(v22, args.model, task)
        else:
            raise ValueError(cond)
        return (t_idx, cond, trial_idx, r)

    done = 0
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
        for t_idx, cond, trial_idx, r in pool.map(exec_job, jobs):
            results_map.setdefault((t_idx, cond), []).append(r)
            done += 1
            if done % 10 == 0 or done == len(jobs):
                print(f"  completed {done}/{len(jobs)} trials", flush=True)

    model_seen = args.model
    task_records = []
    tokenizer_calibration = []

    for t_idx, task in enumerate(tasks):
        rec = {"id": task["id"], "format": task["format"], "spec": task["spec"], "conditions": {}}
        no_spec_trials = results_map[(t_idx, "no_spec")]
        no_spec_base_in = round(statistics.median(t.get("api_input_tokens", 0) for t in no_spec_trials))
        for t in no_spec_trials:
            if t.get("model"):
                model_seen = t["model"]

        for cond in conditions:
            if cond == "raw_spec" and task["raw_spec"] is None:
                raw_text = task["raw_file_for_tokens"].read_text()
                bpe = count_bpe(raw_text)
                n_assert = len(task["grade"]({}))
                rec["conditions"][cond] = {
                    "status": "not_attempted",
                    "trials": 0,
                    "note": f"Not attempted: raw 7.9 MB JSON ({bpe:,} o200k_base BPE tokens; >1,000,000-token context window)",
                    "context_bpe_o200k": bpe,
                    "context_chars": len(raw_text),
                    "context_chars_div_4": (len(raw_text) + 3) // 4,
                    "tasks_passed": 0,
                    "task_pass_rate": 0.0,
                    "assertions_mean": 0.0,
                    "assertions_total": n_assert,
                }
                continue

            trials = results_map[(t_idx, cond)]
            if cond == "no_spec":
                agg = aggregate_trials(trials, context_bpe=0, context_chars=0)
            elif cond == "summary":
                agg = aggregate_trials(trials, context_bpe=count_bpe(task["summary"]), context_chars=len(task["summary"]))
            elif cond == "oracle_slice":
                agg = aggregate_trials(trials, context_bpe=count_bpe(task["oracle_slice"]), context_chars=len(task["oracle_slice"]))
            elif cond == "raw_spec":
                agg = aggregate_trials(trials, context_bpe=count_bpe(task["raw_spec"]), context_chars=len(task["raw_spec"]))
            elif cond == "routed_e2e":
                agg = aggregate_trials(trials)
                agg["total_context_bpe_o200k_median"] = round(statistics.median(t.get("total_context_bpe_o200k", 0) for t in trials))

            # Record Claude API context tokens (net of no_spec system/question overhead)
            if cond in {"summary", "oracle_slice", "raw_spec"} and agg.get("context_bpe_o200k", 0) > 0:
                net_claude = max(1, agg["api_input_tokens_median"] - no_spec_base_in)
                agg["claude_context_tokens_est"] = net_claude
                agg["claude_vs_o200k_ratio"] = round(net_claude / agg["context_bpe_o200k"], 2)
                agg["chars_per_claude_token"] = round(agg["context_chars"] / net_claude, 2)
                tokenizer_calibration.append({
                    "task": task["id"],
                    "condition": cond,
                    "format": task["format"] if cond == "raw_spec" else "Markdown",
                    "chars": agg["context_chars"],
                    "chars_div_4": agg["context_chars_div_4"],
                    "o200k_bpe": agg["context_bpe_o200k"],
                    "claude_tokens": net_claude,
                    "chars_per_o200k": round(agg["context_chars"] / agg["context_bpe_o200k"], 2),
                    "chars_per_claude": agg["chars_per_claude_token"],
                    "claude_over_o200k": agg["claude_vs_o200k_ratio"],
                })

            rec["conditions"][cond] = agg

        task_records.append(rec)
        print(
            f"  {task['id']:25s} | no_spec={rec['conditions']['no_spec']['assertions_mean']}/{rec['conditions']['no_spec']['assertions_total']} "
            f"| summary={rec['conditions']['summary']['assertions_mean']}/{rec['conditions']['summary']['assertions_total']} "
            f"| routed_e2e={rec['conditions']['routed_e2e']['assertions_mean']}/{rec['conditions']['routed_e2e']['assertions_total']} "
            f"(pass {rec['conditions']['routed_e2e']['tasks_passed']}/{rec['conditions']['routed_e2e']['trials']}, ${rec['conditions']['routed_e2e']['cost_usd_mean']:.4f}) "
            f"| oracle={rec['conditions']['oracle_slice']['assertions_mean']}/{rec['conditions']['oracle_slice']['assertions_total']} "
            f"| raw={rec['conditions']['raw_spec'].get('status')}:{rec['conditions']['raw_spec']['assertions_mean']}/{rec['conditions']['raw_spec']['assertions_total']}",
            flush=True,
        )

    def summarize(subset_ids: set[str]) -> dict:
        out = {}
        for cond in conditions:
            recs = [r["conditions"][cond] for r in task_records if r["id"] in subset_ids]
            attempted = [c for c in recs if c["status"] == "ok"]
            not_att = [c for c in recs if c["status"] == "not_attempted"]
            out[cond] = {
                "tasks_total": len(recs),
                "tasks_attempted": len(attempted),
                "tasks_not_attempted": len(not_att),
                "mean_task_pass_rate": round(sum(c["task_pass_rate"] for c in recs) / len(recs), 3),
                "mean_assertions_passed": round(sum(c["assertions_mean"] for c in recs), 2),
                "assertions_total": sum(c["assertions_total"] for c in recs),
                "mean_assertion_rate": round(sum(c["assertions_mean"] for c in recs) / sum(c["assertions_total"] for c in recs), 3),
                "sum_api_input_tokens_median": sum(c.get("api_input_tokens_median", 0) for c in attempted),
                "sum_cost_usd_mean": round(sum(c.get("cost_usd_mean", 0.0) for c in attempted), 6),
                "sum_duration_api_ms_median": sum(c.get("duration_api_ms_median", 0) for c in attempted),
            }
            if cond == "routed_e2e" and attempted:
                out[cond]["mean_turns"] = round(statistics.mean(c["turns_mean"] for c in attempted), 2)
                out[cond]["mean_followup_rate"] = round(statistics.mean(c["followup_rate"] for c in attempted), 3)
                out[cond]["mean_route_cmd_ok_rate"] = round(statistics.mean(c["route_cmd_ok_rate"] for c in attempted), 3)
        return out

    all_ids = {t["id"] for t in tasks}
    fitting_ids = {t["id"] for t in tasks if t["raw_spec"] is not None}
    stripe_ids = all_ids - fitting_ids

    # Summarize Claude vs o200k calibration by format
    cal_summary = {}
    for fmt in sorted({c["format"] for c in tokenizer_calibration}):
        rows = [c for c in tokenizer_calibration if c["format"] == fmt and c["chars"] >= 1000]
        if rows:
            cal_summary[fmt] = {
                "samples": len(rows),
                "median_chars_per_o200k": round(statistics.median(r["chars_per_o200k"] for r in rows), 2),
                "median_chars_per_claude": round(statistics.median(r["chars_per_claude"] for r in rows), 2),
                "median_claude_over_o200k": round(statistics.median(r["claude_over_o200k"] for r in rows), 2),
            }

    report = {
        "meta": {
            "model": model_seen,
            "vimanam_version": subprocess.check_output([v22, "--version"], text=True).strip(),
            "trials_per_cell": args.trials,
            "raw_large_spec_trials": 1,
            "task_count": len(tasks),
            "fitting_task_count": len(fitting_ids),
            "stripe_not_attempted_count": len(stripe_ids),
            "grading": "deterministic programmatic JSON assertions (no LLM judge)",
            "isolation": "claude -p --tools '' --strict-mcp-config --setting-sources ''",
        },
        "summary_8_fitting_tasks_apples_to_apples": summarize(fitting_ids),
        "summary_2_stripe_7_9mb_tasks": summarize(stripe_ids),
        "summary_all_10_tasks": summarize(all_ids),
        "claude_vs_o200k_calibration_summary": cal_summary,
        "claude_vs_o200k_calibration_rows": tokenizer_calibration,
        "tasks": task_records,
    }
    out_path = Path(args.output)
    out_path.write_text(json.dumps(report, indent=2) + "\n")
    print(f"\nWrote {out_path}", flush=True)


if __name__ == "__main__":
    main()
