#!/usr/bin/env bash
# The end-to-end suite: the GUI driven through its accessibility tree
# (egui_kittest, crates/ren-gui/tests/gui.rs and e2e.rs, D24) and ren-cli run as a
# process (crates/ren-cli/tests). docs/COVERAGE.md maps every feature to its test.
#
#   scripts/e2e.sh [-- test-binary args]    e.g. scripts/e2e.sh -- --test-threads=2
#
# A native app: no stack, no ports (PORT_BASE is ignored) and nothing to tear down.
# ARTIFACTS: where e2e.log goes (default: a fresh temp dir).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
ARTIFACTS="${ARTIFACTS:-$(mktemp -d)}"
mkdir -p "$ARTIFACTS"

# Both halves run even when the first fails, so one log names every failure.
{
  rc=0
  cargo test --locked -p ren-gui --test gui --test e2e "$@" || rc=$?
  cargo test --locked -p ren-cli --tests "$@" || rc=$?
  exit "$rc"
} 2>&1 | tee "$ARTIFACTS/e2e.log"
echo "e2e: log in $ARTIFACTS/e2e.log"
