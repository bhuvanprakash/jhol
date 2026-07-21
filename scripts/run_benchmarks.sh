#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPS="${JHOL_BENCH_REPS:-1}"
TOOLS="${JHOL_BENCH_TOOLS:-jhol:jagr3 jhol:pubgrub-v2 bun npm pnpm}"
RUN_ID="$(date +%Y%m%d-%H%M%S)"
OUT_DIR="$ROOT_DIR/benchmarks/results/$RUN_ID"
RESULTS_CSV="$OUT_DIR/results.csv"
SUMMARY_MD="$OUT_DIR/summary.md"
METADATA_JSON="$OUT_DIR/metadata.json"

mkdir -p "$OUT_DIR"

JHOL_BIN="${JHOL_BIN:-$ROOT_DIR/target/release/jhol}"
if [[ ! -x "$JHOL_BIN" ]]; then
  cargo build --release --quiet --manifest-path "$ROOT_DIR/Cargo.toml"
fi

json_escape() {
  python3 -c 'import json,sys; print(json.dumps(sys.stdin.read().strip()))' 2>/dev/null || printf '"%s"' "$1"
}

version_of() {
  local tool="$1"
  if command -v "$tool" >/dev/null 2>&1; then
    "$tool" --version 2>&1 | tr '\n' ' ' | sed 's/[[:space:]]*$//'
  else
    printf 'missing'
  fi
}

write_package_json() {
  local dir="$1"
  local scenario="$2"
  mkdir -p "$dir"
  case "$scenario" in
    simple)
      cat > "$dir/package.json" <<'JSON'
{"name":"bench-final","version":"1.0.0","private":true,"dependencies":{"left-pad":"1.3.0","is-number":"7.0.0","kleur":"4.1.5"}}
JSON
      ;;
    react)
      cat > "$dir/package.json" <<'JSON'
{"name":"test-react","version":"1.0.0","private":true,"dependencies":{"@testing-library/jest-dom":"^5.17.0","@testing-library/react":"^13.4.0","@testing-library/user-event":"^13.5.0","react":"^18.3.1","react-dom":"^18.3.1","react-scripts":"5.0.1","web-vitals":"^2.1.4"}}
JSON
      ;;
    next)
      cat > "$dir/package.json" <<'JSON'
{"name":"test-next","version":"1.0.0","private":true,"dependencies":{"next":"^14.2.30","react":"^18.3.1","react-dom":"^18.3.1"}}
JSON
      ;;
    angular)
      cat > "$dir/package.json" <<'JSON'
{"name":"test-angular-peers","version":"1.0.0","private":true,"dependencies":{"@angular/animations":"^17.3.12","@angular/common":"^17.3.12","@angular/compiler":"^17.3.12","@angular/core":"^17.3.12","@angular/forms":"^17.3.12","@angular/platform-browser":"^17.3.12","rxjs":"^7.8.1","tslib":"^2.6.2","zone.js":"^0.14.10"}}
JSON
      ;;
    large)
      python3 - "$dir/package.json" <<'PY'
import json, sys
names = [
  "@babel/core","@babel/parser","@babel/traverse","@babel/types","accepts","ansi-colors","argparse","axios","balanced-match","brace-expansion","chalk","chokidar","commander","debug","dotenv","esbuild","eslint","express","fast-glob","fs-extra","glob","jest","json5","lodash","micromatch","minimist","ms","postcss","prettier","react","react-dom","resolve","rimraf","semver","source-map","supports-color","tar","typescript","uuid","webpack","yargs"
]
deps = {name: "latest" for name in names}
json.dump({"name":"large-100-plus","version":"1.0.0","private":True,"dependencies":deps}, open(sys.argv[1], "w"))
PY
      ;;
  esac
}

write_monorepo() {
  local dir="$1"
  mkdir -p "$dir/packages/a" "$dir/packages/b"
  cat > "$dir/package.json" <<'JSON'
{"name":"test-monorepo","version":"1.0.0","private":true,"workspaces":["packages/*"],"dependencies":{"typescript":"^5.4.5"}}
JSON
  cat > "$dir/packages/a/package.json" <<'JSON'
{"name":"pkg-a","version":"1.0.0","private":true,"dependencies":{"react":"^18.3.1"}}
JSON
  cat > "$dir/packages/b/package.json" <<'JSON'
{"name":"pkg-b","version":"1.0.0","private":true,"dependencies":{"lodash":"^4.17.21"}}
JSON
}

clean_outputs() {
  local dir="$1"
  rm -rf "$dir/node_modules" "$dir/package-lock.json" "$dir/bun.lockb" "$dir/bun.lock" "$dir/pnpm-lock.yaml" "$dir/jhol-lock.json"
  if [[ -d "$dir/packages" ]]; then
    rm -rf "$dir"/packages/*/node_modules "$dir"/packages/*/package-lock.json "$dir"/packages/*/bun.lockb "$dir"/packages/*/pnpm-lock.yaml "$dir"/packages/*/jhol-lock.json
  fi
}

disk_bytes() {
  local dir="$1"
  if [[ -e "$dir" ]]; then
    du -sk "$dir" 2>/dev/null | awk '{print $1 * 1024}'
  else
    printf '0'
  fi
}

run_install() {
  local scenario="$1" tool_spec="$2" phase="$3" project="$4" cache="$5" rep="$6"
  local tool resolver status elapsed_ms peak_rss disk stdout_file stderr_file time_file start end
  stdout_file="$OUT_DIR/${scenario}-${tool_spec//:/-}-${phase}-r${rep}.stdout.log"
  stderr_file="$OUT_DIR/${scenario}-${tool_spec//:/-}-${phase}-r${rep}.stderr.log"
  time_file="$OUT_DIR/${scenario}-${tool_spec//:/-}-${phase}-r${rep}.time.log"
  tool="${tool_spec%%:*}"
  resolver=""
  if [[ "$tool_spec" == *:* ]]; then resolver="${tool_spec#*:}"; fi

  if [[ "$tool" != "jhol" ]] && ! command -v "$tool" >/dev/null 2>&1; then
    printf '%s,%s,%s,%s,%s,%s,%s,%s,%s\n' "$scenario" "$tool" "$resolver" "$phase" "$rep" "skipped-missing-tool" 0 "" 0 >> "$RESULTS_CSV"
    return 0
  fi

  start="$(python3 - <<'PY'
import time
print(time.perf_counter_ns())
PY
)"
  set +e
  case "$tool" in
    jhol)
      env JHOL_CACHE_DIR="$cache" JHOL_RESOLVER="$resolver" JHOL_PROFILE=1 "$JHOL_BIN" install --resolver "$resolver" --no-scripts --quiet >"$stdout_file" 2>"$stderr_file"
      status=$?
      ;;
    bun)
      bun install --ignore-scripts >"$stdout_file" 2>"$stderr_file"
      status=$?
      ;;
    npm)
      npm install --ignore-scripts --no-audit --no-fund >"$stdout_file" 2>"$stderr_file"
      status=$?
      ;;
    pnpm)
      pnpm install --ignore-scripts >"$stdout_file" 2>"$stderr_file"
      status=$?
      ;;
  esac
  set -e
  end="$(python3 - <<'PY'
import time
print(time.perf_counter_ns())
PY
)"
  elapsed_ms="$(( (end - start) / 1000000 ))"
  disk="$(disk_bytes "$project")"
  if [[ "$status" -eq 0 ]]; then status_text="ok"; else status_text="failed"; fi
  peak_rss=""
  printf '%s,%s,%s,%s,%s,%s,%s,%s,%s\n' "$scenario" "$tool" "$resolver" "$phase" "$rep" "$status_text" "$elapsed_ms" "$peak_rss" "$disk" >> "$RESULTS_CSV"
}

{
  printf '{\n'
  printf '  "run_id": %s,\n' "$(printf '%s' "$RUN_ID" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')"
  printf '  "repetitions": %s,\n' "$REPS"
  printf '  "jhol": %s,\n' "$(version_of "$JHOL_BIN" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read().strip()))')"
  printf '  "bun": %s,\n' "$(version_of bun | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read().strip()))')"
  printf '  "npm": %s,\n' "$(version_of npm | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read().strip()))')"
  printf '  "pnpm": %s,\n' "$(version_of pnpm | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read().strip()))')"
  printf '  "tools": %s\n' "$(printf '%s' "$TOOLS" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read().split()))')"
  printf '}\n'
} > "$METADATA_JSON"

printf 'scenario,tool,resolver,phase,rep,status,install_time_ms,peak_rss_kb,disk_bytes\n' > "$RESULTS_CSV"

for scenario in simple react next angular large; do
  base="/tmp/${scenario}"
  case "$scenario" in
    simple) base="/tmp/bench-final" ;;
    react) base="/tmp/test-react" ;;
    next) base="/tmp/test-next" ;;
  esac
  rm -rf "$base"
  write_package_json "$base" "$scenario"
  for tool_spec in $TOOLS; do
    for rep in $(seq 1 "$REPS"); do
      cache="$OUT_DIR/cache-${scenario}-${tool_spec//:/-}"
      rm -rf "$cache"
      mkdir -p "$cache"
      clean_outputs "$base"
      (cd "$base" && run_install "$scenario" "$tool_spec" cold "$base" "$cache" "$rep")
      clean_outputs "$base"
      (cd "$base" && run_install "$scenario" "$tool_spec" hot "$base" "$cache" "$rep")
    done
  done
done

monorepo="/tmp/test-monorepo"
rm -rf "$monorepo"
write_monorepo "$monorepo"
for rep in $(seq 1 "$REPS"); do
  cache="$OUT_DIR/cache-monorepo-jhol-jagr3"
  rm -rf "$cache"
  mkdir -p "$cache"
  clean_outputs "$monorepo"
  (cd "$monorepo" && run_install monorepo jhol:jagr3 cold "$monorepo" "$cache" "$rep")
  clean_outputs "$monorepo"
  (cd "$monorepo" && run_install monorepo jhol:jagr3 hot "$monorepo" "$cache" "$rep")
done

python3 - "$RESULTS_CSV" "$SUMMARY_MD" "$OUT_DIR/elapsed-ms.svg" <<'PY'
import csv, sys, collections, html
csv_path, md_path, svg_path = sys.argv[1:4]
rows = list(csv.DictReader(open(csv_path)))
with open(md_path, 'w') as f:
    f.write('# Benchmark Summary\n\n')
    f.write('| scenario | tool | resolver | phase | reps | median_ms | best_ms | status | disk_bytes_last |\n')
    f.write('|---|---|---|---:|---:|---:|---:|---|---:|\n')
    groups = collections.defaultdict(list)
    for r in rows:
        groups[(r['scenario'], r['tool'], r['resolver'], r['phase'])].append(r)
    bars = []
    for key, vals in sorted(groups.items()):
        times = sorted(int(v['install_time_ms']) for v in vals if v['status'] == 'ok')
        status = 'ok' if times else vals[-1]['status']
        median = times[len(times)//2] if times else 0
        best = min(times) if times else 0
        disk = vals[-1].get('disk_bytes','0')
        f.write(f"| {key[0]} | {key[1]} | {key[2]} | {key[3]} | {len(vals)} | {median} | {best} | {status} | {disk} |\n")
        if median:
            bars.append((f"{key[0]} {key[1]} {key[2]} {key[3]}", median))
maxv = max([v for _, v in bars] or [1])
w, row_h = 1200, 24
h = 40 + row_h * len(bars)
with open(svg_path, 'w') as f:
    f.write(f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}">')
    f.write('<style>text{font:12px sans-serif}.bar{fill:#4f46e5}</style>')
    for i, (label, val) in enumerate(bars):
        y = 25 + i * row_h
        bw = int((w - 430) * val / maxv)
        f.write(f'<text x="4" y="{y}">{html.escape(label[:58])}</text>')
        f.write(f'<rect class="bar" x="410" y="{y-12}" width="{bw}" height="16"/>')
        f.write(f'<text x="{415+bw}" y="{y}">{val} ms</text>')
    f.write('</svg>')
PY

printf 'Benchmark results: %s\n' "$OUT_DIR"
printf 'Summary: %s\n' "$SUMMARY_MD"
printf 'Graph: %s\n' "$OUT_DIR/elapsed-ms.svg"
