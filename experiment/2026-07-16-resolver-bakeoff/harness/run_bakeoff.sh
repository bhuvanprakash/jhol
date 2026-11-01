#!/usr/bin/env bash
# Resolver bakeoff: dual_proxy vs pulse_proxy vs bun. Original tree untouched.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPO="$(cd "$ROOT/../.." && pwd)"
JHOL="${JHOL_BIN:-$REPO/target/release/jhol}"
BUN="${BUN_BIN:-$(command -v bun)}"
RESULTS="$ROOT/results"
TS="$(date -u +%Y%m%dT%H%M%SZ)"
TIMEOUT_SECS="${BAKEOFF_TIMEOUT_SECS:-90}"
FIXTURES=(express-app overrides-app peer-deps-app)
RAW="$RESULTS/raw-$TS.jsonl"
MICRO="$RESULTS/microbench-$TS.json"
OUT_JSON="$RESULTS/bakeoff-$TS.json"
OUT_MD="$RESULTS/bakeoff-$TS.md"

mkdir -p "$RESULTS" "$ROOT/work"
: >"$RAW"

if [[ ! -x "$JHOL" ]]; then echo "missing jhol: $JHOL" >&2; exit 1; fi
if [[ -z "$BUN" || ! -x "$BUN" ]]; then echo "missing bun" >&2; exit 1; fi

run_timed() {
  local label="$1"; shift
  local start end rc
  start=$(python3 -c 'import time; print(time.perf_counter())')
  set +e
  "$@" >"$RESULTS/${label}.stdout" 2>"$RESULTS/${label}.stderr"
  rc=$?
  set -e
  end=$(python3 -c 'import time; print(time.perf_counter())')
  python3 -c "print(f'{float('$end')-float('$start'):.6f} $rc')"
}

prepare_work() {
  local name="$1"
  local dest="$ROOT/work/$name"
  rm -rf "$dest"
  cp -R "$ROOT/fixtures/$name" "$dest"
  rm -rf "$dest/node_modules" "$dest/bun.lock" "$dest/bun.lockb" \
    "$dest/package-lock.json" "$dest/yarn.lock" 2>/dev/null || true
  echo "$dest"
}

record() {
  local fixture="$1" arm="$2" secs="$3" rc="$4" cmd="$5"
  python3 -c "import json; print(json.dumps({'fixture':'$fixture','arm':'$arm','seconds':float('$secs'),'exit':int('$rc'),'cmd':'''$cmd'''}))" >>"$RAW"
}

echo "== microbench =="
python3 "$ROOT/harness/topo_vs_pulse_microbench.py" --out "$MICRO"

echo "== bakeoff timeout=${TIMEOUT_SECS}s =="
for fix in "${FIXTURES[@]}"; do
  echo "-- $fix --"
  work=$(prepare_work "$fix")
  read -r secs rc < <(run_timed "dual_${fix}" env JHOL_NO_SPLASH=1 \
    perl -e "alarm $TIMEOUT_SECS; exec @ARGV" \
    "$JHOL" jagr --manifest "$work/package.json" --run-all-layers --quiet)
  echo "dual_proxy ${secs}s rc=$rc"
  record "$fix" dual_proxy "$secs" "$rc" "jhol jagr --run-all-layers"

  work=$(prepare_work "$fix")
  read -r secs rc < <(run_timed "pulse_${fix}" env JHOL_NO_SPLASH=1 \
    JHOL_JAGR_SKIP_LAYER2=1 JHOL_JAGR_SKIP_LAYER3=1 \
    perl -e "alarm $TIMEOUT_SECS; exec @ARGV" \
    "$JHOL" jagr --manifest "$work/package.json" --skip-layer2 --skip-layer3 --quiet)
  echo "pulse_proxy ${secs}s rc=$rc"
  record "$fix" pulse_proxy "$secs" "$rc" "jhol jagr --skip-layer2 --skip-layer3"

  work=$(prepare_work "$fix")
  read -r secs rc < <(run_timed "bun_${fix}" \
    perl -e "alarm $TIMEOUT_SECS; exec @ARGV" \
    "$BUN" install --cwd "$work")
  echo "bun ${secs}s rc=$rc"
  record "$fix" bun "$secs" "$rc" "bun install"
done

python3 - "$RAW" "$MICRO" "$OUT_JSON" "$OUT_MD" "$TS" "$TIMEOUT_SECS" "$JHOL" "$BUN" <<'PY'
import json, sys
from pathlib import Path
raw, micro_p, out_j, out_m, ts, timeout, jhol, bun = sys.argv[1:9]
runs = [json.loads(l) for l in Path(raw).read_text().splitlines() if l.strip()]
micro = json.loads(Path(micro_p).read_text())
note = (
  "dual_proxy = jhol jagr --run-all-layers (topology+OL3P). "
  "pulse_proxy = skip L2/L3. Dual-Geometry Loom not implemented; this is best available proxy."
)
data = {
  "timestamp": ts,
  "timeout_secs": int(timeout),
  "jhol": jhol,
  "bun": bun,
  "note": note,
  "runs": runs,
  "microbench": micro,
}
Path(out_j).write_text(json.dumps(data, indent=2) + "\n")
lines = [
  f"# Bakeoff {ts}", "", note, "", f"Timeout: {timeout}s", "",
  "| Fixture | Arm | Seconds | Exit |",
  "|---------|-----|--------:|-----:|",
]
for r in runs:
  lines.append(f"| {r['fixture']} | {r['arm']} | {r['seconds']:.3f} | {r['exit']} |")
lines += ["", "## Microbench", "", "### Dense GF(2) elim (cubic proxy)", ""]
for row in micro["dual_geometry_proxy_cubic"]:
  lines.append(f"- n={row['n']}: {row['seconds_median']*1000:.2f} ms")
lines += ["", "### Bitset force-wave (pulse)", ""]
for row in micro["pulse_force_wave"]:
  lines.append(f"- pkgs={row['packages']} edges={row['edges']}: {row['seconds_median']*1000:.2f} ms")
Path(out_m).write_text("\n".join(lines) + "\n")
print("wrote", out_j)
print("wrote", out_m)
PY

echo "DONE $OUT_JSON"
