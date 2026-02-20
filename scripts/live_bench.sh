#!/usr/bin/env bash
# Live benchmark: jhol vs npm vs yarn vs pnpm vs bun
# Tests: cold install (no cache, no node_modules), warm install (cache hit)
# Package sets: small (lodash, axios, chalk) and medium (react, typescript, express)

set -euo pipefail

JHOL="$(cd "$(dirname "$0")/.." && pwd)/target/release/jhol"
TMPDIR_BASE="/tmp/jhol_bench_$$"
RUNS=5
SMALL_PKGS="lodash axios chalk"
MEDIUM_PKGS="react typescript express"

mkdir -p "$TMPDIR_BASE"

# Colors
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; CYAN='\033[0;36m'; NC='\033[0m'

log() { echo -e "${CYAN}[bench]${NC} $*"; }
header() { echo -e "\n${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"; echo -e "${YELLOW}  $*${NC}"; echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"; }

# Time a command, return median of N runs
# Usage: time_runs N cmd...  -> prints median seconds
time_runs() {
  local n=$1; shift
  local cmd=("$@")
  local times=()
  for i in $(seq 1 "$n"); do
    local start end elapsed
    start=$(date +%s%N)
    "${cmd[@]}" > /dev/null 2>&1 || true
    end=$(date +%s%N)
    elapsed=$(echo "scale=3; ($end - $start) / 1000000000" | bc)
    times+=("$elapsed")
  done
  # Sort and pick median
  local sorted
  sorted=$(printf '%s\n' "${times[@]}" | sort -n)
  local mid=$(( (n + 1) / 2 ))
  echo "$sorted" | sed -n "${mid}p"
}

# Time a cold install (wipe node_modules + cache between runs)
cold_install_time() {
  local tool=$1; local pkglist=$2; local workdir=$3
  local times=()
  for i in $(seq 1 "$RUNS"); do
    rm -rf "$workdir/node_modules" "$workdir/.jhol_cache" "$workdir/.npm" "$workdir"/.yarn "$workdir"/bun.lock "$workdir"/package-lock.json "$workdir"/yarn.lock "$workdir"/pnpm-lock.yaml 2>/dev/null || true
    # Also nuke tool-specific global caches for true cold
    case "$tool" in
      jhol) 
        # Nuke jhol cache
        rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/jhol" ~/.jhol_cache 2>/dev/null || true
        ;;
      npm)
        npm cache clean --force > /dev/null 2>&1 || true
        ;;
      yarn)
        yarn cache clean > /dev/null 2>&1 || true
        ;;
      pnpm)
        pnpm store prune > /dev/null 2>&1 || true
        ;;
      bun)
        bun pm cache rm > /dev/null 2>&1 || true
        ;;
    esac
    local start end elapsed
    start=$(date +%s%N)
    case "$tool" in
      jhol) (cd "$workdir" && "$JHOL" install $pkglist) > /dev/null 2>&1 || true ;;
      npm)  (cd "$workdir" && npm install --no-audit --no-fund $pkglist) > /dev/null 2>&1 || true ;;
      yarn) (cd "$workdir" && yarn add $pkglist) > /dev/null 2>&1 || true ;;
      pnpm) (cd "$workdir" && pnpm add $pkglist) > /dev/null 2>&1 || true ;;
      bun)  (cd "$workdir" && bun add $pkglist) > /dev/null 2>&1 || true ;;
    esac
    end=$(date +%s%N)
    elapsed=$(echo "scale=3; ($end - $start) / 1000000000" | bc)
    times+=("$elapsed")
    echo -n "  run $i: ${elapsed}s  "
  done
  echo ""
  local sorted
  sorted=$(printf '%s\n' "${times[@]}" | sort -n)
  local mid=$(( (RUNS + 1) / 2 ))
  local median
  median=$(echo "$sorted" | sed -n "${mid}p")
  echo "$median"
}

# Time a warm install (cache populated, node_modules wiped)
warm_install_time() {
  local tool=$1; local pkglist=$2; local workdir=$3
  # First do one full install to populate cache
  case "$tool" in
    jhol) (cd "$workdir" && "$JHOL" install $pkglist) > /dev/null 2>&1 || true ;;
    npm)  (cd "$workdir" && npm install --no-audit --no-fund $pkglist) > /dev/null 2>&1 || true ;;
    yarn) (cd "$workdir" && yarn add $pkglist) > /dev/null 2>&1 || true ;;
    pnpm) (cd "$workdir" && pnpm add $pkglist) > /dev/null 2>&1 || true ;;
    bun)  (cd "$workdir" && bun add $pkglist) > /dev/null 2>&1 || true ;;
  esac
  local times=()
  for i in $(seq 1 "$RUNS"); do
    rm -rf "$workdir/node_modules" 2>/dev/null || true
    local start end elapsed
    start=$(date +%s%N)
    case "$tool" in
      jhol) (cd "$workdir" && "$JHOL" install $pkglist) > /dev/null 2>&1 || true ;;
      npm)  (cd "$workdir" && npm install --no-audit --no-fund $pkglist) > /dev/null 2>&1 || true ;;
      yarn) (cd "$workdir" && yarn add $pkglist) > /dev/null 2>&1 || true ;;
      pnpm) (cd "$workdir" && pnpm add $pkglist) > /dev/null 2>&1 || true ;;
      bun)  (cd "$workdir" && bun add $pkglist) > /dev/null 2>&1 || true ;;
    esac
    end=$(date +%s%N)
    elapsed=$(echo "scale=3; ($end - $start) / 1000000000" | bc)
    times+=("$elapsed")
    echo -n "  run $i: ${elapsed}s  "
  done
  echo ""
  local sorted
  sorted=$(printf '%s\n' "${times[@]}" | sort -n)
  local mid=$(( (RUNS + 1) / 2 ))
  echo "$sorted" | sed -n "${mid}p"
}

setup_workdir() {
  local dir="$1"
  mkdir -p "$dir"
  cat > "$dir/package.json" << 'EOF'
{"name":"bench-test","version":"1.0.0","dependencies":{}}
EOF
}

# Check which tools are available
TOOLS=()
command -v "$JHOL" &>/dev/null && TOOLS+=("jhol") || echo "WARNING: jhol not found at $JHOL"
command -v npm &>/dev/null && TOOLS+=("npm")
command -v yarn &>/dev/null && TOOLS+=("yarn")
command -v pnpm &>/dev/null && TOOLS+=("pnpm")
command -v bun &>/dev/null && TOOLS+=("bun")

echo -e "${GREEN}Tools available: ${TOOLS[*]}${NC}"
echo -e "${GREEN}Runs per benchmark: $RUNS${NC}"

# ─── SMALL PACKAGES COLD ───────────────────────────────────────────────────
header "COLD INSTALL: small (lodash axios chalk) — $RUNS runs"
declare -A COLD_SMALL
for tool in "${TOOLS[@]}"; do
  wd="$TMPDIR_BASE/cold_small_$tool"
  setup_workdir "$wd"
  log "[$tool] cold small..."
  result=$(cold_install_time "$tool" "$SMALL_PKGS" "$wd")
  COLD_SMALL[$tool]=$result
  echo -e "  ${GREEN}$tool median: ${result}s${NC}"
done

# ─── MEDIUM PACKAGES COLD ─────────────────────────────────────────────────
header "COLD INSTALL: medium (react typescript express) — $RUNS runs"
declare -A COLD_MEDIUM
for tool in "${TOOLS[@]}"; do
  wd="$TMPDIR_BASE/cold_med_$tool"
  setup_workdir "$wd"
  log "[$tool] cold medium..."
  result=$(cold_install_time "$tool" "$MEDIUM_PKGS" "$wd")
  COLD_MEDIUM[$tool]=$result
  echo -e "  ${GREEN}$tool median: ${result}s${NC}"
done

# ─── WARM INSTALL ──────────────────────────────────────────────────────────
header "WARM INSTALL: small (lodash axios chalk) — $RUNS runs"
declare -A WARM_SMALL
for tool in "${TOOLS[@]}"; do
  wd="$TMPDIR_BASE/warm_small_$tool"
  setup_workdir "$wd"
  log "[$tool] warm small..."
  result=$(warm_install_time "$tool" "$SMALL_PKGS" "$wd")
  WARM_SMALL[$tool]=$result
  echo -e "  ${GREEN}$tool median: ${result}s${NC}"
done

# ─── JHOL PROFILE PASS ────────────────────────────────────────────────────
header "JHOL INTERNAL PROFILE (cold, small packages)"
wd="$TMPDIR_BASE/jhol_profile"
setup_workdir "$wd"
rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/jhol" ~/.jhol_cache 2>/dev/null || true
echo -e "${CYAN}Running jhol with JHOL_PROFILE_INSTALL=1...${NC}"
JHOL_PROFILE_INSTALL=1 "$JHOL" install $SMALL_PKGS --prefix "$wd" 2>&1 | grep -E "\[jhol-profile\]" || \
  (cd "$wd" && JHOL_PROFILE_INSTALL=1 "$JHOL" install $SMALL_PKGS 2>&1) | grep -E "\[jhol-profile\]" || true

# ─── SUMMARY TABLE ────────────────────────────────────────────────────────
header "BENCHMARK SUMMARY (median seconds)"
printf "\n%-8s %15s %15s %15s\n" "Tool" "Cold-Small" "Cold-Medium" "Warm-Small"
printf "%-8s %15s %15s %15s\n" "────────" "───────────────" "───────────────" "───────────────"
for tool in "${TOOLS[@]}"; do
  cs="${COLD_SMALL[$tool]:-N/A}"
  cm="${COLD_MEDIUM[$tool]:-N/A}"
  ws="${WARM_SMALL[$tool]:-N/A}"
  printf "%-8s %15s %15s %15s\n" "$tool" "${cs}s" "${cm}s" "${ws}s"
done

echo ""
# Calculate jhol vs bun ratio if both present
if [[ -n "${COLD_SMALL[jhol]:-}" && -n "${COLD_SMALL[bun]:-}" ]]; then
  ratio=$(echo "scale=2; ${COLD_SMALL[jhol]} / ${COLD_SMALL[bun]}" | bc)
  echo -e "${YELLOW}jhol is ${ratio}x slower than bun on cold-small${NC}"
fi
if [[ -n "${COLD_MEDIUM[jhol]:-}" && -n "${COLD_MEDIUM[bun]:-}" ]]; then
  ratio=$(echo "scale=2; ${COLD_MEDIUM[jhol]} / ${COLD_MEDIUM[bun]}" | bc)
  echo -e "${YELLOW}jhol is ${ratio}x slower than bun on cold-medium${NC}"
fi

# Save results
RESULTS_FILE="/Users/bhuvanprakash/j/jhol/benchmarks/live-bench-$(date +%Y%m%d-%H%M%S).json"
cat > "$RESULTS_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "runs": $RUNS,
  "cold_small": {
$(for tool in "${TOOLS[@]}"; do echo "    \"$tool\": ${COLD_SMALL[$tool]:-null},"; done | sed '$ s/,$//')
  },
  "cold_medium": {
$(for tool in "${TOOLS[@]}"; do echo "    \"$tool\": ${COLD_MEDIUM[$tool]:-null},"; done | sed '$ s/,$//')
  },
  "warm_small": {
$(for tool in "${TOOLS[@]}"; do echo "    \"$tool\": ${WARM_SMALL[$tool]:-null},"; done | sed '$ s/,$//')
  }
}
EOF
echo -e "\n${GREEN}Results saved to: $RESULTS_FILE${NC}"

# Cleanup
rm -rf "$TMPDIR_BASE"
