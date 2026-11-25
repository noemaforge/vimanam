#!/usr/bin/env bash
# Negative checks for the `$ref` diff harness's allowlist: every entry of
# formal/refdiff/negative/checks.json corrupts the Rust's records on one case
# (scripts/formal/refdiff-mutate.py) in a way the allowlist must not excuse,
# and `refdiff-harness.py --ci` must then exit 1. Each case must first pass
# --ci with the unmodified binary.
#
# Usage: scripts/formal/check-refdiff-negative.sh path/to/vimanam
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
bin="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
neg="$root/formal/refdiff/negative"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

run() {  # run CASE BINARY -> harness exit status
  rm -rf "$tmp/cases" && mkdir "$tmp/cases"
  cp "$neg/$1.old.json" "$neg/$1.new.json" "$tmp/cases/"
  python3 -I "$root/scripts/formal/refdiff-harness.py" --ci --jobs 1 --seeds 0 \
    --cases "$tmp/cases" --rust "$2" --out "$tmp/out" > "$tmp/log" 2>&1
}

status=0
for case in $(python3 -I -c 'import json,sys; print(" ".join(sorted({c["case"] for c in json.load(open(sys.argv[1]))["checks"]})))' "$neg/checks.json"); do
  if ! run "$case" "$bin"; then
    echo "error: $case fails --ci with the unmodified binary:" >&2
    cat "$tmp/log" >&2
    status=1
  fi
done

count=$(python3 -I -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["checks"]))' "$neg/checks.json")
for i in $(seq 0 $((count - 1))); do
  eval "$(python3 -I - "$neg/checks.json" "$i" <<'PY'
import json, shlex, sys
c = json.load(open(sys.argv[1]))["checks"][int(sys.argv[2])]
for k in ("case", "OP", "PTR", "TO"):
    print(f"{k}={shlex.quote(c.get(k, ''))}")
PY
)"
  if REFDIFF_REAL_BIN="$bin" OP="$OP" PTR="$PTR" TO="$TO" run "$case" "$root/scripts/formal/refdiff-mutate.py"; then
    echo "error: --ci accepted $case with OP=$OP PTR=$PTR TO=$TO" >&2
    status=1
  elif ! grep -qE '^[1-9][0-9]* disagreement\(s\) outside the allowlist, 0 error\(s\)$' "$tmp/log"; then
    # Rejected, but not by the allowlist (a crash or a Rust error).
    echo "error: $case with OP=$OP PTR=$PTR failed for another reason:" >&2
    cat "$tmp/log" >&2
    status=1
  else
    echo "ok: --ci rejects $case with OP=$OP PTR=$PTR${TO:+ TO=$TO}: $(grep 'not allowlisted' "$tmp/log" | head -1)"
  fi
done
exit $status
