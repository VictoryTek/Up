# Cleanup / Maintenance Actions — Implementation Specification

> Feature: `cleanup_maintenance`  
> Author: Research Subagent (Phase 1)  
> Date: 2026-05-07  
> Status: DRAFT — ready for implementation

---

## 1. Current State Analysis

### 1.1 Backend Trait (`src/backends/mod.rs`)

The `Backend` trait currently exposes:

| Method | Description |
|--------|-------------|
| `kind()` | Returns `BackendKind` enum variant |
| `display_name()` | Human-readable backend name |
| `description()` | Short description string |
| `icon_name()` | GTK icon name |
| `run_update(&runner)` | Performs the update operation |
| `needs_root()` | Whether update requires `pkexec` (default: `false`) |
| `count_available()` | Count packages with updates (default: delegates to `list_available`) |
| `list_available()` | Return pending package names (default: `Ok(vec![])`) |

There is **no** cleanup/maintenance method on the trait. Backends have no way to declare or perform post-update cleanup.

### 1.2 Orchestration (`src/orchestrator.rs`)

`UpdateOrchestrator` owns a `Vec<Arc<dyn Backend>>` and runs them in sequence via `run_all(tx: Sender<OrchestratorEvent>)`. Events are streamed back to the GTK main thread:

```
AuthStarted → AuthSucceeded / AuthFailed
BackendStarted(kind) → BackendLog(kind, line)... → BackendFinished(kind, result)
AllFinished
```

There is no parallel `CleanupOrchestrator`. Cleanup must be triggered separately from the update flow.

### 1.3 Command Execution (`src/executor.rs`, `src/runner.rs`)

`CommandExecutor` trait:
```rust
fn run<'a>(&'a self, program: &'a str, args: &'a [&'a str])
    -> Pin<Box<dyn Future<Output = Result<String, BackendError>> + Send + 'a>>;
```

`CommandRunner` wraps `Option<Arc<Mutex<PrivilegedShell>>>`:
- If `shell` is `Some(_)`, sends the command through the persistent `pkexec sh` process.
- If `shell` is `None`, spawns the command as an unprivileged child process.

The program name is passed verbatim — so `runner.run("pkexec", &["apt", "autoremove", "-y"])` is the correct invocation pattern for privileged cleanup, matching how `run_update` works across all OS-level backends.

### 1.4 UI Layer

**`src/ui/update_row.rs`** — `UpdateRow` exposes status-setting methods:
- `set_status_checking()`, `set_status_running()`, `set_status_success(count)`, `set_status_error(msg)`, `set_status_skipped(msg)`, `set_status_unknown(msg)`

No cleanup-specific status methods exist.

**`src/ui/window.rs`** — The header bar has:
- `refresh_button` (left, runs `run_checks()`)
- `menu_button` (right, opens `app_menu`) — currently contains only **"About Up"**

The "Update All" `gtk::Button` lives in the scrollable content area with `css_classes: ["suggested-action", "pill"]`. An `update_in_progress: Rc<Cell<bool>>` flag gates concurrent operations.

### 1.5 Privilege Model (Current)

| Backend | `needs_root()` | Mechanism |
|---------|---------------|-----------|
| APT | `true` | `pkexec` prefixed via `runner.run("pkexec", ...)` |
| DNF | `true` | same |
| Pacman | `true` | same |
| Zypper | `true` | same |
| Flatpak | `false` | Direct `flatpak` or `flatpak-spawn --host` |
| Homebrew | `false` | Direct `brew` |
| Nix | `false` (default) | Direct `nix` |

---

## 2. Problem Definition and Scope

### 2.1 Problem

Package managers accumulate orphaned packages, unused runtimes, and stale caches over time. Users have no way to trigger cleanup within the Up UI without switching to a terminal. The CODEBASE_ANALYSIS progress tracker lists **"Cleanup / Maintenance actions"** as the next planned quick-win feature.

### 2.2 Scope

**In scope:**
- Add a `run_cleanup` method to the `Backend` trait with a no-op default.
- Add a `supports_cleanup` method returning `false` by default; backends that implement cleanup override to `true`.
- Implement `run_cleanup` + `supports_cleanup` for all seven backends.
- Add a `CleanupOrchestrator` to `src/orchestrator.rs`.
- Add two new status-display methods to `UpdateRow`.
- Add a "Run Maintenance" item to the existing header bar menu.
- Add a `win.maintenance` `SimpleAction` handler in `window.rs`.

**Out of scope:**
- Per-backend opt-in checkboxes (all detected, non-skipped backends run cleanup).
- Auto-running cleanup after every update (future option).
- A separate "Cleanup" tab or dedicated UI page.
- New Cargo dependencies.

---

## 3. Proposed UI Design

### 3.1 Entry Point: Header Bar Menu

Add a second item to `app_menu` in `window.rs::build()`:

```rust
let app_menu = gio::Menu::new();
app_menu.append(Some("Run Maintenance"), Some("win.maintenance"));
app_menu.append(Some("About Up"), Some("win.about"));
let menu_button = gtk::MenuButton::builder()
    .icon_name("open-menu-symbolic")
    .menu_model(&app_menu)
    .tooltip_text("Main menu")
    .build();
```

"Run Maintenance" appears above "About Up". Clicking it triggers the `win.maintenance` action.

### 3.2 Win Action: `win.maintenance`

Register a `gio::SimpleAction::new("maintenance", None)` on the window. The action:

1. Guards against concurrent operations: returns immediately if `update_in_progress.get()`.
2. Sets `update_in_progress.set(true)`.
3. Clears the log panel.
4. Collects all non-skipped detected backends that return `supports_cleanup() == true`.
5. Creates a `CleanupOrchestrator` and calls `run_all(event_tx)`.
6. Processes events in `glib::spawn_future_local`, updating `UpdateRow` status and `LogPanel`.
7. Resets `update_in_progress.set(false)` on completion.

The `update_in_progress` flag gates **both** "Update All" and "Run Maintenance". This prevents concurrent privileged operations.

### 3.3 UpdateRow Status Display

Add two new public methods to `UpdateRow`:

```rust
pub fn set_status_cleaning(&self) {
    self.retry_button.set_visible(false);
    self.skip_checkbox.set_sensitive(false);
    self.spinner.set_visible(true);
    self.spinner.set_spinning(true);
    self.status_label.set_label("Cleaning…");
    self.status_label.set_css_classes(&["accent"]);
}

pub fn set_status_cleaned(&self, removed: usize) {
    self.retry_button.set_visible(false);
    self.skip_checkbox.set_sensitive(true);
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    let msg = if removed == 0 {
        "Already clean".to_string()
    } else {
        format!("{removed} removed")
    };
    self.status_label.set_label(&msg);
    self.status_label.set_css_classes(&["success"]);
}
```

For cleanup errors, the existing `set_status_error(msg)` is reused.

### 3.4 Log Panel Output

All cleanup log lines are prefixed identically to update logs:
```
[APT] Removing unused packages…
[Flatpak] Nothing unused to uninstall.
```

The log panel is **cleared** at the start of maintenance (same as update). A separator line is prepended for clarity:
```
─── Maintenance started ───
```

---

## 4. Backend Trait Changes (`src/backends/mod.rs`)

### 4.1 New Trait Methods

Add two new methods to the `Backend` trait:

```rust
/// Whether this backend supports a cleanup / maintenance operation.
/// Default: false. Override to true in backends that implement run_cleanup.
fn supports_cleanup(&self) -> bool {
    false
}

/// Run the cleanup/maintenance operation for this backend, streaming output
/// through `runner`. Returns UpdateResult where `updated_count` is the number
/// of packages removed (0 = already clean).
/// Default: no-op, returns Success { updated_count: 0 }.
fn run_cleanup<'a>(
    &'a self,
    runner: &'a dyn CommandExecutor,
) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
    let _ = runner; // suppress unused warning
    Box::pin(async { UpdateResult::Success { updated_count: 0 } })
}
```

`supports_cleanup` is a synchronous method (no I/O) used to filter the backend list before building the `CleanupOrchestrator`.

### 4.2 Import Addition

`run_cleanup` uses `CommandExecutor`, which is already in scope in `mod.rs` via `use crate::executor::CommandExecutor;`.

---

## 5. CleanupOrchestrator (`src/orchestrator.rs`)

Add a `CleanupOrchestrator` struct parallel to `UpdateOrchestrator`. It reuses `OrchestratorEvent` — the event semantics are identical:

- `BackendStarted(kind)` signals the row to show a cleaning spinner.
- `BackendLog(kind, line)` streams output to the log panel.
- `BackendFinished(kind, result)` carries the cleanup result.
- `AllFinished` signals the UI to re-enable controls.

```rust
pub struct CleanupOrchestrator {
    backends: Vec<Arc<dyn Backend>>,
}

impl CleanupOrchestrator {
    pub fn new(backends: Vec<Arc<dyn Backend>>) -> Self {
        Self { backends }
    }

    pub fn run_all(&self, tx: async_channel::Sender<OrchestratorEvent>) {
        let backends = self.backends.clone();
        spawn_background(move || async move {
            let any_needs_root = backends.iter().any(|b| b.needs_root());

            let shell: Option<Arc<tokio::sync::Mutex<PrivilegedShell>>> = if any_needs_root {
                let _ = tx.send(OrchestratorEvent::AuthStarted).await;
                match PrivilegedShell::new().await {
                    Ok(s) => Some(Arc::new(tokio::sync::Mutex::new(s))),
                    Err(e) => {
                        let _ = tx.send(OrchestratorEvent::AuthFailed(e)).await;
                        return;
                    }
                }
            } else {
                None
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

            for backend in &backends {
                let kind = backend.kind();
                let _ = tx.send(OrchestratorEvent::BackendStarted(kind)).await;
                let runner = CommandRunner::new(be_tx.clone(), kind, shell.clone());
                let result = backend.run_cleanup(&runner).await;
                let _ = tx
                    .send(OrchestratorEvent::BackendFinished(kind, result))
                    .await;
            }

            drop(be_tx);
            let _ = fwd_handle.await;

            if let Some(s) = shell {
                s.lock().await.close().await;
            }

            let _ = tx.send(OrchestratorEvent::AllFinished).await;
        });
    }
}
```

The `CleanupOrchestrator` uses `needs_root()` to decide whether to open a privileged shell. This is correct because all four root-requiring cleanup commands (APT, DNF, Pacman, Zypper) belong to backends where `needs_root()` returns `true`.

---

## 6. Per-Backend Implementation

### 6.1 APT (`src/backends/os_package_manager.rs`)

```rust
fn supports_cleanup(&self) -> bool { true }

fn run_cleanup<'a>(
    &'a self,
    runner: &'a dyn CommandExecutor,
) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
    Box::pin(async move {
        match runner
            .run(
                "pkexec",
                &[
                    "sh",
                    "-c",
                    "DEBIAN_FRONTEND=noninteractive apt autoremove -y",
                ],
            )
            .await
        {
            Ok(output) => {
                let removed = count_apt_autoremovals(&output);
                UpdateResult::Success { updated_count: removed }
            }
            Err(e) => UpdateResult::Error(e),
        }
    })
}
```

Add a new private parser:

```rust
pub(crate) fn count_apt_autoremovals(output: &str) -> usize {
    // apt autoremove output: "N to remove" or "0 upgraded, 0 newly installed, N to remove"
    for line in output.lines() {
        if line.contains("to remove") {
            for word in line.split_whitespace() {
                if let Ok(n) = word.parse::<usize>() {
                    return n;
                }
            }
        }
    }
    0
}
```

**Privilege:** `pkexec` via persistent shell (same as `run_update`).

### 6.2 DNF (`src/backends/os_package_manager.rs`)

```rust
fn supports_cleanup(&self) -> bool { true }

fn run_cleanup<'a>(
    &'a self,
    runner: &'a dyn CommandExecutor,
) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
    Box::pin(async move {
        match runner
            .run("pkexec", &["dnf", "autoremove", "-y"])
            .await
        {
            Ok(output) => {
                let removed = count_dnf_autoremovals(&output);
                UpdateResult::Success { updated_count: removed }
            }
            Err(e) => UpdateResult::Error(e),
        }
    })
}
```

Add a new private parser:

```rust
pub(crate) fn count_dnf_autoremovals(output: &str) -> usize {
    // DNF4: "  Remove  N Packages"
    // DNF5: "  Removing: N packages"
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Remove ") || trimmed.starts_with("Removing:") {
            for word in trimmed.split_whitespace() {
                if let Ok(n) = word.parse::<usize>() {
                    return n;
                }
            }
        }
    }
    0
}
```

**Privilege:** `pkexec` via persistent shell.

### 6.3 Pacman — Zero-Orphan Edge Case (`src/backends/os_package_manager.rs`)

The naive `pacman -Rns $(pacman -Qtdq)` fails with a usage error when `pacman -Qtdq` produces empty output (no orphans). The implementation must handle this:

**Strategy:** Run `pacman -Qtdq` unprivileged first. If output is empty, return success immediately. If orphans exist, pass them as explicit arguments to the privileged `pacman -Rns`.

```rust
fn supports_cleanup(&self) -> bool { true }

fn run_cleanup<'a>(
    &'a self,
    runner: &'a dyn CommandExecutor,
) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
    Box::pin(async move {
        // Step 1: Check for orphans (unprivileged, direct tokio spawn).
        let qtdq_out = match tokio::process::Command::new("pacman")
            .args(["-Qtdq"])
            .output()
            .await
        {
            Ok(o) => o,
            Err(e) => {
                return UpdateResult::Error(BackendError::Spawn(e.to_string()));
            }
        };

        // `pacman -Qtdq` exits non-zero (1) when there are no orphans on some
        // Pacman versions; treat any exit as "no orphans" when stdout is empty.
        let stdout = String::from_utf8_lossy(&qtdq_out.stdout);
        let orphans: Vec<String> = stdout
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();

        if orphans.is_empty() {
            // No orphans — nothing to do. This is NOT an error.
            return UpdateResult::Success { updated_count: 0 };
        }

        // Step 2: Remove orphans with privilege.
        // Build a single &[&str] slice: ["pacman", "-Rns", "--noconfirm", pkg1, pkg2, ...]
        let mut args: Vec<&str> = vec!["pacman", "-Rns", "--noconfirm"];
        args.extend(orphans.iter().map(|s| s.as_str()));

        match runner.run("pkexec", &args).await {
            Ok(_) => UpdateResult::Success {
                updated_count: orphans.len(),
            },
            Err(e) => UpdateResult::Error(e),
        }
    })
}
```

**Critical notes:**
- `pacman -Qtdq` exits with code 1 on some versions when no orphans exist. This must not be misinterpreted as an error; the stdout check is the authoritative signal.
- The orphan package names come from the user's own package database and do not contain shell metacharacters. However, because `runner.run` routes through `PrivilegedShell::run_command` which validates args for `\n`/`\r`/`\0`, any malformed name is rejected before execution.
- `--noconfirm` prevents interactive prompts from blocking the process inside the persistent shell.

**Privilege:** `pkexec` via persistent shell.

### 6.4 Zypper (`src/backends/os_package_manager.rs`)

Zypper has `zypper packages --orphaned` to list and `zypper remove -y <pkgs>` to remove. Unlike Pacman, `zypper remove` with no arguments is safe (it prompts for a package name), so we check first regardless.

```rust
fn supports_cleanup(&self) -> bool { true }

fn run_cleanup<'a>(
    &'a self,
    runner: &'a dyn CommandExecutor,
) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
    Box::pin(async move {
        // Step 1: List orphaned packages (unprivileged).
        let list_out = match tokio::process::Command::new("zypper")
            .args(["--no-color", "packages", "--orphaned"])
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .output()
            .await
        {
            Ok(o) => o,
            Err(e) => {
                return UpdateResult::Error(BackendError::Spawn(e.to_string()));
            }
        };

        if !list_out.status.success() {
            return UpdateResult::Error(BackendError::Exit {
                code: list_out.status.code().unwrap_or(-1),
                message: String::from_utf8_lossy(&list_out.stderr).to_string(),
            });
        }

        let stdout = String::from_utf8_lossy(&list_out.stdout);
        let orphans: Vec<String> = parse_zypper_orphaned(&stdout);

        if orphans.is_empty() {
            return UpdateResult::Success { updated_count: 0 };
        }

        // Step 2: Remove orphans with privilege.
        let mut args: Vec<&str> = vec!["sh", "-c"];
        // Build the zypper remove command as a single shell string to avoid
        // issues with variable-length arg list through pkexec.
        let pkg_list = orphans.join(" ");
        let cmd = format!(
            "LANG=C LC_ALL=C zypper remove -y {}",
            pkg_list
        );
        let args_final: Vec<&str> = vec!["sh", "-c", &cmd];

        match runner.run("pkexec", &args_final).await {
            Ok(_) => UpdateResult::Success {
                updated_count: orphans.len(),
            },
            Err(e) => UpdateResult::Error(e),
        }
    })
}
```

Add the Zypper orphan parser:

```rust
pub(crate) fn parse_zypper_orphaned(output: &str) -> Vec<String> {
    // `zypper packages --orphaned` output uses the same pipe-delimited table as
    // `zypper list-updates`. Rows with an 'i' status mark are installed packages.
    // Format: "| Status | Name | Version | Arch | Repository |"
    // Lines starting with "i" after the header are installed orphaned packages.
    output
        .lines()
        .filter(|l| l.trim_start().starts_with("i ") || l.trim_start().starts_with("i|"))
        .filter_map(|l| {
            // Split on '|', take 3rd field (0-indexed: 2) which is package name.
            l.split('|').nth(2).map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .collect()
}
```

**Security note for Zypper:** The orphan package names are injected into a shell string via `format!`. Zypper returns package names from the system package database; these should not contain shell metacharacters. However, as a defense-in-depth measure, the implementation should validate that each package name matches `[A-Za-z0-9._+-]+` before constructing the command string. Add this validation:

```rust
fn is_safe_pkg_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

// Filter orphans to safe names only:
let orphans: Vec<String> = parse_zypper_orphaned(&stdout)
    .into_iter()
    .filter(|n| is_safe_pkg_name(n))
    .collect();
```

**Privilege:** `pkexec` via persistent shell.

### 6.5 Nix (`src/backends/nix.rs`)

```rust
fn supports_cleanup(&self) -> bool { true }

fn run_cleanup<'a>(
    &'a self,
    runner: &'a dyn CommandExecutor,
) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
    Box::pin(async move {
        // nix-collect-garbage -d deletes old profile generations and
        // collects unreachable store paths. Unprivileged on non-NixOS Nix
        // (user profile only). On NixOS, system-level GC also requires root
        // but is handled separately by nixos-rebuild; here we collect user
        // profile garbage only without root.
        match runner.run("nix-collect-garbage", &["-d"]).await {
            Ok(output) => {
                let freed = count_nix_freed_paths(&output);
                UpdateResult::Success { updated_count: freed }
            }
            Err(e) => UpdateResult::Error(e),
        }
    })
}
```

Add a parser:

```rust
/// Count store paths freed by `nix-collect-garbage -d`.
/// Output contains lines like: "1234 store paths deleted, 567.89 MiB freed"
pub(crate) fn count_nix_freed_paths(output: &str) -> usize {
    for line in output.lines() {
        if line.contains("store paths deleted") {
            if let Some(n_str) = line.split_whitespace().next() {
                return n_str.parse::<usize>().unwrap_or(0);
            }
        }
    }
    0
}
```

**Privilege:** None (`needs_root()` returns `false` for `NixBackend`). The runner will spawn `nix-collect-garbage` directly as the current user.

**Note on NixOS system GC:** Full system garbage collection (`sudo nix-collect-garbage -d`) is intentionally excluded here. System-level GC is a post-upgrade operation that is more destructive (removes old system generations). This can be addressed in a future enhancement. User-level GC is safe and valuable on its own.

### 6.6 Flatpak (`src/backends/flatpak.rs`)

```rust
fn supports_cleanup(&self) -> bool { true }

fn run_cleanup<'a>(
    &'a self,
    runner: &'a dyn CommandExecutor,
) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
    Box::pin(async move {
        let (cmd, args) = build_flatpak_cmd(&["uninstall", "--unused", "-y"]);
        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        match runner.run(&cmd, &args_refs).await {
            Ok(output) => {
                // Count lines that look like actual removal operations.
                // Flatpak prints "Uninstalling: <ref>" for each removed runtime/app.
                let removed = output
                    .lines()
                    .filter(|l| l.trim().starts_with("Uninstalling:"))
                    .count();
                UpdateResult::Success { updated_count: removed }
            }
            Err(e) => UpdateResult::Error(e),
        }
    })
}
```

**Privilege:** None. Flatpak `uninstall --unused` operates on user installation without root.  
**Sandbox-aware:** Uses the existing `build_flatpak_cmd` helper, which correctly prefixes with `flatpak-spawn --host` when inside the Flatpak sandbox.

### 6.7 Homebrew (`src/backends/homebrew.rs`)

```rust
fn supports_cleanup(&self) -> bool { true }

fn run_cleanup<'a>(
    &'a self,
    runner: &'a dyn CommandExecutor,
) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
    Box::pin(async move {
        // Step 1: Remove unused formulae (dependencies no longer required).
        if let Err(e) = runner.run("brew", &["autoremove"]).await {
            return UpdateResult::Error(e);
        }
        // Step 2: Remove old versions, stale downloads, broken symlinks.
        match runner.run("brew", &["cleanup"]).await {
            Ok(output) => {
                let removed = count_brew_cleaned(&output);
                UpdateResult::Success { updated_count: removed }
            }
            Err(e) => UpdateResult::Error(e),
        }
    })
}
```

Add a parser:

```rust
pub(crate) fn count_brew_cleaned(output: &str) -> usize {
    // brew cleanup prints "Removing <formula>: ..." for each item cleaned.
    output
        .lines()
        .filter(|l| l.trim().starts_with("Removing "))
        .count()
}
```

**Privilege:** None. Homebrew runs fully as the current user.

---

## 7. Privilege Model for Cleanup

| Backend | `needs_root()` | Cleanup Privilege | Mechanism |
|---------|---------------|-------------------|-----------|
| APT | `true` | Root required | `pkexec sh -c "apt autoremove -y"` |
| DNF | `true` | Root required | `pkexec dnf autoremove -y` |
| Pacman | `true` | Root required | `pkexec pacman -Rns --noconfirm <orphans...>` |
| Zypper | `true` | Root required | `pkexec sh -c "zypper remove -y <orphans...>"` |
| Flatpak | `false` | None | `flatpak uninstall --unused -y` |
| Homebrew | `false` | None | `brew autoremove && brew cleanup` |
| Nix | `false` | None | `nix-collect-garbage -d` |

The `CleanupOrchestrator` uses `backend.needs_root()` to decide whether to open a `PrivilegedShell`, exactly mirroring `UpdateOrchestrator`. This is correct because the four OS-level backends that need root for updates also need root for cleanup. No new privilege-detection method is required.

---

## 8. Window UI Handler (`src/ui/window.rs`)

### 8.1 Menu Item

In `build()`:

```rust
let app_menu = gio::Menu::new();
app_menu.append(Some("Run Maintenance"), Some("win.maintenance"));
app_menu.append(Some("About Up"), Some("win.about"));
```

### 8.2 Action Registration

After the `about_action` registration:

```rust
let maintenance_action = gio::SimpleAction::new("maintenance", None);
maintenance_action.connect_activate(glib::clone!(
    #[weak]
    window,
    #[strong]
    rows,
    #[strong]
    detected,
    #[strong]
    log_panel,
    #[strong]
    update_in_progress,
    #[weak]
    status_label,
    #[upgrade_or]
    return,
    move |_, _| {
        if update_in_progress.get() {
            return;
        }
        update_in_progress.set(true);
        log_panel.clear();
        log_panel.append_line("\u{2500}\u{2500}\u{2500} Maintenance started \u{2500}\u{2500}\u{2500}");
        status_label.set_label("Running maintenance\u{2026}");

        // Collect non-skipped backends that support cleanup.
        let backends: Vec<Arc<dyn Backend>> = {
            let detected_borrow = detected.borrow();
            let rows_borrow = rows.borrow();
            detected_borrow
                .iter()
                .filter(|b| b.supports_cleanup())
                .filter(|b| {
                    rows_borrow
                        .iter()
                        .find(|(k, _)| *k == b.kind())
                        .map(|(_, r)| !r.is_skipped())
                        .unwrap_or(true)
                })
                .cloned()
                .collect()
        };

        if backends.is_empty() {
            status_label.set_label("No maintenance actions available.");
            update_in_progress.set(false);
            return;
        }

        glib::spawn_future_local(glib::clone!(
            #[strong]
            rows,
            #[strong]
            log_panel,
            #[strong]
            update_in_progress,
            #[weak]
            status_label,
            async move {
                use crate::orchestrator::{CleanupOrchestrator, OrchestratorEvent};

                let orchestrator = CleanupOrchestrator::new(backends);
                let (event_tx, event_rx) =
                    async_channel::unbounded::<OrchestratorEvent>();
                orchestrator.run_all(event_tx);

                let mut auth_started = false;
                let mut has_error = false;

                while let Ok(event) = event_rx.recv().await {
                    match event {
                        OrchestratorEvent::AuthStarted => {
                            auth_started = true;
                            status_label.set_label("Authenticating\u{2026}");
                            log_panel.append_line(
                                "Requesting administrator privileges\u{2026}",
                            );
                        }
                        OrchestratorEvent::AuthSucceeded => {
                            if auth_started {
                                log_panel.append_line("Authentication successful.");
                            }
                            status_label.set_label("Running maintenance\u{2026}");
                        }
                        OrchestratorEvent::AuthFailed(e) => {
                            log_panel.append_line(&format!(
                                "Authentication failed: {e}"
                            ));
                            status_label.set_label("Maintenance cancelled.");
                            update_in_progress.set(false);
                            return;
                        }
                        OrchestratorEvent::BackendStarted(kind) => {
                            let rows_borrowed = rows.borrow();
                            if let Some((_, row)) =
                                rows_borrowed.iter().find(|(k, _)| *k == kind)
                            {
                                row.set_status_cleaning();
                            }
                        }
                        OrchestratorEvent::BackendLog(kind, line) => {
                            log_panel.append_line(&format!("[{kind}] {line}"));
                        }
                        OrchestratorEvent::BackendFinished(kind, result) => {
                            let rows_borrowed = rows.borrow();
                            if let Some((_, row)) =
                                rows_borrowed.iter().find(|(k, _)| *k == kind)
                            {
                                match &result {
                                    UpdateResult::Success { updated_count } => {
                                        row.set_status_cleaned(*updated_count);
                                    }
                                    UpdateResult::Error(e) => {
                                        row.set_status_error(&e.to_string());
                                        has_error = true;
                                    }
                                    UpdateResult::Skipped(msg) => {
                                        row.set_status_skipped(msg);
                                    }
                                    UpdateResult::SuccessWithSelfUpdate { updated_count } => {
                                        row.set_status_cleaned(*updated_count);
                                    }
                                }
                            }
                        }
                        OrchestratorEvent::AllFinished => break,
                    }
                }

                if has_error {
                    status_label.set_label("Maintenance completed with errors.");
                } else {
                    status_label.set_label("Maintenance complete.");
                }
                update_in_progress.set(false);
            }
        ));
    }
));
window.add_action(&maintenance_action);
```

### 8.3 Concurrency Guard

The existing `update_in_progress: Rc<Cell<bool>>` already gates the "Update All" `button.connect_clicked` callback and the header `refresh_button`. The maintenance action also reads and writes this flag, ensuring that:
- Maintenance cannot start while an update is running.
- An update cannot be triggered while maintenance is running.

The refresh button already guards on `update_in_progress.get()`:
```rust
if update_in_progress.get() { return; }
```
No changes needed to the refresh button.

---

## 9. Affected Files

| File | Change Type | Description |
|------|-------------|-------------|
| `src/backends/mod.rs` | Modify | Add `supports_cleanup()` and `run_cleanup()` to `Backend` trait |
| `src/backends/os_package_manager.rs` | Modify | Implement both methods for APT, DNF, Pacman, Zypper; add parsers `count_apt_autoremovals`, `count_dnf_autoremovals`, `parse_zypper_orphaned`, `is_safe_pkg_name` |
| `src/backends/flatpak.rs` | Modify | Implement `supports_cleanup` and `run_cleanup` for `FlatpakBackend` |
| `src/backends/nix.rs` | Modify | Implement `supports_cleanup` and `run_cleanup` for `NixBackend`; add `count_nix_freed_paths` |
| `src/backends/homebrew.rs` | Modify | Implement `supports_cleanup` and `run_cleanup` for `HomebrewBackend`; add `count_brew_cleaned` |
| `src/orchestrator.rs` | Modify | Add `CleanupOrchestrator` struct with `new()` and `run_all()` |
| `src/ui/update_row.rs` | Modify | Add `set_status_cleaning()` and `set_status_cleaned(removed: usize)` |
| `src/ui/window.rs` | Modify | Add "Run Maintenance" to `app_menu`; add `maintenance_action` with handler |

**Total: 8 files modified, 0 new files, 0 new dependencies.**

---

## 10. Dependency Analysis

No new Cargo dependencies are required. All necessary building blocks already exist:

| Capability | Already Available |
|-----------|-----------------|
| Async command execution | `tokio::process::Command` in `run_cleanup` (unprivileged checks) |
| Privileged command execution | `CommandExecutor::run()` via `runner` parameter |
| Output streaming | `async_channel` + `BackendEvent::LogLine` |
| Log display | `LogPanel::append_line()` |
| Row status display | `UpdateRow` (two new methods added) |
| GTK main-thread async | `glib::spawn_future_local` |
| Action registration | `gio::SimpleAction` |
| Menu construction | `gio::Menu::append()` |

---

## 11. Risks and Mitigations

### R1: Pacman zero-orphan false positive
**Risk:** `pacman -Qtdq` exits non-zero on some versions when there are no orphans. Treating non-zero exit as error would surface spurious errors.  
**Mitigation:** Check stdout content, not exit code. If `stdout.trim().is_empty()`, return `Success { updated_count: 0 }` regardless of exit status.

### R2: Zypper orphan package name injection
**Risk:** Orphan names injected into a shell string could contain shell metacharacters.  
**Mitigation:** Filter orphan names through `is_safe_pkg_name()` which allows only `[A-Za-z0-9._+-]`. Names failing validation are dropped from the removal list (with a log warning). The `PrivilegedShell::run_command` also rejects `\n`/`\r`/`\0` in all arguments as a second defense layer.

### R3: Concurrent update and maintenance
**Risk:** User triggers maintenance while update is in-progress (or vice versa), resulting in two simultaneous `pkexec` sessions.  
**Mitigation:** Both operations gate on `update_in_progress: Rc<Cell<bool>>`. The maintenance action returns immediately if the flag is set. The flag is set at the start and cleared at `AllFinished`.

### R4: Flatpak uninstall removes wanted runtimes
**Risk:** `flatpak uninstall --unused -y` could remove shared runtimes used by other Flatpak apps the user wants to keep.  
**Mitigation:** Flatpak's orphan detection is reliable — it only removes runtimes/extensions genuinely unused by any installed app. This is the standard maintenance command recommended by the Flatpak documentation. No mitigation needed beyond documentation.

### R5: Nix GC is destructive on NixOS
**Risk:** `nix-collect-garbage -d` deletes all old generations, making rollback impossible.  
**Mitigation:** Running as unprivileged user only affects the user profile (not system generations). System-level GC (which would require root) is excluded from this implementation. Document this behavior in a future "About Maintenance" tooltip or help text.

### R6: Homebrew cleanup removes old versions user may want
**Risk:** `brew cleanup` removes all old formula versions. A user relying on an old version symlinked somewhere may be surprised.  
**Mitigation:** This is standard Homebrew maintenance behavior and is widely documented. Low risk in practice.

### R7: Backend detection race
**Risk:** Maintenance action runs before backend detection completes; `detected` is empty.  
**Mitigation:** If `backends.is_empty()` after filtering, the action logs "No maintenance actions available." and returns immediately without calling the orchestrator. Backend detection typically completes in under 200ms so this is primarily a safety guard.

---

## 12. Testing Strategy

### Unit Tests (to add alongside implementation)

Each new parser function should have unit tests matching the existing pattern in the codebase:

| Function | Test cases |
|---------|-----------|
| `count_apt_autoremovals` | Zero removals, N removals, empty output |
| `count_dnf_autoremovals` | DNF4 format, DNF5 format, nothing removed |
| `parse_zypper_orphaned` | Empty table, N orphans, non-orphan rows filtered |
| `is_safe_pkg_name` | Valid names, names with shell chars rejected |
| `count_nix_freed_paths` | N paths deleted, nothing freed, unexpected output |
| `count_brew_cleaned` | N removals, nothing removed, mixed output |

### Integration Tests (via `MockExecutor`)

Each backend's `run_cleanup` should be exercised with `MockExecutor`:

- **APT:** `Ok("N to remove")` → `Success { updated_count: N }`, `Err(...)` → `Error(...)`
- **DNF:** same pattern
- **Pacman (no orphans):** `pacman -Qtdq` produces empty stdout → `Success { updated_count: 0 }` without calling `pkexec`
- **Pacman (with orphans):** stdout lists 2 packages → `pkexec pacman -Rns ...` called with 2 args → `Success { updated_count: 2 }`
- **Zypper (no orphans):** empty table → `Success { updated_count: 0 }`
- **Zypper (with orphans):** N orphans → `pkexec sh -c "zypper remove ..."` called → `Success { updated_count: N }`
- **Flatpak:** "Uninstalling: org.gnome...." line → `Success { updated_count: 1 }`
- **Nix:** "5 store paths deleted" → `Success { updated_count: 5 }`
- **Homebrew:** `brew autoremove` success + `brew cleanup` → `Success { updated_count: N }`

---

## 13. Implementation Steps (Ordered)

1. **`src/backends/mod.rs`** — Add `supports_cleanup()` and `run_cleanup()` trait methods with defaults.
2. **`src/backends/os_package_manager.rs`** — Implement for APT, DNF; add parsers; add `count_apt_autoremovals`, `count_dnf_autoremovals`. Then implement Pacman (two-step with orphan check). Then implement Zypper (two-step with `parse_zypper_orphaned` + `is_safe_pkg_name`).
3. **`src/backends/flatpak.rs`** — Implement for `FlatpakBackend`.
4. **`src/backends/nix.rs`** — Implement for `NixBackend`; add `count_nix_freed_paths`.
5. **`src/backends/homebrew.rs`** — Implement for `HomebrewBackend`; add `count_brew_cleaned`.
6. **`src/orchestrator.rs`** — Add `CleanupOrchestrator` below `UpdateOrchestrator`.
7. **`src/ui/update_row.rs`** — Add `set_status_cleaning()` and `set_status_cleaned()`.
8. **`src/ui/window.rs`** — Add "Run Maintenance" menu item + `maintenance_action` handler.
9. **Tests** — Add unit tests for all new parser functions alongside existing tests in each file.
10. **Build validation** — `cargo build`, `cargo clippy -- -D warnings`, `cargo fmt --check`, `cargo test`.

---

## 14. Summary of Design Choices

| Decision | Choice | Rationale |
|---------|--------|-----------|
| Trait extension vs. separate interface | Extend `Backend` trait with `supports_cleanup` + `run_cleanup` | Consistent with existing `run_update` pattern; no new types; backends self-describe |
| Orchestrator reuse | New `CleanupOrchestrator`; reuse `OrchestratorEvent` | Same event semantics; minimal code duplication |
| UI entry point | Header bar menu item ("Run Maintenance") | Clean, non-intrusive; keeps main UI unchanged; explicit user action |
| Privilege gating | Reuse `needs_root()` + `update_in_progress` flag | No new APIs; correct for all affected backends |
| Pacman orphan check | Two-step: unprivileged `pacman -Qtdq` then conditional privileged remove | Only safe approach to handle zero-orphan case without root prompt |
| Zypper orphan removal | Shell string with `is_safe_pkg_name` validation | Necessary to construct variable-length remove command; validated for safety |
| `UpdateResult` reuse | `updated_count` = packages removed | Semantic fit; no new types needed |
| History logging | Not recorded for cleanup (update-only history) | Cleanup is maintenance, not an update event; avoids cluttering history |
