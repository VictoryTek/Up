# Localization Specification — `gettext-rs` + `po/` Directory

**Feature:** Internationalization (i18n) / Localization (l10n) infrastructure  
**Project:** Up — GTK4/libadwaita Linux desktop application  
**Status:** Specification (Phase 1)  
**Date:** 2026-05-08

---

## Sources Consulted

1. GNU gettext manual — https://www.gnu.org/software/gettext/manual/
2. `gettext-rs` crate usage — confirmed from Fractal GNOME app Cargo.toml (`gettext-rs = { version = "0.7", features = ["gettext-system"] }`)
3. Fractal (official GNOME Rust app) `src/main.rs` — https://gitlab.gnome.org/World/fractal/-/raw/main/src/main.rs
4. Fractal `po/meson.build` — https://gitlab.gnome.org/World/fractal/-/raw/main/po/meson.build
5. Fractal `po/POTFILES.in` — https://gitlab.gnome.org/World/fractal/-/raw/main/po/POTFILES.in
6. Paper Plane (GTK4 Rust app) `src/main.rs` — gettextrs initialization pattern
7. Meson i18n module documentation — https://mesonbuild.com/i18n-module.html
8. Meson Localisation guide — https://mesonbuild.com/Localisation.html
9. GNU gettext Rust page — https://www.gnu.org/software/gettext/manual/html_node/Rust.html

---

## 1. Current State Analysis

### 1.1 No Translation Infrastructure Exists

- Zero calls to any translation function (`gettext`, `_()`, `tr!()`) across all source files
- No `po/` directory
- No `LINGUAS` or `POTFILES.in`
- `meson.build` has no `i18n = import('i18n')` and no `subdir('po')`
- `.desktop` file uses plain `Name=` / `Comment=` (not translatable `_Name=` / `_Comment=`)
- `Cargo.toml` has no `gettext-rs` dependency

### 1.2 Translatable String Count Per File

| File | Unique Translatable Strings |
|------|-----------------------------|
| `src/ui/window.rs` | 43 |
| `src/ui/update_row.rs` | 18 |
| `src/ui/upgrade_page.rs` | 22 |
| `src/ui/log_panel.rs` | 5 |
| `src/ui/history_page.rs` | 6 |
| `src/ui/reboot_dialog.rs` | 7 |
| `data/io.github.up.desktop` | 2 (`Name`, `Comment`) |
| **Total** | **~103** |

---

## 2. Feature Definition

### 2A. Rust Side

#### 2A.1 — Cargo Dependency

Add to `Cargo.toml` `[dependencies]`:

```toml
gettext-rs = { version = "0.7", features = ["gettext-system"] }
```

- Crate name in Cargo.toml: `gettext-rs` (hyphenated)
- Rust module name: `gettextrs` (no hyphen)
- Feature `gettext-system`: links against the system-installed `libintl` (GNU gettext library), which is the correct choice for a Linux desktop app distributed via system packages or Flatpak. Do NOT use the bundled version; Flatpak provides the system gettext runtime.
- Version `0.7` is confirmed current and used by official GNOME Rust applications (Fractal, Paper Plane)

#### 2A.2 — Initialization in `src/main.rs`

Initialize gettext at the very beginning of `main()`, **before** GTK/adw initialization. GTK4 calls `setlocale()` internally during `gtk::init()` (which happens inside `adw::Application::new()` → `app.run()`). To guarantee locale is set before any string lookup, the gettextrs calls must precede app construction.

```rust
use gettextrs::{bindtextdomain, setlocale, textdomain, LocaleCategory};

fn main() -> gtk::glib::ExitCode {
    // i18n — must be before GTK/adw initialization
    setlocale(LocaleCategory::LcAll, "");
    let localedir = option_env!("LOCALEDIR").unwrap_or("/usr/share/locale");
    bindtextdomain(APP_ID, localedir).expect("Unable to bind the text domain");
    textdomain(APP_ID).expect("Unable to switch to the text domain");

    // GTK/resource initialization (unchanged)
    gio::resources_register_include!("compiled.gresource").expect("Failed to register resources.");
    env_logger::init();
    let app = UpApplication::new();
    app.run()
}
```

**Notes:**
- `option_env!("LOCALEDIR")` reads the env var at compile time. Meson sets it when building via `meson compile`. A plain `cargo build` falls back to `/usr/share/locale`.
- `APP_ID = "io.github.up"` is already defined in `main.rs` and doubles as the gettext domain name — no separate constant needed.
- **Do NOT call `bind_textdomain_codeset()`**. Rust strings are always UTF-8; this function is a C-ism. Fractal, Paper Plane, and other GNOME Rust apps omit it.
- Required `use` imports: `gettextrs::{bindtextdomain, setlocale, textdomain, LocaleCategory}`

#### 2A.3 — Per-File String Wrapping Strategy

In each UI file that contains translatable strings, add imports at the top of the file:

```rust
use gettextrs::gettext;
// Only in files with plural strings:
use gettextrs::ngettext;
```

Wrap each user-visible string literal:
- **Simple strings**: `gettext("Some text")` — returns `String`
- **Plural strings**: `ngettext("1 item", "{} items", count as u64).replace("{}", &count.to_string())`
- **Format strings** (string contains runtime values): `format!(gettext("Battery is at {}%..."), value)`

#### 2A.4 — Handling Plural Forms (ngettext)

Several strings in the codebase use Rust's manual English-only plural trick:

```rust
// BEFORE (English-only):
format!("{non_skipped_total} update{} available",
    if non_skipped_total == 1 { "" } else { "s" })

// AFTER (translatable):
ngettext(
    "{} update available",
    "{} updates available",
    non_skipped_total as u64,
).replace("{}", &non_skipped_total.to_string())
```

All occurrences of this pattern must be converted.

---

### 2B. `po/` Directory Structure

Create the following files at the repository root:

```
po/
├── POTFILES.in       # List of source files xgettext should scan
├── LINGUAS           # Space-separated list of supported languages (empty initially)
└── meson.build       # Calls i18n.gettext()
```

**`po/POTFILES.in`** — relative paths from source root:

```
data/io.github.up.desktop.in
src/main.rs
src/app.rs
src/ui/window.rs
src/ui/update_row.rs
src/ui/upgrade_page.rs
src/ui/log_panel.rs
src/ui/history_page.rs
src/ui/reboot_dialog.rs
```

**`po/LINGUAS`** — initially empty (no translators yet):

```
# Add language codes here when translations are contributed.
# Example: de fr es it pt_BR
```

**`po/meson.build`**:

```meson
i18n.gettext(
  'io.github.up',
  args: [
    '--language=Rust',
    '--keyword=gettext:1',
    '--keyword=ngettext:1,2',
    '--from-code=UTF-8',
  ],
  preset: 'glib',
)
```

**Notes:**
- `--language=Rust` is required for xgettext ≥ 0.21 to correctly parse Rust source files. Without it, xgettext may not recognize `.rs` files.
- `--keyword=gettext:1` tells xgettext to extract the first argument of `gettext()` calls. The `glib` preset includes `N_`, `_`, `Q_`, but NOT plain `gettext`.
- `--keyword=ngettext:1,2` extracts both singular and plural forms.
- `preset: 'glib'` adds standard GNOME keywords and sets `--from-code=UTF-8`.
- When LINGUAS is empty, no `.po` / `.mo` files are compiled; this is correct for a project starting i18n.

---

### 2C. Meson Integration

#### 2C.1 — Changes to `meson.build`

**Add `i18n` import** (alongside existing `gnome` and `fs` imports):

```meson
i18n = import('i18n')
```

**Set `localedir`** (used both to pass to cargo build and for `.mo` install path):

```meson
localedir = join_paths(prefix, get_option('localedir'))
```

**Pass `LOCALEDIR` to the Cargo build** so `option_env!("LOCALEDIR")` resolves at compile time:

Update the `cargo_build` command string to include the env var:

```meson
cargo_build = custom_target('cargo-build',
  output: 'up',
  command: [
    'sh', '-c',
    'LOCALEDIR=' + localedir + ' ' +
    cargo.full_path() + ' build ' +
    (rust_target == 'release' ? '--release ' : '') +
    '--manifest-path ' + srcdir / 'Cargo.toml' +
    ' --target-dir ' + meson.build_root() / 'cargo-target' +
    ' && cp ' + meson.build_root() / 'cargo-target' / rust_target / 'up' + ' @OUTPUT@'
  ],
  depend_files: files('Cargo.toml', 'Cargo.lock'),
  console: true,
  install: true,
  install_dir: bindir,
)
```

**Add `po/` subdir** (after the `install_data` calls, before `gnome.post_install`):

```meson
subdir('po')
```

**Replace the plain `install_data` for the desktop file** with `i18n.merge_file()`:

```meson
# Remove this:
# install_data('data/io.github.up.desktop',
#   install_dir: join_paths(datadir, 'applications'),
# )

# Replace with:
desktop_file = i18n.merge_file(
  input: 'data/io.github.up.desktop.in',
  output: 'io.github.up.desktop',
  type: 'desktop',
  po_dir: 'po',
  install: true,
  install_dir: join_paths(datadir, 'applications'),
)
```

#### 2C.2 — Complete Updated `meson.build`

```meson
project('up',
  version: run_command(
    'grep', '-m', '1', '^version', 'Cargo.toml',
    check: true,
  ).stdout().strip().split('"')[1],
  license: 'GPL-3.0-or-later',
)

cargo = find_program('cargo')
fs = import('fs')
gnome = import('gnome')
i18n = import('i18n')

builddir = meson.current_build_dir()
srcdir = meson.current_source_dir()

prefix = get_option('prefix')
bindir = join_paths(prefix, get_option('bindir'))
datadir = join_paths(prefix, get_option('datadir'))
localedir = join_paths(prefix, get_option('localedir'))

if get_option('buildtype') == 'release'
  rust_target = 'release'
else
  rust_target = 'debug'
endif

cargo_build = custom_target('cargo-build',
  output: 'up',
  command: [
    'sh', '-c',
    'LOCALEDIR=' + localedir + ' ' +
    cargo.full_path() + ' build ' +
    (rust_target == 'release' ? '--release ' : '') +
    '--manifest-path ' + srcdir / 'Cargo.toml' +
    ' --target-dir ' + meson.build_root() / 'cargo-target' +
    ' && cp ' + meson.build_root() / 'cargo-target' / rust_target / 'up' + ' @OUTPUT@'
  ],
  depend_files: files('Cargo.toml', 'Cargo.lock'),
  console: true,
  install: true,
  install_dir: bindir,
)

desktop_file = i18n.merge_file(
  input: 'data/io.github.up.desktop.in',
  output: 'io.github.up.desktop',
  type: 'desktop',
  po_dir: 'po',
  install: true,
  install_dir: join_paths(datadir, 'applications'),
)

install_data('data/io.github.up.metainfo.xml',
  install_dir: join_paths(datadir, 'metainfo'),
)

install_data('data/io.github.up.policy',
  install_dir: join_paths(datadir, 'polkit-1', 'actions'),
)

foreach size : ['256x256', '128x128', '48x48']
  png = 'data/icons/hicolor' / size / 'apps/io.github.up.png'
  if fs.exists(png)
    install_data(png,
      install_dir: join_paths(datadir, 'icons', 'hicolor', size, 'apps'),
    )
  endif
endforeach

subdir('po')

gnome.post_install(
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
```

---

### 2D. Desktop File & Metainfo

#### 2D.1 — Desktop File

Rename `data/io.github.up.desktop` → `data/io.github.up.desktop.in`

Change translatable keys to use the `_` prefix convention:

```ini
[Desktop Entry]
_Name=Up
_Comment=Update and upgrade your Linux system
Exec=up
Icon=io.github.up
Terminal=false
Type=Application
Categories=System;PackageManager;
Keywords=update;upgrade;system;flatpak;brew;nix;
StartupNotify=true
```

The `i18n.merge_file()` call in Meson processes `_Name=` and `_Comment=` prefixes to generate proper translated desktop entries.

**Note:** `Keywords=` is not commonly translated; omit `_Keywords=` unless you plan to maintain keyword translations. Only `Name` and `Comment` are required.

#### 2D.2 — Metainfo XML

The `data/io.github.up.metainfo.xml` file is currently not processed through the `i18n` module and does not need changes for the initial translation infrastructure. Translations for the `<name>`, `<summary>`, and `<description>` fields in metainfo are typically contributed by translators using Weblate/Transifex and result in `xml:lang="..."` attributes added to the file over time. For now, leave `io.github.up.metainfo.xml` as a plain `install_data` target.

If translation of the metainfo XML is desired in future, the pattern is:
```meson
# Future: i18n.itstool_join() for metainfo XML
```

---

## 3. Implementation Steps (Ordered, File-by-File)

### Step 1 — Add `gettext-rs` to `Cargo.toml`

File: `Cargo.toml`

In `[dependencies]`, add after the existing entries (keep sorted):
```toml
gettext-rs = { version = "0.7", features = ["gettext-system"] }
```

### Step 2 — Create `po/` Directory Structure

Create these new files:

**`po/POTFILES.in`**:
```
data/io.github.up.desktop.in
src/main.rs
src/app.rs
src/ui/window.rs
src/ui/update_row.rs
src/ui/upgrade_page.rs
src/ui/log_panel.rs
src/ui/history_page.rs
src/ui/reboot_dialog.rs
```

**`po/LINGUAS`**:
```
# Add language codes here when translations are contributed.
# Example: de fr es it pt_BR
```

**`po/meson.build`**:
```meson
i18n.gettext(
  'io.github.up',
  args: [
    '--language=Rust',
    '--keyword=gettext:1',
    '--keyword=ngettext:1,2',
    '--from-code=UTF-8',
  ],
  preset: 'glib',
)
```

### Step 3 — Initialize gettext in `src/main.rs`

Add import at the top of the file:
```rust
use gettextrs::{bindtextdomain, setlocale, textdomain, LocaleCategory};
```

Update `fn main()`:
```rust
fn main() -> gtk::glib::ExitCode {
    // i18n — initialize before GTK
    setlocale(LocaleCategory::LcAll, "");
    let localedir = option_env!("LOCALEDIR").unwrap_or("/usr/share/locale");
    bindtextdomain(APP_ID, localedir).expect("Unable to bind the text domain");
    textdomain(APP_ID).expect("Unable to switch to the text domain");

    gio::resources_register_include!("compiled.gresource").expect("Failed to register resources.");
    env_logger::init();
    let app = UpApplication::new();
    app.run()
}
```

### Step 4 — Wrap Strings in Each UI File

See Section 5 (String Catalog) for the complete list of strings to wrap. For each file:

**`src/ui/window.rs`** — Add at top of file:
```rust
use gettextrs::{gettext, ngettext};
```

**`src/ui/update_row.rs`** — Add at top of file:
```rust
use gettextrs::{gettext, ngettext};
```

**`src/ui/upgrade_page.rs`** — Add at top of file:
```rust
use gettextrs::gettext;
```

**`src/ui/log_panel.rs`** — Add at top of file:
```rust
use gettextrs::gettext;
```

**`src/ui/history_page.rs`** — Add at top of file:
```rust
use gettextrs::gettext;
```

**`src/ui/reboot_dialog.rs`** — Add at top of file:
```rust
use gettextrs::gettext;
```

### Step 5 — Update `meson.build`

Apply all changes as described in Section 2C.2.

### Step 6 — Update Desktop File

Rename `data/io.github.up.desktop` to `data/io.github.up.desktop.in` and update as described in Section 2D.1.

---

## 4. New Dependencies

| Dependency | Version | Cargo.toml key | Notes |
|------------|---------|----------------|-------|
| `gettext-rs` | `"0.7"` | `gettext-rs = { version = "0.7", features = ["gettext-system"] }` | Main crate. `gettextrs-sys` is automatically pulled as a transitive dep. |

**System requirements:** The build host needs `gettext` package installed (`xgettext`, `msgfmt`, `msginit`). The runtime host needs `libintl` (part of GNU gettext, present on all mainstream Linux distros). In Flatpak, gettext is part of the GNOME SDK.

**xgettext version requirement:** `--language=Rust` requires GNU gettext ≥ 0.21. Most modern Linux distros (Ubuntu 22.04+, Fedora 37+, Arch) ship gettext ≥ 0.21. If xgettext < 0.21 is detected at build time, Meson will fail gracefully since `--language=Rust` will be an unrecognized flag.

**Nix flake:** No changes needed. The `flake.nix` uses nixpkgs, which provides `gettext`. If the flake specifies `buildInputs`, `gettext` should be added. However, since the existing flake was not read as part of this spec (no current `buildInputs` list was examined), the implementor should verify this and add `gettext` to `buildInputs` / `nativeBuildInputs` if not already present.

---

## 5. String Catalog

Complete list of all user-visible strings requiring `gettext()` or `ngettext()` wrapping.

### 5.1 `src/ui/window.rs`

| Approx. Line | String | Wrapping | Notes |
|---|---|---|---|
| ~50 | `"Update"` | `gettext("Update")` | ViewStack tab title |
| ~57 | `"Upgrade"` | `gettext("Upgrade")` | ViewStack tab title |
| ~64 | `"History"` | `gettext("History")` | ViewStack tab title |
| ~135 | `"Check for updates"` | `gettext("Check for updates")` | `tooltip_text` |
| ~136 | `"Refresh update list"` | `gettext("Refresh update list")` | accessible label |
| ~148 | `"Run Maintenance"` | `gettext("Run Maintenance")` | menu item |
| ~149 | `"About Up"` | `gettext("About Up")` | menu item |
| ~153 | `"Main menu"` | `gettext("Main menu")` | `tooltip_text` |
| ~155 | `"Application menu"` | `gettext("Application menu")` | accessible label |
| ~172 | `"A system updater for Linux"` | `gettext("A system updater for Linux")` | about dialog comments |
| ~215 | `"Running maintenance…"` | `gettext("Running maintenance\u{2026}")` | status label |
| ~241 | `"No maintenance actions available."` | `gettext("No maintenance actions available.")` | status label |
| ~258 | `"Authenticating…"` | `gettext("Authenticating\u{2026}")` | status label |
| ~259 | `"Requesting administrator privileges…"` | `gettext("Requesting administrator privileges\u{2026}")` | log line |
| ~263 | `"Authentication successful."` | `gettext("Authentication successful.")` | log line |
| ~268 | `"Authentication failed: {e}"` | `format!(gettext("Authentication failed: {}"), e)` | log format |
| ~271 | `"Maintenance cancelled."` | `gettext("Maintenance cancelled.")` | status label |
| ~305 | `"Maintenance completed with errors."` | `gettext("Maintenance completed with errors.")` | status label |
| ~307 | `"Maintenance complete."` | `gettext("Maintenance complete.")` | status label |
| ~340 | `"Detect available updates across your system."` | `gettext("Detect available updates across your system.")` | status label |
| ~347 | `"System Information"` | `gettext("System Information")` | group title |
| ~350 | `"Distribution"` | `gettext("Distribution")` | row title |
| ~356 | `"Current Version"` | `gettext("Current Version")` | row title |
| ~362 | `"Sources"` | `gettext("Sources")` | group title |
| ~363 | `"Package managers detected on this system"` | `gettext("Package managers detected on this system")` | group description |
| ~371 | `"Detecting package managers…"` | `gettext("Detecting package managers\u{2026}")` | placeholder row title |
| ~394 | `"Up was updated — restart to apply changes"` | `gettext("Up was updated \u{2014} restart to apply changes")` | banner title |
| ~395 | `"Close Up"` | `gettext("Close Up")` | banner button label |
| ~404 | `"Update All"` | `gettext("Update All")` | button label |
| ~425 | `"Cancel"` | `gettext("Cancel")` | cancel button label |
| ~427 | `"Cancel update"` | `gettext("Cancel update")` | accessible label |
| ~470 | `"Metered Connection"` | `gettext("Metered Connection")` | dialog heading |
| ~471 | `"You are on a metered connection. Downloading updates may use significant data.\n\nContinue anyway?"` | `gettext("You are on a metered connection. Downloading updates may use significant data.\n\nContinue anyway?")` | dialog body |
| ~475 | `"Cancel"` | (same as above) | dialog response |
| ~476 | `"Update Anyway"` | `gettext("Update Anyway")` | dialog response |
| ~516 | `"Low Battery"` | `gettext("Low Battery")` | dialog heading |
| ~517 | `"Battery is at {}% and discharging. Updates may be interrupted if the device shuts down. Continue anyway?"` | `format!(gettext("Battery is at {}% and discharging. Updates may be interrupted if the device shuts down. Continue anyway?"), bat.capacity)` | dialog body (format string) |
| ~542 | `"Create Snapshot?"` | `gettext("Create Snapshot?")` | dialog heading |
| ~543 | `"Create a system snapshot before updating. This allows you to roll back if something goes wrong."` | `gettext("Create a system snapshot before updating. This allows you to roll back if something goes wrong.")` | dialog body |
| ~547 | `"Skip"` | `gettext("Skip")` | dialog response |
| ~548 | `"Create Snapshot"` | `gettext("Create Snapshot")` | dialog response |
| ~562 | `"Creating pre-update snapshot…"` | `gettext("Creating pre-update snapshot\u{2026}")` | log line |
| ~622 | `"Skipped by user"` | `gettext("Skipped by user")` | status label |
| ~734 | `"Updating…"` | `gettext("Updating\u{2026}")` | status label |
| ~746 | `"Update cancelled."` | `gettext("Update cancelled.")` | status label |
| ~831 | `"Update completed with errors."` | `gettext("Update completed with errors.")` | status label |
| ~832 | `"Update complete."` | `gettext("Update complete.")` | status label |
| ~860 | `"On a metered connection. Consider updating later."` | `gettext("On a metered connection. Consider updating later.")` | banner |
| ~967 | `"Checking for updates..."` | `gettext("Checking for updates...")` | status label |
| ~993 | `"{n} update{s} available"` | `ngettext("{} update available", "{} updates available", n as u64).replace("{}", &n.to_string())` | **plural form — requires ngettext** |
| ~998 | `"Everything is up to date."` | `gettext("Everything is up to date.")` | status label |

**Note on duplicates:** Strings like `"Authenticating…"`, `"Requesting administrator privileges…"`, `"Authentication successful."`, `"Authentication failed: {e}"` appear in BOTH the `maintenance_action` closure and the `update_button` closure. Both must be wrapped consistently; gettext will merge duplicates in the `.pot` file automatically.

### 5.2 `src/ui/update_row.rs`

| Approx. Line | String | Wrapping | Notes |
|---|---|---|---|
| ~37 | `"Ready"` | `gettext("Ready")` | status_label initial |
| ~54 | `"Skip {} during Update All"` | `format!(gettext("Skip {} during Update All"), backend.display_name())` | skip checkbox tooltip/label |
| ~71 | `"Retry"` | `gettext("Retry")` | retry button tooltip |
| ~108 | `"View Changelog"` | `gettext("View Changelog")` | changelog row title |
| ~117 | `"View Changelog"` | `gettext("View Changelog")` | accessible label for changelog button |
| ~82 | `"Skipped"` | `gettext("Skipped")` | status label |
| ~91 | `"Up to date"` | `gettext("Up to date")` | status label (skip un-toggled, 0 count) |
| ~95 | `"{count} available"` | `ngettext("{} available", "{} available", count as u64).replace("{}", &count.to_string())` | plural — note singular/plural may be same in English, but other languages differ |
| ~168 | `"Package Info"` | `gettext("Package Info")` | changelog dialog heading for Pacman/Zypper |
| ~168 | `"Changelog"` | `gettext("Changelog")` | changelog dialog heading for other backends |
| ~196 | `"Close"` | `gettext("Close")` | dialog response |
| ~249 | `"… and {remaining} more"` | `ngettext("\u{2026} and {} more", "\u{2026} and {} more", remaining as u64).replace("{}", &remaining.to_string())` | truncation row |
| ~280 | `"Checking..."` | `gettext("Checking...")` | `set_status_checking()` |
| ~292 | `"Up to date"` | `gettext("Up to date")` | `set_status_available()` 0 count |
| ~296 | `"{count} available"` | `ngettext("{} available", "{} available", count as u64).replace("{}", &count.to_string())` | `set_status_available()` |
| ~305 | `"Updating..."` | `gettext("Updating...")` | `set_status_running()` |
| ~315 | `"Up to date"` | `gettext("Up to date")` | `set_status_success()` 0 count |
| ~316 | `"{count} updated"` | `ngettext("{} updated", "{} updated", count as u64).replace("{}", &count.to_string())` | `set_status_success()` |
| ~325 | `"Error: {}"` | `format!("{} {}", gettext("Error:"), msg)` | `set_status_error()` |
| ~334 | `"Cancelled"` | `gettext("Cancelled")` | `set_status_cancelled()` |
| ~356 | `"Cleaning…"` | `gettext("Cleaning\u{2026}")` | `set_status_cleaning()` |
| ~364 | `"Already clean"` | `gettext("Already clean")` | `set_status_cleaned()` 0 removed |
| ~365 | `"{removed} removed"` | `ngettext("{} removed", "{} removed", removed as u64).replace("{}", &removed.to_string())` | `set_status_cleaned()` |

### 5.3 `src/ui/upgrade_page.rs`

| Approx. Line | String | Wrapping | Notes |
|---|---|---|---|
| ~41 | `"Upgrade your distribution to the next major version."` | `gettext("Upgrade your distribution to the next major version.")` | header label |
| ~49 | `"Upgrade Status"` | `gettext("Upgrade Status")` | info group title |
| ~55 | `"Upgrade Available"` | `gettext("Upgrade Available")` | upgrade row title |
| ~56 | `"Loading…"` | `gettext("Loading\u{2026}")` | upgrade row subtitle |
| ~62 | `"Prerequisites"` | `gettext("Prerequisites")` | prereq group title |
| ~63 | `"These checks must pass before upgrading"` | `gettext("These checks must pass before upgrading")` | prereq group description |
| ~68 | `"All packages up to date"` | `gettext("All packages up to date")` | check row label |
| ~69 | `"Sufficient disk space (10 GB+)"` | `gettext("Sufficient disk space (10 GB+)")` | check row label |
| ~70 | `"Backup recommended"` | `gettext("Backup recommended")` | check row label |
| ~74 | `"Checking..."` | `gettext("Checking...")` | row subtitle |
| ~100 | `"Run Checks"` | `gettext("Run Checks")` | check button label |
| ~105 | `"Start Upgrade"` | `gettext("Start Upgrade")` | upgrade button label |
| ~121 | `"I have backed up my important data"` | `gettext("I have backed up my important data")` | backup checkbox label |
| ~339 | `"NixOS Config Type"` | `gettext("NixOS Config Type")` | config row title |
| ~350 | `"nixos-rebuild available"` | `gettext("nixos-rebuild available")` | first check row title (NixOS) |
| ~307 | `"Not supported for this distribution yet"` | `gettext("Not supported for this distribution yet")` | row subtitle |
| ~372 | `"Could not determine upgrade availability"` | `gettext("Could not determine upgrade availability")` | row subtitle |
| ~259 | `"Upgrade via Flake"` | `gettext("Upgrade via Flake")` | dialog heading |
| ~260-272 | Flake dialog body (multi-line) | `gettext("NixOS {next_ver} may be available, but this system uses Nix Flakes.\n\n...")` | **complex format string** — use `format!()` with `gettext()` |
| ~282 | `"Close"` | `gettext("Close")` | dialog response |
| ~290 | `"Confirm System Upgrade"` | `gettext("Confirm System Upgrade")` | dialog heading |
| ~291-295 | `"This will upgrade {} from version {} to the next major release.\n\n..."` | `format!(gettext("This will upgrade {} from version {} to the next major release.\n\nThis operation may take a long time and require a reboot.\n\nAre you sure you want to continue?"), distro.name, distro.version)` | dialog body (format) |
| ~297 | `"Cancel"` | `gettext("Cancel")` | dialog response |
| ~298 | `"Upgrade"` | `gettext("Upgrade")` | dialog response |
| ~410 | `"Flake-managed system: upgrade via your flake.nix"` | `gettext("Flake-managed system: upgrade via your flake.nix")` | banner title |

**Note on the Flake dialog body:** The multi-line `format!()` string in the Flake dialog body (lines ~260-272) contains embedded newlines and bullet characters (`\u{2022}`). The `gettext()` call should wrap the entire format string template, not individual pieces. The translator must preserve `{}` placeholders.

### 5.4 `src/ui/log_panel.rs`

| Approx. Line | String | Wrapping | Notes |
|---|---|---|---|
| ~50 | `"Save log to file"` | `gettext("Save log to file")` | tooltip_text |
| ~54 | `"Save log to file"` | `gettext("Save log to file")` | accessible label |
| ~59 | `"Terminal Output"` | `gettext("Terminal Output")` | header label |
| ~96 | `"Log saved to ~/{filename}"` | `format!(gettext("Log saved to ~/{}"), filename)` | toast message |
| ~97 | `"Failed to save log: {e}"` | `format!(gettext("Failed to save log: {}"), e)` | toast message |

### 5.5 `src/ui/history_page.rs`

| Approx. Line | String | Wrapping | Notes |
|---|---|---|---|
| ~35 | `"A record of past update sessions."` | `gettext("A record of past update sessions.")` | header label |
| ~40 | `"Update History"` | `gettext("Update History")` | group title |
| ~44 | `"Clear"` | `gettext("Clear")` | clear button label |
| ~46 | `"Clear update history"` | `gettext("Clear update history")` | accessible label |
| ~91 | `"No history yet"` | `gettext("No history yet")` | empty state row title |
| ~92 | `"Update sessions will appear here after you run an update."` | `gettext("Update sessions will appear here after you run an update.")` | empty state row subtitle |

### 5.6 `src/ui/reboot_dialog.rs`

| Approx. Line | String | Wrapping | Notes |
|---|---|---|---|
| ~10 | `"Reboot Required"` | `gettext("Reboot Required")` | dialog heading |
| ~11-13 | `"A reboot is recommended to complete the update. Would you like to reboot now?"` | `gettext("A reboot is recommended to complete the update. Would you like to reboot now?")` | dialog body |
| ~17 | `"Later"` | `gettext("Later")` | response label |
| ~18 | `"Reboot Now"` | `gettext("Reboot Now")` | response label |
| ~42 | `"Reboot Failed"` | `gettext("Reboot Failed")` | error dialog heading |
| ~43-46 | `"The system could not be rebooted.\n\n{err_msg}\n\nPlease reboot manually using your system settings or terminal."` | `format!(gettext("The system could not be rebooted.\n\n{}\n\nPlease reboot manually using your system settings or terminal."), err_msg)` | error dialog body |
| ~48 | `"Close"` | `gettext("Close")` | response label |

### 5.7 `data/io.github.up.desktop.in`

| Key | String | Action |
|-----|--------|--------|
| `_Name=` | `Up` | Mark with `_` prefix |
| `_Comment=` | `Update and upgrade your Linux system` | Mark with `_` prefix |

---

## 6. Strings NOT to Translate

The following strings appear in the codebase and must **NOT** be wrapped:

| String | Reason |
|--------|--------|
| `"io.github.up"` | Application ID — must remain ASCII and stable |
| `"Up"` (window `.title()`) | Window title; will be derived from `.desktop` Name |
| `"Up Contributors"` | Author attribution |
| `"Up"` in `adw::AboutDialog` `.application_name()` | Proper name, not translatable |
| Log format strings passed to `log::warn!`, `log::info!` etc. | Developer-facing, not user-visible |
| Backend internal strings (`"success"`, `"error"`, `"skipped"`) in history JSON | Internal state strings |
| Icon names (`"view-refresh-symbolic"`, etc.) | System constants |
| CSS class names (`"suggested-action"`, `"pill"`, etc.) | System constants |
| Action names (`"win.maintenance"`, `"win.about"`) | Internal identifiers |
| Response IDs (`"cancel"`, `"update"`, `"close"`, `"upgrade"`) | Internal identifiers |

---

## 7. Risks & Mitigations

| Risk | Severity | Mitigation |
|------|----------|------------|
| `xgettext` < 0.21 installed on build host — `--language=Rust` flag rejected | Medium | Document minimum gettext version requirement. Meson will report an error at configure time. Most CI environments (GitHub Actions ubuntu-latest, fedora containers) ship gettext ≥ 0.21. |
| `option_env!("LOCALEDIR")` resolves to wrong path when building via `cargo build` alone | Low | Fallback to `/usr/share/locale` is acceptable for development. Distribution packages always use Meson/Flatpak. |
| Strings with embedded `\u{2026}` (…), `\u{2014}` (—), `\u{2022}` (•) in gettext msgid | Low | These Unicode code points are valid UTF-8 and xgettext handles them correctly with `--from-code=UTF-8`. |
| Format strings with `{}` placeholders may be garbled by translators | Medium | Add translator notes in the `.pot` file header and use `/* Translators: {} is replaced by the count */` comments near each `gettext()` call. |
| Plural forms for `ngettext`: many languages have more than 2 forms | Low | `ngettext` and the `.po` format natively support multi-form plural rules per language (e.g., Slavic languages have 3-4 forms). The gettext infrastructure handles this automatically. |
| Duplicate strings in window.rs (maintenance vs update flows share identical strings) | None | gettext de-duplicates identical msgid strings in the `.pot` file. One translation covers all occurrences. |
| Empty `LINGUAS` means no `.mo` files are compiled — app falls back to English | Intentional | Correct behavior. Translations can be added incrementally by contributors. The infrastructure is ready but no translations are required at launch. |
| `i18n.merge_file()` requires renaming `.desktop` → `.desktop.in` | Low | The Meson build already installs the file. The rename is straightforward. The `.gresource.xml` does not reference the desktop file, so no cascading changes needed. |
| Nix flake build — gettext may not be in `nativeBuildInputs` | Medium | The implementor must audit `flake.nix` and add `gettext` to `nativeBuildInputs` if absent. If the flake uses `naersk` or `crane`, the `buildInputs` list should include `gettext`. |
| Flatpak build — `LOCALEDIR` must resolve to `/app/share/locale` | Low | Flatpak's GNOME SDK provides gettext and sets `LOCALEDIR` automatically during `meson install`. The `localedir` Meson variable resolves to `$prefix/share/locale` which in Flatpak is `/app/share/locale`. |

---

## Return

### Summary

The Up application currently has no translation infrastructure whatsoever — approximately 103 unique user-visible strings are raw Rust string literals scattered across 6 UI source files, with no calls to any translation function and no `po/` directory. The feature is well-scoped: it does not require a generated config file, blueprint templates, or any architectural changes; it is purely additive.

The implementation uses `gettext-rs = { version = "0.7", features = ["gettext-system"] }` (confirmed from official GNOME Rust apps: Fractal, Paper Plane), initialized via `setlocale` + `bindtextdomain` + `textdomain` at the start of `main()` before GTK/adw initialization. The text domain is `"io.github.up"` (matching the existing `APP_ID` constant), and `LOCALEDIR` is injected by Meson via `option_env!()` with a `/usr/share/locale` fallback for plain `cargo build`. No `bind_textdomain_codeset()` is needed — Rust's strings are always UTF-8.

The Meson integration requires adding `i18n = import('i18n')`, a `po/meson.build` that calls `i18n.gettext('io.github.up', preset: 'glib')`, passing `--language=Rust --keyword=gettext:1 --keyword=ngettext:1,2` to xgettext, a `subdir('po')` call, and replacing the plain desktop file `install_data` with `i18n.merge_file()` (which requires renaming `io.github.up.desktop` to `io.github.up.desktop.in` and prefixing translatable keys with `_`). The metainfo XML does not require changes for the initial infrastructure.

### Spec File Path

`c:\Projects\Up\.github\docs\subagent_docs\localization_spec.md`
