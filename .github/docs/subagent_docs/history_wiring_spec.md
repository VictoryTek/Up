# Spec: Wire up the Update History page (item 6)

## Current state analysis

- `src/history.rs` (`#![allow(dead_code)]`): fully working storage layer.
  `HistoryEntry { timestamp: u64, backend: String, result: String,
  updated_count: Option<usize>, error: Option<String> }`,
  `append_entry()`, `load_entries()`, `clear_history()`, `now_secs()`.
  JSONL file at `$XDG_DATA_HOME/up/history.jsonl`. Zero callers anywhere
  in the codebase (confirmed via grep).
- `src/ui/history_page.rs` (`#![allow(dead_code)]`): fully built
  `HistoryPage::build() -> gtk::Box` — a `PreferencesGroup` listing
  entries newest-first with icons per outcome, a "Clear" button wired to
  `history::clear_history()`, and an empty state. It already calls
  `crate::history::load_entries()` (line 88) and interprets
  `entry.result` as one of the literal strings `"success"`,
  `"success_self_update"`, `"error"`, or `"skipped"` (line 105-116); any
  other string falls through to a bare timestamp with no description
  (line 115: `_ => timestamp_str`). Already declared in the module tree
  (`src/ui/mod.rs:2: pub mod history_page;`) but never instantiated —
  `HistoryPage::build()` has zero callers.
- `src/ui/window.rs`: `ViewStack` built at lines 33-52 currently has two
  pages — `"update"` and `"upgrade"` — added via
  `view_stack.add_titled_with_icon(...)`. No third page exists.
- `crate::backends::UpdateResult` (`src/backends/mod.rs:102+`) variants:
  `Success { updated_count, updated_items }`,
  `SuccessWithSelfUpdate { updated_count, updated_items }`,
  `Error(BackendError)`, `Skipped(String)`, `Cancelled`, `CacheMiss`.
  `BackendKind` (same file) implements `Display` (`"APT"`, `"DNF"`, ...,
  `Plugin(id) => id`), giving a ready-made human string for
  `HistoryEntry::backend`.
- Two `BackendFinished` arms need wiring, per the master plan:
  - `src/ui/window.rs:545` — the "Update All" event loop (inside the
    `glib::spawn_future_local` started from the main update button).
  - `src/ui/window.rs:946` — the retry loop (per-row Retry button;
    inside a separate, structurally similar `glib::spawn_future_local`).
  A third `BackendFinished` arm exists at `src/ui/window.rs:1124`, inside
  `spawn_cache_bypass` (the VexOS cache-bypass flow triggered from the
  cache-block dialog). This is **not** listed in the master plan's file
  list for item 6 and represents a distinct, rarer operation (bypassing
  a Nix binary-cache stall) rather than a normal per-backend update
  outcome — left unwired to keep this change scoped to what's specified.
- No `log` crate usage anywhere in `src/ui/*.rs`; the existing convention
  for a non-critical, best-effort I/O call in this file is to discard the
  error (e.g. `history_page.rs:66`: `let _ =
  crate::history::clear_history();`). The history-writing call added here
  follows the same convention rather than introducing a new logging
  pattern.

## Problem definition

Both the storage layer and the UI page are complete but entirely
unreachable: nothing ever calls `history::append_entry()`, and nothing
ever adds `HistoryPage::build()` to the window's `ViewStack`.

## Proposed solution

1. Add a third `ViewStack` page ("History") in `UpWindow::build()`,
   always visible (unlike the "Upgrade" tab, which is conditionally
   hidden per-distro) since history has no distro-support gating.
2. Add a small free function `record_history_entry(kind: &BackendKind,
   result: &UpdateResult)` in `window.rs` that maps an `UpdateResult` to
   a `HistoryEntry` and calls `history::append_entry()`, discarding
   errors per the existing convention. Call it from both `BackendFinished`
   arms (Update All loop, retry loop) — the same mapping logic in both
   places, since the two loops handle the same event type and this keeps
   the mapping in one place rather than duplicating the match arms
   twice.
3. Result-string mapping (matches what `history_page.rs`'s renderer
   already understands):
   - `Success { updated_count, .. }` → `"success"`,
     `updated_count: Some(*updated_count)`, `error: None`
   - `SuccessWithSelfUpdate { updated_count, .. }` →
     `"success_self_update"`, `updated_count: Some(*updated_count)`,
     `error: None`
   - `Error(msg)` → `"error"`, `updated_count: None`,
     `error: Some(msg.to_string())`
   - `Skipped(_) | Cancelled | CacheMiss` → `"skipped"`,
     `updated_count: None`, `error: None` (all three are "nothing was
     applied" outcomes; `history_page.rs` already renders any
     `"skipped"` entry as a fixed `"— skipped"` subtitle without needing
     the original reason string)

## Implementation steps

1. `src/ui/window.rs`:
   - Add `use crate::ui::history_page::HistoryPage;` alongside the
     existing `use crate::ui::update_row::UpdateRow;` /
     `use crate::ui::upgrade_page::UpgradePage;` imports.
   - After the "Upgrade Page" block (~line 52), add:
     ```rust
     // --- History Page ---
     let history_page = HistoryPage::build();
     view_stack.add_titled_with_icon(
         &history_page,
         Some("history"),
         "History",
         "document-open-recent-symbolic",
     );
     ```
   - Simplify the ViewSwitcher visibility logic (lines ~86-92): since a
     History page is now always present, there are always ≥2 tabs, so
     the switcher itself should always stay visible. Only the "Upgrade"
     page's own visibility should still be gated by
     `info.upgrade_supported`:
     ```rust
     if !info.upgrade_supported {
         upgrade_stack_page.set_visible(false);
     }
     ```
     (drop the `view_switcher_async.set_visible(false/true)` calls and
     the now-inaccurate "hidden when only one tab is visible" comment
     above the `ViewSwitcher::builder()` call).
   - Add the `record_history_entry` free function (near
     `spawn_cache_bypass`, the other window-level free function).
   - Call `record_history_entry(&kind, &result)` in the
     `OrchestratorEvent::BackendFinished(kind, result)` arm at line 545,
     after the existing `show_cache_dialog` handling block.
   - Call `record_history_entry(&k, &result)` in the equivalent arm at
     line 946, after its `show_cache_dialog` handling block.

## Dependencies

None — no new crates. `serde`/`serde_json` (already dependencies) are
what `history.rs` already uses for JSONL serialization.

## Configuration changes

None.

## Risks and mitigations

- **Risk:** `history::append_entry()` does synchronous file I/O
  (`std::fs::OpenOptions`, `std::fs::create_dir_all`) called from inside
  a `glib::spawn_future_local` future, which runs on the GTK main thread
  — a slow/contended disk could momentarily block the UI.
  **Mitigation:** out of scope for this fix — the same
  `glib::spawn_future_local` context already does other synchronous
  work (`row.set_status_*`, dialog construction) and the write is a
  single small line-append to a local file; consistent with existing
  code's risk profile, not a regression introduced here. Not worth an
  async wrapper for a few bytes of local I/O per finished backend.
- **Risk:** Hiding history-writing errors (`let _ = ...`) could mask
  real problems (disk full, permissions). **Mitigation:** matches the
  existing convention for `clear_history()` in the same file; history
  is a non-critical convenience feature, and failing loudly here isn't
  part of the requested scope (master plan item 43, "silent error
  swallowing across several UI async paths", already tracks this
  pattern generally as a separate low-priority item).
- **Risk:** Always keeping the ViewSwitcher visible changes the
  single-tab experience for distros without upgrade support (previously
  the switcher was hidden entirely when only "Update" was usable).
  **Mitigation:** intentional and necessary — History is now always a
  second page, so the switcher must stay visible for users to reach it;
  this is a direct, required consequence of adding the third page, not
  scope creep.
