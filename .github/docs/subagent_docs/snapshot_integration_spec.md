# Snapshot Integration — Implementation Specification

**Feature:** Detect Snapper / Timeshift / btrfs root; offer pre-update snapshot  
**Spec version:** 1.0  
**Date:** 2026-05-08  
**Status:** Ready for implementation

---

## 1. Current State Analysis

### 1.1 `UpdateOrchestrator::run_all` — No Pre/Post Hooks

`src/orchestrator.rs` drives backends sequentially. There are **no** pre-run or
post-run hook slots. The event sequence is:

```
AuthStarted → AuthSucceeded → (BackendStarted → BackendLog* → BackendFinished)* → AllFinished
```

Adding snapshot support requires:
- A new field `snapshot_tool: Option<crate::snapshot::SnapshotTool>` on `UpdateOrchestrator`
- New event variants in `OrchestratorEvent`
- A snapshot phase that runs **after** `AuthSucceeded` but **before** the backend loop

### 1.2 Confirmation Dialog Pattern (`upgrade_page.rs`)

The upgrade page uses `adw::AlertDialog::builder()` for confirmation:

```rust
let dialog = adw::AlertDialog::builder()
    .heading("Confirm System Upgrade")
    .body("This will upgrade ...")
    .build();
dialog.add_response("cancel", "Cancel");
dialog.add_response("upgrade", "Upgrade");
dialog.set_response_appearance("upgrade", adw::ResponseAppearance::Destructive);
dialog.set_default_response(Some("cancel"));
dialog.set_close_response("cancel");
dialog.connect_response(None, glib::clone!(
    #[weak] button,
    move |_, response| {
        if response == "upgrade" { ... }
    }
));
dialog.present(Some(button));
return; // critical: exit handler after showing dialog
```

The **metered-connection** and **low-battery** dialogs in `window.rs` use a
slightly different but equivalent pattern with a **bypass flag**:

```rust
// State variable (outside click handler):
let bypass_snapshot: Rc<Cell<bool>> = Rc::new(Cell::new(false));

// Inside connect_clicked:
if <condition> && !bypass_flag.get() {
    let dialog = adw::AlertDialog::new(Some("Title"), Some("Body..."));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("proceed", "Proceed");
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, glib::clone!(
        #[weak] button,
        #[strong] bypass_flag,
        move |_, response| {
            if response == "proceed" {
                bypass_flag.set(true);
                button.emit_clicked();  // re-enter handler with bypass set
                bypass_flag.set(false);
            }
        }
    ));
    dialog.present(Some(button));
    return; // exit handler
}
```

This is the **exact pattern** to use for the snapshot prompt.

### 1.3 Privileged One-Shot Command Pattern

`PrivilegedShell::run_command(&mut self, args: &[&str], tx: &Sender<BackendEvent>, kind: BackendKind) -> Result<String, String>`

- Writes `args` to the persistent `pkexec sh` stdin
- Streams output line-by-line as `BackendEvent::LogLine(kind, line)` through `tx`
- Returns `Ok(captured_output)` on exit-code 0, `Err(message)` on non-zero
- The `kind` argument is only used to populate the `BackendEvent::LogLine` variant

There is **no dedicated one-shot API**. Snapshot commands must go through `run_command`. This requires a `BackendKind::Snapshot` variant (see §3.1).

### 1.4 Existing Config Pattern

`src/config.rs` uses `AppConfig` with `serde_json`:

```rust
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub skipped_backends: Vec<BackendKind>,
    // NEW: snapshot_preference field added here
}
```

The config is loaded in `app.rs` → `on_activate` and passed to `UpWindow::build`. It is saved whenever skip state changes (backend skip toggling in `update_row.rs` callback in `window.rs`).

---

## 2. Feature Definition

### 2.1 Snapshot Tool Priority

Detection follows strict priority order:

1. **Snapper** — preferred; well-integrated with Btrfs/openSUSE/Arch
2. **Timeshift** — fallback; popular on Ubuntu-based systems
3. **Raw btrfs subvolume** — last resort; requires btrfs root AND `/.snapshots/` exists
4. **None detected** — skip prompt entirely, proceed directly to updates

### 2.2 Detection Logic

**Snapper** (priority 1):
- `which snapper` returns Ok AND `/etc/snapper/configs/root` exists
- Indicates snapper is installed AND configured for the root subvolume

**Timeshift** (priority 2):
- `which timeshift` returns Ok AND `/etc/timeshift/timeshift.json` exists
- The JSON config file exists only after initial timeshift setup

**btrfs** (priority 3):
- Root filesystem (`/`) uses btrfs: detected via `/proc/mounts` — find a line with
  `mountpoint == "/"` and `fstype == "btrfs"`
- AND `/.snapshots/` directory exists (ensures there is somewhere to put snapshots)

### 2.3 Snapshot Commands

**Snapper:**
```
snapper -c root create -t pre --print-number --description "Up pre-update"
```
- `-c root` — target the "root" snapper configuration
- `-t pre` — creates a "pre" type snapshot (intended to be paired with a post snapshot after updates)
- `--print-number` — prints the snapshot number on stdout (e.g., `42`)
- `--description` — human-readable label
- Exit code 0 on success; stdout contains the snapshot number
- Returns description: `"Snapper snapshot #42"`
- Requires root; runs inside `PrivilegedShell`

**Timeshift:**
```
timeshift --create --comments "Up pre-update" --scripted
```
- `--scripted` — suppresses interactive prompts, runs non-interactively
- `--comments` — sets the snapshot label
- Exit code 0 on success; blocks until snapshot is complete (synchronous)
- Returns description: `"Timeshift snapshot created"`
- Requires root; runs inside `PrivilegedShell`

**btrfs (raw):**
```
btrfs subvolume snapshot / /.snapshots/pre-update-<unix_timestamp>
```
- `<unix_timestamp>` is the UNIX epoch seconds at time of call (e.g., `1746700000`)
- `/.snapshots/` must exist (verified during detection)
- Exit code 0 on success
- Returns description: `"btrfs snapshot at /.snapshots/pre-update-<timestamp>"`
- Requires root; runs inside `PrivilegedShell`

### 2.4 UI Flow

```
User clicks "Update All"
        │
        ▼
Check metered connection ──(cancel)──► return
        │ (proceed)
        ▼
Check battery ──(cancel)──► return
        │ (proceed)
        ▼
snapshot_tool detected AND preference == Ask?
    ├── NO → proceed directly to orchestrator
    └── YES → show adw::AlertDialog
              "Create a snapshot before updating?"
              Buttons: [Skip] [Create Snapshot] (suggested)
                  │                │
                  ▼                ▼
              bypass_snapshot   bypass_snapshot
                set + continue    set + continue
```

When user chooses "Create Snapshot":
- `bypass_snapshot.set(true)` + `button.emit_clicked()` re-enters the handler
- Inside the orchestrator, `SnapshotStarted` event fires, log panel shows "Creating snapshot…"
- `SnapshotLog(line)` events stream snapshot command output to log panel
- On `SnapshotSucceeded(desc)`: log panel shows "✓ {desc}", update proceeds
- On `SnapshotFailed(err)`: log panel shows "⚠ Snapshot failed: {err}", update proceeds anyway (no abort)

When user chooses "Skip":
- `bypass_snapshot.set(true)` → orchestrator receives `snapshot_tool: None` → skips snapshot phase

### 2.5 Preference Persistence

`SnapshotPreference` enum stored in `AppConfig`:

| Variant | Behavior |
|---------|----------|
| `Ask`   | Show dialog each time (default) |
| `Always`| Always create snapshot without prompting |
| `Never` | Never create snapshot, skip prompt |

The dialog will have a **"Remember my choice"** checkbox. If checked:
- "Skip" → saves `Never`
- "Create Snapshot" → saves `Always`

### 2.6 Failure Handling

- **Snapshot command fails (non-zero exit)**: Send `SnapshotFailed(error)` event. UI logs the failure in the log panel with a warning prefix. Updates **proceed anyway** — snapshot failure is never fatal.
- **Snapshot tool detected but snapperd not running**: The snapper command will fail. Handled as above.
- **`/.snapshots/` doesn't exist** (btrfs): Detected at detection time — only offer btrfs option if `/.snapshots/` exists.
- **Timeout** (snapshot takes >1 hour): Handled by `PrivilegedShell`'s existing `COMMAND_TIMEOUT` (3600 seconds).

---

## 3. Architecture

### 3.1 `src/snapshot.rs` (new file)

```rust
use std::path::Path;
use which::which;

/// The snapshot tool to use for pre-update snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotTool {
    Snapper,
    Timeshift,
    Btrfs,
}

/// User preference for snapshot creation behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum SnapshotPreference {
    #[default]
    Ask,
    Always,
    Never,
}

/// Errors from snapshot creation.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("Snapshot command failed: {0}")]
    CommandFailed(String),
    #[error("Failed to parse snapshot output: {0}")]
    ParseError(String),
}

/// Detect the highest-priority available snapshot tool.
/// Runs blocking I/O; call from a background thread.
pub fn detect_snapshot_tool() -> Option<SnapshotTool> {
    // Priority 1: Snapper
    if which("snapper").is_ok() && Path::new("/etc/snapper/configs/root").exists() {
        return Some(SnapshotTool::Snapper);
    }
    // Priority 2: Timeshift
    if which("timeshift").is_ok() && Path::new("/etc/timeshift/timeshift.json").exists() {
        return Some(SnapshotTool::Timeshift);
    }
    // Priority 3: raw btrfs
    if is_root_btrfs() && Path::new("/.snapshots").exists() {
        return Some(SnapshotTool::Btrfs);
    }
    None
}

fn is_root_btrfs() -> bool {
    std::fs::read_to_string("/proc/mounts")
        .ok()
        .map(|content| {
            content.lines().any(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                parts.len() >= 3 && parts[1] == "/" && parts[2] == "btrfs"
            })
        })
        .unwrap_or(false)
}

/// Create a pre-update snapshot using the given tool.
///
/// Runs the snapshot command inside the provided elevated shell, streaming
/// output lines through `log_tx`. Returns a human-readable description
/// of the snapshot on success.
pub async fn create_snapshot(
    tool: SnapshotTool,
    shell: &mut crate::runner::PrivilegedShell,
    log_tx: &async_channel::Sender<String>,
) -> Result<String, SnapshotError> {
    use crate::backends::BackendKind;
    use crate::runner::BackendEvent;

    // Bridge: BackendEvent channel → String channel (log_tx)
    let (be_tx, be_rx) = async_channel::unbounded::<BackendEvent>();
    let log_fwd = {
        let log_tx = log_tx.clone();
        tokio::spawn(async move {
            while let Ok(BackendEvent::LogLine(_, line)) = be_rx.recv().await {
                let _ = log_tx.send(line).await;
            }
        })
    };

    let result = match tool {
        SnapshotTool::Snapper => {
            let args = [
                "snapper", "-c", "root", "create",
                "-t", "pre", "--print-number",
                "--description", "Up pre-update",
            ];
            shell
                .run_command(&args, &be_tx, BackendKind::Snapshot)
                .await
                .map(|output| format!("Snapper snapshot #{}", output.trim()))
                .map_err(SnapshotError::CommandFailed)
        }
        SnapshotTool::Timeshift => {
            let args = [
                "timeshift", "--create",
                "--comments", "Up pre-update",
                "--scripted",
            ];
            shell
                .run_command(&args, &be_tx, BackendKind::Snapshot)
                .await
                .map(|_| "Timeshift snapshot created".to_string())
                .map_err(SnapshotError::CommandFailed)
        }
        SnapshotTool::Btrfs => {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let dest = format!("/.snapshots/pre-update-{ts}");
            let args = ["btrfs", "subvolume", "snapshot", "/", &dest];
            shell
                .run_command(&args, &be_tx, BackendKind::Snapshot)
                .await
                .map(|_| format!("btrfs snapshot at {dest}"))
                .map_err(SnapshotError::CommandFailed)
        }
    };

    drop(be_tx);
    let _ = log_fwd.await;
    result
}
```

> **Note on `dest` lifetime in btrfs arm:** `dest` is a `String` allocated before
> `args`, so `&dest` borrows from a live local variable within the same arm.
> This is fine because `args` and the `run_command` call are in the same scope.

### 3.2 `src/backends/mod.rs` — Add `BackendKind::Snapshot`

Add `Snapshot` to the `BackendKind` enum. This variant is used **only** for log-line
attribution during snapshot execution. It is never used for `BackendStarted` or
`BackendFinished` events and never appears as an update row in the UI.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendKind {
    Apt, Dnf, Pacman, Zypper, Flatpak, Homebrew, Nix, Fwupd,
    Snapshot,  // ← NEW: used only for snapshot log lines
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // existing arms...
            Self::Snapshot => write!(f, "Snapshot"),
        }
    }
}
```

### 3.3 `src/config.rs` — Add `snapshot_preference`

```rust
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub skipped_backends: Vec<BackendKind>,
    #[serde(default)]
    pub snapshot_preference: crate::snapshot::SnapshotPreference,
}
```

`SnapshotPreference` derives `Default` (→ `Ask`), so existing config files without
this field deserialize cleanly.

### 3.4 `src/orchestrator.rs` — Add Snapshot Phase

#### 3.4.1 New `OrchestratorEvent` variants

```rust
pub enum OrchestratorEvent {
    AuthStarted,
    AuthSucceeded,
    AuthFailed(String),
    // NEW snapshot events:
    SnapshotStarted,
    SnapshotLog(String),
    SnapshotSucceeded(String),
    SnapshotFailed(String),
    // Existing backend events:
    BackendStarted(BackendKind),
    BackendLog(BackendKind, String),
    BackendFinished(BackendKind, UpdateResult),
    AllFinished,
}
```

#### 3.4.2 `UpdateOrchestrator` struct

```rust
pub struct UpdateOrchestrator {
    backends: Vec<Arc<dyn Backend>>,
    snapshot_tool: Option<crate::snapshot::SnapshotTool>,
}

impl UpdateOrchestrator {
    pub fn new(
        backends: Vec<Arc<dyn Backend>>,
        snapshot_tool: Option<crate::snapshot::SnapshotTool>,
    ) -> Self {
        Self { backends, snapshot_tool }
    }
    // ...
}
```

#### 3.4.3 Snapshot phase in `run_all`

Insert the snapshot phase **after** `AuthSucceeded` is sent but **before** the
backend loop. The snapshot reuses the already-opened `PrivilegedShell` when
backends need root; if no backends need root but a snapshot is requested, open a
dedicated shell just for the snapshot.

```rust
// After: let _ = tx.send(OrchestratorEvent::AuthSucceeded).await;

// --- Snapshot phase ---
if let Some(tool) = snapshot_tool {
    let _ = tx.send(OrchestratorEvent::SnapshotStarted).await;

    // Use the existing shell or open a dedicated one.
    let snapshot_shell: Option<Arc<tokio::sync::Mutex<PrivilegedShell>>> =
        if any_needs_root {
            shell.clone()
        } else {
            match PrivilegedShell::new().await {
                Ok(s) => Some(Arc::new(tokio::sync::Mutex::new(s))),
                Err(e) => {
                    let _ = tx.send(OrchestratorEvent::SnapshotFailed(e)).await;
                    None
                }
            }
        };

    if let Some(shell_arc) = snapshot_shell {
        let (log_tx, log_rx) = async_channel::unbounded::<String>();
        let tx_fwd = tx.clone();
        let log_fwd = tokio::spawn(async move {
            while let Ok(line) = log_rx.recv().await {
                let _ = tx_fwd.send(OrchestratorEvent::SnapshotLog(line)).await;
            }
        });

        let mut guard = shell_arc.lock().await;
        match crate::snapshot::create_snapshot(tool, &mut *guard, &log_tx).await {
            Ok(desc) => {
                drop(log_tx);
                let _ = log_fwd.await;
                let _ = tx.send(OrchestratorEvent::SnapshotSucceeded(desc)).await;
            }
            Err(e) => {
                drop(log_tx);
                let _ = log_fwd.await;
                let _ = tx.send(OrchestratorEvent::SnapshotFailed(e.to_string())).await;
            }
        }

        // If we opened a dedicated shell for the snapshot, close it now.
        if !any_needs_root {
            guard.close().await;
        }
    }
}
// --- End snapshot phase; backend loop follows ---
```

> **Auth interaction note:** If only non-root backends exist (Flatpak, Brew, Nix)
> but the user requested a snapshot, a second `pkexec` prompt opens. This is
> acceptable because the snapshot itself is a privileged operation. The alternative
> (always opening a shell even without root backends just to have it ready) would
> pollute the existing auth flow. The dedicated shell is opened and closed cleanly.

### 3.5 `src/ui/window.rs` — Snapshot Detection and Dialog

#### 3.5.1 State variables (in `build_update_page`)

```rust
// Detected snapshot tool — populated at startup by background detection
let detected_snapshot: Rc<RefCell<Option<crate::snapshot::SnapshotTool>>> =
    Rc::new(RefCell::new(None));

// Bypass flag — same pattern as bypass_metered / bypass_battery
let bypass_snapshot: Rc<Cell<bool>> = Rc::new(Cell::new(false));
```

#### 3.5.2 Background snapshot detection at startup

After the existing backend detection block, spawn snapshot detection:

```rust
{
    let (snap_tx, snap_rx) = async_channel::bounded::<Option<crate::snapshot::SnapshotTool>>(1);
    super::spawn_background_async(move || async move {
        let tool = crate::snapshot::detect_snapshot_tool();
        let _ = snap_tx.send(tool).await;
    });
    glib::spawn_future_local(glib::clone!(
        #[strong] detected_snapshot,
        async move {
            if let Ok(tool) = snap_rx.recv().await {
                *detected_snapshot.borrow_mut() = tool;
            }
        }
    ));
}
```

#### 3.5.3 Snapshot dialog in `update_button.connect_clicked`

Insert this block **after** the battery check block, **before** `button.set_sensitive(false)`:

```rust
// Snapshot check
let snapshot_tool = *detected_snapshot.borrow();
if let Some(tool) = snapshot_tool {
    let preference = crate::config::load_config().snapshot_preference;
    let should_skip = matches!(preference, crate::snapshot::SnapshotPreference::Never);
    let auto_create = matches!(preference, crate::snapshot::SnapshotPreference::Always);

    if !should_skip && !auto_create && !bypass_snapshot.get() {
        let tool_name = match tool {
            crate::snapshot::SnapshotTool::Snapper => "Snapper",
            crate::snapshot::SnapshotTool::Timeshift => "Timeshift",
            crate::snapshot::SnapshotTool::Btrfs => "btrfs",
        };
        let dialog = adw::AlertDialog::builder()
            .heading("Create System Snapshot?")
            .body(format!(
                "{tool_name} was detected on this system.\n\n\
                 A snapshot can be taken before updating so you can \
                 roll back if something goes wrong."
            ))
            .build();
        dialog.add_response("skip", "Skip");
        dialog.add_response("snapshot", "Create Snapshot");
        dialog.set_response_appearance("snapshot", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("snapshot"));
        dialog.set_close_response("skip");

        // "Remember my choice" checkbox
        let remember_check = gtk::CheckButton::builder()
            .label("Remember my choice")
            .build();
        dialog.set_extra_child(Some(&remember_check));

        dialog.connect_response(
            None,
            glib::clone!(
                #[weak] button,
                #[strong] bypass_snapshot,
                #[strong] detected_snapshot,
                move |_, response| {
                    // Persist preference if requested
                    if remember_check.is_active() {
                        let pref = if response == "snapshot" {
                            crate::snapshot::SnapshotPreference::Always
                        } else {
                            crate::snapshot::SnapshotPreference::Never
                        };
                        let mut config = crate::config::load_config();
                        config.snapshot_preference = pref;
                        if let Err(e) = crate::config::save_config(&config) {
                            log::warn!("Failed to save snapshot preference: {e}");
                        }
                        if pref == crate::snapshot::SnapshotPreference::Never {
                            // Clear detected tool so future clicks also skip
                            *detected_snapshot.borrow_mut() = None;
                        }
                    }
                    bypass_snapshot.set(true);
                    if response == "snapshot" {
                        // Re-enter the handler — orchestrator will receive the tool
                    } else {
                        // Re-enter without snapshot
                        *detected_snapshot.borrow_mut() = None;
                    }
                    button.emit_clicked();
                    bypass_snapshot.set(false);
                }
            ),
        );
        dialog.present(Some(button));
        return;
    }
}
```

#### 3.5.4 Pass `snapshot_tool` to orchestrator

Change the orchestrator construction from:
```rust
let orchestrator = UpdateOrchestrator::new(backends);
```
to:
```rust
let snap_tool = if bypass_snapshot.get() {
    *detected_snapshot.borrow()
} else {
    None
};
let orchestrator = UpdateOrchestrator::new(backends, snap_tool);
```

Wait — this needs more care. After `bypass_snapshot` re-entry, if the user chose
"Create Snapshot", `detected_snapshot` still holds `Some(tool)` and `bypass_snapshot`
is set. If the user chose "Skip", `detected_snapshot` was set to `None` before
`emit_clicked()`. So the simpler form is:

```rust
let snap_tool = *detected_snapshot.borrow();
let orchestrator = UpdateOrchestrator::new(backends, snap_tool);
```

#### 3.5.5 Handle new orchestrator events in the event loop

In the `while let Ok(event) = event_rx.recv().await` match block, add:

```rust
OrchestratorEvent::SnapshotStarted => {
    log_panel.append_line("─── Creating pre-update snapshot ───");
    status_label.set_label("Creating snapshot…");
}
OrchestratorEvent::SnapshotLog(line) => {
    log_panel.append_line(&format!("[Snapshot] {line}"));
}
OrchestratorEvent::SnapshotSucceeded(desc) => {
    log_panel.append_line(&format!("✓ Snapshot created: {desc}"));
    status_label.set_label("Updating…");
}
OrchestratorEvent::SnapshotFailed(err) => {
    log_panel.append_line(&format!("⚠ Snapshot failed: {err}"));
    log_panel.append_line("Proceeding with updates anyway.");
    status_label.set_label("Updating…");
}
```

### 3.6 `src/main.rs` — Register Module

Add `mod snapshot;` to the module list.

---

## 4. Implementation Steps (Ordered, File-by-File)

### Step 1 — `src/snapshot.rs` (create)
1. Create file with `SnapshotTool`, `SnapshotPreference`, `SnapshotError` types
2. Implement `detect_snapshot_tool()` using `which` crate + filesystem checks
3. Implement private `is_root_btrfs()` helper that reads `/proc/mounts`
4. Implement `create_snapshot()` async fn with bridge channel pattern

### Step 2 — `src/backends/mod.rs` (modify)
1. Add `Snapshot` variant to `BackendKind` enum
2. Add `Self::Snapshot => write!(f, "Snapshot")` arm to `fmt::Display` impl
3. No other changes — `Snapshot` is never used for `BackendStarted`/`BackendFinished`

### Step 3 — `src/config.rs` (modify)
1. Add `pub snapshot_preference: crate::snapshot::SnapshotPreference` field to `AppConfig`
2. Add `#[serde(default)]` attribute to the new field

### Step 4 — `src/orchestrator.rs` (modify)
1. Add four new variants to `OrchestratorEvent`: `SnapshotStarted`, `SnapshotLog(String)`, `SnapshotSucceeded(String)`, `SnapshotFailed(String)`
2. Add `snapshot_tool: Option<crate::snapshot::SnapshotTool>` field to `UpdateOrchestrator`
3. Update `UpdateOrchestrator::new()` to accept `snapshot_tool` parameter
4. Update `CleanupOrchestrator::new()` if it shares any code — check: it has its own `new()`, no changes needed
5. Insert snapshot phase in `run_all` (after `AuthSucceeded` send, before backend loop) per §3.4.3
6. Update all callers of `UpdateOrchestrator::new()` in `window.rs`

### Step 5 — `src/ui/window.rs` (modify)
1. Add `detected_snapshot` and `bypass_snapshot` state variables in `build_update_page`
2. Add background snapshot detection block after backend detection block
3. Add snapshot dialog check in `update_button.connect_clicked` (after battery check)
4. Change orchestrator construction to pass `snap_tool`
5. Add four new match arms for snapshot events in the event loop
6. Verify `UpdateOrchestrator::new(backends)` → `UpdateOrchestrator::new(backends, snap_tool)` in the retry handler too (the retry orchestrator in `UpdateRow::new` callback should pass `None` — no snapshot before individual retries)

### Step 6 — `src/main.rs` (modify)
1. Add `mod snapshot;` to the module declarations

---

## 5. New Dependencies

**None.** All required crates are already in `Cargo.toml`:

| Crate | Already in Cargo.toml | Usage |
|-------|----------------------|-------|
| `which` | ✓ `which = "7"` | Detect `snapper`, `timeshift` binaries |
| `thiserror` | ✓ `thiserror = "2"` | `SnapshotError` derive |
| `serde` | ✓ with `features = ["derive"]` | `SnapshotPreference` serialization |
| `tokio` | ✓ full features | `tokio::spawn`, async fns |
| `async-channel` | ✓ `async-channel = "2"` | Log forwarding channel |

No `Cargo.toml` changes required.

---

## 6. Risks & Mitigations

### R1 — Snapper detected but `snapperd` not running
- **Risk**: `snapper` binary exists and config exists, but the `snapperd` daemon is
  not running (e.g., fresh install with snapper package but never started).
- **Mitigation**: The snapper command will fail with a non-zero exit code. This is
  caught by `PrivilegedShell::run_command` returning `Err(...)`, which is forwarded as
  `SnapshotFailed`. Updates proceed. The error message is shown in the log panel.
- **No special detection needed**: this is an edge case and graceful failure is acceptable.

### R2 — btrfs: `/.snapshots/` doesn't exist
- **Risk**: Root is btrfs but `/.snapshots/` was never created.
- **Mitigation**: `detect_snapshot_tool()` explicitly checks `Path::new("/.snapshots").exists()`.
  If the directory doesn't exist, `SnapshotTool::Btrfs` is **not** returned. The prompt
  never appears. No snapshot is taken.
- **We do NOT create `/.snapshots/` automatically**: creating filesystem directories as
  root without explicit user intent would be surprising.

### R3 — Snapshot takes a long time (UI blocked)
- **Risk**: rsync-based Timeshift on large systems can take minutes; btrfs snapshots
  are near-instant but Snapper on huge subvolumes can be slow.
- **Mitigation**: The snapshot runs inside `run_all` which runs on the background
  Tokio runtime, not the GTK main thread. The UI remains responsive. The log panel
  shows streaming output. The status label shows "Creating snapshot…". `COMMAND_TIMEOUT`
  (1 hour) in `PrivilegedShell` is more than sufficient.
- **Additional**: For Timeshift specifically, since it blocks synchronously, streaming
  output may not appear until completion. The log panel will still show "Creating
  snapshot…" in the status label during the wait.

### R4 — Two pkexec prompts (non-root backends + snapshot)
- **Risk**: A user with only Flatpak/Brew/Nix backends (no root backends) who wants
  a snapshot will see two pkexec prompts: one for the snapshot, one for... wait —
  non-root backends don't need pkexec at all. So in this case, there is only **one**
  pkexec prompt (for the snapshot's dedicated shell). This is acceptable.
- **Root backends + snapshot**: Only **one** pkexec prompt total. The snapshot reuses
  the orchestrator's already-opened `PrivilegedShell`.

### R5 — User always picks "Skip" — nagging
- **Risk**: Showing the dialog every run annoys users who consistently skip it.
- **Mitigation**: The "Remember my choice" checkbox lets users permanently set
  `SnapshotPreference::Never`. Once saved, the dialog never appears again.
  The default is `Ask` (not `Always`) so users who don't engage with the feature
  can simply click Skip without feeling nagged.

### R6 — `BackendKind::Snapshot` in history records
- **Risk**: If `Snapshot` appears in `BackendFinished` events and is written to history,
  it could confuse the history page.
- **Mitigation**: `Snapshot` is **never** used in `BackendStarted` or `BackendFinished`
  events. It is only used as the `kind` argument to `PrivilegedShell::run_command` to
  label log lines as `[Snapshot] ...`. The snapshot result is reported via the separate
  `SnapshotStarted/Succeeded/Failed` events which have no `BackendKind`. No history
  entry is written for snapshot events.

### R7 — `detected_snapshot` mutated mid-dialog
- **Risk**: Snapshot detection runs asynchronously; it could theoretically complete
  after the user clicks "Update All" but before the dialog check.
- **Mitigation**: Detection is fast (syscalls only, no blocking I/O) and starts at
  app launch. By the time a user sees the backends list and clicks "Update All",
  detection has long since completed. The result is stored in an `Rc<RefCell<...>>`
  accessible only from the GTK main thread — no race condition.

### R8 — Timeshift `--create` vs. Timeshift BTRFS mode
- **Risk**: Timeshift in BTRFS mode operates differently than rsync mode. Both modes
  support `--create --scripted` per the Timeshift documentation.
- **Mitigation**: The command is the same for both modes. Timeshift handles the
  distinction internally. No special handling needed.

---

## 7. Source References

1. **Arch Wiki — Snapper**: https://wiki.archlinux.org/title/Snapper  
   — Confirms: `snapper -c root create -t pre --print-number --description <desc>` syntax;
     exit code 0 on success; `/etc/snapper/configs/root` as detection file.

2. **Arch Wiki — Btrfs § Snapshots**: https://wiki.archlinux.org/title/Btrfs#Snapshots  
   — Confirms: `btrfs subvolume snapshot source dest` command; root must be btrfs;
     snapshots are near-instant CoW operations.

3. **Btrfs SysadminGuide — Snapshots**: https://archive.kernel.org/oldwiki/btrfs.wiki.kernel.org/index.php/SysadminGuide.html#Snapshots  
   — Background on snapshot semantics; managing snapshot directories.

4. **Timeshift GitHub (linuxmint/timeshift)**: https://github.com/linuxmint/timeshift  
   — Confirms: `--create --comments <text> --scripted` flags; both rsync and BTRFS modes
     support `--scripted` for non-interactive use; BTRFS mode requires Ubuntu-type
     `@` + `@home` subvolume layout (limitation noted in R8); config at
     `/etc/timeshift/timeshift.json`.

5. **openSUSE Documentation — Snapper**: https://documentation.suse.com/sles/15/html/SLES-all/cha-snapper.html  
   — Confirms Snapper's role as a pre/post snapshot tool for system updates on openSUSE;
     `snapper create --type pre` usage in zypper hooks.

6. **Arch Wiki — Snapper § Pre/post snapshots** (same URL as #1, §3.2.2):  
   — Exact pre/post snapshot creation syntax; `--print-number` flag prints snapshot ID to stdout;
     `snapper -c root create -t pre -p` is the shorthand (`-p` = `--print-number`).

7. **Btrfs Documentation (btrfs.readthedocs.io)**: https://btrfs.readthedocs.io/en/latest/Subvolumes.html  
   — Confirms subvolume snapshot command is `btrfs subvolume snapshot source [dest/]name`;
     read-write by default; near-instant due to CoW.

---

## Summary

**What:** Add a pre-update snapshot feature that detects Snapper, Timeshift, or raw btrfs on the system and offers to create a snapshot before running updates.

**Where:** One new file (`src/snapshot.rs`) + targeted changes to five existing files (`backends/mod.rs`, `config.rs`, `orchestrator.rs`, `ui/window.rs`, `main.rs`).

**How:** Detection is fast (binary + file existence checks) and runs at startup. The user is prompted via an `adw::AlertDialog` (consistent with metered/battery dialogs) when they click "Update All". If agreed, the snapshot command runs inside the already-opened `PrivilegedShell` before any backend updates start. Failure is non-fatal — updates always proceed. User preference is persistable via a "Remember my choice" checkbox.

**No new dependencies required.**

**Spec file path:** `c:\Projects\Up\.github\docs\subagent_docs\snapshot_integration_spec.md`
