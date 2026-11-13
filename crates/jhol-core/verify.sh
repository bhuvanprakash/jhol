#!/bin/sh
# jhol-core (installer) quality gate.
#
# The engine has crates/jhol-vm/verify.sh; this is the same discipline for the
# package manager: unit tests, the adversarial tarball corpus, and a
# real-world end-to-end offline install with tree determinism.
set -u
cd "$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)" || exit 1
note() { printf '%s\n' "$*"; }
fail() { note "INSTALLER GATE: FAIL ($*)"; exit 1; }

note "== build =="
cargo build -q --release -p jhol-core || fail "core build"
cargo build -q --release --features jholvm || fail "CLI build"

note "== unit tests =="
cargo test -q -p jhol-core --lib || fail "unit tests"

note "== adversarial install corpus =="
cargo test -q -p jhol-core --lib adversarial || fail "adversarial tests"

note "== end-to-end: offline install + determinism =="
fixture="$PWD/bench_cold"
jhol="$PWD/target/release/jhol"
sandbox=$(mktemp -d) || fail "create sandbox"
trap 'rm -rf "$sandbox"' 0
trap 'exit 1' HUP INT TERM
mkdir "$sandbox/project" "$sandbox/home" "$sandbox/cache" || fail "prepare sandbox"
cp "$fixture/package.json" "$fixture/package-lock.json" "$sandbox/project/" || fail "copy fixture"
cd "$sandbox/project" || fail "enter sandbox"

# Seed the pinned fixture in isolation so cache GC never touches the user's cache.
HOME="$sandbox/home"
USERPROFILE="$HOME"
JHOL_CACHE_DIR="$sandbox/cache"
export HOME USERPROFILE JHOL_CACHE_DIR
install() {
  "$jhol" install --native-only --frozen-lockfile --no-scripts "$@" >install.log 2>&1 || {
    tail -n 12 install.log
    fail "install $*"
  }
}

tree_hash() {
  python3 - <<'DIGEST'
import hashlib
import json
import os
from pathlib import Path

root = Path("node_modules")
if not root.is_dir():
    raise SystemExit("no installed tree")
digest = hashlib.sha256()
files = 0
def failed_walk(error):
    raise error

for directory, children, names in os.walk(root, onerror=failed_walk):
    folder = Path(directory)
    links = [name for name in children if (folder / name).is_symlink()]
    children[:] = sorted(set(children) - set(links))
    for name in sorted(names + links + children):
        entry = folder / name
        if entry.is_symlink():
            value = ["link", os.readlink(entry)]
        elif entry.is_dir():
            value = ["directory"]
        elif entry.is_file():
            value = ["file", hashlib.sha256(entry.read_bytes()).hexdigest()]
            files += 1
        else:
            raise SystemExit(f"unsupported tree entry: {entry}")
        record = [entry.relative_to(root).as_posix(), value]
        digest.update(json.dumps(record).encode() + b"\n")
if files == 0:
    raise SystemExit("installed tree is empty")
print(digest.hexdigest())
DIGEST
}

install
rm -rf node_modules || fail "clear seed tree"
install --offline
first=$(tree_hash) || fail "hash first offline install"
rm -rf node_modules || fail "clear first offline tree"
install --offline
second=$(tree_hash) || fail "hash second offline install"
[ "$first" = "$second" ] || fail "offline trees differ"
note "  ok   deterministic offline tree"

if ! "$jhol" cache gc --from package-lock.json >gc.log 2>&1; then
  tail -n 12 gc.log
  fail "cache gc"
fi
rm -rf node_modules || fail "clear second offline tree"
install --offline
after_gc=$(tree_hash) || fail "hash offline install after cache gc"
[ "$first" = "$after_gc" ] || fail "offline tree differs after cache gc"
note "  ok   cache gc keeps referenced entries"
note "INSTALLER GATE: PASS"
