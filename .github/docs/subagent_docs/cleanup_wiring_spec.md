# Spec: Wire up Cleanup / maintenance mode (item 8)

## Current state analysis

- `src/orchestrator.rs:255-319` (`#[allow(dead_code)]` on the struct and
  its `impl` block): `CleanupOrchestrator::new(backends)` +
  `run_all(&self, tx: async_channel::Sender<OrchestratorEvent>)` —
  fully implemented, mirrors `UpdateOrchestrator::run_all` exactly (same
  one-time `PrivilegedShell` auth if any backend needs root, same
  `BackendLog`/`BackendStarted`/`BackendFinished`/`AllFinished` event
  stream via the same `OrchestratorEvent` enum). **No `CancelHandle` is
  returned** (`run_all`'s return type is `()`, unlike
  `UpdateOrchestrator::run_all` which returns `CancelHandle`) — cleanup
  cannot be cancelled once started; this is an existing, deliberate
  asymmetry in the orchestrator, not something this item needs to add.
  Zero callers anywhere (confirmed via grep).
- Every backend implements `supports_cleanup()` / `run_cleanup()`:
  `os_package_manager.rs` (Apt/Dnf/Pacman/Zypper), `nix.rs`,
  `flatpak.rs`, `homebrew.rs`, and the generic `PluginBackend`
  (`src/plugins/backend.rs`). Default trait impl in
  `src/backends/mod.rs:178` returns `false` for backends that don't
  override it (`fwupd.rs` has no cleanup — confirmed no override there).
- `src/ui/window.rs`: no cleanup entry point anywhere. The application
  overflow menu (`app_menu`, ~line 135-136) currently has exactly one
  item, "About Up" → `win.about`, registered via
  `window.add_action(&about_action)` (a `gio::SimpleAction`) in
  `build()`. `build_update_page()` (called from `build()`) is where
  `detected` (`Rc<RefCell<Vec<Arc<dyn Backend>>>>`), `log_panel`,
  `status_label`, `update_button`, and `updating`
  (`Rc<Cell<bool>>`) all live — none of these are currently exposed
  outside `build_update_page()` except `updating` (returned as the 5th
  tuple element, aliased `update_in_progress` in `build()`).
  `UpdatePageResult` (line 14-20) is the named tuple type returned by
  `build_update_page()`; `run_checks: Rc<dyn Fn()>` (built at line
  717-837) is the existing precedent for exposing a page-internal action
  as a callable handed back to `build()` — the header's Refresh button
  (line 114-125) already consumes it this way, guarded by `if
  update_in_progress.get() { return; } (*run_checks)()`.
  `spawn_cache_bypass` (line ~1094+) is the existing precedent for a
  free function that runs an orchestrator, streams
  `OrchestratorEvent`s into `log_panel`/`status_label`, and
  disables/re-enables a button around the run — but it *also* updates
  individual `UpdateRow` statuses via `rows`, because it's reporting
  progress for a single already-visible Nix row mid-Update-All-flow.
  Cleanup is a distinct, standalone action (not part of an in-flight
  update), so following `spawn_cache_bypass`'s per-row status updates
  would overload `UpdateRow::set_status_success`'s "N updated" wording
  with a "N removed" meaning — this spec instead reports cleanup
  progress purely through `log_panel` + `status_label` (see below),
  avoiding that semantic mismatch and avoiding touching Update-flow row
  state that a user might be visually relying on for "how many updates
  are pending" at the same time.
- `updating: Rc<Cell<bool>>` is already the app's single source of truth
  for "an operation that shouldn't overlap with another is in progress"
  — the Refresh button, the retry-loop closures, and (implicitly) the
  disabled `update_button` all respect it. Reusing it for cleanup gives
  mutual exclusion between Update All / Refresh / Retry / Cleanup for
  free, without new state.

## Problem definition

`CleanupOrchestrator` and every backend's cleanup logic are fully built
and wired together, but there is no UI entry point — no menu item, no
button — so the feature is completely unreachable.

## Proposed solution

1. Add a "Clean Up" item to the existing application overflow menu,
   backed by a new `win.cleanup` `gio::SimpleAction`, following the exact
   pattern already used for `win.about`.
2. Expose a `run_cleanup: Rc<dyn Fn()>` closure from
   `build_update_page()` (added to `UpdatePageResult`), built the same
   way `run_checks` is — capturing `detected`, `log_panel`,
   `status_label`, `update_button`, and `updating`.
3. The closure: guard on `updating.get()` (no-op if something else is
   running), filter `detected` for backends where `supports_cleanup()`
   is true, show a status message and return early if none qualify,
   otherwise set `updating.set(true)`, disable `update_button`, clear the
   log panel, and delegate to a new free function `spawn_cleanup(...)`
   (structurally parallel to `spawn_cache_bypass`) that runs
   `CleanupOrchestrator::run_all()` and streams events into
   `log_panel`/`status_label`, restoring `updating`/`update_button` on
   `AuthFailed` or `AllFinished`.
4. Remove `#[allow(dead_code)]` from `CleanupOrchestrator` and its impl
   block now that it has a caller.

## Implementation steps

1. `src/ui/window.rs`:
   - Extend `UpdatePageResult` (line 14-20) with a sixth element,
     `Rc<dyn Fn()>`, for the cleanup handler.
   - In `build_update_page()`, right after the existing `run_checks`
     block (~line 837), add:
     ```rust
     let run_cleanup: Rc<dyn Fn()> = {
         let detected = detected.clone();
         let log_panel = log_panel.clone();
         let status_label = status_label.clone();
         let update_button = update_button.clone();
         let updating = updating.clone();
         Rc::new(move || {
             if updating.get() {
                 return;
             }
             let cleanup_backends: Vec<Arc<dyn Backend>> = detected
                 .borrow()
                 .iter()
                 .filter(|b| b.supports_cleanup())
                 .cloned()
                 .collect();
             if cleanup_backends.is_empty() {
                 status_label.set_label("No cleanup available for detected backends.");
                 return;
             }
             updating.set(true);
             update_button.set_sensitive(false);
             log_panel.clear();
             status_label.set_label("Starting cleanup\u{2026}");
             spawn_cleanup(
                 cleanup_backends,
                 log_panel.clone(),
                 status_label.clone(),
                 update_button.clone(),
                 updating.clone(),
             );
         })
     };
     ```
   - Add `run_cleanup` to the final `(page_box, run_checks, distro_row,
     version_row, updating)` return tuple (~line 1086).
   - Add a new free function (near `spawn_cache_bypass`):
     ```rust
     /// Runs the cleanup/maintenance sequence for every backend that
     /// supports it, reporting progress through the log panel and status
     /// label. `update_button` is disabled and `updating` set for the
     /// duration to keep this mutually exclusive with Update All /
     /// Refresh / Retry, matching how those paths already gate on
     /// `updating`.
     fn spawn_cleanup(
         backends: Vec<Arc<dyn Backend>>,
         log_panel: LogPanel,
         status_label: gtk::Label,
         update_button: gtk::Button,
         updating: Rc<Cell<bool>>,
     ) {
         use crate::orchestrator::{CleanupOrchestrator, OrchestratorEvent};

         let (event_tx, event_rx) = async_channel::unbounded::<OrchestratorEvent>();
         CleanupOrchestrator::new(backends).run_all(event_tx);

         glib::spawn_future_local(async move {
             let mut has_error = false;
             while let Ok(event) = event_rx.recv().await {
                 match event {
                     OrchestratorEvent::AuthStarted => {
                         log_panel.append_line("Requesting administrator privileges\u{2026}");
                     }
                     OrchestratorEvent::AuthSucceeded => {
                         status_label.set_label("Cleaning up\u{2026}");
                     }
                     OrchestratorEvent::AuthFailed(e) => {
                         log_panel.append_line(&format!("Authentication failed: {e}"));
                         status_label.set_label("Cleanup cancelled.");
                         updating.set(false);
                         update_button.set_sensitive(true);
                         return;
                     }
                     OrchestratorEvent::BackendStarted(kind) => {
                         log_panel.append_line(&format!(
                             "\u{2500}\u{2500}\u{2500} Cleaning {kind} \u{2500}\u{2500}\u{2500}"
                         ));
                     }
                     OrchestratorEvent::BackendLog(kind, line) => {
                         log_panel.append_line(&format!("[{kind}] {line}"));
                     }
                     OrchestratorEvent::BackendFinished(kind, result) => {
                         match &result {
                             UpdateResult::Success { updated_count, .. }
                             | UpdateResult::SuccessWithSelfUpdate { updated_count, .. } => {
                                 log_panel.append_line(&format!(
                                     "[{kind}] Cleanup finished ({updated_count} removed)"
                                 ));
                             }
                             UpdateResult::Error(msg) => {
                                 log_panel.append_line(&format!("[{kind}] Cleanup failed: {msg}"));
                                 has_error = true;
                             }
                             UpdateResult::Skipped(msg) => {
                                 log_panel.append_line(&format!("[{kind}] Skipped: {msg}"));
                             }
                             UpdateResult::Cancelled => {
                                 log_panel.append_line(&format!("[{kind}] Cancelled"));
                             }
                             UpdateResult::CacheMiss => {
                                 log_panel.append_line(&format!(
                                     "[{kind}] Binary cache syncing, try again later"
                                 ));
                             }
                         }
                     }
                     OrchestratorEvent::AllFinished => break,
                 }
             }
             status_label.set_label(if has_error {
                 "Cleanup completed with errors."
             } else {
                 "Cleanup complete."
             });
             updating.set(false);
             update_button.set_sensitive(true);
         });
     }
     ```
   - In `build()`: destructure the new 6th tuple element (name it
     `run_cleanup`), add `app_menu.append(Some("Clean Up"),
     Some("win.cleanup"));` before the existing "About Up" entry, and
     register the action:
     ```rust
     let cleanup_action = gio::SimpleAction::new("cleanup", None);
     cleanup_action.connect_activate(move |_, _| (*run_cleanup)());
     window.add_action(&cleanup_action);
     ```
2. `src/orchestrator.rs`: remove the two `#[allow(dead_code)]`
   attributes on `CleanupOrchestrator` (struct and impl block) now that
   it has a caller.

## Dependencies

None — no new crates; reuses `async_channel`, `glib::spawn_future_local`,
and the existing `OrchestratorEvent`/`CleanupOrchestrator` machinery.

## Configuration changes

None.

## Risks and mitigations

- **Risk:** Running cleanup while an update is in-flight (or vice versa)
  could race on the same `PrivilegedShell`/backend state.
  **Mitigation:** both paths already gate on the shared `updating` flag;
  cleanup's closure returns immediately if `updating.get()` is true, and
  setting `updating.set(true)` during cleanup blocks Update All (button
  disabled directly), Refresh (already checks `update_in_progress`), and
  Retry (`updating_retry.get()` guard) for its duration.
- **Risk:** No cancellation for cleanup (orchestrator returns no
  `CancelHandle`). **Mitigation:** matches the orchestrator's existing,
  deliberate design — out of scope to add; cleanup operations
  (`apt autoremove`, `nix-collect-garbage`, etc.) are typically short and
  safe to let finish.
- **Risk:** Reusing `log_panel`/`status_label` means cleanup output
  interleaves visually with the same panel used for updates, which could
  confuse a user who has the log panel open from a previous update
  session. **Mitigation:** `log_panel.clear()` at the start of cleanup
  (matching how the Update All flow does `log_panel.clear()` at its own
  start) gives a clean slate; acceptable given both operations already
  share one log panel by design (there being only one panel in the UI).
- **Risk:** `supports_cleanup()` returning `false` for every detected
  backend leaves the user with only a transient status-label message and
  no menu-item graying-out. **Mitigation:** consistent with keeping this
  minimal per the master plan's stated scope ("Add a 'Clean Up' menu
  entry that drives `CleanupOrchestrator::run_all()`") — dynamically
  updating GAction `enabled` state based on live backend detection would
  add meaningfully more plumbing (the menu/action is built once in
  `build()`, before detection completes) for a rare edge case (a system
  with zero cleanup-capable backends detected).
