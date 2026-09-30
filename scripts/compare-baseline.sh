#!/usr/bin/env bash
# Compare a frozen baseline vimanam binary against a candidate build, byte for byte.
# Usage: scripts/compare-baseline.sh [BASELINE_BIN] [CANDIDATE_BIN]
# Env:
#   SKIP_BUILD=1   skip cargo build; use CANDIDATE_BIN or target/release/vimanam
#   KEEP=1         retain the work directory after exit
#   VERBOSE=1      print unified diffs for differing cases
#   CHECK_TESTS=1  compare cargo test --list leaf names to /tmp/vimanam-tests-before.txt
#   ALLOW_SAME=1   allow baseline and candidate binaries with identical SHA-256 content
# Requires: jq (hard requirement), and either shasum or sha256sum.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

if ! command -v jq >/dev/null 2>&1; then
  echo "error: jq is required (schema selection and cargo artifact resolution)" >&2
  exit 1
fi

file_sha256() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    echo "error: need shasum or sha256sum" >&2
    exit 1
  fi
}

BASELINE_BIN="${1:-/tmp/vimanam-baseline/bin/vimanam}"
CANDIDATE_ARG="${2:-}"

if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
  build_json="$(mktemp)"
  cargo build --release --locked --message-format=json-render-diagnostics >"$build_json"
  if [[ -n "$CANDIDATE_ARG" ]]; then
    CANDIDATE_BIN="$CANDIDATE_ARG"
  else
    CANDIDATE_BIN="$(
      jq -r '
        select(.reason == "compiler-artifact")
        | select(.target.name == "vimanam")
        | select((.target.kind // []) | index("bin"))
        | .executable // empty
      ' "$build_json" | tail -n 1
    )"
    if [[ -z "$CANDIDATE_BIN" || "$CANDIDATE_BIN" == "null" ]]; then
      echo "error: could not resolve vimanam executable from cargo build JSON" >&2
      rm -f "$build_json"
      exit 1
    fi
  fi
  rm -f "$build_json"
else
  CANDIDATE_BIN="${CANDIDATE_ARG:-target/release/vimanam}"
fi

if [[ ! -x "$BASELINE_BIN" ]]; then
  echo "baseline binary not executable: $BASELINE_BIN" >&2
  exit 1
fi
if [[ ! -x "$CANDIDATE_BIN" ]]; then
  echo "candidate binary not executable: $CANDIDATE_BIN" >&2
  exit 1
fi

# Resolve to absolute paths so we can cd into per-binary subtrees.
case "$BASELINE_BIN" in
  /*) ;;
  *) BASELINE_BIN="$REPO_ROOT/$BASELINE_BIN" ;;
esac
case "$CANDIDATE_BIN" in
  /*) ;;
  *) CANDIDATE_BIN="$REPO_ROOT/$CANDIDATE_BIN" ;;
esac

BASE_SHA="$(file_sha256 "$BASELINE_BIN")"
CAND_SHA="$(file_sha256 "$CANDIDATE_BIN")"
echo "Baseline:  $BASELINE_BIN"
echo "  sha256:  $BASE_SHA"
echo "Candidate: $CANDIDATE_BIN"
echo "  sha256:  $CAND_SHA"

if cmp -s "$BASELINE_BIN" "$CANDIDATE_BIN"; then
  if [[ "${ALLOW_SAME:-0}" == "1" ]]; then
    echo "warning: baseline and candidate are identical (ALLOW_SAME=1)" >&2
  else
    echo "error: baseline and candidate binaries have identical content; set ALLOW_SAME=1 to override" >&2
    exit 1
  fi
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/vimanam-compare.XXXXXX")"
KEEP="${KEEP:-0}"
cleanup() {
  if [[ "$KEEP" == "1" ]]; then
    echo "keeping work dir: $WORK" >&2
  else
    rm -rf "$WORK"
  fi
}
trap cleanup EXIT

BASE_ROOT="$WORK/baseline"
CAND_ROOT="$WORK/candidate"
CASES_DIR="$WORK/cases"
mkdir -p "$BASE_ROOT" "$CAND_ROOT" "$CASES_DIR"

# Shared invalid JSON for parse-failure diff cases (identical bytes both sides see).
INVALID_JSON="$WORK/invalid.json"
printf '%s\n' '{ not valid json' >"$INVALID_JSON"

MANIFEST="$WORK/manifest.txt"
: >"$MANIFEST"

CASE_NUM=0
DIFFER_COUNT=0
TOTAL_COUNT=0
TESTS_FAILED=0

# --- helpers ---------------------------------------------------------------

fs_safe() {
  printf '%s' "$1" | sed 's/[^A-Za-z0-9._+=-]/_/g'
}

short_flags() {
  local out="" a base
  for a in "$@"; do
    case "$a" in
      /*.json|/*.yaml|/*.yml)
        base="$(basename "$a")"
        base="${base%.*}"
        out="${out}-${base}"
        ;;
      *)
        out="${out}-$(fs_safe "$a")"
        ;;
    esac
  done
  out="${out#-}"
  printf '%s' "${out:0:120}"
}

# add_case KIND OUT_REL -- arg...
# KIND: stream (stdout/stderr/exit) or tree (also compare OUT_REL under each side).
# OUT_REL: relative output under the per-case dir; use literal OUT as the -o placeholder.
add_case() {
  local kind="$1"
  local out_rel="$2"
  shift 2
  if [[ "${1:-}" == "--" ]]; then
    shift
  fi
  CASE_NUM=$((CASE_NUM + 1))
  local num
  num="$(printf '%03d' "$CASE_NUM")"
  local label
  label="$(short_flags "$@")"
  local name="${num}-${label}"
  local dir="$CASES_DIR/$name"
  mkdir -p "$dir"
  printf '%s\n' "$kind" >"$dir/kind"
  printf '%s\n' "$out_rel" >"$dir/out_rel"
  : >"$dir/argv"
  local a
  for a in "$@"; do
    printf '%s\n' "$a" >>"$dir/argv"
  done
  {
    printf '%s\t' "$name"
    local first=1
    for a in "$@"; do
      if [[ $first -eq 1 ]]; then
        first=0
      else
        printf ' '
      fi
      printf '%q' "$a"
    done
    printf '\n'
  } >>"$MANIFEST"
}

json_ptr_escape() {
  # RFC 6901: escape ~ then /
  printf '%s' "$1" | sed -e 's/~/~0/g' -e 's/\//~1/g'
}

# Replace this side's work-root absolute path with <WORK> so stderr compares
# across sides without hiding genuine path differences between binaries.
normalize_stderr() {
  local side_root="$1"
  local stderr_f="$2"
  local tmp line
  tmp="$(mktemp)"
  while IFS= read -r line || [[ -n "$line" ]]; do
    while [[ "$line" == *"$side_root"* ]]; do
      line="${line%%"$side_root"*}"'<WORK>'"${line#*"$side_root"}"
    done
    printf '%s\n' "$line"
  done <"$stderr_f" >"$tmp"
  mv "$tmp" "$stderr_f"
}

run_one_side() {
  local bin="$1"
  local side_root="$2"
  local name="$3"
  local out_rel="$4"
  shift 4

  local side_case="$side_root/$name"
  mkdir -p "$side_case"
  if [[ -n "$out_rel" ]]; then
    mkdir -p "$side_case/$(dirname "$out_rel")"
  fi

  local stdout_f="$side_case/${name}.stdout"
  local stderr_f="$side_case/${name}.stderr"
  local exit_f="$side_case/${name}.exit"

  # cd into the side root so -o paths in stderr stay relative/comparable.
  (
    cd "$side_root"
    set +e
    "$bin" "$@" >"$stdout_f" 2>"$stderr_f"
    ec=$?
    set -e
    printf '%s\n' "$ec" >"$exit_f"
  )
  normalize_stderr "$side_root" "$stderr_f"
}

show_verbose_diff() {
  local bas="$1" can="$2" name="$3" out_rel="$4" diffs="$5"
  local tmp
  tmp="$(mktemp)"
  case " $diffs " in
    *" stdout "*)
      echo "---- $name stdout ----"
      diff -u "$bas/${name}.stdout" "$can/${name}.stdout" >"$tmp" 2>&1 || true
      head -n 40 "$tmp"
      ;;
  esac
  case " $diffs " in
    *" stderr "*)
      echo "---- $name stderr ----"
      diff -u "$bas/${name}.stderr" "$can/${name}.stderr" >"$tmp" 2>&1 || true
      head -n 40 "$tmp"
      ;;
  esac
  case " $diffs " in
    *" exit "*)
      echo "---- $name exit ----"
      echo "baseline: $(cat "$bas/${name}.exit")  candidate: $(cat "$can/${name}.exit")"
      ;;
  esac
  case " $diffs " in
    *" tree "*|*" no-output "*)
      echo "---- $name tree ----"
      diff -ru "$bas/$out_rel" "$can/$out_rel" >"$tmp" 2>&1 || true
      head -n 40 "$tmp"
      ;;
  esac
  rm -f "$tmp"
}

compare_case() {
  local name="$1"
  local kind="$2"
  local out_rel="$3"
  local bas="$BASE_ROOT/$name"
  local can="$CAND_ROOT/$name"
  local diffs=""

  if ! cmp -s "$bas/${name}.stdout" "$can/${name}.stdout"; then
    diffs="${diffs}stdout "
  fi
  if ! cmp -s "$bas/${name}.stderr" "$can/${name}.stderr"; then
    diffs="${diffs}stderr "
  fi
  if ! cmp -s "$bas/${name}.exit" "$can/${name}.exit"; then
    diffs="${diffs}exit "
  fi

  if [[ "$kind" == "tree" && -n "$out_rel" ]]; then
    local bas_out="$bas/$out_rel"
    local can_out="$can/$out_rel"
    local bas_ec can_ec
    bas_ec="$(cat "$bas/${name}.exit")"
    can_ec="$(cat "$can/${name}.exit")"
    if [[ ! -e "$bas_out" && ! -e "$can_out" ]]; then
      # Identical non-zero error with matching stderr is a pass; otherwise no-output.
      if [[ "$bas_ec" != "0" && "$can_ec" != "0" ]] && cmp -s "$bas/${name}.stderr" "$can/${name}.stderr"; then
        :
      else
        diffs="${diffs}no-output "
      fi
    elif ! diff -rq "$bas_out" "$can_out" >/dev/null 2>&1; then
      diffs="${diffs}tree "
    fi
  fi

  TOTAL_COUNT=$((TOTAL_COUNT + 1))
  if [[ -n "$diffs" ]]; then
    DIFFER_COUNT=$((DIFFER_COUNT + 1))
    echo "$name  differs: $diffs"
    if [[ "${VERBOSE:-0}" == "1" ]]; then
      show_verbose_diff "$bas" "$can" "$name" "$out_rel" "$diffs"
    fi
  fi
}

run_all_cases() {
  local name kind out_rel arg_line
  local list_file raw_file d found=0
  list_file="$(mktemp)"
  raw_file="$(mktemp)"
  : >"$raw_file"
  for d in "$CASES_DIR"/*; do
    [[ -d "$d" ]] || continue
    found=1
    basename "$d" >>"$raw_file"
  done
  LC_ALL=C sort <"$raw_file" >"$list_file"
  rm -f "$raw_file"
  if [[ "$found" -eq 0 ]]; then
    echo "no cases registered" >&2
    rm -f "$list_file"
    return 1
  fi
  while IFS= read -r name || [[ -n "$name" ]]; do
    [[ -z "$name" ]] && continue
    kind="$(cat "$CASES_DIR/$name/kind")"
    out_rel="$(cat "$CASES_DIR/$name/out_rel")"

    # Rebuild argv; replace placeholder OUT with name/out_rel.
    local run_args=()
    local prev=""
    while IFS= read -r arg_line || [[ -n "$arg_line" ]]; do
      if [[ -n "$out_rel" && ( "$prev" == "-o" || "$prev" == "--output" ) && "$arg_line" == "OUT" ]]; then
        run_args[${#run_args[@]}]="$name/$out_rel"
      else
        run_args[${#run_args[@]}]="$arg_line"
      fi
      prev="$arg_line"
    done <"$CASES_DIR/$name/argv"

    run_one_side "$BASELINE_BIN" "$BASE_ROOT" "$name" "$out_rel" "${run_args[@]}"
    run_one_side "$CANDIDATE_BIN" "$CAND_ROOT" "$name" "$out_rel" "${run_args[@]}"
    compare_case "$name" "$kind" "$out_rel"
  done <"$list_file"
  rm -f "$list_file"
}

# --- discover inputs -------------------------------------------------------

FIXTURES=()
for f in \
  tests/fixtures/*.json \
  tests/fixtures/*.yaml \
  tests/fixtures/*.yml \
  scripts/compare-fixtures/*.json \
  scripts/compare-fixtures/*.yaml \
  scripts/compare-fixtures/*.yml
do
  if [[ -f "$f" ]]; then
    FIXTURES[${#FIXTURES[@]}]="$(cd "$(dirname "$f")" && pwd)/$(basename "$f")"
  fi
done

LARGE_SPECS=()
for f in swagger.json openapi.json openapiv2.swagger.json; do
  if [[ -f "$REPO_ROOT/$f" ]]; then
    LARGE_SPECS[${#LARGE_SPECS[@]}]="$REPO_ROOT/$f"
  fi
done

# Tree modes are expensive; cover a representative fixture set.
TREE_FIXTURES=()
for f in \
  tests/fixtures/petstore_oas3.json \
  tests/fixtures/petstore_oas2.json \
  tests/fixtures/multi_tag_oas3.json \
  tests/fixtures/schema_refs_oas3.json \
  tests/fixtures/hygiene_oas3.json \
  tests/fixtures/examples_oas3.json \
  tests/fixtures/operation_select_oas3.json
do
  if [[ -f "$f" ]]; then
    TREE_FIXTURES[${#TREE_FIXTURES[@]}]="$(cd "$(dirname "$f")" && pwd)/$(basename "$f")"
  fi
done

# --- case matrix: conversion streams ---------------------------------------

add_conversion_matrix() {
  local spec="$1"
  local reduced="${2:-0}"

  add_case stream "" -- "$spec"

  local d
  for d in summary basic standard full; do
    add_case stream "" -- "$spec" --detail "$d"
  done

  add_case stream "" -- "$spec" --detail full --include-schemas
  add_case stream "" -- "$spec" --detail full --include-schemas --inline-schemas
  add_case stream "" -- "$spec" --detail full --include-schemas --include-examples

  local g
  for g in service method path; do
    add_case stream "" -- "$spec" --group-by "$g"
  done
  add_case stream "" -- "$spec" --flat
  add_case stream "" -- "$spec" --method

  add_case stream "" -- "$spec" --no-report
  add_case stream "" -- "$spec" --max-tokens 200
  add_case stream "" -- "$spec" --max-tokens 2000
  add_case stream "" -- "$spec" --stats

  if [[ "$reduced" == "1" ]]; then
    return 0
  fi

  local depth
  for depth in 0 1 3; do
    add_case stream "" -- "$spec" --detail full --include-schemas --schema-depth "$depth"
  done
  add_case stream "" -- "$spec" --detail full --include-schemas --inline-schemas --schema-depth 0
  add_case stream "" -- "$spec" --detail full --include-schemas --inline-schemas --schema-depth 1

  # Accepted clap combos: schema-depth with split/skill + full schemas (no --inline-schemas).
  add_case tree "split-endpoint-depth1" -- \
    "$spec" --split endpoint --detail full --include-schemas --schema-depth 1 -o OUT
  add_case tree "skill-depth1" -- \
    "$spec" --output-mode skill --detail full --include-schemas --schema-depth 1 -o OUT

  add_case tree "out.md" -- "$spec" -o OUT
  add_case tree "out-full.md" -- "$spec" --detail full --include-schemas -o OUT
}

if [[ ${#FIXTURES[@]} -gt 0 ]]; then
  for spec in "${FIXTURES[@]}"; do
    add_conversion_matrix "$spec" 0
  done
fi
if [[ ${#LARGE_SPECS[@]} -gt 0 ]]; then
  for spec in "${LARGE_SPECS[@]}"; do
    add_conversion_matrix "$spec" 1
  done
fi

# --- schema selection ------------------------------------------------------

add_explicit_schema_fields() {
  local spec="$1"
  shift
  local sel
  for sel in "$@"; do
    add_case stream "" -- "$spec" --schema-field "$sel"
    add_case stream "" -- "$spec" --schema-field "$sel" --schema-depth 0
    add_case stream "" -- "$spec" --schema-field "$sel" --schema-depth 1
  done
}

add_schema_selection_cases() {
  local spec="$1"
  if [[ ! -f "$spec" ]]; then
    return 0
  fi

  local tmp_names
  tmp_names="$(mktemp)"
  jq -r '.components.schemas // {} | keys[]' "$spec" >"$tmp_names"
  if [[ ! -s "$tmp_names" ]]; then
    echo "error: no component schemas found in $spec" >&2
    rm -f "$tmp_names"
    exit 1
  fi

  local name first_schema=""
  while IFS= read -r name || [[ -n "$name" ]]; do
    [[ -z "$name" ]] && continue
    if [[ -z "$first_schema" ]]; then
      first_schema="$name"
    fi
    add_case stream "" -- "$spec" --schema "$name"
    add_case stream "" -- "$spec" --schema "$name" --schema-depth 0
    add_case stream "" -- "$spec" --schema "$name" --schema-depth 1

    local tmp_ptrs
    tmp_ptrs="$(mktemp)"
    : >"$tmp_ptrs"

    local prop
    while IFS= read -r prop || [[ -n "$prop" ]]; do
      [[ -z "$prop" ]] && continue
      printf '/properties/%s\n' "$(json_ptr_escape "$prop")" >>"$tmp_ptrs"
    done < <(jq -r --arg n "$name" '
      .components.schemas[$n].properties // {} | keys[:2][]
    ' "$spec")

    jq -r --arg n "$name" '
      .components.schemas[$n] as $s |
      (if ($s.allOf | type) == "array" then "/allOf/0" else empty end),
      (if ($s.oneOf | type) == "array" then "/oneOf/0" else empty end),
      (if ($s.anyOf | type) == "array" then "/anyOf/0" else empty end)
    ' "$spec" >>"$tmp_ptrs"

    local ptr
    while IFS= read -r ptr || [[ -n "$ptr" ]]; do
      [[ -z "$ptr" ]] && continue
      add_case stream "" -- "$spec" --schema-field "${name}#${ptr}"
      add_case stream "" -- "$spec" --schema-field "${name}#${ptr}" --schema-depth 0
      add_case stream "" -- "$spec" --schema-field "${name}#${ptr}" --schema-depth 1
    done <"$tmp_ptrs"
    rm -f "$tmp_ptrs"
  done <"$tmp_names"

  add_case stream "" -- "$spec" --schema "DefinitelyNotASchema_XYZ"
  add_case stream "" -- "$spec" --schema-field "${first_schema}#/properties/no_such_field_zzz"
  rm -f "$tmp_names"
}

SCHEMA_SELECTION_SPEC="$REPO_ROOT/tests/fixtures/schema_selection_oas3.json"
SCHEMA_REFS_SPEC="$REPO_ROOT/tests/fixtures/schema_refs_oas3.json"
COMPOSITIONS_SPEC="$REPO_ROOT/scripts/compare-fixtures/compositions_oas3.json"

add_schema_selection_cases "$SCHEMA_SELECTION_SPEC"
add_schema_selection_cases "$SCHEMA_REFS_SPEC"
add_schema_selection_cases "$COMPOSITIONS_SPEC"

# Explicit pointers verified against fixture JSON (not guessed).
add_explicit_schema_fields "$SCHEMA_SELECTION_SPEC" \
  "Root#/properties/map/additionalProperties" \
  "Root#/properties/selected/items" \
  "Node#/properties/next/properties/next" \
  "Root#/properties/choice/oneOf/1" \
  "Root#/properties/deep/properties/hop/properties/tail/properties/stop"

add_explicit_schema_fields "$SCHEMA_REFS_SPEC" \
  "CreatePetRequest#/properties/variant/oneOf/1" \
  "Pet#/allOf/1/properties/friends/items" \
  "Node#/properties/next/properties/next" \
  "Pet#/allOf/1/properties/id" \
  "CreatePetRequest#/properties/category"

add_explicit_schema_fields "$COMPOSITIONS_SPEC" \
  "AnyShape#/anyOf/0" \
  "AnyShape#/anyOf/1" \
  "OneShape#/oneOf/0" \
  "OneShape#/oneOf/1" \
  "AllShape#/allOf/0" \
  "AllShape#/allOf/1" \
  "AllShape#/allOf/1/properties/extra" \
  "AllShape#/allOf/1/properties/nested" \
  "AllShape#/allOf/1/properties/nested/anyOf/0" \
  "AllShape#/allOf/1/properties/nested/anyOf/1" \
  "KeyedBag#/properties/entries/additionalProperties" \
  "RecursiveViaAny#/anyOf/0" \
  "RecursiveViaAny#/anyOf/1" \
  "RecursiveViaAny#/anyOf/1/properties/child" \
  "RefMember#/properties/label"

# --- operation selectors ---------------------------------------------------

OP_SPEC="$REPO_ROOT/tests/fixtures/operation_select_oas3.json"
if [[ -f "$OP_SPEC" ]]; then
  add_case stream "" -- "$OP_SPEC" --operation "GET /users"
  add_case stream "" -- "$OP_SPEC" --operation "GET /users/{id}"
  add_case stream "" -- "$OP_SPEC" --operation "POST /users"
  add_case stream "" -- "$OP_SPEC" --operation-id "listUsers"
  add_case stream "" -- "$OP_SPEC" --operation-id "getUser"
  add_case stream "" -- "$OP_SPEC" --operation-id "adminListUsers"
  add_case stream "" -- "$OP_SPEC" --operation "GET /no/such/path"
  add_case stream "" -- "$OP_SPEC" --operation-id "noSuchOperationId"
fi

# --- tree outputs ----------------------------------------------------------

add_tree_cases() {
  local spec="$1"
  local mode
  for mode in service tag endpoint; do
    add_case tree "split-${mode}" -- "$spec" --split "$mode" -o OUT
    add_case tree "split-${mode}-full" -- \
      "$spec" --split "$mode" --detail full --include-schemas -o OUT
  done
  add_case tree "skill" -- "$spec" --output-mode skill -o OUT
  add_case tree "skill-full" -- \
    "$spec" --output-mode skill --detail full --include-schemas -o OUT
}

if [[ ${#TREE_FIXTURES[@]} -gt 0 ]]; then
  for spec in "${TREE_FIXTURES[@]}"; do
    add_tree_cases "$spec"
  done
fi

# --- diff subcommand -------------------------------------------------------

add_diff_pair() {
  local old="$1"
  local new="$2"
  if [[ ! -f "$old" || ! -f "$new" ]]; then
    return 0
  fi
  add_case stream "" -- diff "$old" "$new"
  add_case stream "" -- diff "$old" "$new" --format markdown
  add_case stream "" -- diff "$old" "$new" --format json
  add_case stream "" -- diff "$old" "$new" --report
  add_case stream "" -- diff "$old" "$new" --format json --report
  add_case stream "" -- diff "$old" "$new" --fail-on-breaking
  add_case stream "" -- diff "$old" "$new" --format json --fail-on-breaking
  add_case tree "diff-out.md" -- diff "$old" "$new" -o OUT
  add_case tree "diff-out.json" -- diff "$old" "$new" --format json -o OUT
  add_case stream "" -- diff "$old" "$old"
  add_case stream "" -- diff "$new" "$new"
  # Identical pair with --fail-on-breaking should exit 0 (no breaking changes).
  add_case stream "" -- diff "$old" "$old" --fail-on-breaking
  add_case stream "" -- diff "$new" "$new" --fail-on-breaking
}

add_diff_pair \
  "$REPO_ROOT/tests/fixtures/diff_old_oas2.json" \
  "$REPO_ROOT/tests/fixtures/diff_new_oas2.json"
add_diff_pair \
  "$REPO_ROOT/tests/fixtures/diff_old_oas3.json" \
  "$REPO_ROOT/tests/fixtures/diff_new_oas3.json"
add_diff_pair \
  "$REPO_ROOT/scripts/compare-fixtures/diff_compositions_old_oas3.json" \
  "$REPO_ROOT/scripts/compare-fixtures/diff_compositions_new_oas3.json"

# Parse-failure: invalid JSON staged identically for both sides.
add_case stream "" -- diff "$INVALID_JSON" "$REPO_ROOT/tests/fixtures/diff_old_oas3.json"
add_case stream "" -- diff "$REPO_ROOT/tests/fixtures/diff_old_oas3.json" "$INVALID_JSON"
add_case stream "" -- diff "$WORK/does-not-exist.json" "$REPO_ROOT/tests/fixtures/diff_old_oas3.json"

# OAS2 vs OAS3 and JSON vs YAML of the same schema-refs content.
add_diff_pair \
  "$REPO_ROOT/tests/fixtures/petstore_oas2.json" \
  "$REPO_ROOT/tests/fixtures/petstore_oas3.json"
add_diff_pair \
  "$REPO_ROOT/tests/fixtures/schema_refs_oas3.json" \
  "$REPO_ROOT/tests/fixtures/schema_refs_oas3.yaml"

# --- completions -----------------------------------------------------------

for shell in bash elvish fish powershell zsh; do
  add_case stream "" -- completions "$shell"
done

# --- optional test-list check ----------------------------------------------

check_tests() {
  local before="/tmp/vimanam-tests-before.txt"
  if [[ ! -f "$before" ]]; then
    echo "CHECK_TESTS: missing $before" >&2
    return 1
  fi
  local now_raw now_norm before_filt before_norm list_err
  now_raw="$(mktemp)"
  now_norm="$(mktemp)"
  before_filt="$(mktemp)"
  before_norm="$(mktemp)"
  list_err="$(mktemp)"

  set +e
  cargo test -- --list >"$now_raw" 2>"$list_err"
  local list_ec=$?
  set -e
  if [[ "$list_ec" -ne 0 ]]; then
    echo "CHECK_TESTS: cargo test -- --list failed (exit $list_ec)" >&2
    cat "$list_err" >&2
    rm -f "$now_raw" "$now_norm" "$before_filt" "$before_norm" "$list_err"
    return 1
  fi
  rm -f "$list_err"

  local now_filt before_tmp
  now_filt="$(mktemp)"
  before_tmp="$(mktemp)"
  grep ': test$' "$now_raw" >"$now_filt" || true
  grep ': test$' "$before" >"$before_tmp" || true
  if [[ ! -s "$now_filt" ]]; then
    echo "CHECK_TESTS: cargo test -- --list yielded zero tests" >&2
    rm -f "$now_raw" "$now_norm" "$before_filt" "$before_norm" "$now_filt" "$before_tmp"
    return 1
  fi
  mv "$now_filt" "$now_raw"
  mv "$before_tmp" "$before_filt"

  normalize_test_list() {
    local src="$1" dst="$2"
    local line base
    : >"$dst"
    while IFS= read -r line || [[ -n "$line" ]]; do
      [[ -z "$line" ]] && continue
      case "$line" in
        *::*) base="${line##*::}" ;;
        *) base="$line" ;;
      esac
      printf '%s\n' "$base"
    done <"$src" | LC_ALL=C sort >"$dst"
  }

  normalize_test_list "$now_raw" "$now_norm"
  normalize_test_list "$before_filt" "$before_norm"

  echo "CHECK_TESTS: comparing normalised test names (multiset)"
  local missing new_names
  missing="$(comm -23 "$before_norm" "$now_norm" || true)"
  new_names="$(comm -13 "$before_norm" "$now_norm" || true)"
  local failed=0
  if [[ -n "$missing" ]]; then
    failed=1
    echo "CHECK_TESTS: missing (in baseline list, not in current):"
    printf '%s\n' "$missing"
  fi
  if [[ -n "$new_names" ]]; then
    failed=1
    echo "CHECK_TESTS: new (in current list, not in baseline):"
    printf '%s\n' "$new_names"
  fi
  if [[ "$failed" -eq 0 ]]; then
    echo "CHECK_TESTS: normalised test names identical"
  else
    echo "CHECK_TESTS: test list differs"
  fi

  local dups
  dups="$(uniq -d "$now_norm" || true)"
  if [[ -n "$dups" ]]; then
    echo "CHECK_TESTS: duplicate normalised names in current list:"
    printf '%s\n' "$dups"
  fi
  dups="$(uniq -d "$before_norm" || true)"
  if [[ -n "$dups" ]]; then
    echo "CHECK_TESTS: duplicate normalised names in baseline list:"
    printf '%s\n' "$dups"
  fi

  rm -f "$now_raw" "$now_norm" "$before_filt" "$before_norm"
  return "$failed"
}

if [[ "${CHECK_TESTS:-0}" == "1" ]]; then
  if ! check_tests; then
    TESTS_FAILED=1
  fi
fi

# --- run -------------------------------------------------------------------

echo "Work dir: $WORK"
echo "Cases: $CASE_NUM (manifest: $MANIFEST)"
echo "Baseline: $BASELINE_BIN"
echo "Candidate: $CANDIDATE_BIN"

run_all_cases

echo "${TOTAL_COUNT} cases, ${DIFFER_COUNT} differ"
if [[ "$TESTS_FAILED" -ne 0 ]]; then
  echo "CHECK_TESTS failed" >&2
fi
if [[ "$DIFFER_COUNT" -eq 0 && "$TESTS_FAILED" -eq 0 ]]; then
  exit 0
fi
exit 1
