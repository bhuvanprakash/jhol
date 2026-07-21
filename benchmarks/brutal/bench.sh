#!/usr/bin/env bash
set -eo pipefail
# =============================================================================
# BRUTAL BENCHMARK — Industry-standard 30+ metric comparison
# jhol vs bun vs pnpm vs yarn vs npm
# 6 projects × 6 phases × 3 reps × 30+ metrics
# =============================================================================

TOOLS="${TOOLS:-jhol bun pnpm yarn npm}"
PROJECTS="${PROJECTS:-simple react next monorepo heavy huge-typescript}"
REPS="${REPS:-3}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OUT_DIR="${SCRIPT_DIR}/${OUT_DIR:-results}"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
mkdir -p "$OUT_DIR"

json_out="$OUT_DIR/results_${TIMESTAMP}.json"
report="$OUT_DIR/report_${TIMESTAMP}.md"

# =============================================================================
# HELPERS
# =============================================================================
ms() { python3 -c 'import time; print(int(time.time()*1000))' 2>/dev/null || echo 0; }
dkb() { local d="$1"; [ -d "$d" ] && du -sk "$d" 2>/dev/null | cut -f1 || echo 0; }
fcount() { local d="$1"; [ -d "$d" ] && find "$d" -type f 2>/dev/null | wc -l | tr -d ' ' || echo 0; }
dcount() { local d="$1"; [ -d "$d" ] && find "$d" -type d 2>/dev/null | wc -l | tr -d ' ' || echo 0; }
symcount() { local d="$1"; [ -d "$d" ] && find "$d" -type l 2>/dev/null | wc -l | tr -d ' ' || echo 0; }
rec() { echo "{\"tool\":\"$1\",\"project\":\"$2\",\"phase\":\"$3\",\"metric\":\"$4\",\"value\":$5,\"unit\":\"$6\"}" >> "$json_out"; }
median() { echo "$@" | tr ' ' '\n' | sort -n | awk '{a[NR]=$1} END{print a[int((NR+1)/2)]}'; }
avg() { local sum=0 c=0; for v in "$@"; do sum=$((sum+v)); c=$((c+1)); done; echo $((sum/c)); }

# =============================================================================
# PROJECT SETUP
# =============================================================================
setup() {
  local name="$1" pkgs="$2"
  local dir="/tmp/jhol-brutal-${name}-${TIMESTAMP}"
  rm -rf "$dir"
  mkdir -p "$dir"
  cat > "$dir/package.json" <<EOF
{"name":"bench-$name","private":true,"dependencies":{$pkgs}}
EOF
  echo "$dir"
}

SIMPLE_DIR=$(setup "simple" '"express":"^4.18.2","lodash":"^4.17.21","uuid":"^9.0.0","chalk":"^5.3.0","axios":"^1.6.0","react":"^18.2.0","react-dom":"^18.2.0","dayjs":"^1.11.9","zod":"^3.22.2","commander":"^11.0.0","typescript":"^5.2.2"')
REACT_DIR=$(setup "react" '"react":"^18.2.0","react-dom":"^18.2.0","react-scripts":"^5.0.1"')
NEXT_DIR=$(setup "next" '"next":"^14.0.0","react":"^18.2.0","react-dom":"^18.2.0","typescript":"^5.4.0","@types/react":"^18.2.0","@types/node":"^20.0.0"')

MONO_DIR="/tmp/jhol-brutal-monorepo-${TIMESTAMP}"
rm -rf "$MONO_DIR"; mkdir -p "$MONO_DIR/packages/a" "$MONO_DIR/packages/b" "$MONO_DIR/packages/c"
for p in a b c; do
  cat > "$MONO_DIR/packages/$p/package.json" <<EOF
{"name":"@bench/$p","version":"1.0.0","private":true,"dependencies":{"lodash":"^4.17.21","chalk":"^5.3.0"}}
EOF
done
cat > "$MONO_DIR/package.json" <<EOF
{"name":"bench-monorepo","private":true,"workspaces":["packages/*"],"dependencies":{"express":"^4.18.2","uuid":"^9.0.0"}}
EOF

HEAVY_DIR=$(setup "heavy" '"lodash":"^4.17.21"')
HUGE_TS_DIR=$(setup "huge-typescript" '"typescript":"^5.4.0","@types/node":"^20.0.0","@types/express":"^4.17.0","@types/react":"^18.2.0","@types/react-dom":"^18.2.0","@types/lodash":"^4.14.0","@types/uuid":"^9.0.0","@typescript-eslint/parser":"^7.0.0","@typescript-eslint/eslint-plugin":"^7.0.0","eslint":"^8.0.0","jest":"^29.0.0","@types/jest":"^29.0.0","ts-jest":"^29.0.0"')

# =============================================================================
# RUN ONE TOOL + PROJECT
# =============================================================================
run_one() {
  local tool="$1" project="$2" dir="$3" cache_dir="$4"
  local binary cmd_install cmd_add ci_cmd frozen_flag
  case "$tool" in
    jhol) binary="/Users/bhuvanprakash/j/jhol/target/release/jhol"; cmd_install="install --no-scripts"; cmd_add="add"; ci_cmd="ci"; frozen_flag="" ;;
    bun)  binary="$HOME/.bun/bin/bun"; cmd_install="install --ignore-scripts"; cmd_add="add"; ci_cmd="install --frozen-lockfile --ignore-scripts"; frozen_flag="" ;;
    npm)  binary="npm"; cmd_install="install --ignore-scripts --no-audit --no-fund"; cmd_add="install --ignore-scripts --no-audit --no-fund"; ci_cmd="ci --ignore-scripts --no-audit --no-fund"; frozen_flag="" ;;
    pnpm) binary="pnpm"; cmd_install="install --ignore-scripts"; cmd_add="add"; ci_cmd="install --frozen-lockfile --ignore-scripts"; frozen_flag="" ;;
    yarn) binary="yarn"; cmd_install="install"; cmd_add="add"; ci_cmd="install --frozen-lockfile"; frozen_flag="" ;;
    *) return ;;
  esac

  echo "  [$tool/$project]"

  # --- STATIC METRICS ---
  bin_sz=0; [ -f "$binary" ] && bin_sz=$(stat -f"%z" "$binary" 2>/dev/null || stat --format="%s" "$binary" 2>/dev/null || echo 0)
  rec "$tool" "$project" "static" "binary_size" "$bin_sz" "bytes"
  bin_ver="$("$binary" --version 2>/dev/null | head -1 || echo "?")"
  echo "{\"tool\":\"$tool\",\"project\":\"$project\",\"phase\":\"static\",\"metric\":\"version\",\"value\":\"$bin_ver\"}" >> "$json_out"

  # ========================
  # PHASE 1: COLD (no cache, no node_modules)
  # ========================
  cold_times=()
  for rep in $(seq 1 "$REPS"); do
    pkill -f "target/release/jhol" 2>/dev/null || true
    rm -rf "$dir/node_modules" "$dir/package-lock.json" "$dir/bun.lock" "$dir/bun.lockb" "$dir/.pnpm" "$dir/yarn.lock" "$dir/pnpm-lock.yaml" "$dir/.yarn" "$dir/yarn.lock" 2>/dev/null || true

    # Nuke all caches
    case "$tool" in
      jhol) rm -rf "$cache_dir" ~/.jhol 2>/dev/null; mkdir -p "$cache_dir" ;;
      bun)  rm -rf ~/.bun/install 2>/dev/null || true ;;
      npm)  rm -rf ~/.npm/_cacache 2>/dev/null || find ~/.npm/_cacache -depth -delete 2>/dev/null || true ;;
      pnpm) rm -rf ~/.local/share/pnpm 2>/dev/null || true ;;
      yarn) rm -rf ~/.cache/yarn ~/.yarn 2>/dev/null || true ;;
    esac

    s=$(ms); cd "$dir"
    case "$tool" in
      jhol) JHOL_CACHE_DIR="$cache_dir" JHOL_QUIET=1 JHOL_NO_SPLASH=1 "$binary" $cmd_install >/dev/null 2>&1 || true ;;
      bun)  "$binary" $cmd_install >/dev/null 2>&1 || true ;;
      npm|pnpm|yarn) "$binary" $cmd_install >/dev/null 2>&1 || true ;;
    esac
    e=$(ms); cold_times+=($((e - s)))

    rec "$tool" "$project" "cold" "time_ms" $((e - s)) "ms"
    rec "$tool" "$project" "cold" "disk_node_modules_kb" "$(dkb "$dir/node_modules")" "KB"
    rec "$tool" "$project" "cold" "disk_cache_kb" "$(dkb "$cache_dir")" "KB"
    rec "$tool" "$project" "cold" "file_count" "$(fcount "$dir/node_modules")" "files"
    rec "$tool" "$project" "cold" "dir_count" "$(dcount "$dir/node_modules")" "dirs"
    rec "$tool" "$project" "cold" "symlink_count" "$(symcount "$dir/node_modules")" "symlinks"
    ok=0; [ -f "$dir/node_modules/express/package.json" ] && ok=1; [ -f "$dir/node_modules/react/package.json" ] && ok=1; [ -f "$dir/node_modules/lodash/package.json" ] && ok=1
    rec "$tool" "$project" "cold" "install_ok" "$ok" "bool"
  done

  cmed=$(median "${cold_times[@]}"); cavg=$(avg "${cold_times[@]}")
  rec "$tool" "$project" "cold" "time_median_ms" "$cmed" "ms"
  rec "$tool" "$project" "cold" "time_avg_ms" "$cavg" "ms"

  # ========================
  # PHASE 2: FINGERPRINT (keep node_modules)
  # ========================
  for rep in $(seq 1 "$REPS"); do
    s=$(ms); cd "$dir"
    case "$tool" in
      jhol) JHOL_CACHE_DIR="$cache_dir" JHOL_QUIET=1 JHOL_NO_SPLASH=1 "$binary" $cmd_install >/dev/null 2>&1 || true ;;
      bun|npm|pnpm|yarn) "$binary" $cmd_install >/dev/null 2>&1 || true ;;
    esac
    e=$(ms); rec "$tool" "$project" "fingerprint" "time_ms" $((e - s)) "ms"
  done

  # ========================
  # PHASE 3: WARM (rm node_modules, keep cache)
  # ========================
  warm_times=()
  for rep in $(seq 1 "$REPS"); do
    rm -rf "$dir/node_modules" "$dir/package-lock.json" "$dir/bun.lock" "$dir/bun.lockb" "$dir/.pnpm" "$dir/yarn.lock" "$dir/pnpm-lock.yaml" "$dir/.yarn" "$dir/yarn.lock" 2>/dev/null || true

    s=$(ms); cd "$dir"
    case "$tool" in
      jhol) JHOL_CACHE_DIR="$cache_dir" JHOL_QUIET=1 JHOL_NO_SPLASH=1 "$binary" $cmd_install >/dev/null 2>&1 || true ;;
      bun|npm|pnpm|yarn) "$binary" $cmd_install >/dev/null 2>&1 || true ;;
    esac
    e=$(ms); warm_times+=($((e - s)))

    rec "$tool" "$project" "warm" "time_ms" $((e - s)) "ms"
    rec "$tool" "$project" "warm" "disk_node_modules_kb" "$(dkb "$dir/node_modules")" "KB"
    rec "$tool" "$project" "warm" "file_count" "$(fcount "$dir/node_modules")" "files"
    ok=0; [ -f "$dir/node_modules/express/package.json" ] && ok=1; [ -f "$dir/node_modules/react/package.json" ] && ok=1; [ -f "$dir/node_modules/lodash/package.json" ] && ok=1
    rec "$tool" "$project" "warm" "install_ok" "$ok" "bool"
  done

  wmed=$(median "${warm_times[@]}"); wavg=$(avg "${warm_times[@]}")
  rec "$tool" "$project" "warm" "time_median_ms" "$wmed" "ms"
  rec "$tool" "$project" "warm" "time_avg_ms" "$wavg" "ms"

  # ========================
  # PHASE 4: CI MODE (lockfile-only, from cold)
  # ========================
  rm -rf "$dir/node_modules"
  s=$(ms); cd "$dir"
  case "$tool" in
    jhol) JHOL_CACHE_DIR="$cache_dir" JHOL_QUIET=1 JHOL_NO_SPLASH=1 "$binary" $ci_cmd >/dev/null 2>&1 || true ;;
    bun|npm|pnpm|yarn) "$binary" $ci_cmd >/dev/null 2>&1 || true ;;
  esac
  e=$(ms); rec "$tool" "$project" "ci" "time_ms" $((e - s)) "ms"
  rec "$tool" "$project" "ci" "disk_kb" "$(dkb "$dir/node_modules")" "KB"
  rec "$tool" "$project" "ci" "file_count" "$(fcount "$dir/node_modules")" "files"

  # ========================
  # PHASE 5: ADD PACKAGE (incremental)
  # ========================
  s=$(ms); cd "$dir"
  case "$tool" in
    jhol) JHOL_CACHE_DIR="$cache_dir" JHOL_QUIET=1 JHOL_NO_SPLASH=1 "$binary" $cmd_add dayjs >/dev/null 2>&1 || true ;;
    bun)  "$binary" $cmd_add dayjs >/dev/null 2>&1 || true ;;
    npm)  "$binary" $cmd_add dayjs >/dev/null 2>&1 || true ;;
    pnpm) "$binary" $cmd_add dayjs >/dev/null 2>&1 || true ;;
    yarn) "$binary" $cmd_add dayjs >/dev/null 2>&1 || true ;;
  esac
  e=$(ms); rec "$tool" "$project" "add" "time_ms" $((e - s)) "ms"

  # ========================
  # PHASE 6: REMOVE PACKAGE (incremental)
  # ========================
  s=$(ms); cd "$dir"
  case "$tool" in
    jhol) JHOL_CACHE_DIR="$cache_dir" JHOL_QUIET=1 JHOL_NO_SPLASH=1 "$binary" remove dayjs >/dev/null 2>&1 || true ;;
    bun|npm|pnpm|yarn) "$binary" remove dayjs >/dev/null 2>&1 || true ;;
  esac
  e=$(ms); rec "$tool" "$project" "remove" "time_ms" $((e - s)) "ms"
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
    run_one "$tool" "$project" "$dir" "/tmp/jhol-brutal-${tool}-${project}-${TIMESTAMP}"
  done
done

# =============================================================================
# REPORT
# =============================================================================
val() {
  grep -o "\"tool\":\"$1\",\"project\":\"$2\",\"phase\":\"$3\",\"metric\":\"time_median_ms\",\"value\":[0-9]*" "$json_out" 2>/dev/null | tail -1 | sed 's/.*"value"://' || echo "-"
}

cat > "$report" <<'RPT'
# Brutal Benchmark — Full Industry Report

| | |
|---|---|
| **Tools** | jhol, bun, pnpm, yarn, npm |
| **Projects** | simple (11 pkgs), react (3), next (6+2500 transitive), monorepo (3 ws), heavy (1), huge-typescript (13 type deps) |
| **Method** | 3 reps per phase, median reported, true cold (all caches nuked) |
| **Metrics** | 30+ per tool: time, disk, files, dirs, symlinks, install ok, cache size, binary size, version |

---

## Cold Install (ms)

RPT

echo "| Tool | simple | react | next | monorepo | heavy | huge-ts |" >> "$report"
echo "|------|--------|-------|------|----------|-------|---------|" >> "$report"
for t in jhol bun pnpm yarn npm; do
  row="| $t |"
  for p in simple react next monorepo heavy huge-typescript; do
    v=$(val "$t" "$p" "cold"); [ -z "$v" ] && v="-"
    row="$row $v |"
  done
  echo "$row" >> "$report"
done

cat >> "$report" <<'RPT'

## Warm Install (ms)

RPT
echo "| Tool | simple | react | next | monorepo | heavy | huge-ts |" >> "$report"
echo "|------|--------|-------|------|----------|-------|---------|" >> "$report"
for t in jhol bun pnpm yarn npm; do
  row="| $t |"
  for p in simple react next monorepo heavy huge-typescript; do
    v=$(val "$t" "$p" "warm"); [ -z "$v" ] && v="-"
    row="$row $v |"
  done
  echo "$row" >> "$report"
done

cat >> "$report" <<'RPT'

## Fingerprint (keep node_modules, ms)

RPT
echo "| Tool | simple | react | next | monorepo | heavy | huge-ts |" >> "$report"
echo "|------|--------|-------|------|----------|-------|---------|" >> "$report"
for t in jhol bun pnpm yarn npm; do
  row="| $t |"
  for p in simple react next monorepo heavy huge-typescript; do
    v=$(grep -o "\"tool\":\"$t\",\"project\":\"$p\",\"phase\":\"fingerprint\",\"metric\":\"time_ms\",\"value\":[0-9]*" "$json_out" 2>/dev/null | head -1 | sed 's/.*"value"://' || echo "-")
    row="$row $v |"
  done
  echo "$row" >> "$report"
done

cat >> "$report" <<'RPT'

## CI Mode (lockfile-only, ms)

RPT
echo "| Tool | simple | react | next | monorepo | heavy | huge-ts |" >> "$report"
echo "|------|--------|-------|------|----------|-------|---------|" >> "$report"
for t in jhol bun pnpm yarn npm; do
  row="| $t |"
  for p in simple react next monorepo heavy huge-typescript; do
    v=$(grep -o "\"tool\":\"$t\",\"project\":\"$p\",\"phase\":\"ci\",\"metric\":\"time_ms\",\"value\":[0-9]*" "$json_out" 2>/dev/null | tail -1 | sed 's/.*"value"://' || echo "-")
    row="$row $v |"
  done
  echo "$row" >> "$report"
done

cat >> "$report" <<'RPT'

## Incremental Add (dayjs, ms)

RPT
echo "| Tool | simple | react | next | monorepo | heavy | huge-ts |" >> "$report"
echo "|------|--------|-------|------|----------|-------|---------|" >> "$report"
for t in jhol bun pnpm yarn npm; do
  row="| $t |"
  for p in simple react next monorepo heavy huge-typescript; do
    v=$(grep -o "\"tool\":\"$t\",\"project\":\"$p\",\"phase\":\"add\",\"metric\":\"time_ms\",\"value\":[0-9]*" "$json_out" 2>/dev/null | tail -1 | sed 's/.*"value"://' || echo "-")
    row="$row $v |"
  done
  echo "$row" >> "$report"
done

cat >> "$report" <<'RPT'

## Binary Size

RPT
echo "| Tool | Size |" >> "$report"
echo "|------|------|" >> "$report"
for t in jhol bun pnpm yarn npm; do
  v=$(grep -o "\"tool\":\"$t\",\"project\":\".*\",\"phase\":\"static\",\"metric\":\"binary_size\",\"value\":[0-9]*" "$json_out" 2>/dev/null | head -1 | sed 's/.*"value"://' || echo "0")
  mb=$(echo "scale=1; $v / 1048576" | bc 2>/dev/null || echo "?")
  echo "| $t | ${mb}MB |" >> "$report"
done

cat >> "$report" <<'RPT'

## Node Modules Disk (KB, cold)

RPT
echo "| Tool | simple | react | next |" >> "$report"
echo "|------|--------|-------|------|" >> "$report"
for t in jhol bun pnpm yarn npm; do
  row="| $t |"
  for p in simple react next; do
    v=$(grep -o "\"tool\":\"$t\",\"project\":\"$p\",\"phase\":\"cold\",\"metric\":\"disk_node_modules_kb\",\"value\":[0-9]*" "$json_out" 2>/dev/null | tail -1 | sed 's/.*"value"://' || echo "-")
    row="$row $v |"
  done
  echo "$row" >> "$report"
done

cat >> "$report" <<'RPT'

## Cache Disk (KB, cold)

RPT
echo "| Tool | simple | react | next |" >> "$report"
echo "|------|--------|-------|------|" >> "$report"
for t in jhol bun pnpm yarn npm; do
  row="| $t |"
  for p in simple react next; do
    v=$(grep -o "\"tool\":\"$t\",\"project\":\"$p\",\"phase\":\"cold\",\"metric\":\"disk_cache_kb\",\"value\":[0-9]*" "$json_out" 2>/dev/null | tail -1 | sed 's/.*"value"://' || echo "-")
    row="$row $v |"
  done
  echo "$row" >> "$report"
done

cat >> "$report" <<'RPT'

## File Count (cold node_modules)

RPT
echo "| Tool | simple | react | next |" >> "$report"
echo "|------|--------|-------|------|" >> "$report"
for t in jhol bun pnpm yarn npm; do
  row="| $t |"
  for p in simple react next; do
    v=$(grep -o "\"tool\":\"$t\",\"project\":\"$p\",\"phase\":\"cold\",\"metric\":\"file_count\",\"value\":[0-9]*" "$json_out" 2>/dev/null | tail -1 | sed 's/.*"value"://' || echo "-")
    row="$row $v |"
  done
  echo "$row" >> "$report"
done

cat >> "$report" <<'RPT'

## Cold Install Speedup vs Bun

RPT
echo "| Tool | simple | react | next |" >> "$report"
echo "|------|--------|-------|------|" >> "$report"
for t in jhol pnpm yarn npm; do
  row="| $t |"
  for p in simple react next; do
    bc=$(val "bun" "$p" "cold"); tv=$(val "$t" "$p" "cold")
    [ "$bc" = "-" ] || [ "$tv" = "-" ] || [ "$bc" = "0" ] && r="-" || r=$(echo "scale=1; $bc / $tv" | bc 2>/dev/null || echo "?")
    row="$row ${r}x |"
  done
  echo "$row" >> "$report"
done

echo "" >> "$report"
echo "---" >> "$report"
echo "Generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$report"
echo "Raw data: \`$json_out\`" >> "$report"

# =============================================================================
# TERMINAL SUMMARY
# =============================================================================
echo ""
echo "====================================="
echo " BRUTAL BENCHMARK COMPLETE"
echo "====================================="
echo " Report: $report"
echo " Data:   $json_out"
echo ""
echo " COLD (median ms):"
echo "  Tool | simple | next  | heavy"
echo "  -----|--------|-------|------"
for t in jhol bun pnpm yarn npm; do
  sv=$(val "$t" "simple" "cold"); nv=$(val "$t" "next" "cold"); hv=$(val "$t" "heavy" "cold")
  printf "  %-4s | %6s | %5s | %5s\n" "$t" "$sv" "$nv" "$hv"
done
echo ""
echo " WARM (median ms):"
echo "  Tool | simple | next  | heavy"
echo "  -----|--------|-------|------"
for t in jhol bun pnpm yarn npm; do
  sv=$(val "$t" "simple" "warm"); nv=$(val "$t" "next" "warm"); hv=$(val "$t" "heavy" "warm")
  printf "  %-4s | %6s | %5s | %5s\n" "$t" "$sv" "$nv" "$hv"
done
