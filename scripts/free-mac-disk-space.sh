#!/usr/bin/env bash
# Free disk space on macOS - safe cleanups (run from your user, no sudo)
set -e

echo "=== Disk before ==="
df -h /

echo ""
echo "Cleaning user caches..."
CACHES=(
  "$HOME/Library/Caches/go-build"
  "$HOME/Library/Caches/gopls"
  "$HOME/Library/Caches/com.google.antigravity.ShipIt"
  "$HOME/Library/Caches/ms-playwright-go"
  "$HOME/Library/Caches/ms-playwright"
  "$HOME/Library/Caches/next-swc"
  "$HOME/Library/Caches/pnpm"
  "$HOME/Library/Caches/node-gyp"
  "$HOME/Library/Caches/typescript"
  "$HOME/Library/Caches/com.apple.python"
  "$HOME/Library/Caches/pip"
  "$HOME/Library/Caches/goimports"
  "$HOME/Library/Caches/Google/Chrome"
  "$HOME/Library/Caches/Arc"
)
for d in "${CACHES[@]}"; do
  if [ -d "$d" ]; then
    rm -rf "$d"/* 2>/dev/null || true
    echo "  cleared: $d"
  fi
done

echo "Cleaning Developer caches..."
[ -d "$HOME/Library/Developer/Xcode/DerivedData" ] && rm -rf "$HOME/Library/Developer/Xcode/DerivedData"/* 2>/dev/null || true
[ -d "$HOME/Library/Developer/CoreSimulator/Caches" ] && rm -rf "$HOME/Library/Developer/CoreSimulator/Caches"/* 2>/dev/null || true

echo "Cleaning logs (keep last 7 days)..."
find "$HOME/Library/Logs" -type f -mtime +7 -delete 2>/dev/null || true

echo "Emptying Trash..."
rm -rf "$HOME/.Trash/"* 2>/dev/null || true

# Cursor: remove redundant state backup (~15GB) and old logs
[ -f "$HOME/Library/Application Support/Cursor/User/globalStorage/state.vscdb.backup" ] && rm -f "$HOME/Library/Application Support/Cursor/User/globalStorage/state.vscdb.backup" && echo "Removed Cursor state backup (~15GB)"
[ -d "$HOME/Library/Application Support/Cursor/logs" ] && rm -rf "$HOME/Library/Application Support/Cursor/logs"/* 2>/dev/null && echo "Cleared Cursor logs"
# (Antigravity not touched per user preference)

if command -v npm &>/dev/null; then
  echo "Cleaning npm cache..."
  npm cache clean --force 2>/dev/null || true
fi
if command -v brew &>/dev/null; then
  echo "Cleaning Homebrew..."
  brew cleanup --prune=all -s 2>/dev/null || true
fi
if command -v pnpm &>/dev/null; then
  echo "Cleaning pnpm store (optional, run: pnpm store prune)"
  pnpm store prune 2>/dev/null || true
fi

echo ""
echo "=== Disk after ==="
df -h /
echo ""
echo "Done. For 'System Data' see README in this folder."
