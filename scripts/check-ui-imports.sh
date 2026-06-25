#!/usr/bin/env bash
# Scan ui/ JS files for imports that escape the ui/ directory (would break Tauri WebView).
# Relies on bash + POSIX ERE grep (Git for Windows 自带).
# Known gaps: dynamic import(`${x}`), multi-line imports, URL-form imports —
# these are caught by the vitest contract test at __tests__/ui-import-boundary.test.js
set -euo pipefail

# Escape hatch for false positives (e.g., newly-introduced npm package matched by mistake).
if [ "${CHECK_UI_IMPORTS_ALLOW:-0}" = "1" ]; then
    echo "[check-ui-imports] skipped (CHECK_UI_IMPORTS_ALLOW=1)"
    exit 0
fi

matches=$(grep -rnE "from\s+['\"](\.\./|/)" ui/ --include='*.js' || true)
if [ -n "$matches" ]; then
    echo "[check-ui-imports] FAILED: ui/ files must not import from outside ui/ (frontendDist=../ui):"
    echo "$matches"
    exit 1
fi
echo "[check-ui-imports] OK"
