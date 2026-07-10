# VexOS Cache-Block Dialog — Specification

**Feature:** Popup dialog with bypass options when a VexOS update is blocked by binary-cache lag
**Spec file:** `.github/docs/subagent_docs/vexos_cache_block_dialog_spec.md`
**Date:** 2026-07-09

---

## 1. Current State Analysis

### 1.1 Existing cache-miss detection

`src/backends/nix.rs::run_update()` (VexOS branch, ~line 468-490) already runs
`vexos-update` via a persistent `pkexec sh` shell and maps a non-zero exit
code of `2` to `UpdateResult::CacheMiss` (a unit variant defined in
`src/backends/mod.rs:120`):

```rust
Err(BackendError::Exit { code: 2, .. }) => UpdateResult::CacheMiss,
```

`vexos-update`'s stdout/stderr (merged) is streamed line-by-line as
`BackendEvent::LogLine` → `OrchestratorEvent::BackendLog(kind, line)` →
`window.rs` appends `"[{kind}] {line}"` to the log panel. The raw lines
(before the `[Nix]` prefix is added) contain the block detail, e.g.:

```
VEXOS_CACHE_BLOCK: Update paused — kernel packages require a
VEXOS_CACHE_BLOCK: local source build (typically 1-3 days until Hydra caches them):
VEXOS_CACHE_BLOCK:   linux-7.1.2-modules.drv
VEXOS_CACHE_BLOCK:   linux-7.1.2-modules-shrunk.drv
VEXOS_CACHE_BLOCK:
VEXOS_CACHE_BLOCK: flake.lock restored. No changes were applied.
VEXOS_CACHE_BLOCK: Options:
VEXOS_CACHE_BLOCK:   just deploy     — apply config changes without bumping nixpkgs
VEXOS_CACHE_BLOCK:   just update     — retry in 1-3 days once Hydra has built them
VEXOS_CACHE_BLOCK:   just update-all — force local compile now (may take hours)
```

Today `UpdateResult::CacheMiss` only causes
`row.set_status_skipped("Binary cache syncing, try again later")` in two
places in `src/ui/window.rs` (~line 565 and ~line 913). No dialog, and no
way to invoke `just deploy` / `just update-all` from the UI.

### 1.2 Relevant existing patterns to reuse

- `src/ui/reboot_dialog.rs` — the established pattern for a one-off
  `adw::AlertDialog` with responses that trigger a privileged background
  action and report failure via a second dialog. This feature follows the
  same shape.
- `src/orchestrator.rs::UpdateOrchestrator::run_all()` — spawns a background
  thread, authenticates once via `PrivilegedShell`, forwards `BackendLog`
  events, and reports `BackendFinished` / `AllFinished`. `run_cache_bypass`
  (new) mirrors this for a single ad hoc command instead of a full backend
  list.
- `src/runner.rs::CommandRunner` / `PrivilegedShell` — already used for the
  existing `vexos-update` invocation; reused unchanged.
- `count_nix_store_operations` in `nix.rs` — already parses "these N
  derivations will be built" style output; reused for the bypass commands'
  result counts.

### 1.3 Row status API

`src/ui/update_row.rs` already exposes `set_status_running`,
`set_status_success(count)`, `set_status_error(msg)`, `set_status_skipped(msg)`
— sufficient for reporting the bypass command's outcome on the existing Nix
row. No new row-status method is needed.

---

## 2. Problem Definition

When VexOS blocks an update because kernel packages require a local
source build, the user currently only sees a terse skipped-row message in
the terminal/log panel. They have no visibility into *which* packages are
blocking the update, nor any UI-driven way to choose one of the three
options the CLI output already describes (`just deploy`, `just update`,
`just update-all`). The user must drop to a terminal themselves.

## 3. Proposed Solution

When `BackendFinished(BackendKind::Nix, UpdateResult::CacheMiss)` is
received, in addition to the existing row-status update, present a modal
`adw::AlertDialog` (pattern: `reboot_dialog.rs`) that:

- Shows the blocked-package detail extracted from the accumulated raw log
  lines for that run (`VEXOS_CACHE_BLOCK:` prefixed lines, prefix stripped).
- Offers three responses:
  - **Just Deploy** (`Suggested` appearance) — runs
    `just deploy` in `/etc/nixos` as root; applies pending config changes
    without bumping nixpkgs (no local compile required).
  - **Update All Now** (`Destructive` appearance, since it can take hours) —
    runs `just update-all` in `/etc/nixos` as root; forces the local
    source build immediately.
  - **Wait** (default / close response) — dismisses the dialog and closes
    the Up application window (per user decision — see below), since the
    only remaining action is to retry in 1–3 days once Hydra has cached
    the build.

Choosing "Just Deploy" or "Just Update All" runs the corresponding command
through a new, independent privileged run (its own polkit prompt) and
reports progress/result on the existing Nix row and log panel, exactly as
a normal update does. Choosing "Wait" closes the top-level window (which,
since Up does not call `Application::hold()`, quits the GTK application).

### 3.1 New: `CacheBypassMode` and `run_cache_bypass` (`src/backends/nix.rs`)

```rust
/// Which `just` recipe to run when the user chooses to bypass a
/// VEXOS_CACHE_BLOCK hold from the cache-block dialog.
pub(crate) enum CacheBypassMode {
    Deploy,
    UpdateAll,
}

impl CacheBypassMode {
    fn just_recipe(&self) -> &'static str {
        match self {
            CacheBypassMode::Deploy => "deploy",
            CacheBypassMode::UpdateAll => "update-all",
        }
    }
}

/// Run `just deploy` / `just update-all` in `/etc/nixos` as root, via the
/// same pkexec + PATH-restoration pattern used for `vexos-update`.
pub(crate) async fn run_cache_bypass(
    mode: CacheBypassMode,
    runner: &dyn CommandExecutor,
) -> UpdateResult {
    let cmd = format!(
        "cd /etc/nixos && stdbuf -oL -eL just {}",
        mode.just_recipe()
    );
    match runner
        .run(
            "pkexec",
            &[
                "env",
                "PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin",
                "sh",
                "-c",
                &cmd,
            ],
        )
        .await
    {
        Ok(output) => UpdateResult::Success {
            updated_count: count_nix_store_operations(&output),
        },
        Err(e) => UpdateResult::Error(e),
    }
}
```

### 3.2 New: pure helper to extract block detail (`src/backends/nix.rs`)

```rust
/// Extract the human-readable VEXOS_CACHE_BLOCK explanation from the raw
/// (unprefixed) log lines captured during a single backend run. Returns
/// `None` if no such lines were present.
pub(crate) fn extract_cache_block_message(lines: &[String]) -> Option<String> {
    let msgs: Vec<&str> = lines
        .iter()
        .filter_map(|l| l.strip_prefix("VEXOS_CACHE_BLOCK:"))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if msgs.is_empty() {
        None
    } else {
        Some(msgs.join("\n"))
    }
}
```

Unit tests cover: empty input → `None`; the example output from the bug
report → the 6 non-empty explanatory/derivation lines, prefix stripped.

### 3.3 New: `run_cache_bypass` orchestrator entry point (`src/orchestrator.rs`)

A free function alongside `UpdateOrchestrator`/`CleanupOrchestrator`, since
this is a single ad hoc privileged command rather than a backend list:

```rust
/// Runs a single VexOS cache-bypass command (`just deploy` / `just
/// update-all`) on a background thread, authenticating once via
/// `PrivilegedShell` and reporting progress through the same
/// `OrchestratorEvent` stream used by `UpdateOrchestrator`.
pub fn run_cache_bypass(
    mode: crate::backends::nix::CacheBypassMode,
    tx: async_channel::Sender<OrchestratorEvent>,
) {
    spawn_background(move || async move {
        let _ = tx.send(OrchestratorEvent::AuthStarted).await;
        let shell = match PrivilegedShell::new().await {
            Ok(s) => Arc::new(tokio::sync::Mutex::new(s)),
            Err(e) => {
                let _ = tx.send(OrchestratorEvent::AuthFailed(e)).await;
                return;
            }
        };
        let _ = tx.send(OrchestratorEvent::AuthSucceeded).await;

        let (be_tx, be_rx) = async_channel::unbounded::<BackendEvent>();
        let tx_fwd = tx.clone();
        let fwd_handle = tokio::spawn(async move {
            while let Ok(event) = be_rx.recv().await {
                let BackendEvent::LogLine(k, line) = event;
                let _ = tx_fwd.send(OrchestratorEvent::BackendLog(k, line)).await;
            }
        });

        let kind = BackendKind::Nix;
        let _ = tx.send(OrchestratorEvent::BackendStarted(kind.clone())).await;
        let runner = CommandRunner::new(be_tx.clone(), kind.clone(), Some(shell.clone()));
        let result = crate::backends::nix::run_cache_bypass(mode, &runner).await;
        let _ = tx.send(OrchestratorEvent::BackendFinished(kind, result)).await;

        drop(be_tx);
        let _ = fwd_handle.await;
        shell.lock().await.close().await;
        let _ = tx.send(OrchestratorEvent::AllFinished).await;
    });
}
```

Reuses `OrchestratorEvent`, `BackendEvent`, `CommandRunner`, `PrivilegedShell`
exactly as `UpdateOrchestrator::run_all` does — no new channel/event types.

### 3.4 New: `src/ui/cache_block_dialog.rs`

```rust
/// Present the VexOS cache-block dialog. `details` is the extracted
/// VEXOS_CACHE_BLOCK explanation (already newline-joined, prefix-stripped).
/// `on_deploy` / `on_update_all` are invoked when the user picks the
/// respective bypass option. Choosing "Wait" closes the window containing
/// `parent` (which quits Up, since it does not hold an extra Application
/// reference).
pub fn show_cache_block_dialog(
    parent: &(impl gtk::prelude::IsA<gtk::Widget> + Clone),
    details: &str,
    on_deploy: impl Fn() + 'static,
    on_update_all: impl Fn() + 'static,
)
```

Body text: fixed explanation ("VexOS paused this update because some
packages require a local source build...") followed by `details` verbatim.
Responses: `wait` (label "Wait", default + close response),
`deploy` (label "Just Deploy", `Suggested` appearance),
`update-all` (label "Update All Now", `Destructive` appearance).
`connect_response`: `"deploy"` → `on_deploy()`; `"update-all"` →
`on_update_all()`; `"wait"` (or any other/close response) → resolve
`parent.root()`, downcast to `gtk::Window`, call `.close()`.

### 3.5 `src/ui/window.rs` changes (two call sites: main run-all loop
~line 498-587, and the per-row retry loop ~line 856-926)

At both sites:

1. Add a plain `let mut nix_log_lines: Vec<String> = Vec::new();` local
   before the `while let Ok(event) = event_rx.recv().await` loop (no
   `Rc`/`RefCell` needed — both loops are single sequential async tasks).
2. In the `OrchestratorEvent::BackendLog(kind, line)` arm, when
   `kind == BackendKind::Nix`, push `line.clone()` onto `nix_log_lines`
   (in addition to the existing `log_panel.append_line(...)` call).
3. In the `OrchestratorEvent::BackendFinished(kind, result)` arm's
   `UpdateResult::CacheMiss` match case, after the existing
   `row.set_status_skipped(...)` call, additionally:
   ```rust
   let details = crate::backends::nix::extract_cache_block_message(&nix_log_lines)
       .unwrap_or_else(|| "No further detail was provided.".to_string());
   crate::ui::cache_block_dialog::show_cache_block_dialog(
       &button, // or update_button_spawn at the retry site
       &details,
       glib::clone!(
           #[strong] rows, #[strong] log_panel, #[weak] status_label, #[weak] button,
           move || spawn_cache_bypass(CacheBypassMode::Deploy, rows.clone(), log_panel.clone(), status_label.clone(), button.clone())
       ),
       glib::clone!(
           #[strong] rows, #[strong] log_panel, #[weak] status_label, #[weak] button,
           move || spawn_cache_bypass(CacheBypassMode::UpdateAll, rows.clone(), log_panel.clone(), status_label.clone(), button.clone())
       ),
   );
   ```
   (Exact `glib::clone!` capture list adapted per call site's existing
   variable names, e.g. `update_button_spawn` at the retry site.)

4. New module-level function in `window.rs` (not duplicated — shared by
   both call sites):
   ```rust
   fn spawn_cache_bypass(
       mode: crate::backends::nix::CacheBypassMode,
       rows: Rc<RefCell<Vec<(BackendKind, UpdateRow)>>>,
       log_panel: LogPanel,
       status_label: gtk::Label,
       button: gtk::Button,
   )
   ```
   Spawns `glib::spawn_future_local`, calls
   `crate::orchestrator::run_cache_bypass(mode, tx)`, and drives a small
   event loop handling `AuthStarted` / `AuthSucceeded` / `AuthFailed` /
   `BackendStarted` / `BackendLog` / `BackendFinished` / `AllFinished` on
   the existing Nix row (found via `rows.borrow().iter().find(|(k,_)| *k
   == BackendKind::Nix)`), setting `button` insensitive while running and
   restoring it afterward. Mirrors the existing retry-loop shape in
   `window.rs` (~line 850-945) but scoped to the single Nix row.

No changes to `UpdateResult`, `BackendError`, or the `CacheMiss` variant's
shape — this keeps the diff additive and avoids touching the already
tested exit-code-2 detection path.

---

## 4. Implementation Steps

1. `src/backends/nix.rs`: add `CacheBypassMode`, `run_cache_bypass`,
   `extract_cache_block_message`, plus unit tests for
   `extract_cache_block_message`.
2. `src/orchestrator.rs`: add free function `run_cache_bypass`.
3. `src/ui/cache_block_dialog.rs`: new file, `show_cache_block_dialog`.
4. `src/ui/mod.rs` (or wherever UI submodules are declared): add
   `pub mod cache_block_dialog;`.
5. `src/ui/window.rs`: wire up both `CacheMiss` match arms plus the shared
   `spawn_cache_bypass` function as described in 3.5.
6. Build (`cargo build`), test (`cargo test`), lint (`cargo fmt --check`,
   `cargo clippy -- -D warnings`).

## 5. Dependencies

No new external dependencies. Uses existing `adw`, `gtk4`, `tokio`,
`async-channel` already in `Cargo.toml`. Context7 verification not
required per policy (internal code change, no new dependency).

## 6. Configuration Changes

None. No new D-Bus policy, GResource, or desktop-file changes — the dialog
uses only existing `libadwaita` widgets already linked into the binary.

## 7. Risks and Mitigations

- **Risk:** `just` may not be on `PATH` inside the restored
  `PATH=/run/current-system/sw/bin:...` environment used for pkexec.
  **Mitigation:** Out of scope to guarantee — `just` availability on
  VexOS is an OS-level packaging concern, matching how `vexos-update`
  itself is assumed to be on that PATH. If `just` is absent the command
  exits non-zero and the existing `Error` path reports it on the row.
- **Risk:** Running "Just Update All" can take hours (per the CLI
  message) with only the existing 1-hour `COMMAND_TIMEOUT` in
  `PrivilegedShell::run_command`. **Mitigation:** Explicitly out of scope
  for this change — the timeout is a pre-existing constant shared by all
  privileged commands; raising it is a separate concern the user has not
  asked for. Documented here so the review phase does not flag it as an
  unaddressed regression.
- **Risk:** Closing the window on "Wait" is unusual UX (most alert
  dialogs don't quit the app). **Mitigation:** This is an explicit user
  decision (confirmed via clarifying question), not an inferred default.
- **Risk:** Duplicating the `CacheMiss` handling across two call sites in
  `window.rs`. **Mitigation:** matches the file's existing pattern (the
  `CacheMiss` arm itself, and the whole event loop shape, are already
  duplicated between the two sites); `spawn_cache_bypass` is factored out
  as a single shared function to avoid duplicating the bypass-execution
  logic itself.

---

## Summary

Adds a modal dialog, shown when VexOS reports a binary-cache hold, that
surfaces the blocked-package detail already present in the log stream and
lets the user pick `just deploy`, `just update-all`, or wait (which closes
Up). Implemented via one new pure helper, one new backend function, one
new orchestrator entry point, and one new UI dialog module, wired into the
two existing `CacheMiss` handling sites in `window.rs`. No changes to
existing public result/error types.
