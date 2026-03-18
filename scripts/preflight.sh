#!/usr/bin/env bash
set -euo pipefail

# Run from the repository root directory.

echo "--- Step 1: Formatting check (cargo fmt --check) ---"
if cargo fmt --version &>/dev/null 2>&1; then
    cargo fmt --check
else
    echo "Notice: rustfmt not found, skipping formatting check."
fi

echo "--- Step 2: Lint check (cargo clippy -- -D warnings) ---"
if cargo clippy --version &>/dev/null 2>&1; then
    cargo clippy -- -D warnings
else
    echo "Notice: clippy not found, skipping lint check."
fi

echo "--- Step 3: Build verification (cargo build) ---"
cargo build

echo "--- Step 4: Test execution (cargo test) ---"
cargo test

echo "--- Step 5: Validate desktop entry file ---"
if command -v desktop-file-validate &>/dev/null; then
    desktop-file-validate data/io.github.up.desktop
else
    echo "Notice: desktop-file-validate not found, skipping desktop entry validation."
fi

echo "--- Step 6: Validate AppStream metainfo file ---"
if command -v appstreamcli &>/dev/null; then
    # --no-net: skip URL reachability checks (not appropriate for local CI)
    appstreamcli validate --no-net data/io.github.up.metainfo.xml
else
    echo "Notice: appstreamcli not found, skipping metainfo validation."
fi

echo "All preflight checks passed."
