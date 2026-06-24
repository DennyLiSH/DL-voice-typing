#!/usr/bin/env bash
set -e
git config core.hooksPath .githooks
chmod +x .githooks/pre-commit 2>/dev/null || true
command -v cargo >/dev/null || { echo "ERROR: cargo not in PATH"; exit 1; }
cargo fmt --version >/dev/null 2>&1 || echo "WARN: rustfmt not installed, commit will fail (rustup component add rustfmt)"
cargo clippy --version >/dev/null 2>&1 || echo "WARN: clippy not installed, commit will fail (rustup component add clippy)"
git --version | awk '{
  split($3, v, ".")
  if (v[1] < 2 || (v[1] == 2 && v[2] < 9)) print "WARN: git " $3 " < 2.9, core.hooksPath not supported"
}'
echo "hooks installed; core.hooksPath = $(git config --get core.hooksPath)"
echo "--- hook content summary (eyeball-check no non-cargo commands on first install) ---"
grep -vE "^\s*#|^\s*$" .githooks/pre-commit
