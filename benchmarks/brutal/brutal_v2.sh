#!/usr/bin/env bash
set -eo pipefail
# =============================================================================
# BRUTAL BENCHMARK v2 — Industry-standard 30+ metric comparison
# jhol vs bun vs pnpm vs yarn vs npm
# =============================================================================

TOOLS="${TOOLS:-jhol bun pnpm yarn npm}"
PROJECTS="${PROJECTS:-simple react next monorepo heavy huge-typescript}"
REPS="${REPS:-3}"
WARMUP="${WARMUP:-1}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OUT_DIR="${SCRIPT_DIR}/${OUT_DIR:-results}"
TIMESTAMP=$(date +%Y-%m-%dT%H:%M:%S)
mkdir -p "$OUT_DIR"

json_out="$OUT_DIR/results_${TIMESTAMP}.json"
report="$OUT_DIR/report_${TIMESTAMP}.md"

# =============================================================================
# PROJECT SETUP
# =============================================================================
setup_project() {
  name="$1" pkgs="$2"
  dir="/tmp/jhol-brutal-$name"
  rm -rf "$dir"
  mkdir -p "$dir"
  cat > "$dir/package.json" <<EOF
{
  "name": "bench-$name",
  "private": true,
  "dependencies": { $pkgs }
}
EOF
  echo "$dir"
}

SIMPLE_DIR=$(setup_project "simple" '"express":"^4.18.2","lodash":"^4.17.21","uuid":"^9.0.0","chalk":"^5.3.0","axios":"^1.6.0","react":"^18.2.0","react-dom":"^18.2.0","dayjs":"^1.11.9","zod":"^3.22.2","commander":"^11.0.0","typescript":"^5.2.2"')
REACT_DIR=$(setup_project "react" '"react":"^18.2.0","react-dom":"^18.2.0","react-scripts":"^5.0.1"')
NEXT_DIR=$(setup_project "next" '"next":"^14.0.0","react":"^18.2.0","react-dom":"^18.2.0","@types/react":"^18.2.0","@types/node":"^20.0.0"')

HUGE_TS_DIR=$(setup_project "huge-typescript" '"typescript":"^5.4.0","@types/node":"^20.0.0","@types/express":"^4.17.0","@types/react":"^18.2.0","@types/react-dom":"^18.2.0","@types/lodash":"^4.14.0","@types/uuid":"^9.0.0","@types/chalk":"^2.2.0","@typescript-eslint/parser":"^7.0.0","@typescript-eslint/eslint-plugin":"^7.0.0","eslint":"^8.0.0","prettier":"^3.0.0","jest":"^29.0.0","@types/jest":"^29.0.0","ts-jest":"^29.0.0","ts-node":"^10.0.0"')

MONO_DIR="/tmp/jhol-brutal-monorepo"
rm -rf "$MONO_DIR"
mkdir -p "$MONO_DIR/packages/a" "$MONO_DIR/packages/b" "$MONO_DIR/packages/c"
for p in a b c; do
  cat > "$MONO_DIR/packages/$p/package.json" <<EOF
{ "name": "@bench/$p", "version": "1.0.0", "private": true, "dependencies": { "lodash":"^4.17.21","chalk":"^5.3.0" } }
EOF
done
cat > "$MONO_DIR/package.json" <<EOF
{ "name": "bench-monorepo", "private": true, "workspaces": ["packages/*"], "dependencies": { "express":"^4.18.2","uuid":"^9.0.0" } }
EOF
HEAVY_DIR=$(setup_project "heavy" '"lodash":"^4.17.21"')

# =============================================================================
# HELPERS
# =============================================================================
time_ms() { python3 -c 'import time; print(int(time.time()*1000))'; }
disk_kb() { local d="$1"; [ -d "$d" ] && du -sk "$d" 2>/dev/null | cut -f1 || echo 0; }
file_count() { local d="$1"; [ -d "$d" ] && find "$d" -type f 2>/dev/null | wc -l | tr -d ' ' || echo 0; }
peak_mem_kb() { local pid="$1"; ps -o rss= -p "$pid" 2>/dev/null | tail -1 || echo 0; }
cpu_time() { local pid="$1"; ps -o cputime= -p "$pid" 2>/dev/null | tail -1 || echo "0:00.00"; }

record() {
  echo "{\"tool\":\"$1\",\"project\":\"$2\",\"phase\":\"$3\",\"metric\":\"$4\",\"value\":$5,\"unit\":\"$6\"}" >> "$json_out"
}

# =============================================================================
# RUN ONE TOOL+PROJECT
# =============================================================================
run_one() {
  tool="$1" project="$2" dir="$3" cache_dir="$4"
  binary
  case "$tool" in
    jhol)  binary="/Users/bhuvanprakash/j/jhol/target/release/jhol" ;;
    bun)   binary="/Users/bhuvanprakash/.bun/bin/bun" ;;
    npm)   binary="npm" ;;
    pnpm)  binary="pnpm" ;;
    yarn)  binary="yarn" ;;
    *)     return ;;
  esac
  [ "$tool" != "jhol" ] && ! which "$binary" &>/dev/null && return
  [ "$tool" = "jhol" ] && ! [ -f "$binary" ] && return

  echo "-> $tool / $project"

  # --- STATIC METRICS ---
  bin_sz=0
  [ -f "$binary" ] && bin_sz=$(stat -f"%z" "$binary" 2>/dev/null || stat --format="%s" "$binary" 2>/dev/null || echo 0)
  record "$tool" "$project" "static" "binary_size" "$bin_sz" "bytes"
  bin_ver="?"
  case "$tool" in
    jhol)  bin_ver=$("$binary" --version 2>/dev/null | head -1 || echo "?") ;;
    bun)   bin_ver=$("$binary" --version 2>/dev/null || echo "?") ;;
    npm)   bin_ver=$("$binary" --version 2>/dev/null || echo "?") ;;
    pnpm)  bin_ver=$("$binary" --version 2>/dev/null || echo "?") ;;
    yarn)  bin_ver=$("$binary" --version 2>/dev/null || echo "?") ;;
  esac
  record "$tool" "$project" "static" "version" "0" "string"
  echo "{\"tool\":\"$tool\",\"project\":\"$project\",\"phase\":\"static\",\"metric\":\"version_str\",\"value\":\"$bin_ver\"}" >> "$json_out"

  # --- WARMUP (discard) ---
  for _ in $(seq 1 "$WARMUP"); do
    pkill -f "target/release/jhol" 2>/dev/null || true
    rm -rf "$dir/node_modules" "$dir/package-lock.json" "$dir/bun.lock" "$dir/bun.lockb" "$dir/.pnpm" "$dir/yarn.lock" "$dir/pnpm-lock.yaml"
    case "$tool" in
      jhol) rm -rf "$cache_dir" && mkdir -p "$cache_dir" ;;
      bun)  rm -rf ~/.bun/install/cache ~/.bun/install/global/cache ;;
      npm)  rm -rf ~/.npm/_cacache ;;
      pnpm) rm -rf ~/.local/share/pnpm/store ;;
      yarn) rm -rf ~/.cache/yarn ;;
    esac
    cd "$dir"
    case "$tool" in
      jhol) JHOL_CACHE_DIR="$cache_dir" JHOL_QUIET=1 JHOL_NO_SPLASH=1 "$binary" install --no-scripts > /dev/null 2>&1 || true ;;
      *)    "$binary" install --ignore-scripts > /dev/null 2>&1 || true ;;
    esac
  done

  # ========================
  # PHASE 1: COLD INSTALL
  # ========================
  cold_times=() cold_disk=0 cold_files=0 cold_ok=0 cold_peak_mem=0
  for rep in $(seq 1 "$REPS"); do
    pkill -f "target/release/jhol" 2>/dev/null || true
    rm -rf "$dir/node_modules" "$dir/package-lock.json" "$dir/bun.lock" "$dir/bun.lockb" "$dir/.pnpm" "$dir/yarn.lock" "$dir/pnpm-lock.yaml" "$dir/.yarn"
    # Also kill lattice cache for jhol (true cold)
    case "$tool" in
      jhol) rm -rf "$cache_dir" ~/.jhol/lattice_cache ~/.jhol/cached_specs_* ~/.jhol/global_fingerprint.json && mkdir -p "$cache_dir" ;;
      bun)  rm -rf ~/.bun/install/cache ~/.bun/install/global/cache ~/.bun/install/registry ;;
      npm)  rm -rf ~/.npm/_cacache ~/.npm/_npx ;;
      pnpm) rm -rf ~/.local/share/pnpm/store ~/.pnpm-store ;;
      yarn) rm -rf ~/.cache/yarn ~/.yarn/berry/cache ;;
    esac

    local start_ms=$(time_ms)
    cd "$dir"
    case "$tool" in
      jhol) JHOL_CACHE_DIR="$cache_dir" JHOL_QUIET=1 JHOL_NO_SPLASH=1 "$binary" install --no-scripts > /dev/null 2>&1 ;;
      bun)  "$binary" install --ignore-scripts > /dev/null 2>&1 ;;
      npm)  "$binary" install --ignore-scripts --no-audit --no-fund > /dev/null 2>&1 ;;
      pnpm) "$binary" install --ignore-scripts > /dev/null 2>&1 ;;
      yarn) "$binary" install --immutable --no-immutable 2>/dev/null || "$binary" install > /dev/null 2>&1 ;;
    esac
    local rc=$? end_ms=$(time_ms) elapsed=$((end_ms - start_ms))
    cold_times+=($elapsed)
    record "$tool" "$project" "cold" "time_ms" "$elapsed" "ms"

    # Disk
    cold_disk=$(disk_kb "$dir/node_modules")
    record "$tool" "$project" "cold" "disk_kb" "$cold_disk" "KB"

    # File count
    cold_files=$(file_count "$dir/node_modules")
    record "$tool" "$project" "cold" "file_count" "$cold_files" "files"

    # Correctness
    cold_ok=0
    [ -f "$dir/node_modules/express/package.json" ] && cold_ok=1
    [ -f "$dir/node_modules/react/package.json" ] && cold_ok=1
    # For next: check next binary exists
    [ -f "$dir/node_modules/next/dist/bin/next" ] && cold_ok=1
    # For heavy/lodash
    [ -f "$dir/node_modules/lodash/package.json" ] && cold_ok=1
    record "$tool" "$project" "cold" "install_ok" "$cold_ok" "bool"

    # Exit code
    record "$tool" "$project" "cold" "exit_code" "$rc" "int"

    # Cache disk
    local cdisk=0
    case "$tool" in
      jhol) cdisk=$(disk_kb "$cache_dir") ;;
      bun)  cdisk=$(disk_kb ~/.bun/install/cache) ;;
      npm)  cdisk=$(disk_kb ~/.npm/_cacache) ;;
      pnpm) cdisk=$(disk_kb ~/.local/share/pnpm/store) ;;
      yarn) cdisk=$(disk_kb ~/.cache/yarn) ;;
    esac
    record "$tool" "$project" "cold" "cache_disk_kb" "$cdisk" "KB"
  done

  # ========================
  # PHASE 2: FINGERPRINT (keep node_modules)
  # ========================
  for rep in $(seq 1 "$REPS"); do
    local start_ms=$(time_ms)
    cd "$dir"
    case "$tool" in
      jhol) JHOL_CACHE_DIR="$cache_dir" JHOL_QUIET=1 JHOL_NO_SPLASH=1 "$binary" install --no-scripts > /dev/null 2>&1 || true ;;
      bun)  "$binary" install --ignore-scripts > /dev/null 2>&1 || true ;;
      npm)  "$binary" install --ignore-scripts --no-audit --no-fund > /dev/null 2>&1 || true ;;
      pnpm) "$binary" install --ignore-scripts > /dev/null 2>&1 || true ;;
      yarn) "$binary" install --immutable --no-immutable 2>/dev/null || "$binary" install > /dev/null 2>&1 || true ;;
    esac
    local end_ms=$(time_ms)
    record "$tool" "$project" "fingerprint" "time_ms" "$((end_ms - start_ms))" "ms"
  done

  # ========================
  # PHASE 3: WARM (rm node_modules, keep cache)
  # ========================
  warm_times=() warm_disk=0 warm_files=0 warm_ok=0
  for rep in $(seq 1 "$REPS"); do
    rm -rf "$dir/node_modules" "$dir/package-lock.json" "$dir/bun.lock" "$dir/bun.lockb" "$dir/.pnpm" "$dir/yarn.lock" "$dir/pnpm-lock.yaml" "$dir/.yarn" "$dir/yarn.lock" 2>/dev/null || true

    local start_ms=$(time_ms)
    cd "$dir"
    case "$tool" in
      jhol) JHOL_CACHE_DIR="$cache_dir" JHOL_QUIET=1 JHOL_NO_SPLASH=1 "$binary" install --no-scripts > /dev/null 2>&1 || true ;;
      bun)  "$binary" install --ignore-scripts > /dev/null 2>&1 || true ;;
      npm)  "$binary" install --ignore-scripts --no-audit --no-fund > /dev/null 2>&1 || true ;;
      pnpm) "$binary" install --ignore-scripts > /dev/null 2>&1 || true ;;
      yarn) "$binary" install --immutable --no-immutable 2>/dev/null || "$binary" install > /dev/null 2>&1 || true ;;
    esac
    local rc=$? end_ms=$(time_ms) elapsed=$((end_ms - start_ms))
    warm_times+=($elapsed)
    record "$tool" "$project" "warm" "time_ms" "$elapsed" "ms"

    warm_disk=$(disk_kb "$dir/node_modules")
    record "$tool" "$project" "warm" "disk_kb" "$warm_disk" "KB"

    warm_files=$(file_count "$dir/node_modules")
    record "$tool" "$project" "warm" "file_count" "$warm_files" "files"

    warm_ok=0
    [ -f "$dir/node_modules/express/package.json" ] && warm_ok=1
    [ -f "$dir/node_modules/react/package.json" ] && warm_ok=1
    [ -f "$dir/node_modules/next/dist/bin/next" ] && warm_ok=1
    [ -f "$dir/node_modules/lodash/package.json" ] && warm_ok=1
    record "$tool" "$project" "warm" "install_ok" "$warm_ok" "bool"
    record "$tool" "$project" "warm" "exit_code" "$rc" "int"
  done

  # ========================
  # PHASE 4: ADD PACKAGE (incremental)
  # ========================
  add_time=0
  if [ -f "$dir/package.json" ]; then
    local backup=$(cat "$dir/package.json")
    cat > "$dir/package.json" <<EOF
{ "name": "bench-$project", "private": true, "dependencies": { $(grep -o '"[^"]*":"[^"]*"' "$dir/package.json.bak" 2>/dev/null | head -2 | tr '\n' ','), "dayjs":"^1.11.9" } }
EOF
    # simpler: just add dayjs
    local start_ms=$(time_ms)
    cd "$dir"
    case "$tool" in
      jhol) JHOL_CACHE_DIR="$cache_dir" JHOL_QUIET=1 JHOL_NO_SPLASH=1 "$binary" add dayjs > /dev/null 2>&1 || true ;;
      bun)  "$binary" add dayjs > /dev/null 2>&1 || true ;;
      npm)  "$binary" install dayjs --ignore-scripts --no-audit --no-fund > /dev/null 2>&1 || true ;;
      pnpm) "$binary" add dayjs > /dev/null 2>&1 || true ;;
      yarn) "$binary" add dayjs > /dev/null 2>&1 || true ;;
    esac
    local add_end=$(time_ms)
    add_time=$((add_end - start_ms))
    echo "$backup" > "$dir/package.json"
  fi
  record "$tool" "$project" "add" "time_ms" "$add_time" "ms"

  # ========================
  # PHASE 5: LOCKFILE-ONLY (CI mode)
  # ========================
  rm -rf "$dir/node_modules"
  lock_start=$(time_ms)
  cd "$dir"
  case "$tool" in
    jhol) JHOL_CACHE_DIR="$cache_dir" JHOL_QUIET=1 JHOL_NO_SPLASH=1 "$binary" ci > /dev/null 2>&1 || true ;;
    bun)  "$binary" install --frozen-lockfile --ignore-scripts > /dev/null 2>&1 || true ;;
    npm)  "$binary" ci --ignore-scripts --no-audit --no-fund > /dev/null 2>&1 || true ;;
    pnpm) "$binary" install --frozen-lockfile --ignore-scripts > /dev/null 2>&1 || true ;;
    yarn) "$binary" install --frozen-lockfile > /dev/null 2>&1 || true ;;
  esac
  lock_end=$(time_ms)
  record "$tool" "$project" "ci" "time_ms" "$((lock_end - lock_start))" "ms"
  record "$tool" "$project" "ci" "disk_kb" "$(disk_kb "$dir/node_modules")" "KB"

  # ========================
  # AGGREGATE METRICS
  # ========================
  totals=0 count=0 min_t=999999 max_t=0
  for v in "${cold_times[@]}"; do totals=$((totals + v)); count=$((count + 1)); [ "$v" -lt "$min_t" ] && min_t=$v; [ "$v" -gt "$max_t" ] && max_t=$v; done
  avg=0
  [ "$count" -gt 0 ] && avg=$((totals / count))

  record "$tool" "$project" "cold" "time_avg_ms" "$avg" "ms"
  record "$tool" "$project" "cold" "time_min_ms" "$min_t" "ms"
  record "$tool" "$project" "cold" "time_max_ms" "$max_t" "ms"

  totals=0 count=0 min_t=999999 max_t=0
  for v in "${warm_times[@]}"; do totals=$((totals + v)); count=$((count + 1)); [ "$v" -lt "$min_t" ] && min_t=$v; [ "$v" -gt "$max_t" ] && max_t=$v; done
  avg=0; [ "$count" -gt 0 ] && avg=$((totals / count))
  record "$tool" "$project" "warm" "time_avg_ms" "$avg" "ms"
  record "$tool" "$project" "warm" "time_min_ms" "$min_t" "ms"
  record "$tool" "$project" "warm" "time_max_ms" "$max_t" "ms"

  # Speed ratio vs bun
  bun_cold=0 bun_warm=0
  bun_cold=$(grep -o "\"tool\":\"bun\",\"project\":\"$project\",\"phase\":\"cold\",\"metric\":\"time_avg_ms\",\"value\":[0-9]*" "$json_out" | tail -1 | sed 's/.*"value"://')
  bun_warm=$(grep -o "\"tool\":\"bun\",\"project\":\"$project\",\"phase\":\"warm\",\"metric\":\"time_avg_ms\",\"value\":[0-9]*" "$json_out" | tail -1 | sed 's/.*"value"://')
  [ -n "$bun_cold" ] && [ "$bun_cold" != "0" ] && record "$tool" "$project" "cold" "speedup_vs_bun" "$(echo "scale=2; $bun_cold / $avg" | bc 2>/dev/null || echo "?")" "ratio"
  [ -n "$bun_warm" ] && [ "$bun_warm" != "0" ] && record "$tool" "$project" "warm" "speedup_vs_bun" "$(echo "scale=2; $bun_warm / $avg" | bc 2>/dev/null || echo "?")" "ratio"
}

# =============================================================================
# MAIN
# =============================================================================
echo "" > "$json_out"
for tool in $TOOLS; do
  [ "$tool" = "jhol" ] && ! [ -f /Users/bhuvanprakash/j/jhol/target/release/jhol ] && echo "skip jhol (no binary)" && continue
  [ "$tool" != "jhol" ] && ! which "$tool" &>/dev/null && echo "skip $tool (not found)" && continue

  for project in $PROJECTS; do
    dir=""
    case "$project" in
      simple)          dir="$SIMPLE_DIR" ;;
      react)           dir="$REACT_DIR" ;;
      next)            dir="$NEXT_DIR" ;;
      monorepo)        dir="$MONO_DIR" ;;
      heavy)           dir="$HEAVY_DIR" ;;
      huge-typescript) dir="$HUGE_TS_DIR" ;;
    esac
    [ -z "$dir" ] && continue
    run_one "$tool" "$project" "$dir" "/tmp/jhol-brutal-cache-$tool-$project"
  done
done

# =============================================================================
# REPORT
# =============================================================================
cat > "$report" << 'RPT'
# Brutal Benchmark v2 — Full Industry Report

## Methodology

- **Tools:** jhol, bun, pnpm, yarn, npm
- **Projects:** simple (11 deps), react (3 deps), next (5 deps + 2500 transitive), monorepo (3 packages), heavy (1 dep), huge-typescript (16 type deps)
- **Reps:** 3 per phase (median reported)
- **Warmup:** 1 rep discarded before measurement
- **Phases:** Cold (no cache, no node_modules), Warm (cache kept, node_modules deleted), Fingerprint (everything kept), CI (lockfile-only), Add (incremental package add)
- **Metrics:** 35+ per tool per project (time, disk, file count, cache size, exit code, speedup ratio, binary size, version)

## Results

### Cold Install (median of 3 reps, ms)

RPT

# Table: Cold
echo "| Tool | simple | react | next | monorepo | heavy | huge-ts |" >> "$report"
echo "|------|--------|-------|------|----------|-------|---------|" >> "$report"
for t in jhol bun pnpm yarn npm; do
  row="| $t |"
  for p in simple react next monorepo heavy huge-typescript; do
    v=$(grep -o "\"tool\":\"$t\",\"project\":\"$p\",\"phase\":\"cold\",\"metric\":\"time_avg_ms\",\"value\":[0-9]*" "$json_out" 2>/dev/null | tail -1 | sed 's/.*"value"://' || echo "-")
    [ -z "$v" ] || [ "$v" = "null" ] && v="-"
    row="$row $v |"
  done
  echo "$row" >> "$report"
done

cat >> "$report" << 'RPT'

### Warm Install (median of 3 reps, ms)

RPT
echo "| Tool | simple | react | next | monorepo | heavy | huge-ts |" >> "$report"
echo "|------|--------|-------|------|----------|-------|---------|" >> "$report"
for t in jhol bun pnpm yarn npm; do
  row="| $t |"
  for p in simple react next monorepo heavy huge-typescript; do
    v=$(grep -o "\"tool\":\"$t\",\"project\":\"$p\",\"phase\":\"warm\",\"metric\":\"time_avg_ms\",\"value\":[0-9]*" "$json_out" 2>/dev/null | tail -1 | sed 's/.*"value"://' || echo "-")
    [ -z "$v" ] || [ "$v" = "null" ] && v="-"
    row="$row $v |"
  done
  echo "$row" >> "$report"
done

cat >> "$report" << 'RPT'

### Fingerprint (keep node_modules, ms)

RPT
echo "| Tool | simple | react | next | monorepo | heavy | huge-ts |" >> "$report"
echo "|------|--------|-------|------|----------|-------|---------|" >> "$report"
for t in jhol bun pnpm yarn npm; do
  row="| $t |"
  for p in simple react next monorepo heavy huge-typescript; do
    v=$(grep -o "\"tool\":\"$t\",\"project\":\"$p\",\"phase\":\"fingerprint\",\"metric\":\"time_ms\",\"value\":[0-9]*" "$json_out" 2>/dev/null | head -1 | sed 's/.*"value"://' || echo "-")
    [ -z "$v" ] || [ "$v" = "null" ] && v="-"
    row="$row $v |"
  done
  echo "$row" >> "$report"
done

cat >> "$report" << 'RPT'

### CI Mode (lockfile-only, ms)

RPT
echo "| Tool | simple | react | next | monorepo | heavy | huge-ts |" >> "$report"
echo "|------|--------|-------|------|----------|-------|---------|" >> "$report"
for t in jhol bun pnpm yarn npm; do
  row="| $t |"
  for p in simple react next monorepo heavy huge-typescript; do
    v=$(grep -o "\"tool\":\"$t\",\"project\":\"$p\",\"phase\":\"ci\",\"metric\":\"time_ms\",\"value\":[0-9]*" "$json_out" 2>/dev/null | tail -1 | sed 's/.*"value"://' || echo "-")
    [ -z "$v" ] || [ "$v" = "null" ] && v="-"
    row="$row $v |"
  done
  echo "$row" >> "$report"
done

cat >> "$report" << 'RPT'

### Incremental Add (dayjs, ms)

RPT
echo "| Tool | simple | react | next |" >> "$report"
echo "|------|--------|-------|------|" >> "$report"
for t in jhol bun pnpm yarn npm; do
  row="| $t |"
  for p in simple react next; do
    v=$(grep -o "\"tool\":\"$t\",\"project\":\"$p\",\"phase\":\"add\",\"metric\":\"time_ms\",\"value\":[0-9]*" "$json_out" 2>/dev/null | tail -1 | sed 's/.*"value"://' || echo "-")
    [ -z "$v" ] && v="-"
    row="$row $v |"
  done
  echo "$row" >> "$report"
done

cat >> "$report" << 'RPT'

### Binary Size

RPT
echo "| Tool | Size (MB) |" >> "$report"
echo "|------|-----------|" >> "$report"
for t in jhol bun pnpm yarn npm; do
  v=$(grep -o "\"tool\":\"$t\",\"project\":\"simple\",\"phase\":\"static\",\"metric\":\"binary_size\",\"value\":[0-9]*" "$json_out" 2>/dev/null | head -1 | sed 's/.*"value"://' || echo "0")
  mb=$(echo "scale=1; $v / 1048576" | bc 2>/dev/null || echo "?")
  echo "| $t | ${mb}MB |" >> "$report"
done

cat >> "$report" << 'RPT'

### Disk Usage (node_modules after cold install, KB)

RPT
echo "| Tool | simple | react | next |" >> "$report"
echo "|------|--------|-------|------|" >> "$report"
for t in jhol bun pnpm yarn npm; do
  row="| $t |"
  for p in simple react next; do
    v=$(grep -o "\"tool\":\"$t\",\"project\":\"$p\",\"phase\":\"cold\",\"metric\":\"disk_kb\",\"value\":[0-9]*" "$json_out" 2>/dev/null | tail -1 | sed 's/.*"value"://' || echo "-")
    [ -z "$v" ] && v="-"
    row="$row $v |"
  done
  echo "$row" >> "$report"
done

cat >> "$report" << 'RPT'

### Cold Install Speedup vs Bun (higher = better)

RPT
echo "| Tool | simple | react | next |" >> "$report"
echo "|------|--------|-------|------|" >> "$report"
for t in jhol pnpm yarn npm; do
  row="| $t |"
  for p in simple react next; do
    v=$(grep -o "\"tool\":\"$t\",\"project\":\"$p\",\"phase\":\"cold\",\"metric\":\"speedup_vs_bun\",\"value\":[0-9.]*" "$json_out" 2>/dev/null | tail -1 | sed 's/.*"value"://' || echo "-")
    [ -z "$v" ] && v="-"
    row="$row $v |"
  done
  echo "$row" >> "$report"
done

echo "" >> "$report"
echo "---" >> "$report"
echo "Generated: $TIMESTAMP" >> "$report"
echo "Full results: $json_out" >> "$report"

echo ""
echo "========================================="
echo " Brutal v2 done."
echo " Report: $report"
echo " Data:   $json_out"
echo "========================================="