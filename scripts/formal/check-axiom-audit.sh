#!/usr/bin/env bash
# Elaborate formal/VimanamProofs/Audit.lean and fail unless every theorem
# under formal/VimanamProofs/ has a `#print axioms` line there and every
# answer names only Lean's standard axioms (propext, Quot.sound,
# Classical.choice). Needs lake on PATH.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root/formal"

audit=VimanamProofs/Audit.lean
log="$(mktemp)"
trap 'rm -f "$log"' EXIT

# Every theorem, by fully qualified name (each file is one namespace).
missing=0
for file in VimanamProofs/*.lean; do
  [ "$file" = "$audit" ] && continue
  ns="$(sed -n 's/^namespace \(.*\)$/\1/p' "$file" | head -n 1)"
  for name in $(sed -n 's/^theorem \([A-Za-z0-9_]*\).*/\1/p' "$file"); do
    if ! grep -qx "#print axioms $ns.$name" "$audit"; then
      echo "error: $ns.$name ($file) has no '#print axioms' line in $audit" >&2
      missing=1
    fi
  done
done
[ "$missing" -eq 0 ] || exit 1

lake env lean "$audit" > "$log" 2>&1
expected="$(grep -c '^#print axioms ' "$audit")"
answered="$(grep -cE "depends on axioms: \[|does not depend on any axioms" "$log" || true)"
if [ "$expected" -eq 0 ] || [ "$answered" -ne "$expected" ]; then
  echo "error: the axiom audit answered $answered of $expected '#print axioms' lines" >&2
  cat "$log" >&2
  exit 1
fi
bad="$(grep -oE "depends on axioms: \[[^]]*\]" "$log" \
  | sed -E 's/^depends on axioms: \[//; s/\]$//' | tr ',' '\n' | sed 's/^ *//; s/ *$//' \
  | grep -vxE 'propext|Quot\.sound|Classical\.choice' | sort -u || true)"
if [ -n "$bad" ]; then
  echo "error: non-standard axioms in the audit: $bad" >&2
  exit 1
fi
echo "audited $expected theorems; standard axioms only"
