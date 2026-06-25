#!/usr/bin/env bash
# Assert NSIS installMode stays perMachine. Regression guard for the
# perMachine installer switch (commit c56be60, 2026-06-24). Runs in CI
# before the heavy Rust/Vulkan setup so a regression fails in seconds.
set -euo pipefail

CONF="src-tauri/tauri.conf.json"
if [ ! -f "$CONF" ]; then
    echo "[check-nsis-installmode] FAILED: $CONF not found"
    exit 1
fi

mode=$(jq -r '.bundle.windows.nsis.installMode' "$CONF")
echo "[check-nsis-installmode] installMode=$mode"
if [ "$mode" != "perMachine" ]; then
    echo "[check-nsis-installmode] FAILED: expected perMachine, got '$mode'"
    echo "  If this change is intentional, update this script + release notes."
    exit 1
fi
echo "[check-nsis-installmode] OK"
