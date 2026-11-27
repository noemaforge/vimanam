#!/usr/bin/env python3
"""End-to-end routed, fixed-slice and oracle-slice task-accuracy evaluation on one LLM
(claude-haiku-5-5, zero tools, isolated system prompt, repeated serial trials).

Conditions:
  1. no_spec      — memorization control (question only)
  2. summary      — vimanam --detail summary --no-report
  3. routed_e2e   — real workflow: summary -> model selects vimanam operation/schema
                    flags -> harness runs vimanam at uniform --schema-depth 2 (on both
                    --operation/--operation-id and --schema calls) -> optional 1
                    follow-up {"fetch_schemas": [...]} turn at --schema-depth 2 -> answer
  4. fixed_slice  — naive single-turn slice (--operation ... --detail full
                    --include-schemas --schema-depth 2 --no-report, no --schema)
  5. oracle_slice — single-turn upper bound for retrieval: the fixed slice plus --schema
                    (depth 2) for exactly the named types holding the answers. routed_e2e vs
                    oracle_slice isolates routing; oracle_slice vs raw_spec isolates slice
                    readability.
  6. raw_spec     — raw contract bytes (1 trial on >100k-token specs; the 7.9 MB Stripe JSON
                    returns "Prompt is too long")

Cache protocol: every graded trial prepends a fresh nonce to each user prompt, so all graded
trials are cold-cache calls. Right after trial 1 of each cell, its exact prompts are re-sent
with the same nonce (warm_replay; cost/tokens only, not graded). Compare cold with cold and
warm with warm; mixing the two inflates the cost ratio. Per-call cache token breakdowns are
recorded.

The three synthetic Protobuf tasks (scripts/eval-fixtures/protos/) rule out memorization; two
put requested sub-messages beyond depth 2. pb_otel_trace_export is a real public spec whose
answers are beyond depth 2, so the follow-up path is also measured on non-synthetic input.

Grading is deterministic (strict JSON assertions, no LLM judge). Also records exact target
routing (route_target_matched), follow-up rate, answer_coverage (fraction of answer markers
present in each context), and Claude API tokens alongside o200k_base BPE and chars/4.
"""

from __future__ import annotations

import argparse
import hashlib
import concurrent.futures
import json
import re
import statistics
import subprocess
import sys
import time
import uuid
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
    'you MUST request up to 3 exact schema names from the text by returning ONLY '
    '{"fetch_schemas": ["ExactSchemaName", ...]}. '
    "Do not guess omitted fields. Otherwise return ONLY the final JSON answer matching the "
    "exact requested schema, with no code fences."
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


MAX_SCHEMAS = 3  # schema selectors per routed turn; stated to the model in both prompts


def norm_set(xs) -> set[str]:
    if not isinstance(xs, list):
        return set()
    return {str(x).strip() for x in xs}


def norm_fields(xs) -> set[str]:
    """Field-name set that tolerates the synthetic body-wrapper prefix.

    Vimanam renders request-body fields as `request.<name>`; an answer that
    copies that path (`request.input_file_id`) names the same field.
    """
    out = set()
    for x in norm_set(xs):
        for prefix in ("request.", "body.", "requestBody."):
            if x.startswith(prefix):
                x = x[len(prefix):]
                break
        out.add(x)
    return out


def call_claude(model: str, prompt: str, system_prompt: str, timeout: int = 180, nonce: str | None = None) -> dict:
    """One isolated `claude -p` call.

    `nonce` is prepended to the user prompt. A fresh nonce per measured trial guarantees a
    cold prompt cache for everything after the (shared, tiny) system prompt, so repeated
    trials of one cell cannot read each other's cache. Re-sending the same prompt with the
    same nonce is the warm-cache replay.
    """
    if nonce:
        prompt = f"[eval-run {nonce}]\n{prompt}"
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
    data = None
    try:
        data = json.loads(p.stdout)
    except Exception:
        pass
    if p.returncode != 0 or (isinstance(data, dict) and data.get("is_error")):
        err_msg = (
            str(data.get("result") or p.stderr or p.stdout).strip()[:400]
            if isinstance(data, dict)
            else (p.stderr or p.stdout).strip()[:400]
        )
        return {
            "error": err_msg,
            "exit_code": p.returncode,
            "terminal_reason": data.get("terminal_reason") if isinstance(data, dict) else None,
            "duration_ms": data.get("duration_ms") if isinstance(data, dict) else None,
            "wall_ms": wall_ms,
        }
    if not isinstance(data, dict):
        return {"error": f"non-json cli output: {p.stdout[:300]}", "exit_code": p.returncode, "wall_ms": wall_ms}
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
        "uncached_input_tokens": usage.get("input_tokens", 0),
        "cache_creation_input_tokens": usage.get("cache_creation_input_tokens", 0),
        "cache_read_input_tokens": usage.get("cache_read_input_tokens", 0),
        "api_output_tokens": usage.get("output_tokens", 0),
    }


CACHE_FIELDS = ("uncached_input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens")


def new_nonce() -> str:
    return uuid.uuid4().hex[:12]


def answer_coverage(ctx: str, markers: list[str]) -> float:
    """Fraction of a task's answer markers (distinctive answer strings) present in a context.
    Separates retrieval coverage (was the answer shown at all?) from reading accuracy."""
    if not markers:
        return 1.0
    return round(sum(1 for m in markers if m in ctx) / len(markers), 3)


def vimanam_run(vbin: str, args: list[str]) -> tuple[int, str, str]:
    p = subprocess.run([vbin, *args], capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def vimanam_out(vbin: str, args: list[str]) -> str:
    rc, out, err = vimanam_run(vbin, args)
    if rc != 0:
        raise RuntimeError(f"vimanam failed ({rc}): {err.strip()}")
    return out


def build_tasks(vbin: str, corpus: Path) -> list[dict]:
    openai_yaml = corpus / "openai.yaml"
    stripe_json = corpus / "stripe-openapi/openapi/spec3.json"
    octo_gql = corpus / "octokit-graphql/schema.graphql"
    spacex_gql = REPO / "tests/fixtures/graphql/spacex-land-64a7fa9.graphql"
    otel_root = corpus / "otel-proto"
    otel_proto = otel_root / "opentelemetry/proto/collector/trace/v1/trace_service.proto"

    proto_root = REPO / "scripts/eval-fixtures/protos"
    ledger_proto = proto_root / "ledger/v1/ledger.proto"
    fleet_proto = proto_root / "fleet/v2/dispatch.proto"
    vault_proto = proto_root / "vault/v1/audit.proto"

    def concat_protos(paths: list[Path]) -> str:
        return "\n\n".join(f"// === {p.name} ===\n{p.read_text()}" for p in paths)

    ledger_raw = concat_protos([
        ledger_proto,
        proto_root / "ledger/v1/common.proto",
    ])
    fleet_raw = concat_protos([
        fleet_proto,
        proto_root / "fleet/v2/routing.proto",
        proto_root / "fleet/v2/telemetry.proto",
        proto_root / "fleet/v2/common.proto",
    ])
    vault_raw = concat_protos([
        vault_proto,
        proto_root / "vault/v1/policy.proto",
        proto_root / "vault/v1/record.proto",
        proto_root / "vault/v1/common.proto",
    ])

    otel_raw = concat_protos([
        otel_proto,
        otel_root / "opentelemetry/proto/trace/v1/trace.proto",
        otel_root / "opentelemetry/proto/common/v1/common.proto",
        otel_root / "opentelemetry/proto/resource/v1/resource.proto",
    ])

    stripe_raw = stripe_json.read_text()
    openai_summary = vimanam_out(vbin, [str(openai_yaml), "--detail", "summary", "--no-report"])
    stripe_summary = vimanam_out(vbin, [str(stripe_json), "--detail", "summary", "--no-report"])
    ledger_summary = vimanam_out(vbin, [str(ledger_proto), "-I", str(proto_root), "--detail", "summary", "--no-report"])
    fleet_summary = vimanam_out(vbin, [str(fleet_proto), "-I", str(proto_root), "--detail", "summary", "--no-report"])
    vault_summary = vimanam_out(vbin, [str(vault_proto), "-I", str(proto_root), "--detail", "summary", "--no-report"])
    spacex_summary = vimanam_out(vbin, [str(spacex_gql), "--detail", "summary", "--no-report"])
    octo_summary = vimanam_out(vbin, [str(octo_gql), "--detail", "summary", "--no-report"])
    otel_summary = vimanam_out(vbin, [str(otel_proto), "-I", str(otel_root), "--detail", "summary", "--no-report"])

    tasks = [
        {
            "id": "oa_embeddings",
            "format": "OpenAPI YAML",
            "spec": "OpenAI OpenAPI 3.0 (1.3 MB YAML)",
            "spec_path": openai_yaml,
            "extra_args": [],
            "raw_large": True,
            "expected_operation_id": "createEmbedding",
            "expected_operation": "POST /embeddings",
            "question": (
                'For the operation that creates an embedding vector (`POST /embeddings`), return JSON:\n'
                '{"operation_id": string, "required_fields": [string], "encoding_format_enum": [string], '
                '"dimensions_type": string, "response_schema": string}'
            ),
            "summary": openai_summary,
            "raw_spec": openai_yaml.read_text(),
            "fixed_slice": vimanam_out(vbin, [str(openai_yaml), "--operation", "POST /embeddings", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            "grade": lambda d: {
                "operation_id": d.get("operation_id") == "createEmbedding",
                "required_fields": norm_fields(d.get("required_fields")) == {"input", "model"},
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
            "expected_operation_id": "createBatch",
            "expected_operation": "POST /batches",
            "question": (
                'For the operation that creates a batch (`POST /batches`), return JSON:\n'
                '{"operation_id": string, "required_fields": [string], "completion_window_value": string, '
                '"supported_endpoints": [string], "response_schema": string}'
            ),
            "summary": openai_summary,
            "raw_spec": openai_yaml.read_text(),
            "fixed_slice": vimanam_out(vbin, [str(openai_yaml), "--operation", "POST /batches", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            "grade": lambda d: {
                "operation_id": d.get("operation_id") == "createBatch",
                "required_fields": norm_fields(d.get("required_fields")) == {"input_file_id", "endpoint", "completion_window"},
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
            "expected_operation_id": "createFile",
            "expected_operation": "POST /files",
            "question": (
                'For the operation that uploads a file (`POST /files`), return JSON:\n'
                '{"operation_id": string, "required_fields": [string], "purpose_enum": [string], '
                '"response_schema": string}'
            ),
            "summary": openai_summary,
            "raw_spec": openai_yaml.read_text(),
            "fixed_slice": vimanam_out(vbin, [str(openai_yaml), "--operation", "POST /files", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            "grade": lambda d: {
                "operation_id": d.get("operation_id") == "createFile",
                "required_fields": norm_fields(d.get("required_fields")) == {"file", "purpose"},
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
            "stripe_7_9mb": True,
            "expected_operation_id": "PostCustomers",
            "expected_operation": "POST /v1/customers",
            "question": (
                'For creating a customer (`POST /v1/customers`), return JSON:\n'
                '{"operation_id": string, "tax_exempt_enum": [string], "invoice_prefix_max_len": integer, '
                '"response_schema": string}'
            ),
            "summary": stripe_summary,
            "raw_spec": stripe_raw,
            "fixed_slice": vimanam_out(vbin, [str(stripe_json), "--operation", "POST /v1/customers", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
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
            "stripe_7_9mb": True,
            "expected_operation_id": "PostPaymentIntents",
            "expected_operation": "POST /v1/payment_intents",
            "question": (
                'For creating a PaymentIntent (`POST /v1/payment_intents`), return JSON:\n'
                '{"operation_id": string, "required_fields": [string], "amount_max_digits": integer, '
                '"currency_type": string}'
            ),
            "summary": stripe_summary,
            "raw_spec": stripe_raw,
            "fixed_slice": vimanam_out(vbin, [str(stripe_json), "--operation", "POST /v1/payment_intents", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            "grade": lambda d: {
                "operation_id": d.get("operation_id") == "PostPaymentIntents",
                "required_fields": norm_fields(d.get("required_fields")) == {"amount", "currency"},
                "amount_max_digits": d.get("amount_max_digits") == 8,
                "currency_type": "string" in str(d.get("currency_type", "")).lower(),
            },
        },
        {
            "id": "pb_ledger_stream",
            "format": "Protobuf / gRPC",
            "spec": "Synthetic ledger/v1 (2 .proto files)",
            "spec_path": ledger_proto,
            "extra_args": ["-I", str(proto_root)],
            "raw_large": False,
            "expected_operation_id": "ledger.v1.SettlementStream.WatchSettlements",
            "expected_operation": "RPC ledger.v1.SettlementStream.WatchSettlements",
            "question": (
                'For RPC `ledger.v1.SettlementStream.WatchSettlements`, return JSON:\n'
                '{"streaming_mode": string, "request_message": string, "response_message": string, '
                '"clearing_state_field_number": integer, "clearing_state_json_name": string, '
                '"net_delta_type": string}'
            ),
            "summary": ledger_summary,
            "raw_spec": ledger_raw,
            "fixed_slice": vimanam_out(vbin, [str(ledger_proto), "-I", str(proto_root), "--operation", "RPC ledger.v1.SettlementStream.WatchSettlements", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            "grade": lambda d: {
                "streaming_mode": "server" in str(d.get("streaming_mode", "")).lower(),
                "request_message": str(d.get("request_message", "")).endswith("WatchSettlementsRequest"),
                "response_message": str(d.get("response_message", "")).endswith("SettlementFrame"),
                "clearing_state_field_number": d.get("clearing_state_field_number") == 4,
                "clearing_state_json_name": d.get("clearing_state_json_name") == "clearingState",
                "net_delta_type": d.get("net_delta_type") == "sint64",
            },
        },
        {
            "id": "pb_fleet_dispatch",
            "format": "Protobuf / gRPC",
            "spec": "Synthetic fleet/v2 (4 .proto files, depth-3 follow-up)",
            "spec_path": fleet_proto,
            "extra_args": ["-I", str(proto_root)],
            "raw_large": False,
            "expected_operation_id": "fleet.v2.DispatchService.AssignBatch",
            "expected_operation": "RPC fleet.v2.DispatchService.AssignBatch",
            "question": (
                'For RPC `fleet.v2.DispatchService.AssignBatch` and its nested response `outcome` message, return JSON:\n'
                '{"streaming_mode": string, "request_field_1_name": string, "outcome_scheduled_stops_type": string, '
                '"outcome_rejected_field_number": integer, "outcome_rejected_json_name": string, '
                '"severity_enum_values": [string]}'
            ),
            "summary": fleet_summary,
            "raw_spec": fleet_raw,
            "fixed_slice": vimanam_out(vbin, [str(fleet_proto), "-I", str(proto_root), "--operation", "RPC fleet.v2.DispatchService.AssignBatch", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            "grade": lambda d: {
                "streaming_mode": "unary" in str(d.get("streaming_mode", "")).lower(),
                "request_field_1_name": d.get("request_field_1_name") == "depot_region",
                "outcome_scheduled_stops_type": d.get("outcome_scheduled_stops_type") == "uint32",
                "outcome_rejected_field_number": d.get("outcome_rejected_field_number") == 2,
                "outcome_rejected_json_name": d.get("outcome_rejected_json_name") == "rejectedWaypoints",
                "severity_enum_values": norm_set(d.get("severity_enum_values")) == {
                    "CONSTRAINT_SEVERITY_UNSPECIFIED",
                    "CONSTRAINT_SEVERITY_ADVISORY",
                    "CONSTRAINT_SEVERITY_HARD_BLOCK",
                },
            },
        },
        {
            "id": "pb_vault_audit",
            "format": "Protobuf / gRPC",
            "spec": "Synthetic vault/v1 (4 .proto files, depth-3 follow-up)",
            "spec_path": vault_proto,
            "extra_args": ["-I", str(proto_root)],
            "raw_large": False,
            "expected_operation_id": "vault.v1.AuditExportService.ExportBundle",
            "expected_operation": "RPC vault.v1.AuditExportService.ExportBundle",
            "question": (
                'For RPC `vault.v1.AuditExportService.ExportBundle`, its nested request `export_policy` message, '
                'and its nested response `summary` message, return JSON:\n'
                '{"policy_slug_json_name": string, "policy_signed_manifest_field_number": integer, '
                '"summary_exported_count_type": string, "summary_partial_failure_field_number": integer, '
                '"summary_partial_failure_json_name": string}'
            ),
            "summary": vault_summary,
            "raw_spec": vault_raw,
            "fixed_slice": vimanam_out(vbin, [str(vault_proto), "-I", str(proto_root), "--operation", "RPC vault.v1.AuditExportService.ExportBundle", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            "grade": lambda d: {
                "policy_slug_json_name": d.get("policy_slug_json_name") == "policySlug",
                "policy_signed_manifest_field_number": d.get("policy_signed_manifest_field_number") == 4,
                "summary_exported_count_type": d.get("summary_exported_count_type") == "int64",
                "summary_partial_failure_field_number": d.get("summary_partial_failure_field_number") == 3,
                "summary_partial_failure_json_name": d.get("summary_partial_failure_json_name") == "partialFailure",
            },
        },
        {
            "id": "gql_spacex_launches",
            "format": "GraphQL SDL",
            "spec": "SpaceX Land (12 KB SDL)",
            "spec_path": spacex_gql,
            "extra_args": [],
            "raw_large": False,
            "expected_operation_id": "Query.launchesPastResult",
            "expected_operation": "QUERY Query.launchesPastResult",
            "question": (
                'For GraphQL field `Query.launchesPastResult` and its `LaunchFind` input object, return JSON:\n'
                '{"argument_names": [string], "return_type": string, "apoapsis_km_type": string, '
                '"fairings_recovery_attempt_type": string, "core_flight_type": string}'
            ),
            "summary": spacex_summary,
            "raw_spec": spacex_gql.read_text(),
            "fixed_slice": vimanam_out(vbin, [str(spacex_gql), "--operation", "QUERY Query.launchesPastResult", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
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
            "expected_operation_id": "Mutation.createIssue",
            "expected_operation": "MUTATION Mutation.createIssue",
            "question": (
                'For GraphQL mutation `Mutation.createIssue` and its `CreateIssueInput` input object, return JSON:\n'
                '{"argument_name": string, "return_type": string, "required_input_fields": [string], '
                '"issue_template_type": string, "parent_issue_id_type": string}'
            ),
            "summary": octo_summary,
            "raw_spec": octo_gql.read_text(),
            "fixed_slice": vimanam_out(vbin, [str(octo_gql), "--operation", "MUTATION Mutation.createIssue", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            "grade": lambda d: {
                "argument_name": d.get("argument_name") == "input",
                "return_type": d.get("return_type") == "CreateIssuePayload",
                "required_input_fields": norm_set(d.get("required_input_fields")) == {"repositoryId", "title"},
                "issue_template_type": d.get("issue_template_type") == "String",
                "parent_issue_id_type": d.get("parent_issue_id_type") == "ID",
            },
        },
        {
            # Real public spec (not synthetic) whose answers sit beyond --schema-depth 2:
            # Span is 3 message hops below the request, so a single depth-2 slice cannot
            # answer it and the routed workflow must request more schemas (in Turn 1 or via
            # the Turn-3 follow-up). OTel is public, so no_spec measures memorization here.
            "id": "pb_otel_trace_export",
            "format": "Protobuf / gRPC",
            "spec": "OpenTelemetry TraceService (4 real .proto files, beyond depth 2)",
            "spec_path": otel_proto,
            "extra_args": ["-I", str(otel_root)],
            "raw_large": False,
            "real_spec_deep": True,
            "expected_operation_id": "opentelemetry.proto.collector.trace.v1.TraceService.Export",
            "expected_operation": "RPC opentelemetry.proto.collector.trace.v1.TraceService.Export",
            "question": (
                'For RPC `opentelemetry.proto.collector.trace.v1.TraceService.Export`, the spans it carries, '
                'and its partial-success response, return JSON:\n'
                '{"partial_success_rejected_spans_type": string, "scope_spans_schema_url_field_number": integer, '
                '"span_flags_type": string, "span_flags_field_number": integer, '
                '"span_kind_server_number": integer, "status_code_values": [string]}'
            ),
            "summary": otel_summary,
            "raw_spec": otel_raw,
            "fixed_slice": vimanam_out(vbin, [str(otel_proto), "-I", str(otel_root), "--operation", "RPC opentelemetry.proto.collector.trace.v1.TraceService.Export", "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"]),
            "grade": lambda d: {
                "partial_success_rejected_spans_type": d.get("partial_success_rejected_spans_type") == "int64",
                "scope_spans_schema_url_field_number": d.get("scope_spans_schema_url_field_number") == 3,
                "span_flags_type": d.get("span_flags_type") == "fixed32",
                "span_flags_field_number": d.get("span_flags_field_number") == 16,
                "span_kind_server_number": d.get("span_kind_server_number") == 2,
                "status_code_values": norm_set(d.get("status_code_values")) == {
                    "STATUS_CODE_UNSET",
                    "STATUS_CODE_OK",
                    "STATUS_CODE_ERROR",
                },
            },
        },
    ]

    # oracle_slice: operation at --schema-depth 2 plus `--schema` (depth 2) for exactly the
    # named types that hold the answers. It is the single-turn upper bound for "the right slice
    # was retrieved": routed_e2e vs oracle_slice isolates routing quality, and oracle_slice vs
    # raw_spec isolates slice readability. answer_markers are distinctive answer strings; the
    # build fails if the oracle context does not contain all of them.
    oracle = {
        "oa_embeddings": (["CreateEmbeddingRequest", "CreateEmbeddingResponse"], ["createEmbedding", "base64", "dimensions", "CreateEmbeddingResponse"]),
        "oa_batches": ([], ["createBatch", "input_file_id", "24h", "/v1/responses", "/v1/chat/completions", "/v1/embeddings", "/v1/completions"]),
        "oa_files": (["OpenAIFile"], ["createFile", "user_data", "evals", "fine-tune", "vision", "OpenAIFile"]),
        "oa_stripe_customers": ([], ["PostCustomers", "reverse", "exempt", "invoice_prefix"]),
        "oa_stripe_payment_intents": ([], ["PostPaymentIntents", "amount", "currency"]),
        "pb_ledger_stream": ([], ["WatchSettlementsRequest", "SettlementFrame", "clearingState", "sint64"]),
        "pb_fleet_dispatch": (["fleet.v2.AssignmentOutcome"], ["depot_region", "scheduled_stops", "rejectedWaypoints", "CONSTRAINT_SEVERITY_HARD_BLOCK"]),
        "pb_vault_audit": (["vault.v1.ExportPolicy", "vault.v1.BundleSummary"], ["policySlug", "require_signed_manifest", "exported_record_count", "partialFailure"]),
        "gql_spacex_launches": (["LaunchFind"], ["apoapsis_km", "recovery_attempt", "LaunchesPastResult"]),
        "gql_github_create_issue": (["CreateIssueInput"], ["CreateIssuePayload", "repositoryId", "issueTemplate", "parentIssueId"]),
        "pb_otel_trace_export": (
            [
                "opentelemetry.proto.collector.trace.v1.ExportTracePartialSuccess",
                "opentelemetry.proto.trace.v1.ScopeSpans",
                "opentelemetry.proto.trace.v1.Span",
                "opentelemetry.proto.trace.v1.Status",
            ],
            ["rejected_spans", "SPAN_KIND_SERVER", "STATUS_CODE_ERROR", "fixed32", "schema_url"],
        ),
    }
    for t in tasks:
        schemas, markers = oracle[t["id"]]
        spec = str(t["spec_path"])
        chunks = [vimanam_out(vbin, [spec, *t["extra_args"], "--operation", t["expected_operation"], "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"])]
        for name in schemas:
            chunks.append(vimanam_out(vbin, [spec, *t["extra_args"], "--schema", name, "--schema-depth", "2", "--no-report"]))
        t["oracle_schemas"] = schemas
        t["oracle_slice"] = "\n\n".join(chunks)
        t["answer_markers"] = markers
        missing = [m for m in markers if m not in t["oracle_slice"]]
        if missing:
            raise RuntimeError(f"oracle_slice for {t['id']} is missing answer markers {missing}")
        if t.get("stripe_7_9mb"):
            t["size_class"] = "too_large"
        elif t["raw_large"]:
            t["size_class"] = "large"
        else:
            t["size_class"] = "small"
    return tasks


def run_single_turn_trial(model: str, task: dict, ctx: str, nonce: str) -> dict:
    if ctx:
        prompt = f"<api_context>\n{ctx}\n</api_context>\n\n{task['question']}\n"
    else:
        prompt = f"No API specification context is provided.\n\n{task['question']}\n"
    res = call_claude(model, prompt, ANSWER_SYSTEM_PROMPT, nonce=nonce)
    if "error" in res:
        n_assert = len(task["grade"]({}))
        err_low = str(res["error"]).lower()
        is_too_long = ("prompt is too long" in err_low) or (res.get("terminal_reason") == "blocking_limit")
        return {
            "status": "prompt_too_long" if is_too_long else "error",
            "error": res["error"],
            "exit_code": res.get("exit_code"),
            "terminal_reason": res.get("terminal_reason"),
            "duration_ms": res.get("duration_ms"),
            "wall_ms": res.get("wall_ms"),
            "task_passed": False,
            "assertions_passed": 0,
            "assertions_total": n_assert,
            "api_input_tokens": 0,
            "api_output_tokens": 0,
            "duration_api_ms": 0,
            "cost_usd": 0.0,
            "nonce": nonce,
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
        "wall_ms": res["wall_ms"],
        "cost_usd": res["cost_usd"],
        **{k: res[k] for k in CACHE_FIELDS},
        "task_passed": (passed == total),
        "assertions_passed": passed,
        "assertions_total": total,
        "checks": checks,
        "answer": parsed,
        "nonce": nonce,
        "_replay": [(prompt, ANSWER_SYSTEM_PROMPT)],
    }


def replay_warm(model: str, calls: list[tuple[str, str]], nonce: str) -> dict:
    """Re-send a finished trial's exact prompts (same nonce) right after it: the warm-cache
    cost of asking the same thing again within the cache TTL. Cost/token measurement only;
    the answers are not re-graded."""
    out = {"status": "ok", "calls": len(calls), "api_input_tokens": 0, "api_output_tokens": 0,
           "duration_api_ms": 0, "wall_ms": 0, "cost_usd": 0.0, **{k: 0 for k in CACHE_FIELDS}}
    for prompt, system in calls:
        r = call_claude(model, prompt, system, nonce=nonce)
        if "error" in r:
            return {"status": "error", "error": r["error"]}
        for k in ("api_input_tokens", "api_output_tokens", "duration_api_ms", "wall_ms", *CACHE_FIELDS):
            out[k] += r[k]
        out["cost_usd"] += r["cost_usd"]
    out["cost_usd"] = round(out["cost_usd"], 6)
    return out


def matches_expected_target(plan: dict, task: dict) -> bool:
    exp_id = (task.get("expected_operation_id") or "").strip()
    exp_op = (task.get("expected_operation") or "").strip()
    got_id = str(plan.get("operation_id") or "").strip()
    got_op = str(plan.get("operation") or "").strip()
    return (bool(exp_id) and got_id == exp_id) or (bool(exp_op) and got_op == exp_op)


def run_routed_e2e_trial(vbin: str, model: str, task: dict, nonce: str) -> dict:
    """Real end-to-end workflow with uniform --schema-depth 2 on EVERY vimanam call:
    Turn 1: summary -> model selects --operation / --operation-id / optional --schema names.
    Harness executes vimanam with uniform --schema-depth 2 on both operation and schema calls.
    Turn 2: model sees retrieved slice and either answers directly OR returns
            {"fetch_schemas": [...]} for one follow-up vimanam --schema --schema-depth 2 call.
    Turn 3 (optional): model answers with retrieved slice + follow-up schemas.
    """
    spec_path = str(task["spec_path"])
    extra = list(task["extra_args"])
    total_in = 0
    total_out = 0
    total_ms = 0
    total_wall_ms = 0
    total_cost = 0.0
    total_ctx_bpe = count_bpe(task["summary"])
    turns = 0
    followup_used = False
    followup_schemas: list[str] = []
    routed_cmd_ok = False
    model_seen = model
    cache = {k: 0 for k in CACHE_FIELDS}
    replay: list[tuple[str, str]] = []

    def account(r: dict) -> None:
        nonlocal total_in, total_out, total_ms, total_wall_ms, total_cost
        total_in += r["api_input_tokens"]
        total_out += r["api_output_tokens"]
        total_ms += r["duration_api_ms"]
        total_wall_ms += r["wall_ms"]
        total_cost += r["cost_usd"]
        for k in CACHE_FIELDS:
            cache[k] += r[k]

    # --- Turn 1: Routing from summary map ---
    route_prompt = (
        f"<vimanam_summary>\n{task['summary']}\n</vimanam_summary>\n\n"
        f"User task:\n{task['question']}\n\n"
        "Select the vimanam operation selector to fetch at full detail with schemas (depth 2). "
        "Return JSON with exact fields:\n"
        '{"operation_id": string or null, "operation": string or null, "schemas": [string]}\n'
        "Notes:\n"
        f"- `schemas` takes at most {MAX_SCHEMAS} names; extras are dropped. The operation slice already includes its request/response schemas, so list only types it is unlikely to reach.\n"
        "- For OpenAPI, set `operation_id` to the operation name shown in the summary OR `operation` to `METHOD /path` (e.g. `POST /embeddings`).\n"
        "- For Protobuf/gRPC, set `operation_id` to the full RPC name shown in the summary (e.g. `ledger.v1.SettlementStream.WatchSettlements`).\n"
        "- For GraphQL, set `operation_id` to the root coordinate (e.g. `Mutation.createIssue`) and optionally list any named input schema mentioned in the question under `schemas`.\n"
    )
    r1 = call_claude(model, route_prompt, ROUTE_SYSTEM_PROMPT, nonce=nonce)
    replay.append((route_prompt, ROUTE_SYSTEM_PROMPT))
    turns += 1
    if "error" in r1:
        n_assert = len(task["grade"]({}))
        return {"status": "route_error", "error": r1["error"], "task_passed": False, "assertions_passed": 0, "assertions_total": n_assert, "turns": turns}
    model_seen = r1["model"]
    account(r1)

    plan = extract_json(r1["result"]) or {}
    op_id = plan.get("operation_id")
    op_str = plan.get("operation")
    schemas = [str(s) for s in (plan.get("schemas") or []) if s][:MAX_SCHEMAS]
    route_target_matched = matches_expected_target(plan, task)

    # Execute vimanam with uniform --schema-depth 2 on BOTH operation and schema selectors
    chunks: list[str] = []
    if op_id:
        rc, out, _ = vimanam_run(vbin, [spec_path, *extra, "--operation-id", str(op_id), "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"])
        if rc == 0:
            chunks.append(out)
            routed_cmd_ok = True
    if not routed_cmd_ok and op_str:
        rc, out, _ = vimanam_run(vbin, [spec_path, *extra, "--operation", str(op_str), "--detail", "full", "--include-schemas", "--schema-depth", "2", "--no-report"])
        if rc == 0:
            chunks.append(out)
            routed_cmd_ok = True
    for s in schemas:
        rc, out, _ = vimanam_run(vbin, [spec_path, *extra, "--schema", s, "--schema-depth", "2", "--no-report"])
        if rc == 0:
            chunks.append(out)

    retrieved = "\n\n".join(chunks)
    if not retrieved:
        retrieved = task["summary"]
    total_ctx_bpe += count_bpe(retrieved)

    # --- Turn 2: Answer or request follow-up schemas ---
    t2_prompt = (
        f"<api_context>\n{retrieved}\n</api_context>\n\n"
        f"{task['question']}\n\n"
        'If any needed schema was omitted at depth 2 or not yet shown, return ONLY {"fetch_schemas": ["ExactSchemaName", ...]} '
        f"(at most {MAX_SCHEMAS} names; extras are dropped). "
        "Otherwise return ONLY the final JSON object."
    )
    r2 = call_claude(model, t2_prompt, FOLLOWUP_OR_ANSWER_SYSTEM_PROMPT, nonce=nonce)
    replay.append((t2_prompt, FOLLOWUP_OR_ANSWER_SYSTEM_PROMPT))
    turns += 1
    if "error" in r2:
        n_assert = len(task["grade"]({}))
        return {"status": "turn2_error", "error": r2["error"], "task_passed": False, "assertions_passed": 0, "assertions_total": n_assert, "turns": turns}
    account(r2)
    final_ctx = retrieved

    parsed = extract_json(r2["result"]) or {}
    # Optional Turn 3 if the model explicitly requested follow-up schemas
    if isinstance(parsed.get("fetch_schemas"), list) and parsed["fetch_schemas"]:
        followup_used = True
        followup_schemas = [str(x) for x in parsed["fetch_schemas"] if x][:MAX_SCHEMAS]
        extra_chunks: list[str] = []
        for s in followup_schemas:
            rc, out, _ = vimanam_run(vbin, [spec_path, *extra, "--schema", s, "--schema-depth", "2", "--no-report"])
            if rc == 0:
                extra_chunks.append(out)
        followup_text = "\n\n".join(extra_chunks)
        combined = retrieved + ("\n\n" + followup_text if followup_text else "")
        total_ctx_bpe += count_bpe(followup_text)
        t3_prompt = f"<api_context>\n{combined}\n</api_context>\n\n{task['question']}\n"
        r3 = call_claude(model, t3_prompt, ANSWER_SYSTEM_PROMPT, nonce=nonce)
        replay.append((t3_prompt, ANSWER_SYSTEM_PROMPT))
        turns += 1
        final_ctx = combined
        if "error" not in r3:
            account(r3)
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
        "route_target_matched": route_target_matched,
        "followup_used": followup_used,
        "followup_schemas": followup_schemas,
        "route_choice": plan,
        "total_context_bpe_o200k": total_ctx_bpe,
        "api_input_tokens": total_in,
        "api_output_tokens": total_out,
        "duration_api_ms": total_ms,
        "wall_ms": total_wall_ms,
        "cost_usd": round(total_cost, 6),
        **cache,
        "answer_coverage": answer_coverage(final_ctx, task["answer_markers"]),
        "task_passed": (passed == total),
        "assertions_passed": passed,
        "assertions_total": total,
        "checks": checks,
        "answer": parsed,
        "nonce": nonce,
        "_replay": replay,
    }


def aggregate_trials(
    trials: list[dict],
    context_bpe: int | None = None,
    context_chars: int | None = None,
    warm: dict | None = None,
) -> dict:
    n = len(trials)
    n_pass = sum(1 for t in trials if t.get("task_passed"))
    a_pass = [t["assertions_passed"] for t in trials]
    a_tot = trials[0]["assertions_total"]
    api_ins = [t.get("api_input_tokens", 0) for t in trials]
    durs = [t.get("duration_api_ms", 0) for t in trials]
    costs = [t.get("cost_usd", 0.0) for t in trials]
    statuses = {t.get("status", "ok") for t in trials}
    agg_status = "ok" if statuses == {"ok"} else (next(iter(statuses)) if len(statuses) == 1 else "mixed")
    out = {
        "status": agg_status,
        "trials": n,
        "tasks_passed": n_pass,
        "task_pass_rate": round(n_pass / n, 3),
        "assertions_mean": round(statistics.mean(a_pass), 2),
        "assertions_min": min(a_pass),
        "assertions_max": max(a_pass),
        "assertions_total": a_tot,
        "api_input_tokens_median": round(statistics.median(api_ins)),
        "duration_api_ms_median": round(statistics.median(durs)),
        # Every measured trial runs with its own nonce, so each is a cold-cache call;
        # cost_usd_cold_median is the per-question cost with nothing cached.
        "cost_usd_cold_median": round(statistics.median(costs), 6),
        "cost_usd_mean": round(statistics.mean(costs), 6),
        **{f"{k}_median": round(statistics.median(t.get(k, 0) for t in trials)) for k in CACHE_FIELDS},
        "trial_details": [{k: v for k, v in t.items() if not k.startswith("_")} for t in trials],
    }
    if warm is not None:
        out["warm_replay"] = warm
    if context_bpe is not None:
        out["context_bpe_o200k"] = context_bpe
    if context_chars is not None:
        out["context_chars"] = context_chars
        out["context_chars_div_4"] = (context_chars + 3) // 4
    if "turns" in trials[0]:
        out["turns_mean"] = round(statistics.mean(t["turns"] for t in trials), 2)
        out["followup_rate"] = round(sum(1 for t in trials if t.get("followup_used")) / n, 3)
        out["route_cmd_ok_rate"] = round(sum(1 for t in trials if t.get("routed_cmd_ok")) / n, 3)
        out["route_target_match_rate"] = round(sum(1 for t in trials if t.get("route_target_matched")) / n, 3)
        out["answer_coverage_mean"] = round(statistics.mean(t.get("answer_coverage", 0.0) for t in trials), 3)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--vimanam", "--vbin", dest="vimanam", required=True, help="vimanam release binary to evaluate")
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--model", default="haiku")
    ap.add_argument("--trials", type=int, default=3)
    ap.add_argument("--workers", type=int, default=1)
    ap.add_argument("--output", required=True)
    ap.add_argument("--only-tasks", default="", help="comma-separated task ids (smoke runs)")
    args = ap.parse_args()

    vbin = str(Path(args.vimanam).resolve())
    corpus = Path(args.corpus).resolve()

    # Preflight: one tiny call, so an expired login or a missing CLI stops the run
    # before it writes a results file in which every trial scored 0 and cost $0.
    probe = call_claude(args.model, "Reply with the single word OK.", "Answer tersely.", timeout=120)
    if probe.get("error"):
        sys.exit(f"claude -p preflight failed; no trials were run: {probe['error']}")

    tasks = build_tasks(vbin, corpus)
    corpus_hashes = {}
    for t in tasks:
        sp = Path(t["spec_path"])
        rel = str(sp.relative_to(corpus)) if sp.is_relative_to(corpus) else str(sp.relative_to(REPO))
        corpus_hashes[rel] = hashlib.sha256(sp.read_bytes()).hexdigest()
    conditions = ["no_spec", "summary", "routed_e2e", "fixed_slice", "oracle_slice", "raw_spec"]
    if args.only_tasks:
        keep = set(args.only_tasks.split(","))
        tasks = [t for t in tasks if t["id"] in keep]

    # One job per (task, condition) cell. Inside a cell: cold trial 1, then an immediate
    # warm replay of trial 1's exact prompts (inside the prompt-cache TTL), then the
    # remaining cold trials. Each cold trial carries a fresh nonce, so no measured trial
    # reads another trial's cache.
    jobs = [(t_idx, cond) for t_idx in range(len(tasks)) for cond in conditions]
    n_calls = sum(1 if (c == "raw_spec" and tasks[i]["raw_large"]) else args.trials for i, c in jobs)
    print(f"Running {n_calls} cold trials (+{len(jobs)} warm replays) across {len(tasks)} tasks and {len(conditions)} conditions (workers={args.workers})...", flush=True)

    results_map: dict[tuple[int, str], list[dict]] = {}
    warm_map: dict[tuple[int, str], dict] = {}

    def run_trial(task: dict, cond: str, nonce: str) -> dict:
        if cond == "no_spec":
            return run_single_turn_trial(args.model, task, "", nonce)
        if cond in {"summary", "fixed_slice", "oracle_slice", "raw_spec"}:
            return run_single_turn_trial(args.model, task, task[cond], nonce)
        if cond == "routed_e2e":
            return run_routed_e2e_trial(vbin, args.model, task, nonce)
        raise ValueError(cond)

    def exec_cell(job):
        t_idx, cond = job
        task = tasks[t_idx]
        n_trials = 1 if (cond == "raw_spec" and task["raw_large"]) else args.trials
        trials = []
        warm = None
        for i in range(n_trials):
            r = run_trial(task, cond, new_nonce())
            trials.append(r)
            if i == 0:
                warm = replay_warm(args.model, r["_replay"], r["nonce"]) if r.get("_replay") else {"status": r.get("status", "error")}
        return t_idx, cond, trials, warm

    done = 0

    def record(t_idx, cond, trials, warm):
        nonlocal done
        results_map[(t_idx, cond)] = trials
        warm_map[(t_idx, cond)] = warm
        done += 1
        print(f"  [{done}/{len(jobs)}] {tasks[t_idx]['id']} {cond}: "
              f"{[t['assertions_passed'] for t in trials]}/{trials[0]['assertions_total']} "
              f"cold ${[t.get('cost_usd', 0) for t in trials]} warm ${warm.get('cost_usd')}", flush=True)

    if args.workers <= 1:
        for job in jobs:
            record(*exec_cell(job))
    else:
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
            for res in pool.map(exec_cell, jobs):
                record(*res)

    model_seen = args.model
    task_records = []
    tokenizer_calibration = []

    for t_idx, task in enumerate(tasks):
        rec = {
            "id": task["id"],
            "format": task["format"],
            "spec": task["spec"],
            "expected_operation_id": task["expected_operation_id"],
            "expected_operation": task["expected_operation"],
            "size_class": task["size_class"],
            "raw_spec_chars": len(task["raw_spec"]),
            "oracle_schemas": task["oracle_schemas"],
            "answer_markers": task["answer_markers"],
            "conditions": {},
        }
        no_spec_trials = results_map[(t_idx, "no_spec")]
        no_spec_base_in = round(statistics.median(t.get("api_input_tokens", 0) for t in no_spec_trials))
        for t in no_spec_trials:
            if t.get("model"):
                model_seen = t["model"]

        for cond in conditions:
            trials = results_map[(t_idx, cond)]
            warm = warm_map[(t_idx, cond)]
            if cond == "no_spec":
                agg = aggregate_trials(trials, context_bpe=0, context_chars=0, warm=warm)
                agg["answer_coverage"] = 0.0
            elif cond in {"summary", "fixed_slice", "oracle_slice", "raw_spec"}:
                agg = aggregate_trials(trials, context_bpe=count_bpe(task[cond]), context_chars=len(task[cond]), warm=warm)
                agg["answer_coverage"] = answer_coverage(task[cond], task["answer_markers"])
            elif cond == "routed_e2e":
                agg = aggregate_trials(trials, warm=warm)
                agg["total_context_bpe_o200k_median"] = round(statistics.median(t.get("total_context_bpe_o200k", 0) for t in trials))

            if cond in {"summary", "fixed_slice", "oracle_slice", "raw_spec"} and agg["status"] == "ok" and agg.get("context_bpe_o200k", 0) > 0:
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
            f"(pass {rec['conditions']['routed_e2e']['tasks_passed']}/{rec['conditions']['routed_e2e']['trials']}, "
            f"match {rec['conditions']['routed_e2e']['route_target_match_rate']}, "
            f"fu {rec['conditions']['routed_e2e']['followup_rate']}, ${rec['conditions']['routed_e2e']['cost_usd_mean']:.4f}) "
            f"| fixed={rec['conditions']['fixed_slice']['assertions_mean']}/{rec['conditions']['fixed_slice']['assertions_total']} "
            f"| oracle={rec['conditions']['oracle_slice']['assertions_mean']}/{rec['conditions']['oracle_slice']['assertions_total']} "
            f"| raw={rec['conditions']['raw_spec'].get('status')}:{rec['conditions']['raw_spec']['assertions_mean']}/{rec['conditions']['raw_spec']['assertions_total']}",
            flush=True,
        )

    def summarize(subset_ids: set[str]) -> dict:
        out = {}
        for cond in conditions:
            recs = [r["conditions"][cond] for r in task_records if r["id"] in subset_ids]
            ok_recs = [c for c in recs if c["status"] == "ok"]
            too_long = [c for c in recs if c["status"] == "prompt_too_long"]
            if not recs:
                continue
            out[cond] = {
                "tasks_total": len(recs),
                "tasks_ok": len(ok_recs),
                "tasks_prompt_too_long": len(too_long),
                "mean_task_pass_rate": round(sum(c["task_pass_rate"] for c in recs) / len(recs), 3),
                "mean_assertions_passed": round(sum(c["assertions_mean"] for c in recs), 2),
                "assertions_total": sum(c["assertions_total"] for c in recs),
                "mean_assertion_rate": round(sum(c["assertions_mean"] for c in recs) / sum(c["assertions_total"] for c in recs), 3),
                "sum_api_input_tokens_median": sum(c.get("api_input_tokens_median", 0) for c in ok_recs),
                "sum_cost_usd_cold_median": round(sum(c.get("cost_usd_cold_median", 0.0) for c in ok_recs), 6),
                "sum_cost_usd_warm_replay": round(sum((c.get("warm_replay") or {}).get("cost_usd", 0.0) for c in ok_recs), 6),
                "warm_replay_ok": sum(1 for c in ok_recs if (c.get("warm_replay") or {}).get("status") == "ok"),
                "sum_warm_cache_read_input_tokens": sum((c.get("warm_replay") or {}).get("cache_read_input_tokens", 0) for c in ok_recs),
                "sum_cost_usd_mean": round(sum(c.get("cost_usd_mean", 0.0) for c in ok_recs), 6),
                "sum_duration_api_ms_median": sum(c.get("duration_api_ms_median", 0) for c in ok_recs),
                "mean_answer_coverage": round(statistics.mean(c.get("answer_coverage", c.get("answer_coverage_mean", 0.0)) for c in recs), 3),
            }
            if cond == "routed_e2e" and ok_recs:
                out[cond]["mean_turns"] = round(statistics.mean(c["turns_mean"] for c in ok_recs), 2)
                out[cond]["mean_followup_rate"] = round(statistics.mean(c["followup_rate"] for c in ok_recs), 3)
                out[cond]["mean_route_cmd_ok_rate"] = round(statistics.mean(c["route_cmd_ok_rate"] for c in ok_recs), 3)
                out[cond]["mean_route_target_match_rate"] = round(statistics.mean(c["route_target_match_rate"] for c in ok_recs), 3)
        return out

    def ratios(summary: dict, a: str = "raw_spec", b: str = "routed_e2e") -> dict:
        """raw_spec / routed_e2e ratios, cold vs cold and warm vs warm."""
        ra, rb = summary[a], summary[b]

        def div(x, y):
            return round(x / y, 2) if y else None

        return {
            "tokens": div(ra["sum_api_input_tokens_median"], rb["sum_api_input_tokens_median"]),
            "cost_cold_vs_cold": div(ra["sum_cost_usd_cold_median"], rb["sum_cost_usd_cold_median"]),
            "cost_warm_vs_warm": div(ra["sum_cost_usd_warm_replay"], rb["sum_cost_usd_warm_replay"]),
            "cost_warm_raw_vs_cold_routed": div(ra["sum_cost_usd_warm_replay"], rb["sum_cost_usd_cold_median"]),
        }

    all_ids = {t["id"] for t in tasks}
    by_size = {c: {t["id"] for t in tasks if t["size_class"] == c} for c in ("large", "small", "too_large")}
    fitting_ids = by_size["large"] | by_size["small"]

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
            "vimanam_version": subprocess.check_output([vbin, "--version"], text=True).strip(),
            "trials_per_cell": args.trials,
            "raw_large_spec_trials": 1,
            "workers": args.workers,
            "claude_cli_version": subprocess.check_output(["claude", "--version"], text=True).strip(),
            "task_count": len(tasks),
            "size_classes": {k: sorted(v) for k, v in by_size.items()},
            "corpus_sha256": corpus_hashes,
            "protobuf_fixtures": "synthetic non-memorizable multi-file protos under scripts/eval-fixtures/protos/ plus one real OpenTelemetry task",
            "schema_depth_policy": "uniform --schema-depth 2 on all routed_e2e operation and --schema calls, fixed_slice, and oracle_slice",
            "oracle_slice": "operation at --schema-depth 2 plus --schema (depth 2) for the named types holding the answers; build asserts every answer marker is present",
            "cache_protocol": (
                "every measured trial prepends a fresh nonce to each user prompt, so all graded trials are cold-cache calls; "
                "immediately after trial 1 of each cell its exact prompts are re-sent with the same nonce (warm_replay, cost only, not graded)"
            ),
            "grading": "deterministic programmatic JSON assertions (no LLM judge)",
            "isolation": "claude -p --tools '' --strict-mcp-config --setting-sources ''",
        },
        "summary_all_tasks": summarize(all_ids),
        "summary_fitting_tasks": summarize(fitting_ids),
        "summary_large_fitting_tasks": summarize(by_size["large"]),
        "summary_small_tasks": summarize(by_size["small"]),
        "summary_too_large_tasks": summarize(by_size["too_large"]),
        "claude_vs_o200k_calibration_summary": cal_summary,
        "claude_vs_o200k_calibration_summary_note": "chars/claude-token uses cold-trial median input tokens minus the no_spec baseline",
        "claude_vs_o200k_calibration_rows": tokenizer_calibration,
        "tasks": task_records,
    }
    report["raw_over_routed_ratios"] = {
        k: ratios(report[k])
        for k in ("summary_fitting_tasks", "summary_large_fitting_tasks", "summary_small_tasks")
        if "routed_e2e" in report[k] and "raw_spec" in report[k]
    }
    out_path = Path(args.output)
    out_path.write_text(json.dumps(report, indent=2) + "\n")
    print(f"\nWrote {out_path}", flush=True)


if __name__ == "__main__":
    main()
