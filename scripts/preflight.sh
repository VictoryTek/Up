#!/usr/bin/env bash
set -euo pipefail

# Run from the repository root directory.

echo "--- Step 1: Formatting check (cargo fmt --check) ---"
cargo fmt --check

echo "--- Step 2: Lint check (cargo clippy -- -D warnings) ---"
cargo clippy -- -D warnings

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
    appstreamcli validate data/io.github.up.metainfo.xml
else
    echo "Notice: appstreamcli not found, skipping metainfo validation."
fi

echo "All preflight checks passed."
