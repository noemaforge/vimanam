#!/usr/bin/env bash
# Fail when formal/refdiff/expected/ is stale: the `$ref` diff reference
# model (formal/VimanamProofs/RefDiff.lean) gives different output on the
# fixed cases in formal/refdiff/cases/.
#
# The expected files (the grouped view) pin what the model says, so a change to the semantics
# shows up in review as a diff of concrete outputs. They are not compared
# with the Rust here; scripts/formal/refdiff-harness.py does that.
#
# Usage, from anywhere in the repository (needs lake on PATH):
#   scripts/formal/check-refdiff.sh            # check
#   scripts/formal/check-refdiff.sh --update   # regenerate after a deliberate change
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root/formal"

lake build --wfail refdiff > /dev/null
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
: > "$tmp/batch.tsv"
for old in refdiff/cases/*.old.json; do
  name="$(basename "$old" .old.json)"
  printf '%s\t%s\t%s\n' "$old" "refdiff/cases/$name.new.json" "$tmp/$name.json" >> "$tmp/batch.tsv"
done
.lake/build/bin/refdiff --batch "$tmp/batch.tsv"
rm "$tmp/batch.tsv"
# Keep the grouped view only (the per-path view of an adversarial case runs
# to megabytes), one record per line, sorted, so a review diff names records.
python3 -I - "$tmp" <<'PY'
import json, pathlib, sys
for f in pathlib.Path(sys.argv[1]).glob("*.json"):
    out = json.loads(f.read_text())
    recs = sorted(json.dumps(r, sort_keys=True) for r in out["grouped"])
    f.write_text("[\n" + ",\n".join(recs) + ("\n" if recs else "") + "]\n")
PY

if [ "${1:-}" = "--update" ]; then
  rm -rf refdiff/expected
  mkdir -p refdiff/expected
  cp "$tmp"/*.json refdiff/expected/
  echo "regenerated formal/refdiff/expected/ ($(ls refdiff/expected | wc -l | tr -d ' ') files)"
  exit 0
fi

if ! diff -rq -x '.*' refdiff/expected "$tmp" > /dev/null; then
  echo "error: formal/refdiff/expected/ is stale: the reference model now gives different output." >&2
  echo "Regenerate with 'scripts/formal/check-refdiff.sh --update' and commit it. Differences:" >&2
  diff -ru -x '.*' refdiff/expected "$tmp" | head -n 60 >&2 || true
  exit 1
fi
echo "formal/refdiff/expected/ matches the reference model ($(ls refdiff/expected | wc -l | tr -d ' ') cases)"
