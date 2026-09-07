# Update Failure Details — Specification

## Current State Analysis

When a backend update fails, the UI surfaces the failure in two places:

- **`src/ui/update_row.rs` — `set_status_error(msg)`** sets the inline status label to
  `Error: <first line of msg>` (ellipsized, `error` CSS class) and reveals a retry
  button. A separate "Show error details" button (`error_button`, opens
  `show_error_details_dialog`) is only made visible when `msg.lines().count() > 1`.
- **`src/ui/window.rs`** tints the hero subtitle ("Update finished with errors — see
  the log below") and appends backend output to the shared `LogPanel`.

The full command-output tail is only attached to `BackendError::Exit { message }`
(see `src/backends/mod.rs:25` and `src/runner.rs:388-400`). Other failure paths —
e.g. `src/backends/nix.rs:369` `Failed to read flake.lock: <e>` — produce a
single-line message. For those:

1. `error_button` stays hidden (single line), so the user has **no way** to see
   more than the ellipsized label.
2. The `LogPanel` is capped at `max_content_height(180)` and is not resizable, so
   the relevant `[Nix] …` lines may be scrolled out of view.

Reproduced by the reported screenshot: `Error: Command failed (exit -1): Failed to
rea…` with no details affordance.

## Problem Definition

On a failed update the user is told *that* it failed but cannot readily see *why*.
The details dialog already exists but is gated behind a multi-line message, and
single-line backend errors (the common Nix case) never expose it.

## Proposed Solution

Scope confirmed with user: **always show the details button on a failed row, and
have its dialog always include the captured terminal-output tail for that
backend.** (No auto-open dialog; no terminal-panel resize.)

### 1. `LogPanel::backend_tail(kind, max_lines) -> String`  (`src/ui/log_panel.rs`)

New read-only method. Scans the current text buffer for lines beginning with the
backend's log prefix (`[<kind>] `, matching the `format!("[{kind}] {line}")`
convention already used at every `BackendLog` call site), strips that prefix, and
returns the last `max_lines` joined with `\n`. Returns an empty string when the
backend produced no log lines.

### 2. `UpdateRow::set_status_error(msg, log_tail)`  (`src/ui/update_row.rs`)

- Add a `log_tail: &str` parameter.
- Build `error_details` as `msg`, followed by — when `log_tail` is non-empty and
  not already contained in `msg` — a blank line, a localised
  `"Recent output:"` header, and `log_tail`.
- Make `error_button` **always** visible on error (drop the
  `msg.lines().count() > 1` gate).
- Inline label behaviour unchanged (still `Error: <first line>`, ellipsized).

### 3. Call sites  (`src/ui/window.rs`)

Both `set_status_error` call sites (`apply_backend_finished` ~line 1302 and the
per-backend event loop ~line 1474) already have `log_panel` and the backend
`kind` in scope. Pass `log_panel.backend_tail(kind, 200)` as the new argument.

### 4. Dialog  (`show_error_details_dialog`, `src/ui/update_row.rs`)

No structural change required — it already renders arbitrary multi-line text in a
scrollable monospace view. Keep as-is.

## Implementation Steps

1. Add `backend_tail` to `LogPanel` → verify: `cargo build` compiles; unit test
   covers prefix match + tail truncation + empty result.
2. Extend `set_status_error` signature and body → verify: `cargo build`.
3. Update both call sites in `window.rs` → verify: `cargo build` (no other
   callers — confirmed via `grep -rn set_status_error src`).
4. `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` → verify: all clean.

## Dependencies

None. No new crates. Context7 not required (internal change, no new external API).

## Configuration Changes

None. One new user-visible string (`"Recent output:"`) — add to
`po/POTFILES.in` only if `src/ui/log_panel.rs` / `src/ui/update_row.rs` are not
already listed (they are), so no `po` changes expected; verify during Phase 3.

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| `backend_tail` prefix drifts from the `[{kind}] ` format | Prefix built from the same `Display` impl; add a test asserting the format. |
| Duplicated output when `msg` already carries the tail (Exit errors) | Skip appending when `msg.contains(log_tail)`. |
| Large buffers scanned on every failure | Bounded: scan happens once per failure, buffer is already line-capped at 5 000. |
| Plugin backends with `[]`-bearing ids | Prefix uses the full `[<id>] ` string; exact `starts_with` match, no partial. |
