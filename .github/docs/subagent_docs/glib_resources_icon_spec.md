# GLib Resource Bundle — Application Icon Embedding Spec

**Feature:** `glib_resources_icon`  
**Date:** 2026-03-19  
**Status:** Draft

---

## 1. Current State Analysis

### Files Involved

| File | Role |
|------|------|
| `src/app.rs` | Registers dev icon path via `add_search_path`; sets default icon name |
| `src/main.rs` | Entry point; no resource registration |
| `Cargo.toml` | Dependencies; no `[build-dependencies]`, no `build.rs` registered |
| `meson.build` | Installs icon PNGs to system hicolor dir; runs `gtk-update-icon-cache` |
| `data/icons/hicolor/256x256/apps/io.github.up.png` | The only bundled icon (256 px) |

### Current Icon Registration Logic (`src/app.rs`)

```rust
let dev_icons = concat!(env!("CARGO_MANIFEST_DIR"), "/data/icons");
if std::path::Path::new(dev_icons).exists() {
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::IconTheme::for_display(&display).add_search_path(dev_icons);
    }
}
gtk::Window::set_default_icon_name("io.github.up");
```

`add_search_path` **appends** the local path to the end of the icon theme search list; it does NOT prepend it.

---

## 2. Root Cause Analysis

### Why the icon fails on fresh builds

GTK's `IconTheme` maintains an ordered list of search directories. The lookup algorithm finds the **first match** in that list. `add_search_path` appends to the end — after all system directories.

When the application was previously installed (via `meson install` or a package), the icon was written to the system hicolor cache (e.g. `/usr/share/icons/hicolor/256x256/apps/io.github.up.png`). This system directory appears early in the theme search path.

Consequence: `cargo build` recompiles the binary with the new icon file in `data/icons/`, but the system icon cache is **never updated**. The theme search finds the old system copy first and returns it. `add_search_path` adding the local directory at the end has no effect because the lookup already resolved the name from the higher-priority system entry.

### Why `gtk-update-icon-cache` does not help

`gtk-update-icon-cache` is run by `meson.build`'s `gnome.post_install()` only during `meson install` — not during `cargo build`. A plain `cargo build` never updates the system cache.

### Summary of the bug

1. `add_search_path` appends; system dirs have higher priority.  
2. System icon cache is only refreshed on `meson install`, not on `cargo build`.  
3. Therefore the running dev binary always shows the previously installed icon, never the freshly built one.

---

## 3. Proposed Solution: GLib Resource Bundle

### Approach

Compile the icon PNG into a GLib resource bundle (`.gresource`) and embed it directly into the Rust binary at compile time using `glib-build-tools` and `gio::resources_register_include!`.

Register the embedded bundle as a resource search path on the `IconTheme` via `add_resource_path`. Resources registered this way are looked up **before** file-system paths, so the embedded icon always wins regardless of the system icon cache.

### Why this fixes the problem

| Old approach | New approach |
|---|---|
| `add_search_path(dir)` — appends to end of file-system search list | `add_resource_path(prefix)` — registered in the resource bundle, looked up **before** file-system paths |
| Depends on system icon cache being up-to-date | Each binary build embeds the icon; no external cache dependency |
| Old installed icon takes priority | Binary always wins; installed copy is irrelevant for the running process |
| Requires the `data/icons` directory to exist at runtime | No runtime file-system access needed; bytes are in the binary |

GLib resources are registered globally and are consulted by GTK's icon theme before any file-system directory. `GtkIconTheme` calls `g_resources_lookup_data` for resource paths, while `add_search_path` uses normal file I/O — the resource path has inherently higher priority.

---

## 4. Implementation Plan

### 4.1 Files to Create

#### (a) `data/io.github.up.gresource.xml`

Defines the resource bundle. The prefix `/io/github/up` matches the application ID converted to a path. The `alias` attribute maps the physical file path (relative to `data/`) to the canonical icon theme path structure (`hicolor/256x256/apps/…`) that GTK's icon theme expects under the registered resource prefix.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<gresources>
  <gresource prefix="/io/github/up">
    <file alias="hicolor/256x256/apps/io.github.up.png">icons/hicolor/256x256/apps/io.github.up.png</file>
  </gresource>
</gresources>
```

**Path resolution:**
- Resource bundle prefix: `/io/github/up`
- File alias inside bundle: `hicolor/256x256/apps/io.github.up.png`
- Full resource path: `/io/github/up/hicolor/256x256/apps/io.github.up.png`
- Physical source file: `data/icons/hicolor/256x256/apps/io.github.up.png` (resolved via `source_dirs = ["data"]` in `build.rs`)

GTK's icon theme, when given `add_resource_path("/io/github/up")`, will look up icons at `/io/github/up/hicolor/<size>/apps/<name>.png` — exactly matching the alias above.

---

#### (b) `build.rs` (project root)

Invokes `glib_build_tools::compile_resources` to compile the `.gresource.xml` and write `compiled.gresource` to Cargo's `OUT_DIR`. The `println!` directives tell Cargo to re-run the build script if the XML or the icon file changes.

```rust
fn main() {
    glib_build_tools::compile_resources(
        &["data"],
        "data/io.github.up.gresource.xml",
        "compiled.gresource",
    );
}
```

**Parameter explanation:**
- `&["data"]` — source directory; `glib-compile-resources` searches here for files referenced in the XML, resolving `icons/hicolor/256x256/apps/io.github.up.png` → `data/icons/hicolor/256x256/apps/io.github.up.png`
- `"data/io.github.up.gresource.xml"` — the resource definition file (relative to the workspace root, which is the working directory for `build.rs`)
- `"compiled.gresource"` — output filename, written to `$OUT_DIR/compiled.gresource`

`glib_build_tools::compile_resources` internally emits the necessary `cargo:rerun-if-changed` directives for the XML and all referenced source files, so incremental builds are correct.

---

### 4.2 Files to Modify

#### (c) `Cargo.toml` — add `[build-dependencies]`

Add a `[build-dependencies]` section at the end of the file. The version `0.20` matches the existing `glib = "0.20"` and `gio = "0.20"` dependencies for version consistency.

**Current state:** No `[build-dependencies]` section exists.

**Change — append to `Cargo.toml`:**

```toml
[build-dependencies]
glib-build-tools = "0.20"
```

No other `Cargo.toml` changes are required. The existing `gio = "0.20"` dependency already provides the `resources_register_include!` macro.

---

#### (d) `src/main.rs` — register resource bundle at startup

Call `gio::resources_register_include!("compiled.gresource")` as the very first statement in `main()`, before `env_logger::init()` and before creating the application. This macro expands to `include_bytes!(concat!(env!("OUT_DIR"), "/compiled.gresource"))` and registers the embedded bytes with the global GLib resource registry.

**Current `src/main.rs`:**

```rust
mod app;
mod backends;
mod reboot;
mod runner;
mod ui;
mod upgrade;

use app::UpApplication;

const APP_ID: &str = "io.github.up";

fn main() {
    env_logger::init();
    let app = UpApplication::new();
    app.run();
}
```

**Modified `src/main.rs`:**

```rust
mod app;
mod backends;
mod reboot;
mod runner;
mod ui;
mod upgrade;

use app::UpApplication;

const APP_ID: &str = "io.github.up";

fn main() {
    gio::resources_register_include!("compiled.gresource")
        .expect("Failed to register GLib resources.");

    env_logger::init();
    let app = UpApplication::new();
    app.run();
}
```

**Why first:** GLib resources must be registered before any GTK/GDK initialization. `UpApplication::new()` creates an `adw::Application`, which triggers GTK init. `env_logger::init()` is a pure-Rust call with no GTK involvement, but placing resource registration first is the safest and most idiomatic pattern.

---

#### (e) `src/app.rs` — replace `add_search_path` with `add_resource_path`

Remove the entire file-system-based dev icon path block and replace it with a single `add_resource_path` call pointing at the resource bundle prefix `/io/github/up`.

**Current block in `src/app.rs` (`on_activate`):**

```rust
// Add local icon search path when running from the project root (development mode).
// CARGO_MANIFEST_DIR is a compile-time absolute path; we only add it if the directory
// still exists at runtime, so installed/Flatpak builds are unaffected.
let dev_icons = concat!(env!("CARGO_MANIFEST_DIR"), "/data/icons");
if std::path::Path::new(dev_icons).exists() {
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::IconTheme::for_display(&display).add_search_path(dev_icons);
    }
}

gtk::Window::set_default_icon_name("io.github.up");
```

**Replacement:**

```rust
if let Some(display) = gtk::gdk::Display::default() {
    gtk::IconTheme::for_display(&display).add_resource_path("/io/github/up");
}

gtk::Window::set_default_icon_name("io.github.up");
```

**Why the dev-only guard is no longer needed:** The resource bundle is embedded in the binary. There is no runtime file-system path to check. The bundle is always present regardless of whether the binary is a dev build, an installed binary, or a Flatpak. `add_resource_path` is therefore unconditional.

**Why the `set_default_icon_name` call is unchanged:** GTK will now find `io.github.up` in the resource path. The name lookup goes: resource paths → file-system theme dirs. The embedded icon will be found first.

---

## 5. Exact File Contents (Complete)

### `data/io.github.up.gresource.xml` (new file)

```xml
<?xml version="1.0" encoding="UTF-8"?>
<gresources>
  <gresource prefix="/io/github/up">
    <file alias="hicolor/256x256/apps/io.github.up.png">icons/hicolor/256x256/apps/io.github.up.png</file>
  </gresource>
</gresources>
```

---

### `build.rs` (new file, project root)

```rust
fn main() {
    glib_build_tools::compile_resources(
        &["data"],
        "data/io.github.up.gresource.xml",
        "compiled.gresource",
    );
}
```

---

### `Cargo.toml` (partial — append section)

```toml
[build-dependencies]
glib-build-tools = "0.20"
```

---

### `src/main.rs` (complete modified file)

```rust
mod app;
mod backends;
mod reboot;
mod runner;
mod ui;
mod upgrade;

use app::UpApplication;

const APP_ID: &str = "io.github.up";

fn main() {
    gio::resources_register_include!("compiled.gresource")
        .expect("Failed to register GLib resources.");

    env_logger::init();
    let app = UpApplication::new();
    app.run();
}
```

---

### `src/app.rs` (complete modified file)

```rust
use adw::prelude::*;

use crate::ui::window::UpWindow;
use crate::APP_ID;

pub struct UpApplication {
    app: adw::Application,
}

impl UpApplication {
    pub fn new() -> Self {
        let app = adw::Application::builder().application_id(APP_ID).build();

        app.connect_activate(Self::on_activate);

        Self { app }
    }

    pub fn run(&self) -> gtk::glib::ExitCode {
        self.app.run()
    }

    fn on_activate(app: &adw::Application) {
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::IconTheme::for_display(&display).add_resource_path("/io/github/up");
        }

        gtk::Window::set_default_icon_name("io.github.up");

        let window = UpWindow::new(app);
        window.present();
    }
}
```

---

## 6. Meson Build — No Changes Required

`meson.build` continues to install the PNG to the system hicolor directory and call `gtk-update-icon-cache`. This remains correct for system-installed and Flatpak distributions where desktop environments (GNOME Shell, KDE Plasma app launchers, taskbars) need the icon in the system theme to display it outside the running process.

The GLib resource approach is complementary: it ensures the **running binary** always has the correct icon regardless of system state. The system installation ensures the **desktop environment** can show the icon in app launchers and taskbars.

No conflict exists between the two mechanisms. The running binary's resource path takes priority during the process's own icon lookups; the system installation serves external consumers.

---

## 7. Dependencies

| Package | Version | Role | Section |
|---------|---------|------|---------|
| `glib-build-tools` | `0.20` | Compiles `.gresource.xml` → `.gresource` in `build.rs` | `[build-dependencies]` |
| `gio` | `0.20` | Provides `resources_register_include!` macro | `[dependencies]` (already present) |

---

## 8. Risks and Mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| `glib-compile-resources` not installed on build host | Low (required by gtk4-rs dev environment) | `glib-build-tools` 0.20 can fall back to a pure-Rust compiler when the system tool is unavailable |
| Icon not found in resource bundle (path mismatch) | Low | The `alias` attribute explicitly maps to the expected icon theme path |
| Meson build broken | None | Meson only calls `cargo build`; `build.rs` is transparent to Meson |
| Flatpak sandbox breakage | None | GLib resources are in the binary; no host filesystem access needed for the icon |
| `add_resource_path` API not available in gtk4-rs 0.9 | None | `IconTheme::add_resource_path` has been stable in GTK4 since GTK 4.0; gtk4-rs 0.9 wraps GTK 4.12+ |

---

## 9. Verification Steps

After implementation, confirm correct behavior by:

1. `cargo build` — must compile without errors or warnings
2. `cargo clippy -- -D warnings` — must produce no warnings
3. `cargo fmt --check` — must pass
4. `cargo test` — must pass
5. Run `./target/debug/up` from the project root — window icon should be the current icon PNG
6. Run `./target/debug/up` from a different directory — window icon should still appear (no file-system dependency)
7. If an old installed version exists: rename/delete the system icon cache entry and confirm the app still shows the correct icon

---

## 10. References

1. [GLib Resource Bundles — GNOME developer docs](https://docs.gtk.org/gio/struct.Resource.html)  
2. [gtk4-rs Book — Resources](https://gtk-rs.org/gtk4-rs/stable/latest/book/resources.html)  
3. [glib-build-tools crate — docs.rs](https://docs.rs/glib-build-tools/latest/glib_build_tools/)  
4. [gio::resources_register_include! macro — docs.rs](https://docs.rs/gio/latest/gio/macro.resources_register_include.html)  
5. [GtkIconTheme::add_resource_path — GTK4 docs](https://docs.gtk.org/gtk4/method.IconTheme.add_resource_path.html)  
6. [GtkIconTheme lookup order — GTK source](https://gitlab.gnome.org/GNOME/gtk/-/blob/main/gtk/gtkicontheme.c)  
7. [gresource XML format — GLib docs](https://docs.gtk.org/gio/class.Resource.html#gresource-file-format)  
8. [gtk4-rs ResourceBundle example — GitHub](https://github.com/gtk-rs/gtk4-rs/tree/main/examples/resource_bundle)
