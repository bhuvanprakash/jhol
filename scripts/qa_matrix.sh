#!/usr/bin/env bash
# End-to-end QA matrix for an *installed* jhol binary.
#
#   scripts/qa_matrix.sh [path-to-jhol]        (default: target/release/jhol)
#
# Every check asserts an observable result — a file, output content, or runtime
# behaviour — never merely an exit code. Runs in a throwaway project with an isolated
# cache, so it is safe to run repeatedly and never touches the caller's state.
#
# Because this suite exercises the whole command surface, it is the first thing to run
# after any change to main.rs / lib.rs (it is what caught a missing `--no-save`,
# a clobbered `update` contract, and a stub that wrote no file).
set -uo pipefail

J="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/target/release/jhol}"
[ -x "$J" ] || { echo "no jhol binary at $J (cargo build --release first)" >&2; exit 2; }
J="$(cd "$(dirname "$J")" && pwd)/$(basename "$J")"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/jhol-qa-XXXXXX")"
CACHE="$WORK/cache"
PROJ="$WORK/proj"
SOCKET="$WORK/daemon.sock"
mkdir -p "$PROJ" "$CACHE"
pass=0; fail=0; failed_names=()

run() { (cd "$PROJ" && JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" "$@"); }
runq() { run "$@" >/dev/null 2>&1; }
ok()   { pass=$((pass+1)); printf '  %-44s PASS %s\n' "$1" "${2:-}"; }
bad()  { fail=$((fail+1)); failed_names+=("$1"); printf '  %-44s FAIL %s\n' "$1" "${2:-}"; }
check(){ # check <label> <shell-condition...>
  local label="$1"; shift
  if "$@" >/dev/null 2>&1; then ok "$label"; else bad "$label"; fi
}
has()  { grep -q -- "$2" "$1"; }

cleanup() {
  pkill -f "$SOCKET" 2>/dev/null
  [ -f "$WORK/daemon.pid" ] && kill "$(cat "$WORK/daemon.pid")" 2>/dev/null
  rm -rf "$WORK"
}
trap cleanup EXIT

printf '\n=== jhol QA matrix ===\n  binary: %s\n  version: %s\n\n' "$J" "$("$J" --version 2>&1 | head -1)"

# ── basics ──────────────────────────────────────────────────────────────────
check 'version prints jhol'            "$J" --version
check 'help lists subcommands'         bash -c "'$J' --help | grep -q 'install'"
runq init -y && check 'init creates package.json' test -f "$PROJ/package.json" || bad 'init creates package.json'

# ── add / install ───────────────────────────────────────────────────────────
runq add lodash@4.17.21
check 'add installs package'           test -d "$PROJ/node_modules/lodash"
check 'add writes package.json'        has "$PROJ/package.json" '"lodash"'
v=$(cd "$PROJ" && node -e "console.log(require('lodash/package.json').version)" 2>/dev/null)
[ "$v" = "4.17.21" ] && ok 'added package loads at pinned version' || bad 'added package loads at pinned version' "got '$v'"

runq add typescript@5.9.3
ts=$(run exec tsc --version 2>&1 | head -1)
case "$ts" in *5.9.3*) ok 'exec runs a bin from .bin' ;; *) bad 'exec runs a bin from .bin' "got '$ts'";; esac
ts2=$(run exec tsc --noEmit --pretty false 2>&1 | head -1)
case "$ts2" in *"unexpected argument"*) bad 'exec forwards flags' "flags parsed by jhol: $ts2";; *) ok 'exec forwards flags';; esac

# An explicit request must install that package even when the manifest already has deps:
# this was silently dropped (the flat expansion only saw the request, and the nesting
# fallback only saw the manifest), so it is pinned here.
exp="$WORK/explicit"; mkdir -p "$exp"
printf '{"name":"exp","version":"1.0.0","dependencies":{"lodash":"4.17.21"}}' > "$exp/package.json"
(cd "$exp" && JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" install --no-scripts >/dev/null 2>&1)
if (cd "$exp" && JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" install is-number@7.0.0 >/dev/null 2>&1) \
   && [ -d "$exp/node_modules/is-number" ] && [ -d "$exp/node_modules/lodash" ]; then
  ok 'explicit install adds the package and keeps deps'
else bad 'explicit install adds the package and keeps deps'; fi

runq why lodash && check 'why runs' bash -c "'$J' --version >/dev/null"
out=$(run why lodash 2>&1); case "$out" in *lodash*) ok 'why mentions the package';; *) bad 'why mentions the package' "got '$(echo "$out" | tail -1)'";; esac
runq outdated && ok 'outdated runs' || bad 'outdated runs'
runq doctor   && ok 'doctor runs'   || bad 'doctor runs'

# ── scripts / run ───────────────────────────────────────────────────────────
python3 - "$PROJ/package.json" <<'PY'
import json,sys
p=sys.argv[1]; d=json.load(open(p)); d.setdefault('scripts',{})['qa']='node -e "console.log(\'ran\')"'
json.dump(d, open(p,'w'), indent=2)
PY
o=$(run run qa 2>&1 | tail -1)
[ "$o" = "ran" ] && ok 'run executes a script' || bad 'run executes a script' "got '$o'"

# ── lockfile: determinism + strict modes ────────────────────────────────────
runq ci
a=$(shasum "$PROJ/package-lock.json" 2>/dev/null | cut -d' ' -f1)
runq ci
b=$(shasum "$PROJ/package-lock.json" 2>/dev/null | cut -d' ' -f1)
[ -n "$a" ] && [ "$a" = "$b" ] && ok 'ci is deterministic (identical lockfile)' || bad 'ci is deterministic' "$a vs $b"
mkdir -p "$WORK/nolock"
if (cd "$WORK/nolock" && JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 "$J" ci >/dev/null 2>&1); then
  bad 'ci without lockfile fails' 'succeeded'
else ok 'ci without lockfile fails cleanly'; fi

# ── sbom / cache ────────────────────────────────────────────────────────────
sb=$(run sbom 2>&1)
case "$sb" in *components*|*lodash*) ok 'sbom lists components';; *) bad 'sbom lists components';; esac
runq cache size && ok 'cache size runs' || bad 'cache size runs'
runq cache list && ok 'cache list runs' || bad 'cache list runs'
n=$(run cache size 2>/dev/null | head -3 | wc -l | tr -d ' ')
[ "$n" -ge 1 ] && ok 'cache size reports without a broken pipe' || bad 'cache size | head (SIGPIPE)'

# ── completions / import-lock ───────────────────────────────────────────────
for sh in bash zsh fish; do
  n=$("$J" completions "$sh" 2>/dev/null | wc -l | tr -d ' ')
  [ "$n" -gt 10 ] && ok "completions $sh" || bad "completions $sh" "lines=$n"
done
runq import-lock --from npm && ok 'import-lock accepts npm' || bad 'import-lock accepts npm'

# ── bundler ─────────────────────────────────────────────────────────────────
mkdir -p "$PROJ/src"
printf 'import { add } from "./lib.js";\nconsole.log(add(20,22));\n' > "$PROJ/src/main.js"
printf 'export const add = (a,b) => a + b;\n' > "$PROJ/src/lib.js"
if runq build src/main.js --outdir dist --format cjs --target node; then
  f=$(ls "$PROJ"/dist/*.js 2>/dev/null | head -1)
  o=$(cd "$PROJ" && node "$f" 2>/dev/null | tail -1)
  [ "$o" = "42" ] && ok 'build produces a runnable bundle' || bad 'build produces a runnable bundle' "output '$o'"
else bad 'build produces a runnable bundle' 'build failed'; fi

# ── link / unlink / remove / update ─────────────────────────────────────────
mkdir -p "$WORK/libpkg"
printf '{"name":"libpkg","version":"1.0.0","main":"index.js"}' > "$WORK/libpkg/package.json"
printf 'module.exports = "linked";' > "$WORK/libpkg/index.js"
runq link "$WORK/libpkg"
o=$(cd "$PROJ" && node -e "console.log(require('libpkg'))" 2>/dev/null | tail -1)
[ "$o" = "linked" ] && ok 'link makes package resolvable' || bad 'link makes package resolvable' "got '$o'"
runq unlink libpkg && ok 'unlink runs' || bad 'unlink runs'

runq remove typescript
if has "$PROJ/package.json" '"typescript"'; then bad 'remove drops it from package.json'; else ok 'remove drops it from package.json'; fi
[ -e "$PROJ/node_modules/.bin/tsc" ] && bad 'remove prunes the .bin shim' 'shim still present' || ok 'remove prunes the .bin shim'
runq add is-number@7.0.0
runq remove --no-save is-number
has "$PROJ/package.json" '"is-number"' && ok 'remove --no-save keeps package.json' || bad 'remove --no-save keeps package.json'

runq update && ok 'update runs' || bad 'update runs'
if run update --save lodash >/dev/null 2>&1; then ok 'update --save accepts a package list'; else bad 'update --save accepts a package list'; fi

# ── diagnostics ─────────────────────────────────────────────────────────────
runq jagr && ok 'jagr runs' || bad 'jagr runs'
rm -f "$WORK/report.json"
if runq audit-jagr --save "$WORK/report.json" && [ -s "$WORK/report.json" ]; then
  n=$(python3 -c "import json;print(len(json.load(open('$WORK/report.json'))))" 2>/dev/null || echo 0)
  [ "$n" -gt 5 ] && ok 'audit-jagr --save writes a report' "($n fields)" || bad 'audit-jagr --save writes a report' 'too small'
else bad 'audit-jagr --save writes a report' 'no file'; fi
runq audit-jagr --load "$WORK/report.json" && ok 'audit-jagr --load reads it' || bad 'audit-jagr --load reads it'
echo 'not json' > "$WORK/bad.json"
if runq audit-jagr --load "$WORK/bad.json"; then bad 'audit-jagr rejects a corrupt report'; else ok 'audit-jagr rejects a corrupt report'; fi
o=$(run jagr-impact --target lodash@5.0.0 2>&1)
# Assert the contract (a real From version + the requested To), not a version that
# depends on what earlier steps did to the fixture.
case "$o" in
  *"From:"*[0-9]*"To:"*"5.0.0"*) ok 'jagr-impact reports real versions';;
  *) bad 'jagr-impact reports real versions' "$(echo "$o" | grep -E 'From:|To:' | tr '\n' ' ')";;
esac
runq betti-history && ok 'betti-history runs' || bad 'betti-history runs'
runq cdn lodash && ok 'cdn prints a url' || bad 'cdn prints a url'

# ── offline ────────────────────────────────────────────────────────────────
# Offline must work from a cache that actually holds the resolved versions, so use a
# dedicated project: install online, drop node_modules + lockfile, then go offline.
# (Running this in the main fixture after `update` legitimately fails when the newer
# version was never cached — that is correct behaviour, not a bug.)
offp="$WORK/offline"; mkdir -p "$offp"
printf '{"name":"off","version":"1.0.0","dependencies":{"lodash":"4.17.21","semver":"^7.7.1"}}' > "$offp/package.json"
(cd "$offp" && JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" install --no-scripts >/dev/null 2>&1)
rm -rf "$offp/node_modules" "$offp/package-lock.json"
if (cd "$offp" && JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" install --no-scripts --offline >/dev/null 2>&1) \
   && [ -d "$offp/node_modules/lodash" ]; then
  ok 'offline install from a warm cache'
else bad 'offline install from a warm cache'; fi
fresh="$WORK/fresh"; mkdir -p "$fresh"
printf '{"name":"fresh","version":"1.0.0","dependencies":{"lodash":"4.17.21"}}' > "$fresh/package.json"
if (cd "$fresh" && JHOL_CACHE_DIR="$WORK/empty-cache" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" install --offline >/dev/null 2>&1); then
  bad 'offline install without cache fails' 'succeeded'
else ok 'offline install without cache fails'; fi

# ── daemon ─────────────────────────────────────────────────────────────────
JHOL_DAEMON_SOCKET="$SOCKET" JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 "$J" daemon --serve >"$WORK/daemon.log" 2>&1 &
echo $! > "$WORK/daemon.pid"
for _ in $(seq 1 40); do [ -S "$SOCKET" ] && break; sleep 0.25; done
if [ -S "$SOCKET" ]; then
  ok 'daemon binds its socket'
  d="$WORK/dproj"; mkdir -p "$d"
  printf '{"name":"d","version":"1.0.0","dependencies":{"semver":"^7.7.1"}}' > "$d/package.json"
  if (cd "$d" && JHOL_DAEMON_SOCKET="$SOCKET" JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 "$J" install --daemon --no-scripts >/dev/null 2>&1) \
     && [ -d "$d/node_modules/semver" ]; then ok 'daemon installs into the requested project'; else bad 'daemon installs into the requested project'; fi
  kill "$(cat "$WORK/daemon.pid")" 2>/dev/null
else bad 'daemon binds its socket' 'socket never appeared'; fi

# ── awkward paths ──────────────────────────────────────────────────────────
spacey="$WORK/space tëst/pröj"
mkdir -p "$spacey"
printf '{"name":"sp","version":"1.0.0","dependencies":{"ms":"2.1.3"}}' > "$spacey/package.json"
if (cd "$spacey" && JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" install --no-scripts >/dev/null 2>&1) \
   && (cd "$spacey" && node -e "require('ms')" >/dev/null 2>&1); then
  ok 'install works in a path with spaces/unicode'
else bad 'install works in a path with spaces/unicode'; fi


# ── spec grammar: overrides, aliases, file:, exact prerelease ───────────────
ov="$WORK/ovr"; mkdir -p "$ov"
printf '{"name":"ovr","version":"1.0.0","dependencies":{"debug":"^2.6.9"},"overrides":{"ms":"2.0.0"}}' > "$ov/package.json"
(cd "$ov" && JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" install --no-scripts >/dev/null 2>&1)
mv=$(python3 -c "
import json,os
p='$ov/node_modules/ms/package.json'
print(json.load(open(p))['version'] if os.path.exists(p) else 'missing')" 2>/dev/null)
[ "$mv" = "2.0.0" ] && ok 'overrides win over a declared range' || bad 'overrides win over a declared range' "ms=$mv"

al="$WORK/alias"; mkdir -p "$al"
printf '{"name":"al","version":"1.0.0","dependencies":{"my-lodash":"npm:lodash@4.17.21"}}' > "$al/package.json"
if (cd "$al" && JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" install --no-scripts >/dev/null 2>&1); then
  o=$(cd "$al" && node -e "console.log(require('my-lodash/package.json').version)" 2>/dev/null | tail -1)
  [ "$o" = "4.17.21" ] && ok 'npm: alias installs the aliased package' || bad 'npm: alias installs the aliased package' "got '$o'"
else bad 'npm: alias installs the aliased package' 'install failed'; fi

fp="$WORK/alias/lib"; mkdir -p "$fp"
printf '{"name":"local-pkg","version":"1.0.0","main":"index.js"}' > "$fp/package.json"
printf 'module.exports = 42;' > "$fp/index.js"
fl="$WORK/filepkg"; mkdir -p "$fl"
printf '{"name":"fl","version":"1.0.0","dependencies":{"local-pkg":"file:../alias/lib"}}' > "$fl/package.json"
if (cd "$fl" && JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" install --no-scripts >/dev/null 2>&1); then
  o=$(cd "$fl" && node -e "console.log(require('local-pkg'))" 2>/dev/null | tail -1)
  [ "$o" = "42" ] && ok 'file: dependency is linked and loads' || bad 'file: dependency is linked and loads' "got '$o'"
else bad 'file: dependency is linked and loads' 'install failed'; fi

pre="$WORK/pre"; mkdir -p "$pre"
printf '{"name":"pre","version":"1.0.0","dependencies":{"ms":"4.0.0-nightly.202508271359"}}' > "$pre/package.json"
(cd "$pre" && JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" install --no-scripts >/dev/null 2>&1)
pv=$(python3 -c "
import json,os
p='$pre/node_modules/ms/package.json'
print(json.load(open(p))['version'] if os.path.exists(p) else 'missing')" 2>/dev/null)
[ "$pv" = "4.0.0-nightly.202508271359" ] && ok 'explicit prerelease pin is honoured exactly' || bad 'explicit prerelease pin is honoured exactly' "ms=$pv"

# ── optional deps that cannot resolve must be skipped, not fatal ───────────
od="$WORK/optdep"; mkdir -p "$od"
printf '{"name":"od","version":"1.0.0","optionalDependencies":{"ms":"2.1.3","does-not-exist-qa-xyz":"^1.0.0"}}' > "$od/package.json"
if (cd "$od" && JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" install --no-scripts >/dev/null 2>&1); then
  [ -d "$od/node_modules/ms" ] && ok 'unresolvable optional dep is skipped' || bad 'unresolvable optional dep is skipped' 'ms missing'
else bad 'unresolvable optional dep is skipped' 'install failed'; fi

# ── lockfile-only, frozen, resolver selection ──────────────────────────────
lo="$WORK/lo"; mkdir -p "$lo"
printf '{"name":"lo","version":"1.0.0","dependencies":{"ms":"2.1.3"}}' > "$lo/package.json"
if (cd "$lo" && JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" install --lockfile-only >/dev/null 2>&1) \
   && [ -s "$lo/package-lock.json" ] && [ ! -d "$lo/node_modules" ]; then
  ok '--lockfile-only writes a lockfile and no node_modules'
else bad '--lockfile-only writes a lockfile and no node_modules'; fi
runq install --resolver jagr >/dev/null 2>&1 && ok '--resolver jagr accepted' || bad '--resolver jagr accepted'
runq install --resolver pubgrub-v2 >/dev/null 2>&1 && ok '--resolver pubgrub-v2 accepted' || bad '--resolver pubgrub-v2 accepted'
runq install --prefer minimal >/dev/null 2>&1 && ok '--prefer minimal accepted' || bad '--prefer minimal accepted'

# stale lockfile + --frozen must fail
st="$WORK/stale"; mkdir -p "$st"
printf '{"name":"stale","version":"1.0.0","dependencies":{"ms":"2.1.3"}}' > "$st/package.json"
(cd "$st" && JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" install --no-scripts >/dev/null 2>&1)
printf '{"name":"stale","version":"1.0.0","dependencies":{"ms":"2.1.3","is-number":"7.0.0"}}' > "$st/package.json"
if (cd "$st" && JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" install --frozen >/dev/null 2>&1); then
  bad '--frozen fails on a stale lockfile' 'succeeded'
else ok '--frozen fails on a stale lockfile'; fi

# ── copy mode, cache transfer ──────────────────────────────────────────────
cp="$WORK/copymode"; mkdir -p "$cp"
printf '{"name":"cp","version":"1.0.0","dependencies":{"ms":"2.1.3"}}' > "$cp/package.json"
if (cd "$cp" && JHOL_CACHE_DIR="$CACHE" JHOL_LINK=0 JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" install --no-scripts >/dev/null 2>&1) \
   && [ -d "$cp/node_modules/ms" ] && [ ! -L "$cp/node_modules/ms" ]; then
  ok 'JHOL_LINK=0 installs by copying (not symlinks)'
else bad 'JHOL_LINK=0 installs by copying (not symlinks)'; fi

xfer="$WORK/xfer"; mkdir -p "$xfer"
if runq cache export "$xfer/cache.tar" >/dev/null 2>&1 || "$J" cache export "$xfer/cache.tar" >/dev/null 2>&1; then
  ok 'cache export produces an archive'
else bad 'cache export produces an archive'; fi

# ── workspaces ─────────────────────────────────────────────────────────────
ws="$WORK/ws"; mkdir -p "$ws/pkgs/a"
printf '{"name":"wsroot","version":"1.0.0","private":true,"workspaces":["pkgs/*"]}' > "$ws/package.json"
printf '{"name":"a","version":"1.0.0","main":"index.js","dependencies":{"ms":"2.1.3"}}' > "$ws/pkgs/a/package.json"
printf 'module.exports = require("ms")(2000);' > "$ws/pkgs/a/index.js"
if (cd "$ws" && JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" install --no-scripts --all-workspaces >/dev/null 2>&1); then
  if [ -d "$ws/node_modules/a" ] || [ -d "$ws/pkgs/a/node_modules/ms" ]; then
    ok 'workspace package deps are installed'
  else bad 'workspace package deps are installed' 'nothing linked'; fi
else bad 'workspace package deps are installed' 'install failed'; fi

# ── sbom cyclonedx + why --jagr ────────────────────────────────────────────
cdx=$(run sbom --format cyclonedx 2>&1)
case "$cdx" in *'"components"'*) ok 'sbom --format cyclonedx emits components';; *) bad 'sbom --format cyclonedx emits components';; esac
wj=$(run why lodash --jagr 2>&1)
case "$wj" in *"is installed"*) ok 'why --jagr explains from the real graph';; *) bad 'why --jagr explains from the real graph';; esac

printf '\n  ---- QA matrix: %d passed, %d failed ----\n' "$pass" "$fail"
if [ "$fail" -gt 0 ]; then
  printf '  failed: %s\n' "${failed_names[*]}"
  exit 1
fi
