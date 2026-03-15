# Specification: Auto-Check Counts, Per-Section Progress Bars, and Icon Fix

**Project:** Up — GTK4/libadwaita Linux desktop updater (Rust)  
**Date:** 2026-03-15  
**Features:** Three changes in one implementation pass

---

## Table of Contents

1. [Current State Analysis](#1-current-state-analysis)
2. [Problem Definitions](#2-problem-definitions)
3. [Proposed Solution Architecture](#3-proposed-solution-architecture)
4. [Implementation Steps (Ordered)](#4-implementation-steps-ordered)
5. [All Files to Modify](#5-all-files-to-modify)
6. [Exact Function Signatures](#6-exact-function-signatures)
7. [Risks and Mitigations](#7-risks-and-mitigations)

---

## 1. Current State Analysis

### 1.1 `src/backends/mod.rs`

Defines the `Backend` trait:

```rust
#[async_trait::async_trait]
pub trait Backend: Send + Sync {
    fn kind(&self) -> BackendKind;
    fn display_name(&self) -> &str;
    fn description(&self) -> &str;
    fn icon_name(&self) -> &str;
    async fn run_update(&self, runner: &CommandRunner) -> UpdateResult;
}
```

`detect_backends()` probes APT, DNF, Pacman, Zypper (via `which`) and Flatpak, Homebrew, Nix.  
**There is no `count_available()` or `check_updates()` method.**

### 1.2 `src/ui/update_row.rs`

`UpdateRow` wraps `adw::ActionRow` with these suffix widgets:
- `gtk::Spinner` (visible = false by default)
- `gtk::Label` initialized to `"Ready"`

Existing API:
- `new(backend: &dyn Backend)` — constructs, subtitle = backend description, status = "Ready"
- `set_status_running()` — shows spinner, sets label "Updating...", accent color
- `set_status_success(count)` — hides spinner, "X updated" or "Up to date", success color
- `set_status_error(msg)` — hides spinner, "Error: …", error color
- `set_status_skipped(msg)` — hides spinner, dim-label color

**Missing:** No `set_status_checking()`, no per-row progress bar, no `set_status_available()`.

### 1.3 `src/ui/window.rs` — `build_update_page()`

1. Creates a single **global** `gtk::ProgressBar` (shown/hidden for the whole update run).
2. Calls `backends::detect_backends()`, creates one `UpdateRow` per backend — all start at "Ready".
3. "Update All" button: sets all rows to `set_status_running()`, spawns a `std::thread` with a tokio runtime that calls `backend.run_update(&runner)` sequentially for each backend.
4. Log output is streamed via `async_channel<(BackendKind, String)>` → appended to `LogPanel`.
5. Results are received via a second channel `async_channel<(BackendKind, UpdateResult)>` → calls `set_status_success/error/skipped()` per row.
6. Global progress bar is updated per completed backend (fraction = completed/total).

**No auto-check on launch. All rows start at "Ready" indefinitely.**

### 1.4 `src/runner.rs` — `CommandRunner`

Wraps `async_channel::Sender<(BackendKind, String)>` and uses `tokio::process::Command` to stream output line-by-line. Returns full captured output on success.

**Important:** The runner always streams to the log channel. Using it for "silent" checks would pollute the log panel.

### 1.5 `src/app.rs` — `on_activate()`

```rust
fn on_activate(app: &adw::Application) {
    if let Some(display) = gtk::gdk::Display::default() {
        let theme = gtk::IconTheme::for_display(&display);
        theme.add_search_path("data/icons");   // ← RELATIVE PATH — this is the bug
    }
    gtk::Window::set_default_icon_name("io.github.up");
    let window = UpWindow::new(app);
    window.present();
}
```

`"data/icons"` is resolved relative to the current working directory (CWD) at runtime. When running under `cargo run`, CWD is the project root, so it works accidentally sometimes. When run from any other location (installed binary, Flatpak, desktop launcher), CWD is NOT the project root, so the path resolves to nothing or an unrelated path.

### 1.6 Icon Directory Structure (confirmed via `list_dir`)

```
data/icons/hicolor/
    scalable/apps/io.github.up.svg     ✓ EXISTS
    256x256/apps/io.github.up.png      ✓ EXISTS
    128x128/apps/                      ← EMPTY (no icon)
    48x48/apps/                        ← EMPTY (no icon)
```

The icon name set is `"io.github.up"`. GTK icon theme resolves this by looking for:
- `<search_path>/hicolor/scalable/apps/io.github.up.svg`
- `<search_path>/hicolor/256x256/apps/io.github.up.png`

Structure is correct; the **only** problem is the relative search path.

---

## 2. Problem Definitions

### Problem 1: No launch-time update check

When the app starts, every row shows the static label "Ready". Users have no information about pending updates until they press "Update All". This means the app provides no value on first open.

**Required behaviour:** On launch, each backend row should asynchronously check for available updates and display the count (e.g., "12 available", "Up to date", "Checking...").

### Problem 2: Single global progress bar

The page has one `gtk::ProgressBar` that increments in steps of `1/N` as each backend completes. This gives no per-backend visual feedback. Users cannot tell which backend is actively running at a given moment.

**Required behaviour:** Each backend row has its own progress indicator (pulsing progress bar) that is visible only while that backend is actively running, hidden when idle or complete.

### Problem 3: Relative icon search path

`theme.add_search_path("data/icons")` uses a relative path. This only works when the CWD is the project root. In all other launch contexts it silently fails, causing GTK to fall back to the generic system icon.

**Required behaviour:** The correct SVG icon `io.github.up` is displayed in all launch contexts during development (`cargo run`).

---

## 3. Proposed Solution Architecture

### 3.1 Feature 1 — Auto-check on launch

#### 3.1.1 New `Backend` trait method

Add an async method `count_available` with a default implementation that returns `Ok(0)`. Each backend overrides it with a **read-only, no-privilege, no-streaming** implementation using `tokio::process::Command::output()` directly (not `CommandRunner`, because the result need not appear in the log panel).

```rust
// Default: report 0 so callers show "Up to date" gracefully
async fn count_available(&self) -> Result<usize, String> {
    Ok(0)
}
```

#### 3.1.2 Per-backend implementations

**APT** — `apt list --upgradable 2>/dev/null`  
Counts non-empty output lines that contain `/` (package lines look like `pkg/focal 1.0 amd64 [upgradable from: 0.9]`). Does not require root.

```rust
async fn count_available(&self) -> Result<usize, String> {
    let out = tokio::process::Command::new("apt")
        .args(["list", "--upgradable"])
        .output()
        .await
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.lines().filter(|l| l.contains('/')).count())
}
```

**DNF** — `dnf check-update`  
Exit code 100 means updates available; exit code 0 means up to date. Count non-empty, non-header lines in stdout when exit code is 100.

```rust
async fn count_available(&self) -> Result<usize, String> {
    let out = tokio::process::Command::new("dnf")
        .args(["check-update"])
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if out.status.code() == Some(0) {
        return Ok(0);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let count = text
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with("Last") && !l.starts_with("Obsoleting"))
        .count();
    Ok(count)
}
```

**Pacman** — `pacman -Qu`  
Lists upgradable packages, one per line. Count non-empty lines. Does not require root.

```rust
async fn count_available(&self) -> Result<usize, String> {
    let out = tokio::process::Command::new("pacman")
        .args(["-Qu"])
        .output()
        .await
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.lines().filter(|l| !l.is_empty()).count())
}
```

**Zypper** — `zypper list-updates`  
Table output; lines starting with `v ` (version available marker) are actual update rows. Does not require root.

```rust
async fn count_available(&self) -> Result<usize, String> {
    let out = tokio::process::Command::new("zypper")
        .args(["list-updates"])
        .output()
        .await
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.lines().filter(|l| l.starts_with("v ")).count())
}
```

**Flatpak** — `flatpak remote-ls --updates`  
Lists all apps with available updates, one per line. Does not require root.

```rust
async fn count_available(&self) -> Result<usize, String> {
    let out = tokio::process::Command::new("flatpak")
        .args(["remote-ls", "--updates"])
        .output()
        .await
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.lines().filter(|l| !l.is_empty()).count())
}
```

**Homebrew** — `brew outdated`  
Lists all outdated formulae/casks, one per line. Does not require root.

```rust
async fn count_available(&self) -> Result<usize, String> {
    let out = tokio::process::Command::new("brew")
        .args(["outdated"])
        .output()
        .await
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.lines().filter(|l| !l.is_empty()).count())
}
```

**Nix** — Try `nix-env -u --dry-run`, count stderr lines containing `"upgrading"`. Fallback gracefully to `Ok(0)`.

```rust
async fn count_available(&self) -> Result<usize, String> {
    let out = tokio::process::Command::new("nix-env")
        .args(["-u", "--dry-run"])
        .output()
        .await
        .map_err(|e| e.to_string())?;
    // nix-env dry-run writes "upgrading..." lines to stderr
    let text = String::from_utf8_lossy(&out.stderr);
    Ok(text.lines().filter(|l| l.contains("upgrading")).count())
}
```

#### 3.1.3 New `UpdateRow` methods

```rust
pub fn set_status_checking(&self) {
    self.spinner.set_visible(true);
    self.spinner.set_spinning(true);
    self.status_label.set_label("Checking...");
    self.status_label.set_css_classes(&["dim-label"]);
}

pub fn set_status_available(&self, count: usize) {
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    if count == 0 {
        self.status_label.set_label("Up to date");
        self.status_label.set_css_classes(&["success"]);
    } else {
        self.status_label.set_label(&format!("{count} available"));
        self.status_label.set_css_classes(&["accent"]);
    }
}
```

#### 3.1.4 Window changes

In `build_update_page()`, immediately after the row-creation loop, set each row to `set_status_checking()` and spawn async per-backend check tasks:

```rust
// Set checking state + spawn auto-check for each backend
for (idx, backend) in detected.iter().enumerate() {
    // Set initial checking state (borrow-then-drop before spawn)
    {
        let borrowed = rows.borrow();
        borrowed[idx].1.set_status_checking();
    }

    let backend_clone = backend.clone();
    let rows_ref = rows.clone();

    glib::spawn_future_local(async move {
        let (tx, rx) = async_channel::bounded::<Result<usize, String>>(1);

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let result = backend_clone.count_available().await;
                let _ = tx.send(result).await;
            });
        });

        if let Ok(result) = rx.recv().await {
            let row = rows_ref.borrow()[idx].1.clone();
            match result {
                Ok(count) => row.set_status_available(count),
                Err(_) => row.set_status_available(0),
            }
        }
    });
}
```

**Note:** Each backend check runs in its own thread with its own tokio runtime. Checks are fully concurrent and don't block each other or the UI.

---

### 3.2 Feature 2 — Per-section progress bars

#### 3.2.1 `UpdateRow` changes

Add a `gtk::ProgressBar` field, inserted as a suffix widget **between** the spinner and the status label. The progress bar has a fixed `width_request` of 100px, is vertically centered, and is hidden by default.

```rust
pub struct UpdateRow {
    pub row: adw::ActionRow,
    status_label: gtk::Label,
    spinner: gtk::Spinner,
    progress_bar: gtk::ProgressBar,  // NEW
}
```

In `UpdateRow::new()`, build and add it:

```rust
let progress_bar = gtk::ProgressBar::builder()
    .visible(false)
    .valign(gtk::Align::Center)
    .width_request(100)
    .build();

row.add_suffix(&spinner);
row.add_suffix(&progress_bar);  // NEW — between spinner and label
row.add_suffix(&status_label);
```

Add a new pub method to pulse during update:

```rust
pub fn pulse_progress(&self) {
    self.progress_bar.pulse();
}
```

Modify `set_status_running()` to show the progress bar:

```rust
pub fn set_status_running(&self) {
    self.spinner.set_visible(true);
    self.spinner.set_spinning(true);
    self.progress_bar.set_visible(true);   // NEW
    self.progress_bar.set_fraction(0.0);   // NEW — reset
    self.status_label.set_label("Updating...");
    self.status_label.set_css_classes(&["accent"]);
}
```

Modify `set_status_success()`, `set_status_error()`, `set_status_skipped()` to hide the progress bar:

```rust
pub fn set_status_success(&self, count: usize) {
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    self.progress_bar.set_visible(false);  // NEW
    // ... existing label logic
}

pub fn set_status_error(&self, msg: &str) {
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    self.progress_bar.set_visible(false);  // NEW
    // ... existing label logic
}

pub fn set_status_skipped(&self, msg: &str) {
    self.spinner.set_visible(false);
    self.spinner.set_spinning(false);
    self.progress_bar.set_visible(false);  // NEW
    // ... existing label logic
}
```

Also add `set_status_checking()` (from Feature 1) to hide progress_bar there too:
```rust
pub fn set_status_checking(&self) {
    self.spinner.set_visible(true);
    self.spinner.set_spinning(true);
    self.progress_bar.set_visible(false);  // not pulsing during check
    self.status_label.set_label("Checking...");
    self.status_label.set_css_classes(&["dim-label"]);
}
```

#### 3.2.2 `window.rs` changes

**Remove the global `gtk::ProgressBar`** from `build_update_page()`. Remove all `progress_ref.*` calls on the global bar.

**Pulse per-row progress bars** in the log output processing future. The current inner `glib::spawn_future_local` that processes `rx` (log channel) needs access to `rows`. Add a clone:

```rust
// BEFORE the inner spawn_future_local:
let rows_for_log = rows_clone.clone();  // NEW clone for log processor

// Inside the existing log future:
glib::spawn_future_local(async move {
    while let Ok((kind, line)) = rx.recv().await {
        log_ref2.append_line(&format!("[{kind}] {line}"));
        // NEW: pulse the appropriate row's progress bar
        let borrowed = rows_for_log.borrow();
        if let Some((_, row)) = borrowed.iter().find(|(k, _)| *k == kind) {
            row.pulse_progress();
        }
    }
});
```

Remove all `progress_clone` / `progress_ref` variable declarations, visibility changes, and fraction-setting calls from `build_update_page()`.

---

### 3.3 Feature 3 — Icon search path fix

#### 3.3.1 Root cause

`theme.add_search_path("data/icons")` passes a relative path to GTK. GTK resolves this relative to `std::env::current_dir()` at the time the icon is first requested. When `cargo run` is used from the project directory, CWD happens to be the project root so it works. In all other contexts (installed binary, desktop launcher, Flatpak), CWD is elsewhere and the call silently does nothing.

#### 3.3.2 The fix

Use `env!("CARGO_MANIFEST_DIR")` — a Rust compile-time macro that expands to the **absolute path** of the directory containing `Cargo.toml` (the project root). Combined with `concat!`, this produces a compile-time absolute path string.

Replace in `src/app.rs`:

```rust
// OLD
theme.add_search_path("data/icons");

// NEW
theme.add_search_path(concat!(env!("CARGO_MANIFEST_DIR"), "/data/icons"));
```

Wrap with `#[cfg(debug_assertions)]` so the dev-only search path is not baked into release binaries (in release/installed mode the icon is in the system icon theme, so no custom path is needed):

```rust
fn on_activate(app: &adw::Application) {
    #[cfg(debug_assertions)]
    {
        if let Some(display) = gtk::gdk::Display::default() {
            let theme = gtk::IconTheme::for_display(&display);
            theme.add_search_path(concat!(env!("CARGO_MANIFEST_DIR"), "/data/icons"));
        }
    }

    gtk::Window::set_default_icon_name("io.github.up");

    let window = UpWindow::new(app);
    window.present();
}
```

#### 3.3.3 Why this is sufficient

- `data/icons/hicolor/scalable/apps/io.github.up.svg` exists ✓
- `data/icons/hicolor/256x256/apps/io.github.up.png` exists ✓
- The icon name `"io.github.up"` matches the file names ✓
- The empty `48x48/apps/` and `128x128/apps/` directories do not cause errors; GTK scales from available sizes
- For installed builds (Meson/Flatpak), the icon is already placed at the system icon prefix (`/usr/share/icons/hicolor/...`), so no path override is needed

No GResource embedding is needed for this fix. The `env!("CARGO_MANIFEST_DIR")` approach is correct and idiomatic for Rust dev-mode paths.

---

## 4. Implementation Steps (Ordered)

### Step 1: Fix the icon (lowest risk, independent)

**File:** `src/app.rs`

- Wrap the `add_search_path` block in `#[cfg(debug_assertions)]`
- Change `"data/icons"` to `concat!(env!("CARGO_MANIFEST_DIR"), "/data/icons")`

### Step 2: Add `count_available` to `Backend` trait and all implementations

**File:** `src/backends/mod.rs`

- Add the default `async fn count_available(&self) -> Result<usize, String>` returning `Ok(0)` to the `Backend` trait

**File:** `src/backends/os_package_manager.rs`

- Implement `count_available` on `AptBackend`, `DnfBackend`, `PacmanBackend`, `ZypperBackend` per §3.1.2

**File:** `src/backends/flatpak.rs`

- Implement `count_available` on `FlatpakBackend`

**File:** `src/backends/homebrew.rs`

- Implement `count_available` on `HomebrewBackend`

**File:** `src/backends/nix.rs`

- Implement `count_available` on `NixBackend`

### Step 3: Extend `UpdateRow` with new methods and `progress_bar` field

**File:** `src/ui/update_row.rs`

- Add `progress_bar: gtk::ProgressBar` to the struct
- Add `gtk::ProgressBar` construction in `new()`, insert as suffix between spinner and status_label
- Add `set_status_checking()` method
- Add `set_status_available(count: usize)` method
- Add `pulse_progress()` method
- Modify `set_status_running()` to show `progress_bar`
- Modify `set_status_success()`, `set_status_error()`, `set_status_skipped()` to hide `progress_bar`

### Step 4: Update `window.rs` — remove global progress bar, add auto-check, add per-row pulsing

**File:** `src/ui/window.rs`

- Remove: `progress_bar` variable, `progress_clone`, `progress_ref` variables, all `.set_visible()` / `.set_fraction()` / `.set_text()` calls on the global bar, and the `content_box.append(&progress_bar)` call
- Remove: the `status_label` "Updating..." and "Update complete" messages (keep the label itself for possible future use or remove it)
- Add: auto-check spawn loop after the row-creation loop (per §3.1.4)
- Add: `rows_for_log` clone before the inner log-processing `glib::spawn_future_local`
- Add: `row.pulse_progress()` call inside the log-processing future (per §3.2.2)

---

## 5. All Files to Modify

| File | Changes |
|------|---------|
| `src/app.rs` | Fix icon search path with `cfg(debug_assertions)` + `env!("CARGO_MANIFEST_DIR")` |
| `src/backends/mod.rs` | Add default `count_available` to `Backend` trait |
| `src/backends/os_package_manager.rs` | Implement `count_available` for APT, DNF, Pacman, Zypper |
| `src/backends/flatpak.rs` | Implement `count_available` for Flatpak |
| `src/backends/homebrew.rs` | Implement `count_available` for Homebrew |
| `src/backends/nix.rs` | Implement `count_available` for Nix |
| `src/ui/update_row.rs` | Add `progress_bar` field, new methods, modify existing methods |
| `src/ui/window.rs` | Remove global bar, add auto-check loop, add per-row pulse |

**Files NOT to modify:**  
`src/main.rs`, `src/upgrade.rs`, `src/ui/upgrade_page.rs`, `src/ui/log_panel.rs`, `src/ui/mod.rs`, `src/runner.rs`, `Cargo.toml`, `meson.build`, `flake.nix`

---

## 6. Exact Function Signatures

### `src/backends/mod.rs` — `Backend` trait addition

```rust
/// Count packages available for update (read-only, no privilege required).
/// Returns Ok(0) if up to date, Ok(N) if N updates available, Err(_) on failure.
/// Default implementation returns Ok(0) for backends that do not support checking.
async fn count_available(&self) -> Result<usize, String> {
    Ok(0)
}
```

### `src/ui/update_row.rs` — new/modified API

```rust
impl UpdateRow {
    // EXISTING — constructor, add progress_bar field and suffix
    pub fn new(backend: &dyn Backend) -> Self { ... }

    // NEW — shown during launch auto-check
    pub fn set_status_checking(&self) { ... }

    // NEW — shown after auto-check completes
    pub fn set_status_available(&self, count: usize) { ... }

    // NEW — called by window log processor to animate during update
    pub fn pulse_progress(&self) { ... }

    // MODIFIED — now also shows progress_bar
    pub fn set_status_running(&self) { ... }

    // MODIFIED — now also hides progress_bar
    pub fn set_status_success(&self, count: usize) { ... }

    // MODIFIED — now also hides progress_bar
    pub fn set_status_error(&self, msg: &str) { ... }

    // MODIFIED — now also hides progress_bar
    pub fn set_status_skipped(&self, msg: &str) { ... }
}
```

### `src/ui/window.rs` — structural changes (no new public API)

The `build_update_page()` function signature does not change — it still returns `gtk::Box`. Internal changes only:
- Remove 5 lines relating to global `progress_bar`
- Add ~20 lines for per-backend auto-check spawning after the detection loop
- Add 1 clone + 5 lines in the log processing inner future for per-row pulsing

---

## 7. Risks and Mitigations

### Risk 1: `count_available` takes too long on slow systems

**Concern:** `dnf check-update` or `apt list --upgradable` can take 10–30 seconds on slow networks or after a long cache miss.

**Mitigation:** Each check runs in its own thread (fully concurrent) and the UI remains responsive. The row shows "Checking..." spinner during the wait. No timeout is needed — results arrive whenever ready. The "Update All" button remains clickable during the check.

### Risk 2: `dnf check-update` exit code behaviour

**Concern:** `dnf check-update` exits with code 100 when updates are available, 0 when up to date. The `?` operator on `output()` does NOT propagate non-zero exit codes (it only propagates IO errors), so this is safe. The code explicitly checks `out.status.code() == Some(0)` to distinguish.

**Mitigation:** The implementation in §3.1.2 handles both cases correctly by checking the exit code before parsing.

### Risk 3: `async_trait` default method with `tokio::process::Command`

**Concern:** The default `count_available` returns `Ok(0)` and uses no tokio. The per-backend implementations use `tokio::process::Command`. These are called inside a `tokio::runtime::Builder::new_current_thread()` runtime spawned by `build_update_page()` — identical to how `run_update` is already called. No runtime nesting issues.

**Mitigation:** The pattern is identical to existing `run_update` call sites and is already proven to compile and work.

### Risk 4: `progress_bar` layout shift

**Concern:** Adding a `width_request(100)` progress bar as a suffix widget that appears/disappears during update will cause the row's suffix area to resize, which may shift the status label position.

**Mitigation:** The progress bar is shown during updates (when the spinner is also visible) and hidden at rest. The mild layout shift during update start is acceptable. If desired, an implementation could set `progress_bar.set_visible(true)` at app start but keep `fraction = 0.0` and no pulse — providing a static "empty bar" placeholder. The spec leaves this to implementer discretion.

### Risk 5: Borrow checker with `Rc<RefCell<Vec<...>>>` in async closures

**Concern:** Capturing `rows_clone` (an `Rc<RefCell<...>>`) inside `glib::spawn_future_local` requires care. The `Rc` is not `Send`, but `spawn_future_local` runs on the GTK main thread so `!Send` types are allowed.

**Mitigation:** The auto-check loop borrows `rows` only to get the initial `set_status_checking()` state before spawning the inner future; the inner future uses `idx` (a `usize`) to borrow `rows_ref` only when the result arrives. This mirrors exactly the existing pattern in the `result_rx` handler and is already proven correct.

### Risk 6: Icon fix `env!("CARGO_MANIFEST_DIR")` in non-Cargo builds

**Concern:** `env!("CARGO_MANIFEST_DIR")` is a Cargo-specific compile-time macro. If the binary is built with Meson or Flatpak-builder (not Cargo), this macro still expands — it's set by `cargo build` during compilation. The `#[cfg(debug_assertions)]` guard ensures this code path is ONLY compiled into debug builds. Meson and Flatpak always use `--release` passes (`cargo build --release`), so the block is not compiled at all for production builds.

**Mitigation:** The `#[cfg(debug_assertions)]` guard is the correct dividing line — it eliminates this code from all release/installed builds and keeps it only for `cargo run` development use.

---

## Summary for Implementer

Three independent changes:

1. **Icon fix** (1 file, ~5 lines): Wrap icon search path in `#[cfg(debug_assertions)]`, change relative path to `concat!(env!("CARGO_MANIFEST_DIR"), "/data/icons")`.

2. **Auto-check on launch** (7 files, ~80 lines total):
   - Add `count_available()` default to `Backend` trait
   - Implement it in all 7 backend files using read-only commands
   - Add `set_status_checking()` and `set_status_available()` to `UpdateRow`
   - Add auto-check spawn loop to `window.rs`

3. **Per-row progress bars** (2 files, ~25 lines total):
   - Add `progress_bar` field + `pulse_progress()` method to `UpdateRow`
   - Show/hide `progress_bar` in all status-setting methods
   - Remove global progress bar from `window.rs`
   - Add per-row pulse call in the log-processing future

All three changes are orthogonal. Implement in the order given (icon → count → progress) to minimize context switching.
