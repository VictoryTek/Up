#!/usr/bin/env bash
set -euo pipefail

# Run from the repository root directory.

# ---------------------------------------------------------------------------
# Environment bootstrap
#
# On NixOS (or any system using the Nix flake dev shell) the GTK4 system
# libraries are only available inside `nix develop`.  If pkg-config is not
# on PATH we are outside the dev shell, so re-exec this script inside it.
# The IN_NIX_SHELL variable is set automatically by `nix develop` / `nix-shell`,
# so we use its absence as the signal to re-invoke.
# ---------------------------------------------------------------------------
if [[ -z "${IN_NIX_SHELL:-}" ]] && ! command -v pkg-config &>/dev/null; then
    if [[ -f flake.nix ]] && command -v nix &>/dev/null; then
        echo "Notice: pkg-config not found outside Nix dev shell — re-invoking via 'nix develop'."
        exec nix develop --command bash "$0" "$@"
    else
        echo "ERROR: pkg-config not found and Nix is not available."
        echo "       On Debian/Ubuntu:  sudo apt-get install pkg-config libgtk-4-dev libadwaita-1-dev"
        echo "       On Fedora:         sudo dnf install pkgconf gtk4-devel libadwaita-devel"
        echo "       On NixOS:          run this script inside 'nix develop'"
        exit 1
    fi
fi

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
