#!/usr/bin/env bash
# Development runner for Up.
#
# Sets XDG_DATA_DIRS to include the project's data/ directory so that GNOME Shell
# (and other desktop environments) can find the app's .desktop file and icon when
# running via `cargo run` without a system install.
#
# Usage: ./scripts/run-dev.sh [cargo run args...]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

export XDG_DATA_DIRS="$PROJECT_ROOT/data:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"

exec cargo run --manifest-path "$PROJECT_ROOT/Cargo.toml" "$@"
