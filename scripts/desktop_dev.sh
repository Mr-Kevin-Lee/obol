#!/usr/bin/env bash
# Launches the desktop app against a committed fixture set instead of real
# data (spec D43's mock seam) — no network, no providers, nothing read from
# or written to ~/Library/Application Support/obol.
#
#   scripts/desktop_dev.sh [typical|all-failed|empty]    (default: typical)
#
# Usually run as `make desktop-dev FIXTURE=<set>`. Requires
# `cargo install tauri-cli` (one-time, not a project dependency).
set -euo pipefail

fixture="${1:-typical}"
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
fixtures="$repo_root/crates/desktop/tests/fixtures"

if [[ ! -d "$fixtures/$fixture" ]]; then
  echo "no fixture set named '$fixture'; available: $(ls "$fixtures" | tr '\n' ' ')" >&2
  exit 1
fi

# Work on a scratch copy: Refresh writes a sources.yaml and a new snapshot
# into the storage directory, which must never be the committed fixtures.
scratch="$(mktemp -d -t obol-desktop-dev)"
cp -R "$fixtures/$fixture/." "$scratch/"
rm -f "$scratch/.gitkeep"

# Snapshot history is ordered by mtime, and git doesn't preserve mtimes,
# so a fresh checkout would order the fixtures arbitrarily. Set each one's
# mtime from its YYYY-MM-DD filename so newest-by-date is newest-by-mtime.
# crates/desktop/tests/dev_fixtures.rs stages fixtures the same way.
shopt -s nullglob
for snapshot in "$scratch"/snapshots/*.json; do
  date="$(basename "$snapshot" .json | tr -d '-')"
  touch -t "${date}0900" "$snapshot"
done

echo "fixture: $fixture"
echo "OBOL_STORAGE_DIR=$scratch"

cd "$repo_root/crates/desktop"
OBOL_STORAGE_DIR="$scratch" exec cargo tauri dev
