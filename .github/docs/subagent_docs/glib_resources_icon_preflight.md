# GLib Resources Icon — Preflight Validation Report

**Date:** 2026-03-19  
**Validator:** Preflight Subagent (static analysis, Windows host — no cargo/bash execution)  
**Project:** Up — GTK4/libadwaita Linux desktop application  
**Workspace:** `c:\Projects\Up`

---

## 1. File Existence Checks

| File | Test-Path Result | Status |
|------|-----------------|--------|
| `data/icons/hicolor/256x256/apps/io.github.up.png` | `True` | ✔ PASS |
| `build.rs` | `True` | ✔ PASS |
| `data/io.github.up.gresource.xml` | `True` | ✔ PASS |

---

## 2. `scripts/preflight.sh` — What It Checks

The script is a `bash` script with `set -euo pipefail`. It executes six steps in order:

| Step | Command | Purpose |
|------|---------|---------|
| 1 | `cargo fmt --check` | Formatting — fails if any source file is not `rustfmt`-clean |
| 2 | `cargo clippy -- -D warnings` | Linting — fails on any Clippy warning |
| 3 | `cargo build` | Build verification — full debug build |
| 4 | `cargo test` | Test execution — all unit and integration tests |
| 5 | `desktop-file-validate data/io.github.up.desktop` | Desktop entry validation (skipped if tool absent) |
| 6 | `appstreamcli validate data/io.github.up.metainfo.xml` | AppStream metainfo validation (skipped if tool absent) |

**Static assessment:** Script is well-formed. Shebang `#!/usr/bin/env bash` is correct. All commands are standard. Optional tools are gated with `command -v` guards. No issues found.

---

## 3. `build.rs` — Rust Syntax Check

```rust
fn main() {
    glib_build_tools::compile_resources(
        &["data"],
        "data/io.github.up.gresource.xml",
        "compiled.gresource",
    );
}
```

**Assessment:** Syntactically valid Rust. Single `main()` function, one call to `glib_build_tools::compile_resources`. Arguments are:
- `source_dirs`: `["data"]` — glib-build-tools will resolve file entries relative to `{project_root}/data/`
- `gresource_xml`: `"data/io.github.up.gresource.xml"` — correct path from project root
- `output`: `"compiled.gresource"` — written to `OUT_DIR` by cargo, embedded by `gio::resources_register_include!` in `main.rs`

**Status:** ✔ PASS

---

## 4. `data/io.github.up.gresource.xml` — XML and Resource Path Validation

```xml
<?xml version="1.0" encoding="UTF-8"?>
<gresources>
  <gresource prefix="/io/github/up">
    <file alias="hicolor/256x256/apps/io.github.up.png">icons/hicolor/256x256/apps/io.github.up.png</file>
  </gresource>
</gresources>
```

**XML validity:** Well-formed. Standard declaration, single `<gresources>` root, one `<gresource>` with one `<file>` element.

**Resource path resolution:**

| Field | Value |
|-------|-------|
| XML file content (source path) | `icons/hicolor/256x256/apps/io.github.up.png` |
| `source_dirs` from `build.rs` | `["data"]` |
| Physical file glib-build-tools resolves | `{project_root}/data/icons/hicolor/256x256/apps/io.github.up.png` |
| File exists at that path | ✔ `True` (confirmed by Test-Path) |
| `alias` attribute | `hicolor/256x256/apps/io.github.up.png` |
| GLib resource prefix | `/io/github/up` |
| **Final embedded resource path** | **`/io/github/up/hicolor/256x256/apps/io.github.up.png`** |

**Status:** ✔ PASS — icon file exists and resource path is correctly formed.

---

## 5. `Cargo.toml` — TOML and Dependency Check

**Validity:** Valid TOML. All required sections present (`[package]`, `[dependencies]`, `[build-dependencies]`, `[profile.release]`).

**glib-build-tools check:**

```toml
[build-dependencies]
glib-build-tools = "0.20"
```

`glib-build-tools = "0.20"` is present in `[build-dependencies]`. Version `0.20` is consistent with the `glib = "0.20"` and `gio = "0.20"` runtime dependencies.

**Status:** ✔ PASS

---

## 6. `src/main.rs` — Rust Syntax Check

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
        .expect("Failed to register resources.");
    env_logger::init();
    let app = UpApplication::new();
    app.run();
}
```

**Assessment:** Syntactically valid Rust. All module declarations present. `gio::resources_register_include!("compiled.gresource")` is the correct macro for embedding the compiled GLib resource bundle at build time and registering it at runtime. The `APP_ID` constant matches the application ID used in `app.rs`.

**Status:** ✔ PASS

---

## 7. `src/app.rs` — Rust Syntax Check

**Assessment:** Syntactically valid Rust. Key observations:

- `adw::prelude::*` correctly imported for libadwaita builder patterns.
- `on_activate` registers `/io/github/up` as an icon theme resource path via `gtk::IconTheme::for_display(&display).add_resource_path("/io/github/up")`.
- `gtk::Window::set_default_icon_name("io.github.up")` sets the window icon.

**Cross-check against gresource.xml:**

The icon theme will search `{resource_path}/hicolor/256x256/apps/{icon_name}.png`:
- Resource path registered: `/io/github/up`
- Icon name: `io.github.up`
- GTK resolves to: `/io/github/up/hicolor/256x256/apps/io.github.up.png`
- This matches the embedded resource path from Section 4 exactly. ✔

**Status:** ✔ PASS

---

## Summary

| Check | Result |
|-------|--------|
| `data/icons/hicolor/256x256/apps/io.github.up.png` exists | ✔ PASS |
| `build.rs` exists | ✔ PASS |
| `data/io.github.up.gresource.xml` exists | ✔ PASS |
| `scripts/preflight.sh` — structure and commands valid | ✔ PASS |
| `build.rs` — Rust syntax valid | ✔ PASS |
| `gresource.xml` — XML valid | ✔ PASS |
| `gresource.xml` — referenced PNG file exists on disk | ✔ PASS |
| `gresource.xml` — resource path correctly formed with alias | ✔ PASS |
| `Cargo.toml` — TOML valid | ✔ PASS |
| `Cargo.toml` — `glib-build-tools = "0.20"` present | ✔ PASS |
| `src/main.rs` — Rust syntax valid | ✔ PASS |
| `src/app.rs` — Rust syntax valid | ✔ PASS |
| Icon resource path wired end-to-end (`gresource.xml` → `build.rs` → `main.rs` → `app.rs`) | ✔ PASS |

---

## Verdict

**PASS**

All static validations passed. No syntax issues, no missing files, and the GLib resource icon pipeline is correctly wired end-to-end from the physical PNG file through `gresource.xml`, `build.rs`, `main.rs`, and `app.rs`.
