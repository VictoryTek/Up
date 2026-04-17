# Specification: Front-Load pkexec Authentication on "Update All"

## Current State Analysis

### Application Architecture
Up is a GTK4/libadwaita system updater. The main update flow is:

1. **Backend detection** (`src/backends/mod.rs` → `detect_backends()`): Discovers available package managers in this order:
   - OS package manager (APT, DNF, Pacman, or Zypper) via `os_package_manager::detect()`
   - Flatpak via `flatpak::is_available()` / `flatpak::is_running_in_flatpak()`
   - Homebrew via `homebrew::is_available()`
   - Nix via `nix::is_available()`

2. **Update execution** (`src/ui/window.rs` → `update_button.connect_clicked`): When the user clicks "Update All":
   - The button is disabled, all rows set to "Updating..." state.
   - A `glib::spawn_future_local` async block is entered.
   - `spawn_background_async` creates a background OS thread with a single-threaded Tokio runtime.
   - Backends run **sequentially** in detection order via `for backend in &backends_thread { backend.run_update(&runner).await }`.
   - Output and results flow back to the GTK main loop via `async_channel`.

3. **Privilege escalation**: OS package manager backends and NixOS Nix use `pkexec` inside their `run_update()` method:
   - **APT**: `pkexec sh -c "DEBIAN_FRONTEND=noninteractive apt update && apt upgrade -y"`
   - **DNF**: `pkexec dnf upgrade -y`
   - **Pacman**: `pkexec pacman -Syu --noconfirm`
   - **Zypper**: `pkexec sh -c "zypper refresh && zypper update -y"`
   - **NixOS Flake**: `pkexec env PATH=... sh -c "nix flake update ... && nixos-rebuild switch ..."`
   - **NixOS Legacy**: `pkexec env PATH=... sh -c "nix-channel --update && nixos-rebuild switch"`

4. **Unprivileged backends**: Flatpak, Homebrew, and non-NixOS Nix run without `pkexec`.

### How pkexec Authentication Currently Works

Each backend's `run_update()` calls `runner.run("pkexec", &[...])`, which delegates to `CommandRunner::run()` in `src/runner.rs`. This spawns `pkexec` as a child process via `tokio::process::Command`. The polkit agent (GNOME, KDE, etc.) intercepts the pkexec call and presents a graphical password dialog.

The pkexec prompt appears **only when the privileged backend's turn arrives** in the sequential loop. If unprivileged backends (Flatpak) run before the privileged one, the user sees a delay between clicking "Update All" and seeing the password dialog.

### Backend Execution Order Issue

The `detect_backends()` function pushes OS package manager first, then Flatpak. In the sequential loop, OS PM would normally run first. However, the user experiences a delay — the pkexec prompt does not appear "immediately" when "Update All" is pressed. This is because:

1. The sequential loop runs inside a background thread. Even with OS PM first, there is inherent latency between the button click and the pkexec prompt appearing.
2. There is no explicit pre-authentication step — the password dialog is a side effect of running the first privileged command.
3. The user perceives this as the prompt appearing "late" because there's no visual indication that authentication is about to be requested.

---

## Problem Definition

When the user clicks "Update All":
- There is no immediate authentication prompt. The pkexec dialog appears only when a privileged backend's command is spawned.
- The user has no visual feedback that authentication is needed until the polkit agent dialog appears.
- If backend ordering changes (or the privileged backend is not first), the delay becomes more pronounced.
- There is no graceful cancellation if the user declines authentication — the backend reports a generic command failure.

The desired behavior: the password prompt appears **immediately** when "Update All" is clicked, before any backend begins its work. If the user cancels authentication, the entire update is aborted cleanly.

---

## Proposed Solution Architecture

### Overview

1. Add a `needs_root() -> bool` method to the `Backend` trait.
2. When "Update All" is clicked, check if any detected backend requires root.
3. If yes, immediately run `pkexec /bin/true` as a lightweight pre-authentication step.
4. If authentication succeeds, proceed to run backends with privileged ones first.
5. If authentication fails or is cancelled, abort cleanly with a user-friendly message.

### Why This Works: polkit Credential Caching

The default polkit policy for `org.freedesktop.policykit.exec` uses `auth_admin_keep` for active sessions. This means:
- After the user authenticates for `pkexec /bin/true`, polkit **caches the authorization** for the caller's session.
- The default cache duration is **300 seconds (5 minutes)**, controlled by `org.freedesktop.policykit.imply-timeout`.
- Subsequent `pkexec` calls from the same process/session within this window are **auto-authorized without re-prompting**.
- This is the standard behavior on GNOME, KDE, and most modern Linux desktops.

By running `pkexec /bin/true` immediately, the user authenticates once, and all subsequent backend `pkexec` calls proceed without additional prompts.

### Edge Case: polkit Without Caching

On rare systems where the polkit policy uses `auth_admin` (no caching) instead of `auth_admin_keep`, the user would see a second prompt when the actual privileged backend runs. This is acceptable because:
- The first prompt still appears immediately (front-loaded UX improvement).
- The second prompt is the same behavior the user experiences today.
- This is strictly better than the current flow, never worse.

### Backend Reordering

After pre-authentication, backends are reordered so privileged ones execute first. This maximizes the benefit of the polkit cache window: privileged commands run immediately after authentication while the cache is freshest.

The sort uses `sort_by_key` (which is stable in Rust), so backends with the same privilege level maintain their original detection order.

---

## Implementation Steps

### File 1: `src/backends/mod.rs`

**Change**: Add `needs_root()` default method to the `Backend` trait.

```rust
pub trait Backend: Send + Sync {
    fn kind(&self) -> BackendKind;
    fn display_name(&self) -> &str;
    fn description(&self) -> &str;
    fn icon_name(&self) -> &str;

    fn run_update<'a>(
        &'a self,
        runner: &'a CommandRunner,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>>;

    /// Whether this backend requires root privileges (pkexec) to perform updates.
    /// Used by the UI to determine if pre-authentication is needed before starting.
    /// Default: false (no root required).
    fn needs_root(&self) -> bool {
        false
    }

    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async { Ok(0) })
    }

    fn list_available(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}
```

**Rationale**: Default `false` means Flatpak, Homebrew, and other unprivileged backends require no changes.

---

### File 2: `src/backends/os_package_manager.rs`

**Change**: Add `needs_root() -> true` to all four OS package manager backend implementations (AptBackend, DnfBackend, PacmanBackend, ZypperBackend).

For each `impl Backend for XxxBackend` block, add:

```rust
fn needs_root(&self) -> bool {
    true
}
```

Add this method after the existing `icon_name()` method in each impl block, before `run_update()`.

---

### File 3: `src/backends/nix.rs`

**Change**: Add `needs_root()` to NixBackend that returns `true` only when running on NixOS (where `pkexec` is used for `nixos-rebuild`).

```rust
fn needs_root(&self) -> bool {
    is_nixos()
}
```

**Rationale**: Non-NixOS Nix updates (`nix profile upgrade` or `nix-env -u`) run as the current user without pkexec. Only NixOS system rebuilds require root.

---

### File 4: `src/ui/window.rs`

**Change**: Restructure the `update_button.connect_clicked` handler to:
1. Check if pre-authentication is needed.
2. Show "Authenticating..." status and run `pkexec /bin/true` if needed.
3. On auth failure, abort cleanly and reset UI state.
4. On auth success, set rows to "Updating..." and proceed.
5. Reorder backends (privileged first) before passing to the background thread.

The modified handler logic (within the existing `update_button.connect_clicked` closure):

```rust
update_button.connect_clicked(move |button| {
    button.set_sensitive(false);
    log_clone.clear();

    let rows_ref = rows_clone.clone();
    let log_ref = log_clone.clone();
    let status_ref = status_clone.clone();
    let button_ref = button.clone();
    let backends = detected_clone.borrow().clone();
    let banner_ref = restart_banner_clone.clone();

    // Check if any backend requires root privileges.
    let any_needs_root = backends.iter().any(|b| b.needs_root());

    glib::spawn_future_local(async move {
        // --- Phase 1: Pre-authenticate if needed ---
        if any_needs_root {
            status_ref.set_label("Authenticating\u{2026}");
            log_ref.append_line("Requesting administrator privileges\u{2026}");

            let (auth_tx, auth_rx) = async_channel::bounded::<Result<(), String>>(1);
            super::spawn_background_async(move || async move {
                let result = tokio::process::Command::new("pkexec")
                    .arg("/bin/true")
                    .status()
                    .await;
                let outcome = match result {
                    Ok(status) if status.success() => Ok(()),
                    Ok(status) => Err(format!(
                        "Authentication failed (exit code {})",
                        status.code().unwrap_or(-1)
                    )),
                    Err(e) => Err(format!("Failed to start pkexec: {e}")),
                };
                let _ = auth_tx.send(outcome).await;
            });

            match auth_rx.recv().await {
                Ok(Ok(())) => {
                    log_ref.append_line("Authentication successful.");
                }
                Ok(Err(e)) => {
                    log_ref.append_line(&format!("Authentication failed: {e}"));
                    status_ref.set_label("Update cancelled.");
                    button_ref.set_sensitive(true);
                    return;
                }
                Err(_) => {
                    log_ref.append_line("Authentication channel closed unexpectedly.");
                    status_ref.set_label("Update cancelled.");
                    button_ref.set_sensitive(true);
                    return;
                }
            }
        }

        // --- Phase 2: Begin updates ---
        status_ref.set_label("Updating\u{2026}");
        {
            let rows_borrowed = rows_ref.borrow();
            for (_, row) in rows_borrowed.iter() {
                row.set_status_running();
            }
        }

        // Reorder: privileged backends first, then unprivileged.
        // This maximises the polkit credential cache benefit.
        let mut ordered_backends = backends.clone();
        ordered_backends.sort_by_key(|b| u8::from(!b.needs_root()));

        let (tx, rx) = async_channel::unbounded::<(BackendKind, String)>();
        let (result_tx, result_rx) =
            async_channel::unbounded::<(BackendKind, UpdateResult)>();

        let tx_thread = tx.clone();
        let result_tx_thread = result_tx.clone();

        super::spawn_background_async(move || async move {
            for backend in &ordered_backends {
                let kind = backend.kind();
                let runner = CommandRunner::new(tx_thread.clone(), kind);
                let result = backend.run_update(&runner).await;
                let _ = result_tx_thread.send((kind, result)).await;
            }
        });

        // Drop the original senders so channels close when the thread finishes
        drop(tx);
        drop(result_tx);

        // (rest of the handler — log output processing and result processing — remains unchanged)
        // ...
    });
});
```

**Key changes in the handler**:
1. Removed `status_clone.set_label("Updating...");` and the row `set_status_running()` calls from before the async block. They now happen AFTER authentication succeeds, inside the async block.
2. Added the pre-authentication block with `pkexec /bin/true`.
3. Added backend reordering with `sort_by_key(|b| u8::from(!b.needs_root()))` — `needs_root() == true` maps to `0` (first), `false` maps to `1` (second).
4. Changed `backends_thread` to `ordered_backends` for the background async closure.
5. The log output processing and result processing sections remain **completely unchanged**.

---

### Files NOT Modified

- **`src/backends/flatpak.rs`**: No changes. `needs_root()` defaults to `false` (correct — Flatpak runs unprivileged).
- **`src/backends/homebrew.rs`**: No changes. `needs_root()` defaults to `false` (correct — Homebrew runs unprivileged).
- **`src/runner.rs`**: No changes. The `CommandRunner` is unchanged.
- **`src/main.rs`**, **`src/app.rs`**, **`src/reboot.rs`**, **`src/upgrade.rs`**: No changes.
- **`src/ui/update_row.rs`**, **`src/ui/upgrade_page.rs`**, **`src/ui/log_panel.rs`**, **`src/ui/mod.rs`**, **`src/ui/reboot_dialog.rs`**: No changes.
- **`Cargo.toml`**: No new dependencies.

---

## Dependencies

No new external dependencies are required. The solution uses:
- `tokio::process::Command` (already in Cargo.toml via `tokio` with `process` feature)
- `async_channel::bounded` (already in Cargo.toml)
- `pkexec` / `/bin/true` (standard Linux utilities, already relied upon by existing backends)

---

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| **polkit agent does not cache authorization** (policy uses `auth_admin` instead of `auth_admin_keep`) | Low — `auth_admin_keep` is the standard default on all major desktops | Medium — user sees two password prompts (pre-auth + backend) | Strictly better than current behavior: first prompt is still front-loaded. No regression. |
| **`pkexec` binary not installed on system** | Very Low — pkexec is part of polkit, required for existing backend functionality | Low — pre-auth fails, update aborted cleanly | Error message clearly states "Failed to start pkexec". Same failure mode as existing backend pkexec calls. |
| **`/bin/true` not available** | Extremely Low — part of coreutils on every Linux system | Low — pre-auth fails, update aborted | Could fall back to `/usr/bin/true` but unnecessary; `/bin/true` is universally present (often symlinked from `/usr/bin/true`). |
| **Pre-auth succeeds but polkit cache expires before privileged backend runs** | Low — default cache is 300 seconds; backends typically complete in under 60 seconds | Low — user sees another prompt for the backend's pkexec call | Same behavior as today. Backend reordering (privileged first) minimises this window. |
| **Flatpak sandbox: pkexec not available** | N/A — in Flatpak sandbox, no OS PM is detected, `any_needs_root` is always `false` | None | Pre-auth is skipped entirely in Flatpak sandbox. No behavioral change. |
| **Backend reordering changes user-visible execution order** | Certain — intentional change | Negligible — log output shows backend tags `[APT]`, `[Flatpak]` etc.; row updates match by `BackendKind` not index | Results are matched by kind, not position. UI correctly updates regardless of execution order. |
| **Race condition between auth and UI state** | None — all UI updates happen on GTK main thread via `glib::spawn_future_local` | None | The existing async pattern is maintained; all GTK operations remain on the main loop. |

---

## Testing Guidance

Since the project does not yet have test infrastructure, manual testing should verify:

1. **With privileged backend (e.g., APT system)**: Click "Update All" → pkexec dialog appears immediately → authenticate → backends run without re-prompting → update completes.
2. **With only unprivileged backends (e.g., Flatpak-only)**: Click "Update All" → no pkexec dialog → backends run normally.
3. **Cancel authentication**: Click "Update All" → pkexec dialog appears → click Cancel → status shows "Update cancelled." → button re-enabled → rows not stuck in "Updating..." state.
4. **Backend ordering**: Verify privileged backends run before unprivileged ones in the log output.
5. **Flatpak sandbox**: Run Up from Flatpak → "Update All" should not trigger any pkexec prompt (only Flatpak backend detected).
