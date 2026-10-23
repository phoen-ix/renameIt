#!/usr/bin/env bash
# Render the docs screenshots: the snapshot suite (crates/ren-gui/tests/snapshots.rs,
# D248) with every image rewritten, then oxipng over the result.
#
#   scripts/docs-screenshots.sh [outdir]      # default: docs/screenshots
#
# Linux only, rendered by Mesa's lavapipe (apt: mesa-vulkan-drivers). No stack and
# no ports. OXIPNG: the optimiser (default: `oxipng` on PATH; skipped if absent).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
OUT="$(mkdir -p "${1:-docs/screenshots}" && cd "${1:-docs/screenshots}" && pwd)"

RENAMEIT_SHOTS="$OUT" UPDATE_SNAPSHOTS=force cargo test --locked -p ren-gui --test snapshots
# A forced update keeps each replaced image as <name>.old.png.
rm -f "$OUT"/*.old.png
if command -v "${OXIPNG:-oxipng}" >/dev/null; then
  "${OXIPNG:-oxipng}" -q -o 4 --strip safe "$OUT"/*.png
else
  echo "docs-screenshots: no oxipng; run oxipng -o 4 --strip safe on them before committing" >&2
fi
echo "docs-screenshots: $(find "$OUT" -maxdepth 1 -name '*.png' | wc -l) PNGs in $OUT"
