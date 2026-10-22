#!/usr/bin/env bash
# Snapshot the working tree without touching git (the repository forbids commits).
#
# Use before a large refactor or any scripted bulk edit:
#     scripts/snapshot_tree.sh            # ~/.jhol-tree-backups/<timestamp>/
#     scripts/snapshot_tree.sh --list
#     scripts/snapshot_tree.sh --restore <dir>
#
# Only source and docs are copied (never target/, node_modules/, .git/).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST_ROOT="${JHOL_TREE_BACKUPS:-$HOME/.jhol-tree-backups}"

case "${1:-}" in
  --list) ls -1 "$DEST_ROOT" 2>/dev/null || echo "(no snapshots)"; exit 0 ;;
  --restore)
    src="${2:?usage: snapshot_tree.sh --restore <snapshot-dir>}"
    [ -d "$src" ] || { echo "no such snapshot: $src" >&2; exit 1; }
    rsync -a --exclude 'target/' "$src/" "$ROOT/"
    echo "restored $(find "$src" -type f | wc -l | tr -d ' ') files from $src"
    exit 0 ;;
esac

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
DEST="$DEST_ROOT/$STAMP"
mkdir -p "$DEST"
for d in src crates tests scripts benchmarks binary-packages; do
  [ -d "$ROOT/$d" ] && rsync -a --exclude 'target/' --exclude 'node_modules/' "$ROOT/$d" "$DEST/"
done
# docs/ includes user/, engineering/, plans/, archive/
[ -d "$ROOT/docs" ] && rsync -a "$ROOT/docs" "$DEST/"
[ -d "$ROOT/Documentation" ] && rsync -a "$ROOT/Documentation" "$DEST/"
for f in Cargo.toml Cargo.lock README.md CHANGELOG.md AGENTS.md INNOVATION_LOG.md install.sh install_jhol.bat; do
  [ -f "$ROOT/$f" ] && cp "$ROOT/$f" "$DEST/"
done
n=$(find "$DEST" -type f | wc -l | tr -d ' ')
echo "snapshot: $n files -> $DEST"
echo "restore with: scripts/snapshot_tree.sh --restore $DEST"
