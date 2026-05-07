# Specification: Shared Tokio Runtime + `glib::clone!` Macro

**Backlog Items Addressed:**
- §4 [LOW] — Use `glib::clone!` macro to reduce verbose `Rc::clone()` chains in UI code
- §5 [MED] — Create a single shared Tokio runtime in `main` instead of one fresh runtime per background spawn
- §5 [LOW] — Use `rt-multi-thread` Tokio feature + a shared runtime instead of per-thread `current_thread` runtimes

---

## 1. Current State Analysis

### 1.1 Tokio Runtime Creation

Two private helper functions both independently build fresh `new_current_thread` runtimes and block an OS thread:

**`src/ui/mod.rs` (lines 14–31) — `spawn_background_async`**
```rust
pub(crate) fn spawn_background_async<F, Fut>(f: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    std::thread::spawn(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => {
                rt.block_on(f());
            }
            Err(e) => {
                eprintln!("Failed to build Tokio runtime: {e}");
            }
        }
    });
}
```

**`src/orchestrator.rs` (lines 100–113) — `spawn_background` (private)**
```rust
fn spawn_background<F, Fut>(f: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    std::thread::spawn(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt.block_on(f()),
            Err(e) => eprintln!("Failed to build Tokio runtime: {e}"),
        }
    });
}
```

Both are identical in shape: spawn an OS thread → build a single-thread runtime → block on a future.

**Problems:**
- Every background operation allocates a new OS thread + a new Tokio runtime.
- Thread creation is expensive; Tokio's multi-thread runtime provides a reusable thread pool.
- Two separate helpers with the same pattern create unnecessary duplication.

### 1.2 Tokio Feature Flags

`Cargo.toml` currently declares:
```toml
tokio = { version = "1", features = ["rt", "macros", "io-util", "process", "fs", "sync", "time"] }
```

The `rt-multi-thread` feature is absent. `Builder::new_multi_thread()` compiles but panics at runtime without this feature.

### 1.3 Verbose Clone Chains in UI Code

The codebase does not use `Rc::clone(...)` explicitly; instead it uses the `.clone()` method. The pattern is identical: several variables are cloned immediately before a closure and given `_clone` / `_ref` / `_for_xxx` suffixes, then the closure captures those renamed bindings. This creates multi-line boilerplate blocks before every signal handler or `glib::spawn_future_local` body.

The `glib::clone!` macro (available since glib-rs ~0.15, present in glib 0.20 used here) inlines the capture list directly in the closure syntax:

```rust
// Before (manual):
let rows_ref = rows_clone.clone();
let log_ref  = log_clone.clone();
let btn_ref  = button.clone();
glib::spawn_future_local(move || async move {
    // uses rows_ref, log_ref, btn_ref
});

// After (glib::clone!):
glib::spawn_future_local(glib::clone!(
    #[strong] rows_clone,
    #[weak]   log_panel,
    #[weak]   button,
    => async move {
        // uses rows_clone, log_panel, button
    }
));
```

---

## 2. Dependency / API Verification

### 2.1 `glib::clone!` macro — glib 0.20

The `glib` crate version is `0.20` (confirmed from `Cargo.toml`).

**Capture modes:**
| Attribute | Behavior | Use for |
|-----------|----------|---------|
| `#[weak] x` | Calls `x.downgrade()` before closure; auto-upgrades on entry. | GTK GObject widgets (`gtk::Button`, `adw::ActionRow`, etc.) |
| `#[weak(rename_to = y)] x` | Same but renames to `y` inside the closure. | Disambiguation |
| `#[strong] x` | Calls `x.clone()`. | `Rc<T>`, `LogPanel`, `Arc<T>` |
| `#[upgrade_or] expr` | Inline fallback if a weak upgrade fails; returns `expr`. | Required when closure returns non-`()` |
| `#[upgrade_or_else] \|\| expr` | Closure-form fallback. | Same |

For closures returning `()` (signal handlers, `spawn_future_local` blocks), `#[upgrade_or] return` is sufficient.

**Arrow syntax:**
```rust
glib::clone!(#[weak] a, #[strong] b, => move |arg| { ... })
// or for async:
glib::clone!(#[weak] a, #[strong] b, => async move { ... })
```

GTK widgets implement `glib::clone::Downgrade`, so `#[weak]` works on any `gtk::Widget`, `adw::Widget`, `gtk::Button`, `adw::ActionRow`, etc.

`Rc<T>` does NOT implement `Downgrade`, so `#[strong]` is required for all `Rc<RefCell<T>>` and `Rc<dyn Fn()>` values.

`LogPanel` is a custom struct (derives `Clone`) containing GTK widgets and an `Rc`; it does not implement `Downgrade` — use `#[strong]`.

### 2.2 Tokio multi-thread runtime — tokio 1.x

`tokio::runtime::Runtime` is `Send + Sync`. It can therefore live in a `static` and be accessed from any thread.

`std::sync::OnceLock<T>` (stable since Rust 1.70, Edition 2021 target is fine) provides thread-safe lazy initialization.

`Runtime::spawn(future)` requires `Future + Send + 'static`. All futures currently passed to `spawn_background_async` / `spawn_background` capture only `Send` types:
- `async_channel::Sender<T>` — `Send`
- `Arc<dyn Backend>` — `Send + Sync`
- Plain data types in `upgrade::*` — `Send`
- `tokio::sync::Mutex<PrivilegedShell>` inside `Arc` — `Send`

No GTK GObject types are captured in any background future. ✓

`Runtime::spawn` does **not** block the calling thread; it schedules the future onto the thread pool and immediately returns a `JoinHandle`. The calling thread (GTK main loop) is not blocked. ✓

`tokio::spawn` inside a future that itself runs on the multi-thread runtime works correctly — the Tokio context is set on the worker thread, so nested spawns go onto the same runtime. This is used inside `orchestrator.rs` `spawn_background`'s async closure (`tokio::spawn` for the log-forwarding task). ✓

---

## 3. Required Changes

### 3.1 `Cargo.toml` — Add `rt-multi-thread`

**File:** `Cargo.toml`

**Change:** Add `rt-multi-thread` to the tokio features list.

```toml
# Before:
tokio = { version = "1", features = ["rt", "macros", "io-util", "process", "fs", "sync", "time"] }

# After:
tokio = { version = "1", features = ["rt", "rt-multi-thread", "macros", "io-util", "process", "fs", "sync", "time"] }
```

---

### 3.2 New file: `src/runtime.rs`

Create a new module that owns the single shared `tokio::runtime::Runtime` for the entire process lifetime.

```rust
//! Process-global Tokio runtime.
//!
//! All background async work is scheduled onto a single multi-threaded
//! runtime instead of spinning up one runtime per background spawn.
//! The runtime is initialized lazily on first use and lives for the
//! entire process lifetime.

use std::sync::OnceLock;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// Returns a reference to the shared Tokio runtime.
///
/// Panics at startup if the runtime cannot be built (extremely rare;
/// would indicate severe OS resource exhaustion).
pub fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build Tokio runtime")
    })
}
```

---

### 3.3 `src/main.rs` — Declare the new module

**File:** `src/main.rs`

Add `mod runtime;` to the module declarations.

```rust
// Before:
mod app;
mod backends;
mod executor;
mod orchestrator;
mod reboot;
mod runner;
mod ui;
mod upgrade;

// After:
mod app;
mod backends;
mod executor;
mod orchestrator;
mod reboot;
mod runtime;
mod runner;
mod ui;
mod upgrade;
```

---

### 3.4 `src/ui/mod.rs` — Replace `spawn_background_async`

Remove the `std::thread::spawn` + `new_current_thread` pattern. Use the shared runtime directly.

**The `Fut: Future + Send + 'static` bound must be added** because `Runtime::spawn` requires `Send`.

```rust
// Before:
use std::future::Future;

pub(crate) fn spawn_background_async<F, Fut>(f: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    std::thread::spawn(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => {
                rt.block_on(f());
            }
            Err(e) => {
                eprintln!("Failed to build Tokio runtime: {e}");
            }
        }
    });
}

// After:
use std::future::Future;

pub(crate) fn spawn_background_async<F, Fut>(f: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    crate::runtime::runtime().spawn(f());
}
```

The `std::future::Future` import is still needed. The `use` statement at the top of `mod.rs` should be retained unchanged.

---

### 3.5 `src/orchestrator.rs` — Replace private `spawn_background`

Same change: eliminate the OS thread + `new_current_thread` and use the shared runtime.

```rust
// Before:
fn spawn_background<F, Fut>(f: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    std::thread::spawn(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt.block_on(f()),
            Err(e) => eprintln!("Failed to build Tokio runtime: {e}"),
        }
    });
}

// After:
fn spawn_background<F, Fut>(f: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    crate::runtime::runtime().spawn(f());
}
```

The `use std::future::Future;` import at the top of `orchestrator.rs` is already present and must be retained.

---

### 3.6 `src/ui/window.rs` — Apply `glib::clone!`

#### 3.6.1 `refresh_button.connect_clicked` (lines ~107–115)

**Before:**
```rust
let run_checks_btn = run_checks.clone();
let update_in_progress_ref = update_in_progress.clone();
refresh_button.connect_clicked(move |_| {
    if update_in_progress_ref.get() {
        return; // silently ignore clicks during active update
    }
    (*run_checks_btn)()
});
```

**After:**
```rust
refresh_button.connect_clicked(glib::clone!(
    #[strong] run_checks,
    #[strong] update_in_progress,
    => move |_| {
        if update_in_progress.get() {
            return;
        }
        (run_checks)()
    }
));
```

Both `run_checks: Rc<dyn Fn()>` and `update_in_progress: Rc<Cell<bool>>` are `Rc` types → `#[strong]`.

#### 3.6.2 `about_action.connect_activate` (lines ~136–145)

This site already manually calls `window.downgrade()`. Replace with `glib::clone! #[weak]`.

**Before:**
```rust
let window_ref = window.downgrade();
about_action.connect_activate(move |_, _| {
    let Some(win) = window_ref.upgrade() else {
        return;
    };
    let dialog = adw::AboutDialog::builder()
        // ...
        .build();
    dialog.present(Some(&win));
});
```

**After:**
```rust
about_action.connect_activate(glib::clone!(
    #[weak] window,
    #[upgrade_or] return,
    => move |_, _| {
        let dialog = adw::AboutDialog::builder()
            // ...
            .build();
        dialog.present(Some(&window));
    }
));
```

`window` is an `adw::ApplicationWindow` (GObject) → `#[weak]`.

#### 3.6.3 `update_button.connect_clicked` (lines ~250–345)

This has two layers: the outer `connect_clicked` closure, and an inner `glib::spawn_future_local` block.

**Before (outer pre-clones):**
```rust
let status_clone = status_label.clone();
let rows_clone = rows.clone();
let log_clone = log_panel.clone();
let detected_clone = detected.clone();
let restart_banner_clone = restart_banner.clone();

let updating: Rc<Cell<bool>> = Rc::new(Cell::new(false));
let updating_for_btn = updating.clone();

update_button.connect_clicked(move |button| {
    button.set_sensitive(false);
    updating_for_btn.set(true);
    log_clone.clear();

    let rows_ref = rows_clone.clone();
    let log_ref = log_clone.clone();
    let status_ref = status_clone.clone();
    let button_ref = button.clone();
    let backends = detected_clone.borrow().clone();
    let banner_ref = restart_banner_clone.clone();
    let updating_ref = updating_for_btn.clone();

    glib::spawn_future_local(async move {
        // uses rows_ref, log_ref, status_ref, button_ref, backends,
        //       banner_ref, updating_ref
        ...
    });
});
```

**After:**
```rust
let updating: Rc<Cell<bool>> = Rc::new(Cell::new(false));

update_button.connect_clicked(glib::clone!(
    #[weak]   status_label,
    #[strong] rows,
    #[strong] log_panel,
    #[strong] detected,
    #[weak]   restart_banner,
    #[strong] updating,
    => move |button| {
        button.set_sensitive(false);
        updating.set(true);
        log_panel.clear();

        let backends = detected.borrow().clone();

        glib::spawn_future_local(glib::clone!(
            #[strong] rows,
            #[strong] log_panel,
            #[weak]   status_label,
            #[weak]   button,
            #[weak]   restart_banner,
            #[strong] updating,
            => async move {
                // uses rows, log_panel, status_label, button,
                //      restart_banner, updating, backends
                ...
            }
        ));
    }
));
```

Type guide:
- `status_label: gtk::Label` — GObject → `#[weak]`
- `rows: Rc<RefCell<Vec<...>>>` — Rc → `#[strong]`
- `log_panel: LogPanel` — custom Clone struct → `#[strong]`
- `detected: Rc<RefCell<Vec<Arc<dyn Backend>>>>` — Rc → `#[strong]`
- `restart_banner: adw::Banner` — GObject → `#[weak]`
- `updating: Rc<Cell<bool>>` — Rc → `#[strong]`
- `button: gtk::Button` (closure arg) — captured by `clone!` from the outer arg — GObject → `#[weak]`

> **Note on `button` capture:** The inner `glib::clone!` captures `button` which is a closure argument of the outer closure (type `gtk::Button`). Closure arguments can be captured by the inner `glib::clone!` since they are in scope. Use `#[weak] button` to avoid a reference cycle (the button holds its signal handlers strongly; a strong capture of the button itself in those handlers creates a cycle).

#### 3.6.4 `run_checks: Rc<dyn Fn()>` closure (lines ~360–460)

This is the most verbose site. The outer `Rc::new(move || {...})` captures 7 values, and inside, per-backend spawns capture 6 more.

**Before (outer closure captures):**
```rust
let run_checks: Rc<dyn Fn()> = {
    let rows = rows.clone();
    let detected = detected.clone();
    let update_button_checks = update_button.clone();
    let pending_checks = pending_checks.clone();
    let total_available = total_available.clone();
    let check_epoch = check_epoch.clone();
    let status_label_checks = status_label.clone();
    Rc::new(move || {
        ...
        for backend in detected.borrow().iter() {
            let backend_clone = backend.clone();
            let rows_ref = rows.clone();
            let pending_ref = pending_checks.clone();
            let total_ref = total_available.clone();
            let btn_ref = update_button_checks.clone();
            let status_ref = status_label_checks.clone();
            let epoch_ref = check_epoch.clone();
            glib::spawn_future_local(async move {
                // uses backend_clone, rows_ref, pending_ref, total_ref,
                //       btn_ref, status_ref, epoch_ref
            });
        }
    })
};
```

**After:**
```rust
let run_checks: Rc<dyn Fn()> = {
    // Direct clones still needed here because Rc<dyn Fn()> is constructed
    // via Rc::new(move ||{}), not via a signal connector.
    // glib::clone! can be used for the inner glib::spawn_future_local.
    let rows = rows.clone();
    let detected = detected.clone();
    let update_button_checks = update_button.clone();
    let pending_checks = pending_checks.clone();
    let total_available = total_available.clone();
    let check_epoch = check_epoch.clone();
    let status_label_checks = status_label.clone();
    Rc::new(move || {
        ...
        for backend in detected.borrow().iter() {
            let backend_clone = backend.clone();
            let kind = backend.kind();
            glib::spawn_future_local(glib::clone!(
                #[strong] rows,
                #[strong] pending_checks,
                #[strong] total_available,
                #[weak]   update_button_checks,
                #[weak]   status_label_checks,
                #[strong] check_epoch,
                => async move {
                    // uses backend_clone, rows, pending_checks,
                    //       total_available, update_button_checks,
                    //       status_label_checks, check_epoch
                    ...
                }
            ));
        }
    })
};
```

> **Note:** The outer `Rc::new(move || {...})` cannot itself use `glib::clone!` because `glib::clone!` is designed for GTK signal connectors (`.connect_*`) and `glib::spawn_future_local`. The per-loop `glib::spawn_future_local` calls inside CAN use it. The 7 outer pre-clones (`rows`, `detected`, etc.) stay as manual `let x = x.clone()` lines.

#### 3.6.5 Backend detection (lines ~460–490)

**Before:**
```rust
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
        group_fill.remove(&placeholder_row);
        ...
        *detected_fill.borrow_mut() = new_backends;
        (*run_checks_after_detect)();
    } else {
        ...
        group_fill.remove(&placeholder_row);
    }
});
```

**After:**
```rust
super::spawn_background_async(move || async move {
    let backends = crate::backends::detect_backends();
    let _ = detect_tx.send(backends).await;
});

glib::spawn_future_local(glib::clone!(
    #[strong] detected,
    #[strong] rows,
    #[weak]   backends_group,
    #[strong] run_checks,
    => async move {
        if let Ok(new_backends) = detect_rx.recv().await {
            backends_group.remove(&placeholder_row);
            ...
            *detected.borrow_mut() = new_backends;
            (run_checks)();
        } else {
            ...
            backends_group.remove(&placeholder_row);
        }
    }
));
```

---

### 3.7 `src/ui/upgrade_page.rs` — Apply `glib::clone!`

#### 3.7.1 `recompute_state` Rc::new closure (lines ~135–143)

The `Rc::new(move || {...})` pattern cannot use `glib::clone!` on the outer wrapper, but the 4 pre-clones are still needed. No change here — `glib::clone!` is not applicable to `Rc::new(...)` constructors (there is no signal connector or `spawn_future_local` to wrap). Leave as-is.

#### 3.7.2 `backup_check.connect_toggled` (lines ~151–153)

**Before:**
```rust
let recompute_for_toggle = recompute_state.clone();
backup_check.connect_toggled(move |_| {
    recompute_for_toggle();
});
```

**After:**
```rust
backup_check.connect_toggled(glib::clone!(
    #[strong] recompute_state,
    => move |_| {
        recompute_state();
    }
));
```

`recompute_state: Rc<dyn Fn()>` → `#[strong]`.

#### 3.7.3 `check_button.connect_clicked` (lines ~159–197)

**Before:**
```rust
let check_rows_clone = check_rows.clone();
let check_icons_clone = check_icons.clone();
let log_clone = log_panel.clone();
let distro_state_for_check = distro_info_state.clone();
let all_checks_passed_clone = all_checks_passed.clone();
let recompute_state_for_check = recompute_state.clone();
check_button.connect_clicked(move |button| {
    let Some(distro) = distro_state_for_check.borrow().clone() else {
        return;
    };
    button.set_sensitive(false);
    log_clone.clear();

    let check_rows_ref = check_rows_clone.clone();
    let check_icons_ref = check_icons_clone.clone();
    let log_ref = log_clone.clone();
    let button_ref = button.clone();
    let all_checks_passed_ref = all_checks_passed_clone.clone();
    let recompute_ref = recompute_state_for_check.clone();

    glib::spawn_future_local(async move {
        // uses check_rows_ref, check_icons_ref, log_ref, button_ref,
        //       all_checks_passed_ref, recompute_ref
        ...
    });
});
```

**After:**
```rust
check_button.connect_clicked(glib::clone!(
    #[strong] check_rows,
    #[strong] check_icons,
    #[strong] log_panel,
    #[strong] distro_info_state,
    #[strong] all_checks_passed,
    #[strong] recompute_state,
    => move |button| {
        let Some(distro) = distro_info_state.borrow().clone() else {
            return;
        };
        button.set_sensitive(false);
        log_panel.clear();

        glib::spawn_future_local(glib::clone!(
            #[strong] check_rows,
            #[strong] check_icons,
            #[strong] log_panel,
            #[weak]   button,
            #[strong] all_checks_passed,
            #[strong] recompute_state,
            => async move {
                ...
            }
        ));
    }
));
```

#### 3.7.4 `upgrade_button.connect_clicked` (lines ~230+)

**Before (outer pre-clones):**
```rust
let log_clone2 = log_panel.clone();
let distro_state_for_upgrade = distro_info_state.clone();
let nixos_config_type_for_upgrade = nixos_config_type.clone();
upgrade_button.connect_clicked(move |button| {
    ...
    let log_ref = log_clone2.clone();
    let button_ref = button.clone();
    ...
    dialog.connect_response(None, move |_dialog, response| {
        if response == "upgrade" {
            ...
            let log_ref2 = log_ref.clone();
            let distro2 = distro.clone();
            let button_ref2 = button_ref.clone();
            let button_ref3 = button_ref.clone();
            glib::spawn_future_local(async move {
                ...
            });
        }
    });
    dialog.present(Some(&widget));
});
```

**After:**
```rust
upgrade_button.connect_clicked(glib::clone!(
    #[strong] log_panel,
    #[strong] distro_info_state,
    #[strong] nixos_config_type,
    => move |button| {
        ...
        // dialog.connect_response inner closure:
        dialog.connect_response(None, glib::clone!(
            #[strong] log_panel,
            #[weak]   button,
            => move |_dialog, response| {
                if response == "upgrade" {
                    ...
                    glib::spawn_future_local(glib::clone!(
                        #[strong] log_panel,
                        #[weak]   button,
                        => async move {
                            ...
                        }
                    ));
                }
            }
        ));
        dialog.present(Some(&button));
    }
));
```

#### 3.7.5 `init_rx` handler — `glib::spawn_future_local` (lines ~340+)

**Before:**
```rust
let nixos_config_type_fill = nixos_config_type.clone();
let flake_banner_fill = flake_banner.clone();
let distro_state_fill = distro_info_state.clone();
let upgrade_available_row_fill = upgrade_available_row.clone();
let info_group_fill = info_group.clone();
let check_rows_fill = check_rows.clone();
let upgrade_available_fill = upgrade_available.clone();
let check_btn_fill = check_button.clone();
let recompute_state_for_init = recompute_state.clone();

glib::spawn_future_local(async move {
    if let Ok(init) = init_rx.recv().await {
        ...
        if info.upgrade_supported {
            let upgrade_row_clone = upgrade_available_row_fill.clone();
            let distro_check = info.clone();
            let upgrade_available_clone = upgrade_available_fill.clone();
            let recompute_for_avail = recompute_state_for_init.clone();
            glib::spawn_future_local(async move {
                ...
            });
        }
        ...
    }
});
```

**After:**
```rust
glib::spawn_future_local(glib::clone!(
    #[strong] nixos_config_type,
    #[weak]   flake_banner,
    #[strong] distro_info_state,
    #[weak]   upgrade_available_row,
    #[weak]   info_group,
    #[strong] check_rows,
    #[strong] upgrade_available,
    #[weak]   check_button,
    #[strong] recompute_state,
    => async move {
        if let Ok(init) = init_rx.recv().await {
            ...
            if info.upgrade_supported {
                glib::spawn_future_local(glib::clone!(
                    #[weak]   upgrade_available_row,
                    #[strong] upgrade_available,
                    #[strong] recompute_state,
                    => async move {
                        ...
                    }
                ));
            }
            ...
        }
    }
));
```

Type guide for this block:
- `nixos_config_type: Rc<RefCell<Option<...>>>` — `#[strong]`
- `flake_banner: adw::Banner` — GObject → `#[weak]`
- `distro_info_state: Rc<RefCell<Option<DistroInfo>>>` — `#[strong]`
- `upgrade_available_row: adw::ActionRow` — GObject → `#[weak]`
- `info_group: adw::PreferencesGroup` — GObject → `#[weak]`
- `check_rows: Rc<RefCell<Vec<adw::ActionRow>>>` — `#[strong]`
- `upgrade_available: Rc<RefCell<bool>>` — `#[strong]`
- `check_button: gtk::Button` — GObject → `#[weak]`
- `recompute_state: Rc<dyn Fn()>` — `#[strong]`

---

## 4. Implementation Summary

### Files to create
| File | Action |
|------|--------|
| `src/runtime.rs` | Create new — shared Tokio runtime via `OnceLock` |

### Files to modify
| File | Change |
|------|--------|
| `Cargo.toml` | Add `rt-multi-thread` to tokio features |
| `src/main.rs` | Add `mod runtime;` |
| `src/ui/mod.rs` | Replace `spawn_background_async` body; add `Send + 'static` bound to `Fut` |
| `src/orchestrator.rs` | Replace `spawn_background` body; add `Send + 'static` bound to `Fut` |
| `src/ui/window.rs` | Apply `glib::clone!` to ~5 closure groups |
| `src/ui/upgrade_page.rs` | Apply `glib::clone!` to ~5 closure groups |

### Files not changed
| File | Reason |
|------|--------|
| `src/ui/update_row.rs` | No pre-closure clone chains (struct clone used only inside methods) |
| `src/ui/log_panel.rs` | No signal handlers with pre-clone chains |
| `src/orchestrator.rs` logic | Only the private `spawn_background` helper body changes |

---

## 5. Risks and Mitigations

### Risk 1: `Send` bound on futures

**Risk:** Adding `+ Send + 'static` to `Fut` in `spawn_background_async` / `spawn_background` will cause compile errors if any call site passes a future that captures `!Send` types (e.g., `Rc<T>`, GTK GObjects).

**Assessment:** All current call sites pass futures that capture only `Send` types:
- `async_channel::Sender<T>` — `Send` ✓
- `Arc<dyn Backend>` — `Send` ✓
- Data types in `upgrade::*` — `Send` ✓
- No GTK objects are captured in background futures ✓

**Mitigation:** The Rust compiler will catch any `!Send` captures as errors at the call sites. If a new call site captures a `!Send` type, the developer will receive a clear compile error.

### Risk 2: `glib::clone!` with `#[weak]` and `#[upgrade_or] return`

**Risk:** If a widget is dropped before a closure fires (e.g., window closed while an async task is running), `#[upgrade_or] return` causes the closure to silently do nothing. This is the **correct** behavior — it avoids use-after-free of UI widgets.

**Assessment:** Existing code already handles this for the `about_action` (using manual `window.downgrade()`). Extending `#[weak]` to other closures follows the same semantics. The strong refcount elsewhere in the widget tree ensures widgets stay alive as long as the window is visible.

**Mitigation:** No behavioral change for active windows. For closed/destroyed windows, the `return` fallback is correct GTK practice.

### Risk 3: OnceLock initialization order

**Risk:** If `crate::runtime::runtime()` is called before `gtk::glib::ExitCode` is returned (i.e., before any shutdown hooks), there is no issue. If the runtime is used after `gtk::Application::run()` returns, tasks on the runtime may still be executing.

**Assessment:** The Tokio runtime lives for the entire process lifetime (it is in a `static`). When `main` returns, the process exits, and all runtime threads are killed. There is no graceful shutdown. This is the same behavior as before (background threads with `block_on` are also killed on exit).

**Mitigation:** Acceptable for a GTK desktop app. If graceful shutdown is needed in the future, a `tokio::sync::CancellationToken` can be added.

### Risk 4: `tokio::spawn` inside futures running on the shared runtime

**Risk:** The orchestrator's async closure calls `tokio::spawn(...)` for the log-forwarding task. With `new_current_thread`, this `tokio::spawn` required that the current thread had an active Tokio context (set by `block_on`). With the multi-thread runtime, the async closure runs on a Tokio worker thread, which also has a Tokio context. `tokio::spawn` works correctly.

**Assessment:** No behavior change. The `tokio::spawn` inside the orchestrator continues to work because it executes on a Tokio worker thread.

**Mitigation:** None required.

### Risk 5: `block_on` calls removed

**Risk:** The old `spawn_background` used `rt.block_on(f())`, which ran the future to completion before the OS thread exited. The new `runtime().spawn(f())` is fire-and-forget (from the caller's perspective). If background work is abandoned mid-execution when the window closes, tasks still running on the multi-thread runtime will continue until the process exits.

**Assessment:** The behavior is effectively the same as before — the GTK window close does not currently wait for background tasks. The existing `glib::spawn_future_local` handlers receive completion signals via `async_channel`, and if the receiver is dropped (window closed), the channel closes and the background future's `tx.send(...)` returns `SendError`. Existing code silently ignores those errors (`let _ = tx.send(...).await`). ✓

**Mitigation:** No action required. The existing error-ignoring pattern is intentional and sufficient.

---

## 6. Verification Steps

After implementation:

1. `cargo build` — must compile without errors (especially: no `!Send` errors at `spawn_background_async` / `spawn_background` call sites)
2. `cargo clippy -- -D warnings` — must pass (no unused imports, no `let _ =` warnings from `JoinHandle` returned by `runtime().spawn(...)`)
   - **Note:** `runtime().spawn(f())` returns `JoinHandle<()>`. It must be suppressed: `let _ = runtime().spawn(f());` or `drop(runtime().spawn(f()));`
3. `cargo fmt --check` — formatting must match
4. `cargo test` — no existing tests should regress

> **Important Clippy note (§6.2):** `tokio::runtime::Runtime::spawn` returns `JoinHandle<T>`. Clippy's `must_use` lint will fire if the return value is silently dropped. The implementation must explicitly discard it: `let _handle = crate::runtime::runtime().spawn(f());` or simply `crate::runtime::runtime().spawn(f());` (in Rust, calling a function and ignoring the return value is not a hard error, but Clippy may warn). Use `drop(crate::runtime::runtime().spawn(f()));` to be explicit.

---

## 7. Clone Site Inventory

### `src/ui/window.rs` — Clone groups (pre-closure boilerplate)

| # | Location | Variables cloned | Proposed macro | Notes |
|---|----------|-----------------|----------------|-------|
| W1 | ~line 107 — `refresh_button.connect_clicked` | `run_checks`, `update_in_progress` | `glib::clone!(#[strong] run_checks, #[strong] update_in_progress, => ...)` | Both Rc types |
| W2 | ~line 136 — `about_action.connect_activate` | `window` (via `.downgrade()`) | `glib::clone!(#[weak] window, #[upgrade_or] return, => ...)` | Already manual weak; convert to macro |
| W3 | ~line 250 — `update_button.connect_clicked` outer | 5 pre-clones + inner 7 clones | Two-level `glib::clone!` | Mixed GObject + Rc types |
| W4 | ~line 365 — `run_checks: Rc<dyn Fn()>` outer | 7 outer clones | Outer: manual; inner spawn: `glib::clone!` | Cannot use macro on `Rc::new(...)` wrapper |
| W5 | ~line 460 — backend detection `glib::spawn_future_local` | 4 pre-clones | `glib::clone!(#[strong]/#[weak] ..., => async move {...})` | |

### `src/ui/upgrade_page.rs` — Clone groups

| # | Location | Variables cloned | Proposed macro | Notes |
|---|----------|-----------------|----------------|-------|
| U1 | ~line 135 — `recompute_state` `Rc::new` | 4 pre-clones | Leave as manual | `Rc::new(...)` cannot use macro |
| U2 | ~line 151 — `backup_check.connect_toggled` | 1 (`recompute_state`) | `glib::clone!(#[strong] recompute_state, => ...)` | Trivial |
| U3 | ~line 159 — `check_button.connect_clicked` | 6 outer + 6 inner | Two-level `glib::clone!` | All Rc types |
| U4 | ~line 230 — `upgrade_button.connect_clicked` | 3 outer + nested dialog | `glib::clone!` on connect_clicked and connect_response | Three closure nesting levels |
| U5 | ~line 340 — `init_rx` `glib::spawn_future_local` | 9 pre-clones + 4 inner | `glib::clone!` with mixed `#[weak]`/`#[strong]` | Largest single site |

### `src/ui/update_row.rs` — No pre-closure clone chains

`update_row.rs` uses `.clone()` only inside struct methods (`pkg_rows.borrow_mut()` etc.), not for pre-closure boilerplate. **No changes needed.**

### `src/ui/log_panel.rs` — No pre-closure clone chains

`log_panel.rs` uses `Rc<Cell<bool>>` internally but has no signal handlers with pre-clone chains. **No changes needed.**

---

## 8. Runtime Creation Site Inventory

| File | Line | Function | Current Pattern | Proposed Replacement |
|------|------|----------|-----------------|----------------------|
| `src/ui/mod.rs` | 21 | `spawn_background_async` | `std::thread::spawn` + `Builder::new_current_thread()` + `block_on` | `crate::runtime::runtime().spawn(f())` |
| `src/orchestrator.rs` | 108 | `spawn_background` (private) | `std::thread::spawn` + `Builder::new_current_thread()` + `block_on` | `crate::runtime::runtime().spawn(f())` |

**Total runtime creation sites: 2** (both eliminated in favor of the shared runtime).
