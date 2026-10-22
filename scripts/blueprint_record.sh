#!/usr/bin/env bash
# Record every Phase A–D blueprint check as pass/fail (JSONL + latest markdown).
# Usage: scripts/blueprint_record.sh
# History: docs/architecture/blueprint-records/history.jsonl
# Latest:  docs/architecture/blueprint-records/LATEST.md
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
REC_DIR="$ROOT/docs/architecture/blueprint-records"
mkdir -p "$REC_DIR"
TS="$(date -u +%Y%m%dT%H%M%SZ)"
HOST="$(uname -s)-$(uname -m)"
RUN_ID="${TS}-${HOST}"
JSONL="$REC_DIR/history.jsonl"
LATEST="$REC_DIR/LATEST.md"
RUN_LOG="$REC_DIR/runs/${RUN_ID}.md"
mkdir -p "$REC_DIR/runs"

BIN="$ROOT/target/debug/jhol"
PASS=0
FAIL=0
SKIP=0
ROWS=()

record() {
  local phase="$1" id="$2" status="$3" ms="$4" detail="$5"
  local line
  line=$(
    JHOL_REC_PHASE="$phase" JHOL_REC_ID="$id" JHOL_REC_STATUS="$status" \
    JHOL_REC_MS="$ms" JHOL_REC_DETAIL="$detail" JHOL_REC_RUN="$RUN_ID" \
    JHOL_REC_TS="$TS" JHOL_REC_HOST="$HOST" python3 - <<'PY'
import json, os
print(json.dumps({
  "run_id": os.environ["JHOL_REC_RUN"],
  "ts": os.environ["JHOL_REC_TS"],
  "host": os.environ["JHOL_REC_HOST"],
  "phase": os.environ["JHOL_REC_PHASE"],
  "id": os.environ["JHOL_REC_ID"],
  "status": os.environ["JHOL_REC_STATUS"],
  "duration_ms": int(os.environ["JHOL_REC_MS"]),
  "detail": os.environ["JHOL_REC_DETAIL"].replace("\n", " ")[:500],
}, ensure_ascii=False))
PY
  )
  echo "$line" >>"$JSONL"
  local safe="${detail//|/\\|}"
  safe="${safe//$'\n'/ }"
  ROWS+=("| $phase | \`$id\` | **$status** | ${ms}ms | $safe |")
  case "$status" in
    PASS) PASS=$((PASS + 1)); echo "  PASS  [$phase] $id (${ms}ms)" ;;
    FAIL) FAIL=$((FAIL + 1)); echo "  FAIL  [$phase] $id — $detail" >&2 ;;
    SKIP) SKIP=$((SKIP + 1)); echo "  SKIP  [$phase] $id — $detail" ;;
  esac
}

run_check() {
  local phase="$1" id="$2" desc="$3"
  shift 3
  local start end ms out rc
  start=$(python3 -c 'import time; print(int(time.time()*1000))')
  set +e
  out=$("$@" 2>&1)
  rc=$?
  set -e
  end=$(python3 -c 'import time; print(int(time.time()*1000))')
  ms=$((end - start))
  if [[ $rc -eq 0 ]]; then
    record "$phase" "$id" "PASS" "$ms" "$desc"
  else
    local snippet
    snippet=$(printf '%s' "$out" | tr '\n' ' ' | head -c 240)
    record "$phase" "$id" "FAIL" "$ms" "$desc | exit=$rc | $snippet"
  fi
  return 0
}

echo "== blueprint record $RUN_ID =="

# --- build once ---
run_check "A" "build_bin" "cargo build --bin jhol" \
  cargo build -q --bin jhol

if [[ ! -x "$BIN" ]]; then
  record "A" "bin_exists" "FAIL" "0" "target/debug/jhol missing after build"
else
  record "A" "bin_exists" "PASS" "0" "target/debug/jhol"
fi

# --- Phase A ---
run_check "A" "foundation_lib_tests" "foundation crate --lib tests" \
  cargo test \
    -p jhol-graph -p jhol-adapters -p jhol-receipt -p jhol-policy \
    -p jhol-task -p jhol-env -p jhol-diff -p jhol-ai -p jhol-db \
    -p jhol-remote \
    --lib --quiet

run_check "A" "core_lib_tests" "jhol-core --lib" \
  cargo test -p jhol-core --lib --quiet

run_check "A" "compare_versions" "npm/Bun/Jhol correctness harness" \
  bash "$ROOT/tests/compatibility/compare_versions.sh"

# CLI help / dispatch
if [[ -x "$BIN" ]]; then
  HELP="$("$BIN" --help 2>&1 || true)"
  for cmd in plan diff sync snapshot restore db ai dev; do
    if grep -qE "^[[:space:]]+${cmd}[[:space:]]" <<<"$HELP"; then
      record "A" "cli_help_${cmd}" "PASS" "0" "listed in jhol --help"
    else
      record "A" "cli_help_${cmd}" "FAIL" "0" "missing from jhol --help"
    fi
  done
  DBHELP="$("$BIN" db --help 2>&1 || true)"
  if grep -q plan <<<"$DBHELP" && grep -q probe <<<"$DBHELP"; then
    record "A" "cli_db_subcommands" "PASS" "0" "db plan + probe in help"
  else
    record "A" "cli_db_subcommands" "FAIL" "0" "db help missing plan/probe"
  fi
  AIHELP="$("$BIN" ai --help 2>&1 || true)"
  if grep -q verify <<<"$AIHELP" && grep -q prepare <<<"$AIHELP"; then
    record "A" "cli_ai_subcommands" "PASS" "0" "ai verify + prepare in help"
  else
    record "A" "cli_ai_subcommands" "FAIL" "0" "ai help missing verify/prepare"
  fi
else
  record "A" "cli_help_all" "SKIP" "0" "no binary"
fi

# --- Phase B ---
EX="$ROOT/examples/blueprint"
export JHOL_BIN="$BIN"
if [[ -x "$BIN" ]]; then
  run_check "B" "plan_json" "jhol plan --json shows javascript" \
    bash -c '"$0" plan --path "$1" --json | grep -q javascript' "$BIN" "$EX"
  run_check "B" "sync_project_lock" "jhol sync writes jhol.project.lock" \
    bash -c '"$0" sync --path "$1" >/dev/null && test -f "$1/jhol.project.lock" && grep -q package-lock.json "$1/jhol.project.lock"' "$BIN" "$EX"
  run_check "B" "project_lock_format" "project.lock format_version 1" \
    grep -q '"format_version": 1' "$EX/jhol.project.lock"
else
  record "B" "plan_json" "SKIP" "0" "no binary"
fi

# --- Phase C ---
if [[ -x "$BIN" ]]; then
  run_check "C" "snapshot_create" "snapshot create returns id" \
    bash -c 'sid=$("$0" snapshot create --path "$1" --json | python3 -c "import sys,json; print(json.load(sys.stdin)[\"id\"])"); test -n "$sid"' "$BIN" "$EX"
  run_check "C" "restore_plan" "restore --plan ok" \
    bash -c 'sid=$("$0" snapshot create --path "$1" --json | python3 -c "import sys,json; print(json.load(sys.stdin)[\"id\"])"); "$0" restore "$sid" --plan --path "$1" >/dev/null' "$BIN" "$EX"
  run_check "C" "restore_databases_refused" "restore --databases must fail" \
    bash -c 'sid=$("$0" snapshot create --path "$1" --json | python3 -c "import sys,json; print(json.load(sys.stdin)[\"id\"])"); ! "$0" restore "$sid" --apply --databases --path "$1" >/dev/null 2>&1' "$BIN" "$EX"
  run_check "C" "db_plan" "db plan finds 001_init" \
    bash -c '"$0" db plan --path "$1" --json | grep -q 001_init' "$BIN" "$EX"
  run_check "C" "db_probe" "db probe json" \
    bash -c '"$0" db probe --json | grep -q database_url_set' "$BIN" "$EX"
  run_check "C" "ai_verify" "ai verify model pin" \
    bash -c '"$0" ai verify --path "$1" --manifest models/demo.jhol-model.json >/dev/null' "$BIN" "$EX"
  run_check "C" "ai_prepare" "ai prepare offline" \
    bash -c '"$0" ai prepare --path "$1" --manifest models/demo.jhol-model.json >/dev/null' "$BIN" "$EX"
  run_check "C" "dev_boot_order" "dev --json boot_order" \
    bash -c '"$0" dev --path "$1" --json | grep -q boot_order' "$BIN" "$EX"
  run_check "C" "dev_spawn_cleanup" "dev --spawn hold 1s" \
    bash -c '"$0" dev --path "$1" --spawn --hold-secs 1 >/dev/null' "$BIN" "$EX"
fi

# --- Phase D (stubs) ---
run_check "D" "remote_crate_tests" "jhol-remote --lib" \
  cargo test -p jhol-remote --lib --quiet
run_check "D" "remote_schema" "remote-cas-v1 schema file" \
  test -f "$ROOT/schemas/remote-cas-v1.schema.json"
if rg -n 'jhol-remote' "$ROOT/src/main.rs" >/dev/null 2>&1; then
  record "D" "remote_not_in_cli" "FAIL" "0" "jhol-remote imported in main.rs (should stay stub/opt-in)"
else
  record "D" "remote_not_in_cli" "PASS" "0" "CLI does not link jhol-remote"
fi

TOTAL=$((PASS + FAIL + SKIP))
STATUS="OK"
[[ $FAIL -gt 0 ]] && STATUS="FAILED"

{
  echo "# Blueprint record — $RUN_ID"
  echo
  echo "- **status:** $STATUS"
  echo "- **host:** $HOST"
  echo "- **utc:** $TS"
  echo "- **totals:** pass=$PASS fail=$FAIL skip=$SKIP total=$TOTAL"
  echo "- **history:** \`docs/architecture/blueprint-records/history.jsonl\`"
  echo
  echo "| Phase | Id | Status | Duration | Detail |"
  echo "|-------|----|--------|----------|--------|"
  for r in "${ROWS[@]}"; do echo "$r"; done
  echo
  echo "## How to re-run"
  echo
  echo '```sh'
  echo 'bash scripts/blueprint_record.sh'
  echo '```'
} | tee "$LATEST" >"$RUN_LOG"

echo
echo "blueprint_record: $STATUS (pass=$PASS fail=$FAIL skip=$SKIP)"
echo "wrote $LATEST"
echo "appended $JSONL"
[[ $FAIL -eq 0 ]]
