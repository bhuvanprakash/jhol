#!/bin/sh
# Benchmark runner: JholVM vs V8/bun. POSIX sh.
set -u
BENCH_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
JHOL_BIN="${JHOL_BIN:-$BENCH_DIR/../../../target/release/jhol}"
CANON="jholvm qjs jsc node bun"
BACKENDS=""; RUNS=3; JSONOUT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --jholvm|--qjs|--jsc|--node|--bun) BACKENDS="$BACKENDS ${1#--}" ;;
    --all) BACKENDS="$CANON" ;;
    --runs) shift; RUNS="${1:-3}" ;;
    --json) shift; JSONOUT="${1:-}" ;;
    -h|--help) echo "Usage: run_bench.sh [--jholvm|--qjs|--jsc|--node|--bun|--all] [--runs N] [--json OUT.json]"; exit 0 ;;
    *) echo "run_bench.sh: ignoring '$1'" >&2 ;;
  esac; shift
done
[ -n "$BACKENDS" ] || BACKENDS="$CANON"
case "$RUNS" in ''|*[!0-9]*) RUNS=3;; esac; [ "$RUNS" -lt 1 ] && RUNS=1
COLUMNS=""; for b in $CANON; do case " $BACKENDS " in *" $b "*) COLUMNS="$COLUMNS $b";; esac; done
TAB=$(printf '\t'); PY=""; command -v python3 >/dev/null 2>&1 && PY=python3
TMP_DIR=$(mktemp -d); trap 'rm -rf "$TMP_DIR"' EXIT INT TERM

available() { case "$1" in jholvm|qjs|jsc) [ -x "$JHOL_BIN" ];; node) command -v node>/dev/null 2>&1;; bun) command -v bun>/dev/null 2>&1;; *) return 1;; esac; }
sel() { case " $COLUMNS " in *" $1 "*) return 0;; *) return 1;; esac; }

timed_run() {
  case "$1" in
    jholvm) set -- env "JHOL_RUNTIME_ENGINE=jholvm" "$JHOL_BIN" run "$2" ;;
    qjs) set -- env "JHOL_RUNTIME_ENGINE=qjs" "$JHOL_BIN" run "$2" ;;
    jsc) set -- env "JHOL_RUNTIME_ENGINE=jsc" "$JHOL_BIN" run "$2" ;;
    node) set -- node "$2" ;; bun) set -- bun "$2" ;;
  esac
  if [ -n "$PY" ]; then
    BENCH_TMP="$TMP_DIR" "$PY" - "$@" <<'PYEOF'
import os,sys,time,subprocess
t=os.environ["BENCH_TMP"]; s=time.perf_counter()
with open(t+"/out","wb") as f: rc=subprocess.call(sys.argv[1:],stdout=f,stderr=subprocess.DEVNULL)
open(t+"/rc","w").write(str(int(rc))); open(t+"/ms","w").write("%.4f"%((time.perf_counter()-s)*1000))
PYEOF
  else
    s=$(date +%s%N); "$@" >"$TMP_DIR/out" 2>/dev/null; r=$?; e=$(date +%s%N)
    echo "$(( (e-s)/1000000 ))">"$TMP_DIR/ms"; echo "$r">"$TMP_DIR/rc"
  fi
}

median() {
  if [ -n "$PY" ]; then "$PY" -c 'import sys,statistics as st
v=[float(x) for x in sys.argv[1:] if x];print(round(st.median(v),1) if v else 0)' "$@"
  else printf '%s\n' "$@"|sort -n|awk '{a[NR]=$1}END{ if(NR==0)print 0; else if(NR%2)printf "%.1f\n",a[(NR+1)/2]; else printf "%.1f\n",(a[NR/2]+a[NR/2+1])/2 }'; fi
}

TABLE="$TMP_DIR/t"; : >"$TABLE"; NODE_FAIL=0; VM_FAIL=0; ALL_FAIL=0; NB=0
for js in $(ls "$BENCH_DIR"/*.js 2>/dev/null|sort); do
  base=$(basename "$js"); NB=$((NB+1)); row="$base"
  if ! awk -F'\t' -v name="$base" '$1 == name {print substr($0,index($0,"\t")+1); found=1; exit} END {if (!found) exit 1}' \
    "$BENCH_DIR/expected_outputs.txt" >"$TMP_DIR/expected"; then
    echo "run_bench.sh: missing expected output for $base" >&2; exit 1
  fi
  for b in $COLUMNS; do
    if available "$b"; then
      times=""; ri=1; this_fail=0
      while [ "$ri" -le "$RUNS" ]; do timed_run "$b" "$js"; times="$times $(cat "$TMP_DIR/ms")"
        # A non-zero exit means the run CRASHED, not that it was fast. Timing a
        # panic and calling it a speedup is worse than reporting no number at
        # all, so every backend's exit code is checked here, not just node's.
        _rc=$(cat "$TMP_DIR/rc")
        if [ "${_rc:-1}" -ne 0 ] || ! cmp -s "$TMP_DIR/out" "$TMP_DIR/expected"; then
          ALL_FAIL=1; this_fail=1
          printf '%s\t%s\t%s\n' "$base" "$b" "${_rc:-1}" >>"$TMP_DIR/failures"
          case "$b" in node) NODE_FAIL=1 ;; jholvm) VM_FAIL=1 ;;
            *) eval "${b}_FAIL=1" 2>/dev/null || true ;;
          esac
        fi
        ri=$((ri+1)); done
      if [ "$this_fail" -eq 1 ]; then row="$row${TAB}FAIL"; continue; fi
      med=$(median $times); printf '%s\t%s\t%s\n' "$base" "$b" "$med">>"$TMP_DIR/cells"
      echo "$med">>"$TMP_DIR/sum.$b"; row="$row${TAB}$med"
    else
      row="$row${TAB}n/a"; ALL_FAIL=1
      printf '%s\t%s\t127\n' "$base" "$b" >>"$TMP_DIR/failures"
    fi
  done; printf '%s\n' "$row">>"$TABLE"
done

{ printf 'bench'; for b in $COLUMNS; do printf '\t%s' "$b"; done; printf '\n'; cat "$TABLE"; }>"$TMP_DIR/full"
printf '# Benchmark results (%d runs, median ms)\n\n' "$RUNS"
awk -F'\t' '{for(i=1;i<=NF;i++){c[NR,i]=$i;if(length($i)>w[i])w[i]=length($i)}if(NF>nf)nf=NF}END{for(r=1;r<=NR;r++){o="|";for(i=1;i<=nf;i++){s=c[r,i];p=w[i]-length(s);sp="";for(j=0;j<p;j++)sp=sp" ";o=o" "s sp" |"}print o;if(r==1){d="|";for(i=1;i<=nf;i++){t="";for(j=0;j<w[i]+2;j++)t=t"-";d=d t"|"}print d}}}' "$TMP_DIR/full"

if [ "$ALL_FAIL" -eq 0 ] && sel jholvm && available jholvm; then
  ref=""; sel node && available node && ref=node; [ -n "$ref" ] || { sel bun && available bun && ref=bun; }
  if [ -n "$ref" ]; then
    jt=$(awk '{s+=$1}END{printf "%.2f",s+0}' "$TMP_DIR/sum.jholvm")
    rt=$(awk '{s+=$1}END{printf "%.2f",s+0}' "$TMP_DIR/sum.$ref")
    printf 'Speedup vs %s: %sx (jholvm %.2f ms vs %s %.2f ms over %d benches)\n' "$ref" "$(awk -v a=$rt -v b=$jt 'BEGIN{printf "%.2f",(b>0?a/b:0)}')" "$jt" "$ref" "$rt" "$NB"
  fi
fi

if [ -n "$JSONOUT" ] && [ -n "$PY" ]; then
  BENCH_TMP="$TMP_DIR" JB="$JSONOUT" JB_RUNS="$RUNS" "$PY" - <<'PYEOF'
import os,json
t=os.environ["BENCH_TMP"]; res={}; p=t+"/cells"
if os.path.exists(p):
    for line in open(p):
        line=line.rstrip("\n")
        if line:
            b,be,m=line.split("\t"); res.setdefault(be,{})[b]=float(m)
failures=[]
if os.path.exists(t+"/failures"):
    for line in open(t+"/failures"):
        name,backend,rc=line.rstrip("\n").split("\t")
        failures.append({"benchmark":name,"backend":backend,"exit_code":int(rc),"reason":"exit_or_output"})
open(os.environ["JB"],"w").write(json.dumps({"runs":int(os.environ.get("JB_RUNS","3")),"benchmarks":res,"valid":not failures,"failures":failures},indent=2)+"\n")
PYEOF
fi

[ "$NODE_FAIL" -eq 1 ] && { echo "run_bench.sh: a bench failed under node" >&2; exit 1; }
if [ "$VM_FAIL" -eq 1 ]; then
  echo "run_bench.sh: a bench FAILED under jholvm (crashed or errored)." >&2
  echo "  Its times above are crash-exit times, not real runs — do not trust the comparison." >&2
  exit 1
fi
[ "$ALL_FAIL" -eq 1 ] && { echo "run_bench.sh: a selected backend was unavailable, failed, or produced incorrect output" >&2; exit 1; }
exit 0
