# Specification: Finding #7 — Blocking I/O on GTK Main Thread

**Feature:** Async backend detection and distro detection  
**Priority:** High  
**Spec Author:** Research Subagent  
**Date:** 2026-03-19

---

## 1. Current State Analysis

### 1.1 Blocking Call #1 — `detect_backends()` in `src/ui/window.rs`

**Function:** `UpWindow::build_update_page()` (called from `UpWindow::new()`)  
**Location:** `src/ui/window.rs`, approximately line 97  
**Blocking call:**

```rust
let detected = backends::detect_backends();
```

**What `detect_backends()` does (`src/backends/mod.rs`):**
- Calls `os_package_manager::detect()` → up to **4 sequential** `which::which()` calls:
  - `which::which("apt")`
  - `which::which("dnf")`
  - `which::which("pacman")`
  - `which::which("zypper")`
- Calls `flatpak::is_available()` → `which::which("flatpak")`
- Calls `homebrew::is_available()` → `which::which("brew")`
- Calls `nix::is_available()` → `which::which("nix")`

**Total:** up to 7 sequential `which::which()` calls, each traversing every directory in `$PATH`.

**`which::which()` I/O behavior (Rust `which` crate):**  
The `which` crate implements POSIX `which` semantics. For each call it:
1. Splits `$PATH` into directory components (typically 10–20 directories on a typical Linux install)
2. For each directory, calls `stat("<dir>/<program>")` to check existence + executable bit
3. Returns on first match or exhausts all PATH entries

On a system with 15 PATH entries and 7 binary names, worst-case `detect_backends()` issues up to **105 `stat()` syscalls** synchronously. On NFS-mounted home directories or slow network filesystems, each `stat()` is a network round-trip (1–100 ms+). Total detection time can range from **100 ms to several seconds**, freezing the GTK UI entirely.

**How `detected` is used after the call:**

```rust
// ~line 98-103: Immediately used to build UpdateRow widgets
for backend in &detected {
    let row = UpdateRow::new(backend.as_ref());
    backends_group.add(&row.row);
    rows.borrow_mut().push((backend.kind(), row));
}

// ~line 128: Captured by value into update_button handler
let detected_clone = detected.clone();
update_button.connect_clicked(move |button| {
    // ...
    let backends = detected_clone.clone();       // cloned again for background thread
    super::spawn_background_async(move || async move {
        for backend in &backends_thread { ... }
    });
});

// ~line 224-251: Captured by value into run_checks closure
let run_checks: Rc<dyn Fn()> = {
    let detected = detected.clone();
    Rc::new(move || {
        for (idx, backend) in detected.iter().enumerate() { ... }
    })
};
```

**Current type:** `Vec<Arc<dyn Backend>>` (owned, cloneable, `Send`)

---

### 1.2 Blocking Call #2 — `detect_distro()` in `src/ui/upgrade_page.rs`

**Function:** `UpgradePage::build()`  
**Location:** `src/ui/upgrade_page.rs`, approximately line 53  
**Blocking call:**

```rust
let distro_info = upgrade::detect_distro();
```

**What `detect_distro()` does (`src/upgrade.rs`):**

```rust
pub fn detect_distro() -> DistroInfo {
    let os_release = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let fields = parse_os_release(&os_release);
    // ... parse and return DistroInfo
}
```

This calls `fs::read_to_string("/etc/os-release")` — a **synchronous file read** on the GTK main thread.

**Additional blocking calls in `UpgradePage::build()` (`src/ui/upgrade_page.rs`, lines ~78-88):**

```rust
if distro_info.id == "nixos" {
    let config_type = upgrade::detect_nixos_config_type();  // stat("/etc/nixos/flake.nix")
    let hostname = upgrade::detect_hostname();               // read("/proc/sys/kernel/hostname")
    // ... build NixOS config row
}
```

- `detect_nixos_config_type()` → `std::path::Path::new("/etc/nixos/flake.nix").exists()` (a `stat()` syscall)
- `detect_hostname()` → `std::fs::read_to_string("/proc/sys/kernel/hostname")`

**How `distro_info` is used afterwards:**

```rust
// ~line 57-72: Immediately populates UI rows
let distro_row = adw::ActionRow::builder().subtitle(&distro_info.name)...;
let version_row = adw::ActionRow::builder().subtitle(&distro_info.version)...;
let upgrade_row = adw::ActionRow::builder()
    .subtitle(if distro_info.upgrade_supported { "Checking..." } else { "Not supported" })...;

// ~line 101-114: Determines which prerequisite check rows to build
let checks: Vec<(&str, &str)> = if distro_info.id == "nixos" { ... } else { ... };

// ~line 158-178: Conditional async task to check upgrade availability
if distro_info.upgrade_supported {
    glib::spawn_future_local(async move { ... });
}

// ~line 233: Captured in check_button.connect_clicked
let distro_clone = distro_info.clone();
check_button.connect_clicked(move |button| {
    let distro = distro_clone.clone();
    std::thread::spawn(move || { upgrade::run_prerequisite_checks(&distro_thread, ...) });
});

// ~line 319: Captured in upgrade_button.connect_clicked
let distro_clone2 = distro_info.clone();
upgrade_button.connect_clicked(move |button| { ... distro_clone2 ... });

// ~line 365: Conditional auto-trigger
if distro_info.upgrade_supported {
    check_button.emit_clicked();
}
```

**Current type:** `upgrade::DistroInfo` (owned, `Clone`, `Serialize`/`Deserialize`)

---

## 2. Problem Definition

### 2.1 GTK Main Thread Requirement

GTK4's threading model mandates that:

1. **All widget operations must occur on the thread that called `gtk::init()`** (the main thread)
2. **The main thread must never block** — any blocking call prevents event processing, redraws, and input handling
3. **Widget construction callbacks (called during `window.present()` or immediately after `ApplicationWindow::new()`) must return promptly**

Violating rule 2 causes visible UI freezes: the window appears to hang, animations stop, and the OS may display the "application not responding" indicator.

### 2.2 Impact Assessment

| Scenario | Expected Latency | Impact |
|---|---|---|
| Local filesystem, warm kernel cache | < 5 ms | Imperceptible |
| Local SSD, cold cache (many PATH dirs) | 10–50 ms | Minor stutter |
| NFS `/home` or network-mounted root | 100 ms – 5 s | Visible freeze |
| Slow Flatpak sandbox with host spawn | 50–500 ms | Noticeable lag |
| Docker / container with overlayfs | 20–200 ms | Variable stutter |

The **worst case is silent** — there's no error surfaced to the user, just a frozen UI.

### 2.3 Root Cause

Both blocking calls occur **synchronously during widget construction** and precede any `glib::spawn_future_local` call, so they cannot be intercepted by the async scheduler. They run entirely on the GTK event loop thread before the window ever becomes visible.

---

## 3. Research Sources

1. **GTK4-rs book — Main Event Loop** (`https://gtk-rs.org/gtk4-rs/stable/latest/book/main_event_loop.html`)  
   Documents `glib::spawn_future_local` for async work on the GTK thread, `async_channel` for cross-thread communication, and the `spawn_blocking` pattern for embedding blocking calls within async contexts.

2. **Context7 — gtk4-rs library ID `/gtk-rs/gtk4-rs`** (fetched via MCP tools)  
   Confirms: "Spawn a future on the main loop" using `glib::spawn_future_local`. Channel pattern with `async_channel::unbounded::<T>()`. Blocking calls via `std::thread::spawn` with sender.

3. **GTK4 C API documentation — Threads** (`https://docs.gtk.org/gtk4/thread.html`)  
   States: *"GTK is not thread-safe. All GTK and GDK calls must be made from the main thread."* — The Rust bindings enforce this via `!Send` bounds on widget types.

4. **GLib documentation — `g_main_context_invoke`** (`https://docs.gtk.org/glib/method.MainContext.invoke.html`)  
   The underlying mechanism `glib::spawn_future_local` uses to schedule work on the GLib main context. Work scheduled this way executes on the next iteration of the event loop, not inline.

5. **Rust `which` crate source** (`https://crates.io/crates/which`)  
   `which::which(name)` implementation: calls `std::env::split_paths(&env::var_os("PATH"))` then `find_executable_in_path()` which calls `metadata()` on each candidate. All I/O is synchronous blocking. There is no async variant.

6. **Tokio documentation — `spawn_blocking`** (`https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html`)  
   `tokio::task::spawn_blocking` moves a blocking closure to a dedicated blocking thread pool and returns a `JoinHandle`. The existing `spawn_background_async` helper in `src/ui/mod.rs` provides an equivalent pattern already established in this project.

7. **GNOME Human Interface Guidelines — Responsiveness** (`https://developer.gnome.org/hig/patterns/feedback/progress.html`)  
   Recommends showing placeholder/loading UI immediately for operations that take more than ~100 ms, with spinners or skeleton content.

---

## 4. Architecture Decision Record

### 4.1 Spawn Mechanism: `spawn_background_async` (existing helper)

**Decision:** Use `super::spawn_background_async(move || async move { ... })` for both detection tasks.

**Rationale:**
- `detect_backends()` and `detect_distro()` are pure synchronous functions; they do not use Tokio async I/O
- The project has an established `spawn_background_async` helper in `src/ui/mod.rs` — use it consistently
- `tokio::task::spawn_blocking` inside an existing `glib::spawn_future_local` would require ensuring a Tokio runtime is present at that call site; the helper manages this transparently
- `spawn_background_async` wraps `std::thread::spawn` + Tokio current-thread runtime, matching the exact pattern used everywhere else in the codebase

### 4.2 State Storage: `Rc<RefCell<Vec<Arc<dyn Backend>>>>` and `Rc<RefCell<Option<DistroInfo>>>`

**Decision:**
- `detected` (window.rs): change from `Vec<Arc<dyn Backend>>` to `Rc<RefCell<Vec<Arc<dyn Backend>>>>`
- `distro_info` (upgrade_page.rs): change from `DistroInfo` to `Rc<RefCell<Option<DistroInfo>>>`

**Rationale:**
- All UI closures in GTK4-rs run on the main thread, so `Rc<RefCell<T>>` (non-Send) is safe
- `Rc<RefCell<T>>` allows multiple closures to share mutable access to the state
- `OnceCell` is not appropriate because the `run_checks` closure in `window.rs` is called multiple times (refresh button), not just once

### 4.3 Placeholder UI During Detection

**Decision:**
- `window.rs` → Show a single `adw::ActionRow` with title `"Detecting package managers…"` and a `gtk::Spinner` suffix in `backends_group`
- `upgrade_page.rs` → Set `distro_row` subtitle to `"Loading…"` and `version_row` subtitle to `"Loading…"` initially; disable the `check_button` until detection completes

**Rationale:**
- Choosing the simplest placeholder that is consistent with existing UI patterns
- Spinner on action row matches how `UpdateRow` shows pending work
- Disabling check_button prevents crashes if user clicks before `distro_info` is available

### 4.4 Error Handling

**Decision:** Log errors silently; do not surface detection failures in the UI.

**Rationale:**
- `detect_backends()` never fails — at worst it returns an empty `Vec`
- `detect_distro()` never fails — it has `unwrap_or_default()` and fallback values
- The channel's `recv().await` returning `Err` (sender dropped without sending) means detection panicked — an extreme edge case; log to stderr

### 4.5 Run Checks Trigger

**Decision:** Remove the initial `(*run_checks)()` call from `UpWindow::new()`. Instead, trigger `(*run_checks)()` from inside the backend detection completion handler after populating `detected` and `rows`.

**Rationale:**
- The current initial call in `UpWindow::new()` at the bottom of `new()` would be a no-op after refactoring (because `detected` would be empty), but is confusing
- Detection completion is the natural trigger point for availability checks
- The refresh button connects to `run_checks` directly and remains functional

---

## 5. New Types

### 5.1 `window.rs` — No New Enum Required

The channel carries `Vec<Arc<dyn Backend>>` directly:

```rust
let (detect_tx, detect_rx) = async_channel::unbounded::<Vec<Arc<dyn Backend>>>();
```

`Arc<dyn Backend>` implements `Send`, `Sync`, so the channel message is `Send`.

### 5.2 `upgrade_page.rs` — No New Enum Required

The channel carries a tuple with all needed values:

```rust
/// (DistroInfo, Option<(NixOsConfigType, String /* validated hostname */)>)
let (detect_tx, detect_rx) =
    async_channel::unbounded::<(upgrade::DistroInfo, Option<(upgrade::NixOsConfigType, String)>)>();
```

Both `DistroInfo` and `NixOsConfigType` already derive `Clone`, `Serialize`, `Deserialize`.

---

## 6. Step-by-Step Implementation Plan

### 6.1 Changes to `src/ui/window.rs`

#### Step 1 — Change `detected` type

Remove:
```rust
let detected = backends::detect_backends();
```

Replace with:
```rust
let detected: Rc<RefCell<Vec<Arc<dyn Backend>>>> = Rc::new(RefCell::new(Vec::new()));
```

Add `Arc` to imports at the top:
```rust
use std::sync::Arc;
```
(already present; verify)

#### Step 2 — Remove the synchronous row-creation loop

Remove the existing loop that creates `UpdateRow` widgets immediately:
```rust
for backend in &detected {
    let row = UpdateRow::new(backend.as_ref());
    backends_group.add(&row.row);
    rows.borrow_mut().push((backend.kind(), row));
}
```

#### Step 3 — Add placeholder row to `backends_group`

After the `backends_group` builder call, add:
```rust
let placeholder_row = adw::ActionRow::builder()
    .title("Detecting package managers\u{2026}")
    .build();
let placeholder_spinner = gtk::Spinner::new();
placeholder_spinner.start();
placeholder_row.add_suffix(&placeholder_spinner);
backends_group.add(&placeholder_row);
```

#### Step 4 — Update `update_button.connect_clicked` to borrow `detected` at click time

Change:
```rust
let detected_clone = detected.clone();
// ...inside clicked handler:
let backends = detected_clone.clone();
// ...inside spawn_background_async:
let backends_thread = backends.clone();
```

Replace with:
```rust
let detected_clone = detected.clone();   // Rc<RefCell<Vec<...>>>
// ...inside clicked handler:
let backends = detected_clone.borrow().clone();  // Vec<Arc<dyn Backend>> snapshot
// ...inside spawn_background_async (backends_thread = backends, as before):
let backends_thread = backends.clone();
```

The closure continues unchanged; only the `borrow().clone()` snapshot is new.

#### Step 5 — Update `run_checks` closure

Change `detected.iter().enumerate()` to `detected.borrow().iter().enumerate()`:

```rust
let run_checks: Rc<dyn Fn()> = {
    let rows = rows.clone();
    let detected = detected.clone();   // Rc<RefCell<Vec<...>>>
    Rc::new(move || {
        for (idx, backend) in detected.borrow().iter().enumerate() {
            // ... rest unchanged
        }
    })
};
```

#### Step 6 — Spawn async backend detection AFTER `run_checks` is defined

Add the following block after `run_checks` is defined and before `(page_box, run_checks)` is returned:

```rust
// Spawn backend detection off the GTK thread.
{
    let (detect_tx, detect_rx) =
        async_channel::unbounded::<Vec<Arc<dyn Backend>>>();

    let detected_fill = detected.clone();
    let rows_fill = rows.clone();
    let group_fill = backends_group.clone();
    let run_checks_after_detect = run_checks.clone();

    super::spawn_background_async(move || async move {
        let backends = crate::backends::detect_backends();
        let _ = detect_tx.send(backends).await;
    });

    glib::spawn_future_local(async move {
        if let Ok(new_backends) = detect_rx.recv().await {
            // Remove placeholder
            group_fill.remove(&placeholder_row);
            // Populate rows
            {
                let mut rows_mut = rows_fill.borrow_mut();
                for backend in &new_backends {
                    let row = UpdateRow::new(backend.as_ref());
                    group_fill.add(&row.row);
                    rows_mut.push((backend.kind(), row));
                }
            }
            // Store backends
            *detected_fill.borrow_mut() = new_backends;
            // Trigger availability check
            (*run_checks_after_detect)();
        } else {
            eprintln!("Backend detection failed; no backends detected.");
            group_fill.remove(&placeholder_row);
        }
    });
}
```

#### Step 7 — Remove initial `(*run_checks)()` from `UpWindow::new()`

In `UpWindow::new()`, remove:
```rust
// Trigger availability checks now that the window is fully assembled.
(*run_checks)();
```

The detection completion handler in step 6 now triggers `run_checks` instead.

---

### 6.2 Changes to `src/ui/upgrade_page.rs`

#### Step 1 — Replace synchronous `detect_distro()` with `Rc<RefCell<Option<DistroInfo>>>`

Remove:
```rust
let distro_info = upgrade::detect_distro();
```

Replace with:
```rust
let distro_info_state: Rc<RefCell<Option<upgrade::DistroInfo>>> =
    Rc::new(RefCell::new(None));
```

#### Step 2 — Update `distro_row`, `version_row`, `upgrade_available_row` to use placeholder text

Change:
```rust
let distro_row = adw::ActionRow::builder()
    .title("Distribution")
    .subtitle(&distro_info.name)
    .build();

let version_row = adw::ActionRow::builder()
    .title("Current Version")
    .subtitle(&distro_info.version)
    .build();

let upgrade_available_row = adw::ActionRow::builder()
    .title("Upgrade Available")
    .subtitle(if distro_info.upgrade_supported {
        "Checking..."
    } else {
        "Not supported for this distribution yet"
    })
    .build();
```

Replace with:
```rust
let distro_row = adw::ActionRow::builder()
    .title("Distribution")
    .subtitle("Loading\u{2026}")
    .build();
distro_row.add_prefix(&gtk::Image::from_icon_name("computer-symbolic"));

let version_row = adw::ActionRow::builder()
    .title("Current Version")
    .subtitle("Loading\u{2026}")
    .build();

let upgrade_available_row = adw::ActionRow::builder()
    .title("Upgrade Available")
    .subtitle("Loading\u{2026}")
    .build();
```

Note: `distro_row.add_prefix(...)` was previously added after the row was built; keep it here.

#### Step 3 — Remove the NixOS config row block from inline construction

Remove the conditional block:
```rust
if distro_info.id == "nixos" {
    let config_type = upgrade::detect_nixos_config_type();
    let hostname = upgrade::detect_hostname();
    // ...build config_row...
    info_group.add(&config_row);
}
```

This block will be moved into the detection completion handler (step 7 below).

#### Step 4 — Update prerequisite check rows to use generic placeholders initially

Change the conditional check definition:
```rust
let checks: Vec<(&str, &str)> = if distro_info.id == "nixos" {
    vec![("nixos-rebuild available", ...), ...]
} else {
    vec![("All packages up to date", ...), ...]
};
```

Replace with a generic (non-NixOS) set initially, to be rebuilt after detection:
```rust
let checks: Vec<(&str, &str)> = vec![
    ("All packages up to date", "system-software-update-symbolic"),
    ("Sufficient disk space (10 GB+)", "drive-harddisk-symbolic"),
    ("Backup recommended", "document-save-symbolic"),
];
```

The check_rows and check_icons `Rc<RefCell<Vec<...>>>` are populated from these as before. After detection, if NixOS is detected, update `check_rows[0]`'s title via `row.set_title("nixos-rebuild available")`.

#### Step 5 — Disable `check_button` until detection completes

After building the check_button:
```rust
let check_button = gtk::Button::builder()
    .label("Run Checks")
    .css_classes(vec!["pill"])
    .sensitive(false)   // disabled until distro info is available
    .build();
```

#### Step 6 — Update button click closures to borrow `distro_info_state` at click time

For `check_button.connect_clicked`:

Change:
```rust
let distro_clone = distro_info.clone();
check_button.connect_clicked(move |button| {
    // ...
    let distro = distro_clone.clone();
    std::thread::spawn(move || {
        let results = upgrade::run_prerequisite_checks(&distro_thread, &bridge_tx);
    });
});
```

Replace with:
```rust
let distro_state_for_check = distro_info_state.clone();
check_button.connect_clicked(move |button| {
    // Snapshot at click time — detection must be complete for button to be sensitive
    let distro = distro_state_for_check.borrow().clone().expect("distro info available");
    // ... rest unchanged; distro used as before
});
```

For `upgrade_button.connect_clicked`:

Change:
```rust
let distro_clone2 = distro_info.clone();
upgrade_button.connect_clicked(move |button| {
    // ... uses distro_clone2.name, distro_clone2.version
});
```

Replace with:
```rust
let distro_state_for_upgrade = distro_info_state.clone();
upgrade_button.connect_clicked(move |button| {
    let distro = distro_state_for_upgrade.borrow().clone().expect("distro info available");
    // ... uses distro.name, distro.version
});
```

#### Step 7 — Spawn async distro detection and wire completion handler

Remove the `check_upgrade_available` spawn block currently gated on `distro_info.upgrade_supported` (lines ~158-178); it will be moved into the completion handler.

Remove the auto-trigger block:
```rust
if distro_info.upgrade_supported {
    check_button.emit_clicked();
}
```

Add the following block after all closures are wired but before returning `page_box`:

```rust
// Spawn distro detection off the GTK thread.
{
    let (detect_tx, detect_rx) =
        async_channel::unbounded::<(upgrade::DistroInfo, Option<(upgrade::NixOsConfigType, String)>)>();

    super::spawn_background_async(move || async move {
        let info = upgrade::detect_distro();
        let nixos_extra = if info.id == "nixos" {
            let config_type = upgrade::detect_nixos_config_type();
            let raw_hostname = upgrade::detect_hostname();
            Some((config_type, raw_hostname))
        } else {
            None
        };
        let _ = detect_tx.send((info, nixos_extra)).await;
    });

    let distro_state_fill = distro_info_state.clone();
    let distro_row_fill = distro_row.clone();
    let version_row_fill = version_row.clone();
    let upgrade_available_row_fill = upgrade_available_row.clone();
    let info_group_fill = info_group.clone();
    let check_rows_fill = check_rows.clone();
    let upgrade_available_fill = upgrade_available.clone();
    let upgrade_btn_fill = upgrade_button.clone();
    let check_btn_fill = check_button.clone();

    glib::spawn_future_local(async move {
        match detect_rx.recv().await {
            Ok((info, nixos_extra)) => {
                // Populate distro info rows
                distro_row_fill.set_subtitle(&info.name);
                version_row_fill.set_subtitle(&info.version);
                upgrade_available_row_fill.set_subtitle(if info.upgrade_supported {
                    "Checking\u{2026}"
                } else {
                    "Not supported for this distribution yet"
                });

                // Conditionally add NixOS config row
                if let Some((config_type, raw_hostname)) = &nixos_extra {
                    let config_label = match config_type {
                        upgrade::NixOsConfigType::Flake => {
                            let safe_hostname = glib::markup_escape_text(raw_hostname);
                            format!("Flake-based (/etc/nixos#{})", safe_hostname)
                        }
                        upgrade::NixOsConfigType::LegacyChannel => {
                            "Channel-based (/etc/nixos/configuration.nix)".to_string()
                        }
                    };
                    let config_row = adw::ActionRow::builder()
                        .title("NixOS Config Type")
                        .subtitle(&config_label)
                        .build();
                    config_row.add_prefix(&gtk::Image::from_icon_name("emblem-system-symbolic"));
                    info_group_fill.add(&config_row);

                    // Update first check row title for NixOS
                    if let Some(row) = check_rows_fill.borrow().first() {
                        row.set_title("nixos-rebuild available");
                    }
                }

                // Store distro info
                *distro_state_fill.borrow_mut() = Some(info.clone());

                // Spawn upgrade availability check if supported
                if info.upgrade_supported {
                    let upgrade_row_clone = upgrade_available_row_fill.clone();
                    let distro_check = info.clone();
                    let upgrade_available_clone = upgrade_available_fill.clone();
                    let upgrade_btn_for_avail = upgrade_btn_fill.clone();
                    glib::spawn_future_local(async move {
                        let (tx, rx) = async_channel::unbounded::<String>();
                        std::thread::spawn(move || {
                            let result = upgrade::check_upgrade_available(&distro_check);
                            let _ = tx.send_blocking(result);
                            drop(tx);
                        });
                        if let Ok(result_msg) = rx.recv().await {
                            let is_available = result_msg.starts_with("Yes");
                            *upgrade_available_clone.borrow_mut() = is_available;
                            upgrade_row_clone.set_subtitle(&result_msg);
                            if !is_available {
                                upgrade_btn_for_avail.set_sensitive(false);
                            }
                        } else {
                            upgrade_available_row_fill.set_subtitle(
                                "Could not determine upgrade availability",
                            );
                        }
                    });
                }

                // Enable check button and auto-trigger if supported
                check_btn_fill.set_sensitive(true);
                if info.upgrade_supported {
                    check_btn_fill.emit_clicked();
                }
            }
            Err(_) => {
                // Detection failed (should never happen with current detect_distro impl)
                eprintln!("Distro detection channel closed unexpectedly");
                distro_row_fill.set_subtitle("Unknown");
                version_row_fill.set_subtitle("Unknown");
                check_btn_fill.set_sensitive(false);
            }
        }
    });
}
```

#### Step 8 — Make `detect_hostname()` and `detect_nixos_config_type()` public if needed

Both `upgrade::detect_hostname()` and `upgrade::detect_nixos_config_type()` are currently `pub fn` — no change needed.

---

## 7. Visual State Machine

### 7.1 Update Page (`window.rs`)

```
Window opens
    │
    ├─► backends_group shows: "Detecting package managers…" [spinner]
    │   buttons: "Update All" enabled (but detected is empty — no-op if clicked early)
    │
    │   [background thread: detect_backends() running]
    │
    ├─► Detection complete (100ms–500ms later)
    │   ├─ Remove placeholder row
    │   ├─ Add one UpdateRow per detected backend
    │   ├─ Show each row with "Checking…" status
    │   └─ Trigger availability count for each backend
    │
    └─► Normal state: rows show available update counts
```

### 7.2 Upgrade Page (`upgrade_page.rs`)

```
Upgrade tab opened
    │
    ├─► distro_row subtitle: "Loading…"
    │   version_row subtitle: "Loading…"
    │   upgrade_available_row subtitle: "Loading…"
    │   check_button: DISABLED
    │
    │   [background thread: detect_distro() + optional NixOS checks running]
    │
    ├─► Detection complete (< 5ms typical, up to 100ms on NFS)
    │   ├─ Update distro_row subtitle: e.g. "Ubuntu 24.04 LTS"
    │   ├─ Update version_row subtitle: e.g. "24.04"
    │   ├─ Update upgrade_available_row subtitle: "Checking…" or "Not supported"
    │   ├─ If NixOS: add config row, update check row title
    │   ├─ Enable check_button
    │   └─ If upgrade_supported: auto-click check_button
    │
    │   [background thread: check_upgrade_available() if supported]
    │
    └─► Normal state
```

---

## 8. Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| User clicks "Update All" before backends detected | Medium | No-op (empty `detected`) | Acceptable UX; the placeholder row signals detection in progress |
| `detect_distro()` panics unexpectedly | Very Low | Channel closes; `Err` branch fires | Log to stderr, show "Unknown" subtitle, leave check_button disabled |
| NixOS hostname contains UI-unsafe characters | Low | XSS in `format!()` string | Already sanitized via `glib::markup_escape_text()` — preserve this in new code |
| `check_button` clicked before `distro_info_state` is `Some` | Low | `expect()` panic | Prevented by disabling `check_button` until detection completes |
| Extra variable captures increase reference count | Low | Minor memory overhead | `Rc` clones are cheap; total added refs < 10 |
| Re-ordering `run_checks` trigger changes UX timing | Low | Availability check runs later | No user-visible change; detection < 500ms on any reasonable system |
| `backends_group.remove(&placeholder_row)` fails silently | Low | Stale placeholder stays | `remove()` is safe if widget not found; spinner stops/widget is hidden at worst |

---

## 9. Files to Modify

| File | Change |
|---|---|
| `src/ui/window.rs` | Replace sync `detect_backends()` with async pattern; placeholder row; `Rc<RefCell<...>>` for `detected` |
| `src/ui/upgrade_page.rs` | Replace sync `detect_distro()` + NixOS checks with async pattern; placeholder text; disabled check_button |

No changes to:
- `src/backends/mod.rs` — `detect_backends()` stays synchronous; it's simply called from a background thread
- `src/upgrade.rs` — `detect_distro()`, `detect_nixos_config_type()`, `detect_hostname()` stay synchronous
- `src/ui/mod.rs` — `spawn_background_async` already handles this use case perfectly

---

## 10. Dependency Verification

| Library | Current Version (Cargo.toml) | Context7 ID | Usage |
|---|---|---|---|
| `gtk4` | 0.9 | `/gtk-rs/gtk4-rs` | `glib::spawn_future_local`, `gtk::Spinner` |
| `async-channel` | (from gtk4-rs deps) | — | `unbounded::<T>()` channel |
| `which` | existing | — | No changes; still called from background thread |

No new dependencies are required.

---

## 11. Summary

**Root cause:** `backends::detect_backends()` in `window.rs` (line ~97) and `upgrade::detect_distro()` + NixOS filesystem checks in `upgrade_page.rs` (line ~53 and ~78) are called synchronously during GTK widget construction, blocking the GTK main event loop.

**Solution:** Both detections are moved to background threads using the existing `spawn_background_async` helper. Results flow back to the GTK thread via `async_channel`, where `glib::spawn_future_local` receives them and populates the UI. Placeholders are shown during the detection window.

**Spec file path:** `/home/nimda/Projects/Up/.github/docs/subagent_docs/blocking_io_detection_spec.md`
