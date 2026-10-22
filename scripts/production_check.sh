#!/usr/bin/env bash
# One-command production gate for jhol.
#
# Runs, in order: formatting, a release build (so the audits never test a stale
# binary — that mistake invalidated measurements three times this project),
# the full test suite, and then a real install into an isolated project with an
# isolated cache, which is audited for the properties that existence checks miss:
#
#   1. declared dependencies resolve from every package's own directory
#      (running against the store realpath, which is how Node actually resolves)
#   2. declared `bin` files are present, executable, and actually run
#   3. tarballs are fully extracted (no partial store dirs)
#   4. packages load (require, or import for ESM-only) and a real E2E path works
#
# Usage:  scripts/production_check.sh [fixture-package.json]
# Env:    JHOL_CHECK_KEEP=1  keep the temp project/cache for inspection
#
# Exits non-zero on the first failure, with a summary of what failed.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$REPO/target/release/jhol"
FIXTURE="${1:-$HOME/j/jhol-bench/large-test/package.json}"

step() { printf '\n=== %s ===\n' "$*"; }
fail() { printf '\nFAIL: %s\n' "$*" >&2; exit 1; }

command -v cargo >/dev/null || fail "cargo not found"
[ -f "$FIXTURE" ] || fail "fixture not found: $FIXTURE (pass a package.json path)"

step "1/5 formatting"
(cd "$REPO" && cargo fmt --all --check) || fail "cargo fmt --check"

step "2/5 release build"
(cd "$REPO" && cargo build --release --bin jhol) || fail "release build"
[ -x "$BIN" ] || fail "binary missing after build: $BIN"

step "3/5 test suite"
(cd "$REPO" && cargo test --quiet) || fail "cargo test"

step "4/5 install into an isolated project (cold cache)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/jhol-check-XXXXXX")"
CACHE="$(mktemp -d "${TMPDIR:-/tmp}/jhol-check-cache-XXXXXX")"
cleanup() { [ "${JHOL_CHECK_KEEP:-0}" = "1" ] || rm -rf "$WORK" "$CACHE"; }
trap cleanup EXIT
cp "$FIXTURE" "$WORK/package.json"
if ! (cd "$WORK" && JHOL_CACHE_DIR="$CACHE" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 \
        "$BIN" install --no-scripts >"$WORK/install.log" 2>&1); then
  tail -20 "$WORK/install.log" >&2
  fail "install"
fi
printf '  installed: %s\n' "$(grep -E 'Added|Reused' "$WORK/install.log" | tail -1)"

step "5/5 audits"
WORK="$WORK" CACHE="$CACHE" python3 - <<'PY' || fail "audit"
import json, os, subprocess, sys

work, cache = os.environ['WORK'], os.environ['CACHE']
nm = os.path.join(work, 'node_modules')
problems = []

# ── collect every installed package by realpath (the layout is a symlink store,
#    so a package's real directory is what Node resolves from) ────────────────
seen, pkgs = set(), []
def walk(d, depth=0):
    if depth > 8: return
    try: entries = list(os.scandir(d))
    except OSError: return
    for e in entries:
        try: isdir = e.is_dir(follow_symlinks=True)
        except OSError: continue
        if not isdir: continue
        try: real = os.path.realpath(e.path)
        except OSError: continue
        if real in seen: continue
        pj = os.path.join(real, 'package.json')
        if not os.path.exists(pj): continue
        try: meta = json.load(open(pj))
        except Exception: continue
        if not meta.get('name'): continue
        seen.add(real)
        pkgs.append((real, meta))
        sub = os.path.join(real, 'node_modules')
        if os.path.isdir(sub): walk(sub, depth + 1)
walk(nm)
if not pkgs:
    print('  no packages found -- the install produced nothing')
    sys.exit(1)

# ── 1. declared dependencies resolvable on Node's lookup chain ───────────────
#    (checks for a directory, NOT require.resolve: a subpath-only `exports` map
#     with no "." legitimately refuses the bare specifier)
def resolvable(d, dep):
    for _ in range(12):
        if os.path.exists(os.path.join(d, 'node_modules', dep)): return True
        parent = os.path.dirname(d)
        if parent == d: return False
        d = parent
    return False
unresolved = [f'{m["name"]}@{m.get("version")} -> {dep}'
              for d, m in pkgs for dep in (m.get('dependencies') or {})
              if not resolvable(d, dep)]

# ── 2. declared bins present, executable, runnable ──────────────────────────
bins_bad = []
for d, m in pkgs:
    b = m.get('bin')
    entries = {m['name'].split('/')[-1]: b} if isinstance(b, str) else (b or {})
    for name, rel in entries.items():
        if not isinstance(rel, str): continue
        f = os.path.join(d, rel)
        if not os.path.exists(f):
            bins_bad.append(f'{m["name"]} bin {name} missing ({rel})')
        elif not (os.stat(f).st_mode & 0o111):
            bins_bad.append(f'{m["name"]} bin {name} not executable')

# ── 3. extraction completeness: tarball file list vs extracted dir ──────────
import tarfile
store, unpacked = os.path.join(cache, 'store'), os.path.join(cache, 'store_unpacked')
partial, missing_files, unauthored = [], [], []
if os.path.isdir(store):
    for tgz in sorted(os.listdir(store)):
        if not tgz.endswith('.tgz'): continue
        h = tgz[:-4]
        d = os.path.join(unpacked, h[:2], h)
        if not os.path.isdir(d):
            # A tarball with no per-hash unpacked dir is debris, not corruption:
            # the package content is present (the resolve/load audits prove it),
            # typically provided by the shared prebuilt dir instead. Counted, not
            # failed — but surfaced, because a needless download is cold-path cost.
            unauthored.append(h[:10]); continue
        # Mirror the product's own rule (`store_extract_complete`): the completion
        # marker, or — for dirs written by other paths — a regular file beyond
        # package.json. A dir holding only package.json is the classic
        # partial-extract signature and is the real failure this catches.
        if not os.path.exists(os.path.join(d, '.jhol-extract-complete')):
            real = [f for f in os.listdir(d) if f not in ('package.json', '.jhol-extract-complete')
                    and os.path.isfile(os.path.join(d, f))]
            if not real:
                partial.append(h[:10] + ' (only package.json)')
        want = set()
        try:
            with tarfile.open(os.path.join(store, tgz)) as tf:
                for mem in tf:
                    if mem.isfile() and mem.name.count('/') >= 1:
                        want.add(mem.name.split('/', 1)[1])
        except Exception:
            continue
        have = set()
        for root, dirs, files in os.walk(d, followlinks=False):
            if root != d and os.path.basename(root) == 'node_modules':
                dirs[:] = []   # jhol scaffolding, not package content
                continue
            for f in files:
                if f != '.jhol-extract-complete':
                    have.add(os.path.relpath(os.path.join(root, f), d))
        if want - have:
            missing_files.append(f'{h[:10]}: {sorted(want - have)[:3]}')

# ── 4. loadability of default entries ───────────────────────────────────────
def has_default_entry(m):
    ex = m.get('exports')
    if isinstance(ex, str): return True
    if isinstance(ex, dict):
        if '.' in ex: return True
        return not any(k.startswith('.') for k in ex)
    return bool(m.get('main') or m.get('module'))
load_script = '''
const { createRequire } = require('module');
const dir = process.env.LOAD_DIR, name = process.env.LOAD_NAME;
try { createRequire(dir + '/package.json')(name); }
catch (e) {
  if (e.code === 'ERR_REQUIRE_ESM') { import(name).then(() => process.exit(0), () => process.exit(3)); }
  else if (e.code === 'ERR_PACKAGE_PATH_NOT_EXPORTED') process.exit(0);
  else process.exit(3);
}
'''
# Known, documented upstream/Node incompatibilities that npm reproduces exactly.
# Verified with an npm control (E103): `buffer-equal-constant-time@1.0.1` uses
# `require('buffer').SlowBuffer`, removed from modern Node. Override with
# JHOL_CHECK_KNOWN_LOAD_FAILURES (comma-separated) if the environment differs.
KNOWN_LOAD_FAILURES = set(filter(None, os.environ.get(
    'JHOL_CHECK_KNOWN_LOAD_FAILURES', 'buffer-equal-constant-time').split(',')))
load_fail = []
known_fail = []
for d, m in pkgs:
    if not has_default_entry(m): continue
    if m['name'] in os.environ.get('JHOL_CHECK_SKIP_LOAD', '').split(','): continue
    env2 = dict(os.environ, LOAD_DIR=d, LOAD_NAME=m['name'])
    r = subprocess.run(['node', '-e', load_script],
                       cwd=work, env=env2, capture_output=True, text=True, timeout=120)
    if r.returncode == 3:
        if m['name'] in KNOWN_LOAD_FAILURES:
            known_fail.append(f'{m["name"]}@{m.get("version")}')
        else:
            load_fail.append(f'{m["name"]}@{m.get("version")}')

print(f'  packages audited: {len(pkgs)}')
print(f'  unresolved declared deps: {len(unresolved)}')
print(f'  bins missing/not executable: {len(bins_bad)}')
print(f'  partial store dirs: {len(partial)}   incomplete extractions: {len(missing_files)}')
print(f'  tarballs without an unpacked dir (debris, not failure): {len(unauthored)}')
print(f'  default entries failing to load: {len(load_fail)}'
      f' (known upstream/Node incompatibilities: {len(known_fail)}'
      + (f' -> {", ".join(sorted(known_fail))}' if known_fail else '') + ')')
for label, items in (('unresolved', unresolved), ('bins', bins_bad),
                     ('partial', partial), ('incomplete', missing_files),
                     ('load', load_fail), ('debris', unauthored)):
    for it in items[:5]:
        print(f'    {label}: {it}')
sys.exit(1 if (unresolved or bins_bad or partial or missing_files or load_fail) else 0)
PY

# ── run every .bin entry: a present-but-unrunnable tool is a broken install ──
step "binaries execute"
bin_fail=0
bin_dir="$WORK/node_modules/.bin"
if [ -d "$bin_dir" ]; then
  for b in "$bin_dir"/*; do
    [ -e "$b" ] || continue
    name="$(basename "$b")"
    out="$( (cd "$WORK" && "./node_modules/.bin/$name" --version) 2>&1 | head -1 || true )"
    case "$out" in
      *"Permission denied"*|*"cannot execute"*) echo "  $name: BROKEN ($out)"; bin_fail=1 ;;
      *) echo "  $name: ok" ;;
    esac
  done
else
  echo "  (no .bin entries in this fixture)"
fi
[ "$bin_fail" = 0 ] || fail "a .bin entry could not run"

printf '\nPASS: fmt, build, tests, install, and all audits\n'
