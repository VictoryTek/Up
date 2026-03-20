# Exit Code Propagation — Specification

**Feature:** Fix `main()` to propagate `ExitCode` returned by `app.run()`  
**Severity:** Medium  
**Affected File(s):** `src/main.rs`  
**Date:** 2026-03-19  

---

## 1. Current State Analysis

### `fn main()` body (verbatim from `src/main.rs`)

```rust
fn main() {
    gio::resources_register_include!("compiled.gresource")
        .expect("Failed to register resources.");
    env_logger::init();
    let app = UpApplication::new();
    app.run();
}
```

- **Return type:** `fn main()` — no return type declared; the function implicitly returns `()`.
- **`app.run()` call:** The call ends with a semicolon (`app.run();`), so the `ExitCode` value is evaluated and then **discarded**. The OS always receives exit code 0.

### `UpApplication::run()` signature (verbatim from `src/app.rs`)

```rust
pub fn run(&self) -> gtk::glib::ExitCode {
    self.app.run()
}
```

- **Return type:** `gtk::glib::ExitCode` — already correct. No changes to `src/app.rs` are needed.

### Confirmation

`UpApplication::run()` **already** returns `gtk::glib::ExitCode`. The only bug is that `main()` discards this value by:
1. Declaring no return type (`fn main()`), and  
2. Calling `app.run();` with a terminating semicolon.

---

## 2. Problem Definition

### Exit code is always 0

When `fn main()` returns `()`, the OS always sees exit code `0` — regardless of whether the GTK application reported an error through its `ApplicationExitCode`.

### Masked failure conditions

`gtk::gio::Application::run()` (and by extension `adw::Application::run()`) communicates several abnormal conditions through its return value:

| Condition | GTK exit code |
|---|---|
| Normal exit | `0` |
| Another instance already running (not primary) | `1` (or non-zero, platform-dependent) |
| Startup/activation failure | non-zero |
| `g_application_quit()` called with a status | non-zero |

By discarding this value, the following consumers cannot detect failures:

- **Shell scripts** that invoke `up` and check `$?`
- **systemd units** (`ExecStart=`) that react to non-zero exit codes
- **Desktop launchers / process supervisors** that restart on failure
- **CI/CD pipelines** that wrap the launched GTK binary

### Standard Rust/GTK practice

The idiomatic pattern for GTK4 applications in Rust is:

```rust
fn main() -> gtk::glib::ExitCode {
    Application::new(...).run()
}
```

This is the pattern used in the official `gtk4-rs` examples and is consistent with how `std::process::ExitCode` is used across the Rust ecosystem.

---

## 3. Proposed Solution

Change `fn main()` to return `gtk::glib::ExitCode` and drop the semicolon on the final `app.run()` call so the value is returned rather than discarded.

### Before (`src/main.rs`)

```rust
fn main() {
    gio::resources_register_include!("compiled.gresource")
        .expect("Failed to register resources.");
    env_logger::init();
    let app = UpApplication::new();
    app.run();
}
```

### After (`src/main.rs`)

```rust
fn main() -> gtk::glib::ExitCode {
    gio::resources_register_include!("compiled.gresource")
        .expect("Failed to register resources.");
    env_logger::init();
    UpApplication::new().run()
}
```

**Changes:**
1. `fn main()` → `fn main() -> gtk::glib::ExitCode`
2. Remove the `let app = UpApplication::new();` binding and the separate `app.run();` statement; replace with the single tail expression `UpApplication::new().run()` (no semicolon) so it is returned.

> **Note on the `let app` binding:** The binding can be inlined safely. `UpApplication::new()` returns `Self` and `run(&self)` borrows it; the temporary lives for the duration of the expression, which is sufficient. If any future code between `new()` and `run()` requires the binding, restore `let app = UpApplication::new();` and change the final call to `app.run()` (no semicolon).

No changes are required to `src/app.rs`.

---

## 4. Implementation Steps

1. Open `src/main.rs`.
2. Change the function signature from `fn main()` to `fn main() -> gtk::glib::ExitCode`.
3. Replace the two lines:
   ```rust
   let app = UpApplication::new();
   app.run();
   ```
   with the single tail expression:
   ```rust
   UpApplication::new().run()
   ```
4. Run `cargo build` to confirm compilation succeeds.
5. Run `cargo clippy -- -D warnings` to confirm no new warnings.
6. Run `cargo fmt --check` to confirm formatting.

---

## 5. Dependencies

No new dependencies. `gtk::glib::ExitCode` is already available via the existing `gtk = { version = "0.9", package = "gtk4" }` dependency declared in `Cargo.toml`.

---

## 6. Affected Files

| File | Change Required |
|---|---|
| `src/main.rs` | Yes — signature change + remove semicolon |
| `src/app.rs` | **No** — `run()` already returns `gtk::glib::ExitCode` |

---

## 7. Risks and Mitigations

### Risk: `gtk::glib::ExitCode` must implement `std::process::Termination`

For `fn main() -> gtk::glib::ExitCode` to compile, `gtk::glib::ExitCode` must implement the `std::process::Termination` trait (stabilised in Rust 1.61).

**Status: CONFIRMED SAFE.**

The project uses `glib = "0.20"` (part of the gtk4-rs 0.9 / libadwaita 0.7 ecosystem). In `glib-rs` 0.16+ (released alongside gtk4-rs 0.5+), `glib::ExitCode` implements `std::process::Termination`. This has been stable and unchanged through glib-rs 0.18, 0.19, and 0.20. The implementation is:

```rust
// From glib-rs source (glib/src/main_context.rs / lib.rs)
impl std::process::Termination for ExitCode {
    fn report(self) -> std::process::ExitCode {
        std::process::ExitCode::from(self.0 as u8)
    }
}
```

No compatibility risk exists for this project's dependency versions.

### Fallback (not needed, documented for completeness)

If a future glib-rs version were to remove the `Termination` impl (extremely unlikely), the alternative is:

```rust
fn main() {
    // ...setup...
    let code = UpApplication::new().run();
    std::process::exit(code.value() as i32);
}
```

This is not required for the current dependency set and should **not** be implemented.

### Risk: Behaviour change visible to callers

Previously, the process always exited 0. After this fix, it may exit non-zero when GTK reports a failure. This is the **intended and correct** behaviour. No regression is expected for normal successful runs.
