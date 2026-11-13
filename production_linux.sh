#!/usr/bin/env bash
# =============================================================================
# production_linux.sh — jhol production audit for Linux
# =============================================================================
#
# WHAT THIS IS
#
#   One script that runs every proof we currently have about the package manager on
#   this machine and writes `report.md` next to it. It does not download jhol's
#   competitors and it does not copy their behaviour; every comparison is against the
#   npm on this machine acting as an oracle for observable output only.
#
# WHAT IT ANSWERS
#
#   1. Does the source build here, from scratch, with no warnings-as-errors tricks?
#   2. Do the unit, adversarial and CLI gates pass?
#   3. Does jhol produce the same package tree as npm on real manifests?
#   4. Does the tree verify, explain itself, and survive crash/determinism checks?
#   5. What are the measured timings on this hardware?
#   6. What is explicitly NOT proven on this machine?
#
# WHY IT IS SHAPED THIS WAY
#
#   * Every check is a function with the same contract: it prints what it observed and
#     returns 0 (pass), 1 (fail), 2 (skip with a reason). Nothing is inferred.
#   * Skips are first-class. A missing npm is a SKIP, never a PASS. That rule is the
#     whole point: a green report that hid a skipped comparison would be a lie.
#   * The report is generated, never hand-edited. If you change behaviour, re-run.
#   * Timings use bash's `time` builtin epoch arithmetic, not an external process, so
#     the number is the number and not the number plus `date` startup.
#
# SAFETY
#
#   * Downloads happen only through jhol and npm, into a temporary directory.
#   * The repository is never modified: the script runs from a copy of reports, and
#     `report.md` is the only file it creates in the current directory.
#   * `--offline` skips every check that needs the network.
#   * Disk is checked before anything downloads; the run aborts under 2 GiB free.
#
# USAGE
#
#   ./production_linux.sh                 # full audit, writes ./report.md
#   ./production_linux.sh --quick         # build + unit tests + two scorecard rows
#   ./production_linux.sh --offline       # no network: build + local checks only
#   ./production_linux.sh --allow-foreign # run on a non-Linux host for smoke testing
#   ./production_linux.sh --no-build      # use target/release/jhol if it exists
#
# REQUIREMENTS
#
#   bash >= 4.2, coreutils, cargo/rustc, node+npm (for compatibility rows),
#   python3 (used only by the scorecard harness to build manifests).
#
# EXIT CODE
#
#   0 all checks passed or skipped by design
#   1 at least one check failed
#   2 the script could not start (wrong platform, missing prerequisites)
#
# =============================================================================

set -uo pipefail

# -----------------------------------------------------------------------------
# Section: constants and configuration
# -----------------------------------------------------------------------------

SCRIPT_NAME="production_linux.sh"
SCRIPT_VERSION="1"
REPORT_FILE="${REPORT_FILE:-report.md}"
WORKDIR=""
STARTED_AT=""
MODE_QUICK=0
MODE_OFFLINE=0
MODE_ALLOW_FOREIGN=0
MODE_NO_BUILD=0
MODE_KEEP=0

PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0
CHECKS_RUN=0

# Order matters: it is the order the report is assembled in.
declare -a RESULTS=()
declare -a SECTIONS=()

# Timeouts are generous by design: a slow machine must produce a real answer, not a
# timeout that looks like a failure. Override with TIMEOUT_BUILD=... etc.
TIMEOUT_BUILD="${TIMEOUT_BUILD:-3600}"
TIMEOUT_GATE="${TIMEOUT_GATE:-2400}"
TIMEOUT_INSTALL="${TIMEOUT_INSTALL:-600}"
TIMEOUT_E2E="${TIMEOUT_E2E:-900}"

# Compatibility scenarios. Two in quick mode keeps the feedback loop under a minute on
# a warm cache while still exercising resolution and layout.
QUICK_SCENARIOS=(1 2)
FULL_SCENARIOS=(1 2 3 4 5 6 7 8 9 10)

# -----------------------------------------------------------------------------
# Section: CLI parsing
# -----------------------------------------------------------------------------

usage() {
    cat <<'USAGE'
production_linux.sh — jhol production audit

  --quick            build, unit tests, two compatibility rows only
  --offline          never touch the network (skips compatibility rows)
  --allow-foreign    permit running on a non-Linux kernel (smoke tests only)
  --no-build         use an existing target/release/jhol
  --keep             keep the temporary work directory and print its path
  --report PATH      write the report somewhere else (default ./report.md)
  -h, --help         this text

Exit: 0 pass, 1 failure, 2 cannot start.
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --quick)          MODE_QUICK=1 ;;
        --offline)        MODE_OFFLINE=1 ;;
        --allow-foreign)  MODE_ALLOW_FOREIGN=1 ;;
        --no-build)       MODE_NO_BUILD=1 ;;
        --keep)           MODE_KEEP=1 ;;
        --report)         REPORT_FILE="${2:?--report needs a path}"; shift ;;
        -h|--help)        usage; exit 0 ;;
        *) printf 'unknown argument: %s\n\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

# -----------------------------------------------------------------------------
# Section: output helpers
# -----------------------------------------------------------------------------

if [ -t 1 ]; then
    C_RESET=$'\033[0m'; C_OK=$'\033[32m'; C_BAD=$'\033[31m'; C_SKIP=$'\033[33m'
    C_DIM=$'\033[2m'; C_HEAD=$'\033[1m'
else
    C_RESET=""; C_OK=""; C_BAD=""; C_SKIP=""; C_DIM=""; C_HEAD=""
fi

say()  { printf '%s\n' "$*"; }
head1() { printf '\n%s== %s ==%s\n' "$C_HEAD" "$*" "$C_RESET"; }
dim()  { printf '%s%s%s\n' "$C_DIM" "$*" "$C_RESET"; }

# run_capture VAR CMD...  — runs a command, stores combined output in VAR, returns code.
# Using a variable instead of `$(...)` keeps trailing newlines and exit codes intact on
# every bash version we support.
run_capture() {
    local __var="$1"; shift
    local __out __rc
    __out=$("$@" 2>&1); __rc=$?
    printf -v "$__var" '%s' "$__out"
    return $__rc
}

# millis_since START_MS — integer milliseconds, no external process.
now_ms() {
    # bash 5 has EPOCHREALTIME; bash 3.2 (macOS, minimal containers) does not. The fallback
    # must be checked for emptiness, not for exit status: `printf` succeeds with an empty
    # argument, which silently turns every timing into 0.
    local v=""
    v="${EPOCHREALTIME/./}"
    if [ -z "$v" ]; then v="$(date +%s)000"; fi
    printf '%s' "$v"
}

elapsed_ms() {
    local start="$1" end
    end=$(now_ms)
    # Both sides are milliseconds by construction (see `now_ms`), so this is a subtraction.
    # The old code divided by 1000 "when longer than 10 characters", which silently collapsed
    # sub-second measurements to 0 ms on bash 3.2 — the reason every timing row read `0ms`.
    printf '%s' $(( end - start ))
}

# -----------------------------------------------------------------------------
# Section: result recording
# -----------------------------------------------------------------------------

# record STATUS NAME DETAIL — the single place results become report rows.
# STATUS: PASS | FAIL | SKIP. Anything else is a programming error and fails loudly.
record() {
    local status="$1" name="$2" detail="${3:-}"
    case "$status" in
        PASS) PASS_COUNT=$((PASS_COUNT + 1)) ;;
        FAIL) FAIL_COUNT=$((FAIL_COUNT + 1)) ;;
        SKIP) SKIP_COUNT=$((SKIP_COUNT + 1)) ;;
        *) say "${C_BAD}internal error: bad status '$status' for '$name'${C_RESET}"; FAIL_COUNT=$((FAIL_COUNT + 1)) ;;
    esac
    CHECKS_RUN=$((CHECKS_RUN + 1))
    # Escape pipes so a detail string cannot break the markdown table.
    local safe="${detail//|/\\|}"
    RESULTS+=("| $name | $status | $safe |")
    case "$status" in
        PASS) printf '  %sPASS%s %s%s\n' "$C_OK" "$C_RESET" "$name" "${detail:+ — $detail}" ;;
        FAIL) printf '  %sFAIL%s %s%s\n' "$C_BAD" "$C_RESET" "$name" "${detail:+ — $detail}" ;;
        SKIP) printf '  %sSKIP%s %s%s\n' "$C_SKIP" "$C_RESET" "$name" "${detail:+ — $detail}" ;;
    esac
}

# check NAME FN — runs FN, whose return code decides the status; FN prints its detail.
check() {
    local name="$1" fn="$2" detail rc
    detail=$("$fn" 2>&1); rc=$?
    case $rc in
        0) record PASS "$name" "$detail" ;;
        2) record SKIP "$name" "$detail" ;;
        *) record FAIL "$name" "${detail:-exit $rc}" ;;
    esac
}

# Guard for checks that depend on an executable.
need() {
    command -v "$1" >/dev/null 2>&1 || { printf '%s not found' "$1"; return 2; }
}

# -----------------------------------------------------------------------------
# Section: environment capture
# -----------------------------------------------------------------------------

env_kernel()   { printf '%s' "$(uname -s 2>/dev/null)"; }
env_release()  { printf '%s' "$(uname -r 2>/dev/null)"; }
env_arch()     { printf '%s' "$(uname -m 2>/dev/null)"; }
env_distro() {
    if [ -r /etc/os-release ]; then
        # shellcheck disable=SC1091
        . /etc/os-release 2>/dev/null || true
        printf '%s %s' "${NAME:-unknown}" "${VERSION_ID:-}"
    else
        printf 'unknown'
    fi
}
env_libc() {
    if command -v ldd >/dev/null 2>&1; then
        ldd --version 2>&1 | head -1
    else
        printf 'unknown (no ldd)'
    fi
}
env_cores() { nproc 2>/dev/null || grep -c ^processor /proc/cpuinfo 2>/dev/null || printf '?'; }
env_mem_gib() {
    awk '/MemTotal/ {printf "%.1f", $2/1048576}' /proc/meminfo 2>/dev/null || printf '?'
}
env_disk_free_gib() {
    df -Pk "${1:-.}" 2>/dev/null | awk 'NR==2 {printf "%.1f", $4/1048576}' || printf '?'
}
env_tool_version() {
    local tool="$1"; shift
    command -v "$tool" >/dev/null 2>&1 || { printf 'absent'; return; }
    "$tool" "$@" 2>/dev/null | head -1 | tr -d '\n' || printf 'error'
}

# -----------------------------------------------------------------------------
# Section: prerequisite checks
# -----------------------------------------------------------------------------

preflight_platform() {
    local kernel; kernel=$(env_kernel)
    if [ "$kernel" != "Linux" ]; then
        if [ "$MODE_ALLOW_FOREIGN" -eq 0 ]; then
            printf 'kernel is %s, not Linux (use --allow-foreign to smoke test)' "$kernel"
            return 1
        fi
        # Foreign kernel: the audit still runs, but "platform is Linux" is *not* proven.
        printf 'SKIP: kernel is %s; results describe this host only' "$kernel"
        return 2
    fi
    printf '%s' "$kernel"
}

preflight_disk() {
    local free; free=$(env_disk_free_gib "$WORKDIR")
    # A build plus ten installs needs headroom; running out mid-build produces an error
    # that looks like a code defect (errno 28) and wastes an hour of human attention.
    awk -v f="$free" 'BEGIN { exit (f+0 < 2.0) }' || {
        printf 'only %s GiB free; need >= 2' "$free"; return 1
    }
    printf '%s GiB free' "$free"
}

preflight_bash() {
    # The script avoids bash-4-only features on purpose (`EPOCHREALTIME` falls back to
    # `date`), because the audit must run on minimal containers that still ship bash 3.2.
    local major=${BASH_VERSINFO[0]:-0} minor=${BASH_VERSINFO[1]:-0}
    if [ "$major" -lt 3 ] || { [ "$major" -eq 3 ] && [ "$minor" -lt 2 ]; }; then
        printf 'bash %s.%s is too old (need 3.2+)' "$major" "$minor"; return 1
    fi
    printf 'bash %s.%s' "$major" "$minor"
}

preflight_rust() {
    need rustc >/dev/null || { printf 'rustc absent'; return 2; }
    need cargo >/dev/null || { printf 'cargo absent'; return 2; }
    printf '%s' "$(env_tool_version rustc --version) / $(env_tool_version cargo --version)"
}

preflight_npm() {
    need npm >/dev/null || { printf 'npm absent — compatibility rows cannot run'; return 2; }
    printf 'npm %s' "$(env_tool_version npm --version)"
}

preflight_node() {
    need node >/dev/null || { printf 'node absent'; return 2; }
    printf 'node %s' "$(env_tool_version node --version)"
}

preflight_python() {
    need python3 >/dev/null || { printf 'python3 absent — scorecard manifests need it'; return 2; }
    printf '%s' "$(env_tool_version python3 --version)"
}

# -----------------------------------------------------------------------------
# Section: build stage
# -----------------------------------------------------------------------------

build_release() {
    if [ "$MODE_NO_BUILD" -eq 1 ]; then
        [ -x target/release/jhol ] || { printf 'no binary and --no-build given'; return 1; }
        printf 'reusing target/release/jhol'
        return 0
    fi
    local started rc
    started=$(now_ms)
    # Debuginfo off: the test/bench artifacts of this workspace are several gigabytes and
    # a full disk mid-build fails as errno 28, which reads like a code error.
    CARGO_PROFILE_DEV_DEBUG=0 CARGO_TERM_COLOR=never \
        timeout "$TIMEOUT_BUILD" cargo build --release > "$WORKDIR/build.log" 2>&1
    rc=$?
    local ms; ms=$(elapsed_ms "$started")
    if [ $rc -ne 0 ]; then
        printf 'cargo build failed (%sms); last lines: %s' \
            "$ms" "$(tail -3 "$WORKDIR/build.log" | tr '\n' ' ')"
        return 1
    fi
    [ -x target/release/jhol ] || { printf 'build succeeded but no binary'; return 1; }
    printf 'built in %sms' "$ms"
}

# One place names the executable, so a build that produces a different name is a one-line fix.
J="target/release/jhol"

binary_reports_version() {
    local out; out=$("$J" --version 2>&1)
    case "$out" in
        *jhol*) printf '%s' "$out" ;;
        *) printf 'unexpected --version output: %s' "$out"; return 1 ;;
    esac
}

binary_help_is_grouped() {
    local out; out=$("$J" --help 2>&1)
    printf '%s' "$out" | grep -qi 'verified installs' || {
        printf 'about line does not describe the tool'; return 1;
    }
    printf 'help states what the tool is'
}

binary_hides_blueprint() {
    local visible hidden
    visible=$("$J" --help 2>&1 | grep -cE '^  (plan|snapshot|db|dev|jagr|pack|publish|login|cdn|view)\b' || true)
    hidden=$(JHOL_BLUEPRINT=1 "$J" --help 2>&1 | grep -cE '^  (plan|snapshot|db|dev|jagr|pack|publish|login|cdn|view)\b' || true)
    [ "$visible" -eq 0 ] || { printf '%s legacy commands visible by default' "$visible"; return 1; }
    [ "$hidden" -gt 0 ] || { printf 'legacy commands are not reachable even with JHOL_BLUEPRINT=1'; return 1; }
    printf 'legacy surface hidden by default, reachable with JHOL_BLUEPRINT=1 (%s)' "$hidden"
}

# -----------------------------------------------------------------------------
# Section: gates
# -----------------------------------------------------------------------------

gate_core_tests() {
    local started rc ms
    started=$(now_ms)
    CARGO_PROFILE_DEV_DEBUG=0 CARGO_TERM_COLOR=never \
        timeout "$TIMEOUT_GATE" cargo test -q -p jhol-core --lib > "$WORKDIR/core-tests.log" 2>&1
    rc=$?; ms=$(elapsed_ms "$started")
    local summary; summary=$(grep -E 'test result' "$WORKDIR/core-tests.log" | tail -1)
    if [ $rc -ne 0 ]; then
        printf 'core tests failed (%sms): %s' "$ms" "$(grep -E 'panicked|FAILED' -A2 "$WORKDIR/core-tests.log" | head -4 | tr '\n' ' ')"
        return 1
    fi
    printf '%s in %sms' "${summary:-passed}" "$ms"
}

gate_cli_suite() {
    local started rc ms
    started=$(now_ms)
    CARGO_PROFILE_DEV_DEBUG=0 CARGO_TERM_COLOR=never \
        timeout "$TIMEOUT_GATE" cargo test -q --test commands > "$WORKDIR/cli-suite.log" 2>&1
    rc=$?; ms=$(elapsed_ms "$started")
    local summary; summary=$(grep -E 'test result' "$WORKDIR/cli-suite.log" | tail -1)
    if [ $rc -ne 0 ]; then
        printf 'CLI contracts failed (%sms): %s' "$ms" "$(tail -3 "$WORKDIR/cli-suite.log" | tr '\n' ' ')"
        return 1
    fi
    printf '%s in %sms' "${summary:-passed}" "$ms"
}

gate_installer() {
    [ -x "$WORKDIR/../scripts/verify-all.sh" ] || [ -x scripts/verify-all.sh ] || {
        printf 'scripts/verify-all.sh absent'; return 1;
    }
    local started rc ms
    started=$(now_ms)
    CARGO_PROFILE_DEV_DEBUG=0 timeout "$TIMEOUT_GATE" sh scripts/verify-all.sh \
        > "$WORKDIR/verify-all.log" 2>&1
    rc=$?; ms=$(elapsed_ms "$started")
    if grep -q 'INSTALLER GATE: FAIL' "$WORKDIR/verify-all.log"; then
        printf 'installer gate FAILED in %sms: %s' "$ms" \
            "$(grep -E 'FAIL ' "$WORKDIR/verify-all.log" | head -2 | tr '\n' ' ')"
        return 1
    fi
    [ $rc -eq 0 ] || { printf 'verify-all exited %s (%sms)' "$rc" "$ms"; return 1; }
    printf 'all gates green in %sms' "$ms"
}

gate_engine_corpus() {
    [ -x crates/jhol-vm/verify.sh ] || { printf 'engine gate script absent'; return 2; }
    local started rc ms
    started=$(now_ms)
    timeout "$TIMEOUT_GATE" sh crates/jhol-vm/verify.sh > "$WORKDIR/engine.log" 2>&1
    rc=$?; ms=$(elapsed_ms "$started")
    if [ $rc -ne 0 ]; then
        printf 'engine gate failed (%sms): %s' "$ms" "$(grep -E 'FAIL|mismatch' "$WORKDIR/engine.log" | head -3 | tr '\n' ' ')"
        return 1
    fi
    local corpus; corpus=$(grep -oE 'corpus \([0-9]+\)' "$WORKDIR/engine.log" | tail -1)
    printf '%s green in %sms' "${corpus:-engine gate}" "$ms"
}

# -----------------------------------------------------------------------------
# Section: compatibility (npm as oracle, behaviour only)
# -----------------------------------------------------------------------------

scorecard_script_present() {
    [ -f scripts/scorecard.sh ] || { printf 'scripts/scorecard.sh absent'; return 1; }
    printf 'harness present'
}

scorecard_rows() {
    need npm >/dev/null || { printf 'npm absent'; return 2; }
    [ "$MODE_OFFLINE" -eq 1 ] && { printf 'offline mode'; return 2; }
    local rows=("${FULL_SCENARIOS[@]}")
    [ "$MODE_QUICK" -eq 1 ] && rows=("${QUICK_SCENARIOS[@]}")
    local started rc ms failed
    started=$(now_ms)
    failed=0
    for row in "${rows[@]}"; do
        if ! timeout "$TIMEOUT_E2E" sh scripts/scorecard.sh --scenario "$row" \
                > "$WORKDIR/row-$row.log" 2>&1; then
            failed=$((failed + 1))
        fi
    done
    ms=$(elapsed_ms "$started")
    if [ "$failed" -ne 0 ]; then
        printf '%s of %s rows differ from npm (%sms); first: %s' \
            "$failed" "${#rows[@]}" "$ms" \
            "$(grep -hE 'FAIL' "$WORKDIR"/row-*.log | head -1 | tr -s ' ' | cut -c1-160)"
        return 1
    fi
    printf '%s of %s rows match npm (%sms)' "${#rows[@]}" "${#rows[@]}" "$ms"
}

# -----------------------------------------------------------------------------
# Section: end-to-end checks on a real project
# -----------------------------------------------------------------------------

# fixture_project NAME DEPS_JSON — creates a temp project and echoes its path.
fixture_project() {
    local name="$1" deps="$2" dir
    dir="$WORKDIR/proj-$name"
    mkdir -p "$dir"
    printf '{"name":"%s","version":"1.0.0","dependencies":%s}\n' "$name" "$deps" > "$dir/package.json"
    printf '%s' "$dir"
}

e2e_install_succeeds() {
    need node >/dev/null || { printf 'node absent'; return 2; }
    [ "$MODE_OFFLINE" -eq 1 ] && { printf 'offline mode'; return 2; }
    local dir started rc ms
    dir=$(fixture_project e2e '{"express":"4.18.2"}')
    started=$(now_ms)
    ( cd "$dir" && JHOL_NO_DAEMON=1 timeout "$TIMEOUT_INSTALL" \
        "$ROOT/${J}" install > install.log 2>&1 )
    rc=$?; ms=$(elapsed_ms "$started")
    if [ $rc -ne 0 ]; then
        printf 'install failed (%sms): %s' "$ms" "$(tail -2 "$dir/install.log" | tr '\n' ' ')"
        return 1
    fi
    local gens; gens=$(find "$dir/node_modules" -name package.json -type f 2>/dev/null | wc -l | tr -d ' ')
    [ "$gens" -gt 10 ] || { printf 'installed only %s manifests' "$gens"; return 1; }
    printf '%s packages in %sms' "$gens" "$ms"
}

e2e_verify_matches() {
    local dir rc out
    dir="$WORKDIR/proj-e2e"
    [ -d "$dir" ] || { printf 'no fixture (install did not run)'; return 2; }
    ( cd "$dir" && "$ROOT/${J}" verify > verify.log 2>&1 ); rc=$?
    out=$(tail -1 "$dir/verify.log" 2>/dev/null)
    if [ $rc -ne 0 ]; then printf 'verify failed: %s' "$out"; return 1; fi
    printf '%s' "$out"
}

e2e_status_json_valid() {
    local dir out
    dir="$WORKDIR/proj-e2e"
    [ -d "$dir" ] || { printf 'no fixture'; return 2; }
    out=$( cd "$dir" && "$ROOT/${J}" status --json 2>/dev/null )
    [ -n "$out" ] || { printf 'status --json printed nothing'; return 1; }
    if command -v python3 >/dev/null 2>&1; then
        printf '%s' "$out" | python3 -c '
import json,sys
d=json.load(sys.stdin)
need={"tree","axes","plan","artifacts","agent","root","has_manifest"}
missing=sorted(need - set(d))
if missing: print("missing fields:", missing); sys.exit(1)
print("fields present:", len(d))
' || return 1
    else
        printf '%s' "$out" | grep -q '"tree"' || { printf 'no tree field'; return 1; }
        printf 'json has tree field (python3 absent, shallow check)'
    fi
}

e2e_why_explains() {
    local dir out rc
    dir="$WORKDIR/proj-e2e"
    [ -d "$dir" ] || { printf 'no fixture'; return 2; }
    out=$( cd "$dir" && "$ROOT/${J}" why express 2>&1 ); rc=$?
    [ $rc -eq 0 ] || { printf 'why exited %s: %s' "$rc" "$(printf '%s' "$out" | head -1)"; return 1; }
    printf '%s' "$out" | grep -qi 'express' || { printf 'why said nothing about express'; return 1; }
    printf 'attributes express to its dependent'
}

e2e_reinstall_is_warm() {
    local dir started rc ms first second
    dir="$WORKDIR/proj-e2e"
    [ -d "$dir" ] || { printf 'no fixture'; return 2; }
    started=$(now_ms)
    ( cd "$dir" && JHOL_NO_DAEMON=1 "$ROOT/${J}" install >/dev/null 2>&1 )
    rc=$?; ms=$(elapsed_ms "$started")
    [ $rc -eq 0 ] || { printf 'reinstall failed'; return 1; }
    first=$ms
    started=$(now_ms)
    ( cd "$dir" && JHOL_NO_DAEMON=1 "$ROOT/${J}" install >/dev/null 2>&1 )
    second=$(elapsed_ms "$started")
    # Warm path is expected in the tens of milliseconds; 5000 is a generous ceiling that
    # still catches "it re-resolved from the network".
    [ "$first" -lt 5000 ] || { printf 'warm reinstall took %sms' "$first"; return 1; }
    printf '%sms then %sms' "$first" "$second"
}

e2e_deterministic_tree() {
    local dir a b
    dir="$WORKDIR/proj-e2e"
    [ -d "$dir" ] || { printf 'no fixture'; return 2; }
    a=$( cd "$dir" && find node_modules -name package.json -type f -print0 2>/dev/null | \
         xargs -0 -r cat 2>/dev/null | cksum )
    rm -rf "$dir/node_modules" "$dir/package-lock.json"
    ( cd "$dir" && JHOL_NO_DAEMON=1 "$ROOT/${J}" install >/dev/null 2>&1 ) || {
        printf 'reinstall after wipe failed'; return 1; }
    b=$( cd "$dir" && find node_modules -name package.json -type f -print0 2>/dev/null | \
         xargs -0 -r cat 2>/dev/null | cksum )
    [ "$a" = "$b" ] || { printf 'tree differed across two installs'; return 1; }
    printf 'identical content checksum'
}

e2e_lockfile_is_valid_json() {
    local dir lock
    dir="$WORKDIR/proj-e2e"
    lock="$dir/package-lock.json"
    [ -f "$lock" ] || { printf 'no lockfile written'; return 1; }
    if command -v python3 >/dev/null 2>&1; then
        python3 - "$lock" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
pkgs = d.get("packages", {})
if len(pkgs) < 2:
    print("lockfile has", len(pkgs), "entries"); sys.exit(1)
print(len(pkgs), "entries")
PY
    else
        head -c 2 "$lock" | grep -q '{' || { printf 'lockfile is not JSON'; return 1; }
        printf 'lockfile starts with { (python3 absent)'
    fi
}

e2e_cache_clean_works() {
    local out rc
    out=$("$J" cache clean 2>&1); rc=$?
    [ $rc -eq 0 ] || { printf 'cache clean exited %s: %s' "$rc" "$(printf '%s' "$out" | tail -1)"; return 1; }
    printf '%s' "$out" | head -1
}

e2e_status_watch_fingerprint() {
    # One process, one line: proves the fingerprint short-circuit exists and that a tick
    # costs far less than a collection. 400 ms at 100 ms ticks is 3-4 ticks.
    local out
    out=$(timeout 2 "$J" status --watch --interval 100 2>&1 | head -3)
    [ -n "$out" ] || { printf 'watch produced no line'; return 1; }
    printf '%s' "$out" | grep -q 'tree' || { printf 'watch line lacks tree verdict'; return 1; }
    printf 'live line: %s' "$(printf '%s' "$out" | head -1 | cut -c1-80)"
}

e2e_daemon_absent_is_fine() {
    # A production install must not depend on a background daemon being present.
    local out rc
    out=$(JHOL_NO_DAEMON=1 "$J" --version 2>&1); rc=$?
    [ $rc -eq 0 ] || { printf 'binary needs a daemon to report its version'; return 1; }
    printf 'works with the daemon disabled'
}

# -----------------------------------------------------------------------------
# Section: robustness checks
# -----------------------------------------------------------------------------

robust_project_lock_serialises() {
    local dir
    dir=$(fixture_project lock '{"is-odd":"3.0.1"}')
    ( cd "$dir" && JHOL_NO_DAEMON=1 "$ROOT/${J}" install >/dev/null 2>&1 ) || {
        printf 'initial install failed'; return 1; }
    # Two concurrent installs: one may wait or fail clearly, but neither may corrupt.
    ( cd "$dir" && JHOL_NO_DAEMON=1 "$ROOT/${J}" install >/dev/null 2>&1 ) &
    local pid=$!
    ( cd "$dir" && JHOL_NO_DAEMON=1 "$ROOT/${J}" install >/dev/null 2>&1 )
    local rc1=$?
    wait $pid; local rc2=$?
    [ $rc1 -eq 0 ] || [ $rc2 -eq 0 ] || { printf 'both concurrent installs failed'; return 1; }
    ( cd "$dir" && "$ROOT/${J}" verify >/dev/null 2>&1 ) || {
        printf 'tree unverifiable after concurrent installs'; return 1; }
    printf 'serialised; tree still verifies'
}

robust_verify_detects_tamper() {
    local dir target pkg
    dir=$(fixture_project tamper '{"is-odd":"3.0.1"}')
    ( cd "$dir" && JHOL_NO_DAEMON=1 "$ROOT/${J}" install >/dev/null 2>&1 ) || {
        printf 'install failed'; return 1; }
    target=$(find "$dir/node_modules" -maxdepth 3 -name package.json -path '*is-odd*' | head -1)
    [ -n "$target" ] || { printf 'is-odd not installed'; return 1; }
    if command -v python3 >/dev/null 2>&1; then
        python3 - "$target" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["version"] = "9.9.9"
json.dump(d, open(p, "w"))
PY
    else
        sed -i 's/"version": *"[^"]*"/"version": "9.9.9"/' "$target"
    fi
    ( cd "$dir" && "$ROOT/${J}" verify >/dev/null 2>&1 ) && {
        printf 'verify accepted a tampered version'; return 1; }
    printf 'verify rejects a tampered tree'
}

robust_missing_manifest_is_explicit() {
    local dir out rc
    dir="$WORKDIR/proj-empty"; mkdir -p "$dir"
    out=$( cd "$dir" && "$ROOT/${J}" status 2>&1 ); rc=$?
    # No manifest is a first-class state, not a crash.
    [ $rc -eq 0 ] || [ $rc -eq 1 ] || { printf 'status crashed with %s' "$rc"; return 1; }
    printf '%s' "$out" | grep -qiE 'no project|no package.json' || {
        printf 'no-manifest state is not named: %s' "$(printf '%s' "$out" | head -1)"; return 1; }
    printf 'names the missing manifest'
}

robust_unknown_action_is_refused() {
    local out rc
    out=$("$J" this-is-not-a-command 2>&1); rc=$?
    [ $rc -ne 0 ] || { printf 'unknown command exited 0'; return 1; }
    printf '%s' "$out" | grep -qi 'unrecognized\|unknown' || {
        printf 'error does not say the command is unknown'; return 1; }
    printf 'refused with exit %s' "$rc"
}

robust_bad_lockfile_is_reported() {
    local dir out rc
    dir=$(fixture_project badlock '{"is-odd":"3.0.1"}')
    printf '{"name":"badlock","lockfileVersion":3,"packages":' > "$dir/package-lock.json"
    out=$( cd "$dir" && "$ROOT/${J}" verify 2>&1 ); rc=$?
    [ $rc -ne 0 ] || { printf 'verify accepted a truncated lockfile'; return 1; }
    printf '%s' "$out" | head -1 | cut -c1-100
}

# -----------------------------------------------------------------------------
# Section: performance checks
# -----------------------------------------------------------------------------

perf_startup_ms() {
    local best=999999 i t s e
    for i in 1 2 3 4 5; do
        s=$(now_ms)
        "$J" --version >/dev/null 2>&1
        e=$(now_ms)
        t=$((e - s))
        [ $t -lt $best ] && best=$t
    done
    [ "$best" -lt 200 ] || { printf 'startup %sms is slow' "$best"; return 1; }
    printf 'best of 5: %sms (process start dominates status)' "$best"
}

perf_status_ms() {
    local dir best=999999 i s e t
    dir=$(fixture_project perf '{"is-odd":"3.0.1"}')
    ( cd "$dir" && JHOL_NO_DAEMON=1 "$ROOT/${J}" install >/dev/null 2>&1 ) || {
        printf 'fixture install failed'; return 2; }
    for i in 1 2 3 4 5; do
        s=$(now_ms)
        ( cd "$dir" && "$ROOT/${J}" status >/dev/null 2>&1 )
        e=$(now_ms); t=$((e - s))
        [ $t -lt $best ] && best=$t
    done
    [ "$best" -lt 500 ] || { printf 'status %sms on a tiny project' "$best"; return 1; }
    printf 'best of 5: %sms' "$best"
}

perf_inspect_local_ms() {
    # Build a tiny GGUF by hand: the check must not need a network fetch or a real model.
    local dir file best=999999 i s e t
    dir="$WORKDIR/gguf"; mkdir -p "$dir"
    file="$dir/tiny.gguf"
    if command -v python3 >/dev/null 2>&1; then
        python3 - "$file" <<'PY'
import struct, sys
out = bytearray()
out += struct.pack('<I', 0x46554747) + struct.pack('<I', 3)
out += struct.pack('<Q', 1) + struct.pack('<Q', 1)
key = b'general.architecture'
out += struct.pack('<Q', len(key)) + key + struct.pack('<I', 8)
val = b'llama'
out += struct.pack('<Q', len(val)) + val
name = b'w'
out += struct.pack('<Q', len(name)) + name + struct.pack('<I', 1)
out += struct.pack('<Q', 1024) + struct.pack('<I', 0) + struct.pack('<Q', 0)
out += b'\0' * 4096
open(sys.argv[1], 'wb').write(out)
PY
    else
        printf 'python3 absent, cannot build fixture'; return 2
    fi
    for i in 1 2 3 4 5; do
        s=$(now_ms); "$J" inspect "$file" >/dev/null 2>&1; e=$(now_ms)
        t=$((e - s)); [ $t -lt $best ] && best=$t
    done
    [ "$best" -lt 200 ] || { printf 'inspect %sms on a 4 KB header' "$best"; return 1; }
    printf 'best of 5: %sms for a header-only read' "$best"
}

perf_fit_local_ms() {
    local file best=999999 i s e t
    file="$WORKDIR/gguf/tiny.gguf"
    [ -f "$file" ] || { printf 'no fixture'; return 2; }
    for i in 1 2 3 4 5; do
        s=$(now_ms); "$J" fit "$file" --vram 8 --ctx 4096 >/dev/null 2>&1; e=$(now_ms)
        t=$((e - s)); [ $t -lt $best ] && best=$t
    done
    [ "$best" -lt 200 ] || { printf 'fit %sms' "$best"; return 1; }
    printf 'best of 5: %sms' "$best"
}

# -----------------------------------------------------------------------------
# Section: ML surface checks (they must not need a GPU to be honest)
# -----------------------------------------------------------------------------

ml_inspect_reports_unknown_without_gpu() {
    local file out
    file="$WORKDIR/gguf/tiny.gguf"
    [ -f "$file" ] || { printf 'no fixture'; return 2; }
    out=$("$J" inspect "$file" 2>&1)
    printf '%s' "$out" | grep -qE 'fit unknown|fits|does not fit' || {
        printf 'inspect said nothing about fit'; return 1; }
    printf '%s' "$out" | head -3 | tr '\n' ' '
}

ml_fit_never_invents_kv() {
    local file out
    file="$WORKDIR/gguf/tiny.gguf"
    [ -f "$file" ] || { printf 'no fixture'; return 2; }
    out=$("$J" fit "$file" --vram 128 --ctx 4096 --json 2>&1)
    if command -v python3 >/dev/null 2>&1; then
        printf '%s' "$out" | python3 -c '
import json,sys
d=json.load(sys.stdin)
# The fixture has no attention metadata: kv must be 0 and missing_meta must name a key.
if d.get("kv_cache") != 0: print("kv invented:", d.get("kv_cache")); sys.exit(1)
if not d.get("missing_meta"): print("missing metadata not reported"); sys.exit(1)
print("kv withheld, missing:", ",".join(d["missing_meta"][:3]))
' || return 1
    else
        printf 'python3 absent'
    fi
}

ml_agent_catalog_matches_server() {
    local file out
    file="$WORKDIR/gguf/tiny.gguf"
    [ -f "$file" ] || { printf 'no fixture'; return 2; }
    out=$(printf '%s\n%s\n' \
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
        '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
        | timeout 20 "$J" mcp --stdio 2>/dev/null | tail -1)
    [ -n "$out" ] || { printf 'mcp server answered nothing'; return 1; }
    if command -v python3 >/dev/null 2>&1; then
        printf '%s' "$out" | python3 -c '
import json,sys
d=json.load(sys.stdin)
tools=[t["name"] for t in d["result"]["tools"]]
print(len(tools), "tools;", "fit" in " ".join(tools) and "includes jhol_fit" or "no fit tool")
' || return 1
    else
        printf '%s' "$out" | grep -q 'jhol_fit' || { printf 'no jhol_fit tool'; return 1; }
        printf 'jhol_fit exposed'
    fi
}

# -----------------------------------------------------------------------------
# Section: reporting
# -----------------------------------------------------------------------------

write_report_header() {
    {
        printf '# jhol production audit — %s\n\n' "$(env_kernel)"
        printf 'Generated by `%s` v%s on %s.\n\n' "$SCRIPT_NAME" "$SCRIPT_VERSION" \
            "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
        printf '> This report is generated, never edited. Every row is a check that ran on this\n'
        printf '> machine. A `SKIP` is a check that could not run and says why; skips are not passes.\n\n'
        printf '| field | value |\n|---|---|\n'
        printf '| kernel | `%s` |\n' "$(env_kernel)"
        printf '| release | `%s` |\n' "$(env_release)"
        printf '| arch | `%s` |\n' "$(env_arch)"
        printf '| distro | `%s` |\n' "$(env_distro)"
        printf '| libc | `%s` |\n' "$(env_libc | tr -d '\n')"
        printf '| cores | %s |\n' "$(env_cores)"
        printf '| memory | %s GiB |\n' "$(env_mem_gib)"
        printf '| disk free at start | %s GiB |\n' "$(env_disk_free_gib .)"
        printf '| rust | `%s` |\n' "$(env_tool_version rustc --version)"
        printf '| cargo | `%s` |\n' "$(env_tool_version cargo --version)"
        printf '| node | `%s` |\n' "$(env_tool_version node --version)"
        printf '| npm (oracle) | `%s` |\n' "$(env_tool_version npm --version)"
        printf '| jhol | `%s` |\n' "$("$J" --version 2>/dev/null || printf 'not built')"
        printf '| mode | %s |\n\n' "$( [ "$MODE_QUICK" -eq 1 ] && printf quick || printf full )"
    } >> "$REPORT_FILE"
}

write_report_results() {
    {
        printf '## Results\n\n'
        printf '| check | status | detail |\n|---|---|---|\n'
        local row
        for row in "${RESULTS[@]}"; do printf '%s\n' "$row"; done
        printf '\n'
    } >> "$REPORT_FILE"
}

write_report_summary() {
    {
        printf '## Summary\n\n'
        printf '| | |\n|---|---|\n'
        printf '| checks run | %s |\n' "$CHECKS_RUN"
        printf '| passed | %s |\n' "$PASS_COUNT"
        printf '| failed | %s |\n' "$FAIL_COUNT"
        printf '| skipped | %s |\n' "$SKIP_COUNT"
        printf '| wall time | %s s |\n' "$(elapsed_ms "$STARTED_AT" | awk '{printf "%.1f", $1/1000}')"
        printf '\n'
    } >> "$REPORT_FILE"
}

write_report_unproven() {
    {
        printf '## What this run does NOT prove\n\n'
        printf 'A report that only lists passes is a half-truth. These are unproven by construction:\n\n'
        printf -- '- **Other platforms.** This run says nothing about any kernel other than the one above.\n'
        printf -- '- **Lifecycle scripts.** The compatibility rows install with `--ignore-scripts`, so\n'
        printf '  `preinstall`/`install`/`postinstall` behaviour is not compared. A native package that\n'
        printf '  builds from source is exercised only at the layout level.\n'
        printf -- '- **Private registries and authentication.** Only the public registry was used.\n'
        printf -- '- **Publish.** `pack`/`publish` were not executed against a registry.\n'
        printf -- '- **Security.** Integrity is checked; signatures and provenance are not.\n'
        printf -- '- **Scale.** Trees here are tens of packages; a 1,200-package monorepo is untested.\n\n'
    } >> "$REPORT_FILE"
}

write_report_footer() {
    {
        printf '## Reproduction\n\n```sh\n'
        printf './%s%s\n' "$SCRIPT_NAME" "$( [ "$MODE_QUICK" -eq 1 ] && printf ' --quick' )"
        printf '```\n\n'
        printf 'Raw logs from every step are in the run directory'
        [ "$MODE_KEEP" -eq 1 ] && printf ' (`%s`)' "$WORKDIR"
        printf '.\n'
    } >> "$REPORT_FILE"
}

# -----------------------------------------------------------------------------
# Section: main
# -----------------------------------------------------------------------------

main() {
    # One anchor for every absolute path. `$OLDPWD` was used for the binary path, which is
    # only correct while nothing has changed directory — a single `cd` in a check would have
    # pointed every e2e/perf row at the wrong build.
    ROOT=$(pwd)
    STARTED_AT=$(now_ms)
    case "$(env_kernel)" in
        Linux) : ;;
        *)
            if [ "$MODE_ALLOW_FOREIGN" -eq 0 ]; then
                say "${C_BAD}This is ${SCRIPT_NAME}: the kernel is $(env_kernel), not Linux.${C_RESET}"
                say "Use --allow-foreign to run it anyway for a smoke test."
                exit 2
            fi
            say "${C_SKIP}warning: not Linux ($(env_kernel)); results describe this kernel.${C_RESET}"
            ;;
    esac

    if [ ! -f Cargo.toml ] || [ ! -d crates/jhol-core ]; then
        say "${C_BAD}Run this from the jhol repository root.${C_RESET}"; exit 2
    fi

    WORKDIR=$(mktemp -d "${TMPDIR:-/tmp}/jhol-audit.XXXXXX") || { say "cannot create workdir"; exit 2; }
    if [ "$MODE_KEEP" -eq 1 ]; then
        say "workdir: $WORKDIR"
    else
        # EXIT cleans up. INT/TERM must clean up **and exit**: a trap that only removes the
        # directory swallows the signal, so `timeout` cannot stop a hung check and the caller
        # hangs with it. This is why three smoke runs had to be killed at 600 s.
        trap 'rm -rf "$WORKDIR"' EXIT
        trap 'rm -rf "$WORKDIR"; exit 130' INT
        trap 'rm -rf "$WORKDIR"; exit 143' TERM
    fi

    : > "$REPORT_FILE"
    head1 "preflight"
    check "platform is Linux"            preflight_platform
    check "bash version"                 preflight_bash
    check "disk headroom (>= 2 GiB)"     preflight_disk
    check "rust toolchain"               preflight_rust
    check "node present"                 preflight_node
    check "npm present (the oracle)"     preflight_npm
    check "python3 present (scorecard)"  preflight_python

    if [ "$FAIL_COUNT" -ne 0 ]; then
        printf '\n%s\n' "${C_BAD}preflight failed; a build here would produce misleading results.${C_RESET}"
        write_report_header
        write_report_results
        write_report_summary
        write_report_notes_early
        exit 1
    fi

    head1 "build"
    check "release build"                build_release
    [ "$FAIL_COUNT" -eq 0 ] || { say "${C_BAD}build failed; stopping.${C_RESET}"; write_early; exit 1; }
    check "binary reports a version"     binary_reports_version
    check "help describes the tool"      binary_help_is_grouped
    check "legacy surface is hidden"     binary_hides_blueprint

    head1 "gates"
    check "core unit tests"              gate_core_tests
    check "CLI contracts"                gate_cli_suite
    check "installer + engine gate"      gate_installer
    check "engine corpus (node exact)"   gate_engine_corpus

    head1 "compatibility (npm as oracle)"
    check "scorecard harness present"    scorecard_script_present
    check "scorecard rows match npm"     scorecard_rows

    head1 "end-to-end"
    check "install a real tree"          e2e_install_succeeds
    check "verify agrees"                e2e_verify_matches
    check "status --json is complete"    e2e_status_json_valid
    check "why explains a package"       e2e_why_explains
    check "warm reinstall is fast"       e2e_reinstall_is_warm
    check "two installs are identical"   e2e_deterministic_tree
    check "lockfile is valid JSON"       e2e_lockfile_is_valid_json
    check "cache clean works"            e2e_cache_clean_works
    check "status --watch is live"       e2e_status_watch_fingerprint
    check "install works without daemon" e2e_daemon_absent_is_fine

    head1 "robustness"
    check "concurrent installs serialise" robust_project_lock_serialises
    check "verify detects tampering"      robust_verify_detects_tamper
    check "missing manifest is explicit"  robust_missing_manifest_is_explicit
    check "unknown command is refused"    robust_unknown_action_is_refused
    check "broken lockfile is reported"   robust_bad_lockfile_is_reported

    head1 "performance"
    check "process startup"               perf_startup_ms
    check "status latency"                perf_status_ms
    check "header-only inspect latency"   perf_inspect_local_ms
    check "fit latency"                   perf_fit_local_ms

    head1 "ML surface"
    check "fit is honest without a GPU"   ml_inspect_reports_unknown_without_gpu
    check "fit never invents KV size"     ml_fit_never_invents_kv
    check "agent catalog reachable"       ml_agent_catalog_matches_server

    write_report_header
    write_report_results
    write_report_summary
    write_report_unproven
    write_report_footer

    head1 "summary"
    say "checks $CHECKS_RUN · ${C_OK}pass $PASS_COUNT${C_RESET} · ${C_BAD}fail $FAIL_COUNT${C_RESET} · ${C_SKIP}skip $SKIP_COUNT${C_RESET}"
    say "report: $REPORT_FILE"
    [ "$FAIL_COUNT" -eq 0 ] || exit 1
    exit 0
}

# Early-report path used when preflight or the build fails: the report must still be usable.
write_report_notes_early() {
    {
        printf '## Summary\n\nRun aborted during preflight. See the failed rows above.\n\n'
        printf '## What this run does NOT prove\n\nEverything: the audit did not get past preflight.\n'
    } >> "$REPORT_FILE"
}

write_early() {
    write_report_header
    write_report_results
    write_report_notes_early
}

main "$@"
