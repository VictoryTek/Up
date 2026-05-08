# Changelog Viewer — Implementation Specification

**Feature:** Update changelog viewer — `apt changelog`, `dnf updateinfo info`, OSTree commit
summaries, and per-backend release notes visible per `UpdateRow`

**Status:** Draft  
**Author:** Research Subagent  
**Date:** 2026-05-08

---

## 1. Current State Analysis

### 1.1 UpdateRow Expander Structure

`src/ui/update_row.rs` provides `UpdateRow`, which wraps an `adw::ExpanderRow`.

**Current child rows** (added by `set_packages()`):
- Up to 50 `adw::ActionRow` widgets, one per pending package name (the row title is the
  package identifier string, e.g. `"htop"`).
- If more than 50 packages are pending, a trailing `adw::ActionRow` reading `"… and N more"`
  is appended.
- The expander is hidden (`set_enable_expansion(false)`) when the package list is empty.

**Exact code that adds child rows:**
```rust
for pkg in &packages[..display_count] {
    let pkg_row = adw::ActionRow::builder().title(pkg.as_str()).build();
    self.row.add_row(&pkg_row);
    tracked.push(pkg_row);
}
```

**What is NOT present:**
- No "changelog", "info", or "release notes" button anywhere in the row or in the window.
- No secondary action attached to any child row.
- No dialog or side panel for changelog text.

### 1.2 Package Cap

Display is capped at `MAX_PACKAGES = 50`. The `pkg_rows` `Vec` tracks all added rows
(including the "…and N more" overflow row) so they can be cleared on re-check.

### 1.3 Backend Trait

`src/backends/mod.rs` — `Backend` trait currently provides:
- `kind() -> BackendKind`
- `display_name() -> &str`
- `description() -> &str`
- `icon_name() -> &str`
- `run_update(runner) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + '_>>`
- `needs_root() -> bool` (default `false`)
- `count_available() -> Pin<Box<...>>` (default delegates to `list_available`)
- `list_available() -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>>`
  (default returns `Ok(vec![])`)
- `supports_cleanup() -> bool` (default `false`)
- `run_cleanup(runner) -> Pin<Box<...>>` (default no-op)

There is **no** existing changelog or release-notes method.

### 1.4 Changelog Commands — Per-Backend Research

#### APT
- `apt changelog <pkg>` — contacts `changelogs.ubuntu.com` or Debian mirrors; requires
  internet and only works for packages with published `.changelog` files. Hangs indefinitely
  if the network is unavailable. **Not suitable** as a primary source.
- `apt show <pkg>` — returns `Description`, `Version`, `Installed-Size`, `Depends` etc. from
  the local package cache. **No network, always available once the cache is populated.**
  Output is human-readable key-value text.
- `apt-cache show <pkg>` — equivalent to `apt show`, slightly more machine-friendly.
- **Decision:** Use `apt-cache show <pkg1> <pkg2> ...` for all pending packages (multi-arg
  accepted). Slice to a sensible maximum (20 packages) to keep output readable. The
  Description field gives a useful snapshot. This approach is unprivileged, offline, and fast.

#### DNF
- `dnf updateinfo info` (no package args) — lists all available security/bugfix/enhancement
  advisories for pending upgrades. Exit code 0 even when no advisories are present (output
  contains "No advisory information available" or is empty). Available unprivileged.
- `dnf updateinfo info --updates` — same but filtered to only packages with pending updates.
  **Use this variant.**
- Output format: human-readable advisory blocks with `Type`, `Severity`, `Title`, `Description`.
- **Decision:** Run `dnf updateinfo info --updates` once per check. Ignore empty / no-advisory
  output gracefully.

#### Pacman
- Pacman has **no built-in changelog** command. `pacman -Si <pkg>` shows repository metadata
  (version, URL, description) but not a changelog.
- Arch packages include an optional `CHANGELOG` file that is rarely populated by maintainers.
  `pacman -Qc <pkg>` prints it only when present, otherwise exits non-zero.
- **Decision:** Use `pacman -Si <pkg1> <pkg2> ...` for the pending packages (capped at 10),
  extracting the `Description` and `URL` fields. Present this as "package info" rather than
  calling it a "changelog."

#### Zypper
- `zypper info --changelog <pkg>` is **not a valid option** in standard Zypper.
- `zypper info <pkg>` — shows description, version, URL. Offline, unprivileged.
- `zypper patch-info <patchname>` — only useful for named patches, not general packages.
- **Decision:** Use `zypper info <pkg1> <pkg2> ...` (capped at 10 packages) to extract
  `Description` fields. Present as "package info."

#### Flatpak
- `flatpak remote-info --log <remote> <app-id>` — fetches OSTree commit history for an app
  from its remote. Requires network. Shows commit subjects (which Flathub populates with
  version+changelog links).
- `flatpak remote-info <remote> <app-id>` — single latest release metadata, offline if
  the local metadata is fresh. Shows `subject` (brief release note).
- **Decision:** Run `flatpak remote-info --log <remote> <app-id>` for each pending app ID.
  This requires knowing the remote name. Use `flatpak list --app --columns=application,origin`
  to map app IDs to remotes, then run `remote-info --log`. Cap to 5 apps. Mark as
  "may require network." When inside the sandbox, prefix with `flatpak-spawn --host`.

#### Nix
- Nix packages do not have a local changelog. `nix-env --query --description` lists
  descriptions for installed packages.
- For flake-based NixOS, the flake inputs (nixpkgs) are the relevant unit; per-package
  changelogs are not accessible without querying the Nixpkgs git log.
- `nix eval nixpkgs#<pkg>.meta.description` — requires `nix-command` experimental feature
  and network (evaluates against current nixpkgs).
- **Decision:** Nix does **not support** changelog viewing. The "View Changelog" button is
  hidden for the Nix backend.

#### fwupd
- `fwupdmgr get-updates --json` — already called by `list_available()`. The JSON structure
  includes per-device `Releases[].Description` and `Releases[].Summary` fields, which contain
  human-readable release notes from LVFS metadata. This data is **already fetched** as part of
  the normal update check.
- **Decision:** Cache the raw fwupd JSON output from `list_available()` in `FwupdBackend`
  and expose a `release_notes() -> Option<String>` method, OR re-run
  `fwupdmgr get-updates --json` in the changelog fetch and parse the `Description`/`Summary`
  fields from all devices. Use the latter for simplicity (stateless, re-fetches on demand).

#### Homebrew
- `brew info --json=v2 <pkg>` — returns structured JSON with `desc`, `homepage`, and
  `versions` (current, stable, head). Offline-capable for formulae metadata; no network
  required if the Homebrew tap is cached.
- **Decision:** Use `brew info --json=v2 <pkg1> <pkg2> ...` (capped at 10) and extract
  `formulae[].desc` and `formulae[].homepage` fields.

---

## 2. Feature Definition

### 2.1 Summary

When a `UpdateRow` expander is populated with pending packages and the user expands it,
a **"View Changelog"** button (`gtk::Button`) appears at the bottom of the expanded content
(added via `adw::ExpanderRow::add_row()` as a non-package row). Clicking this button:

1. Disables itself and shows a `gtk::Spinner` while fetching.
2. Fetches changelog/info text asynchronously (unprivileged, background thread).
3. Displays the result in an `adw::AlertDialog` containing a scrollable `gtk::TextView`.
4. Re-enables the button when the dialog is dismissed.

**Fallback:** For `BackendKind::Nix`, the button is never added. For backends that produce
empty output, the dialog shows a "No changelog information available" message.

### 2.2 Per-Backend Support Matrix

| Backend  | Command                                  | Network? | Notes                                      |
|----------|------------------------------------------|----------|--------------------------------------------|
| APT      | `apt-cache show <pkg>...` (cap 20)       | No       | Shows Description + Version from cache     |
| DNF      | `dnf updateinfo info --updates`          | No       | Security/advisory info for pending updates |
| Pacman   | `pacman -Si <pkg>...` (cap 10)           | No       | Package metadata; labelled "Package Info"  |
| Zypper   | `zypper info <pkg>...` (cap 10)          | No       | Package metadata; labelled "Package Info"  |
| Flatpak  | `flatpak remote-info --log <r> <app>`    | Yes      | OSTree commit history per app (cap 5)      |
| Homebrew | `brew info --json=v2 <pkg>...` (cap 10)  | No       | Formula description + homepage             |
| fwupd    | `fwupdmgr get-updates --json` (re-fetch) | No       | LVFS release notes from daemon metadata    |
| Nix      | (not supported)                          | N/A      | Button hidden                              |

---

## 3. Architecture

### 3.1 New File: `src/changelog.rs`

Single public function — no trait method extension to `Backend` is needed. The approach
avoids making `Backend` non-object-safe or adding async trait complexity.

```rust
// src/changelog.rs

use crate::backends::BackendKind;

#[derive(Debug, thiserror::Error)]
pub enum ChangelogError {
    #[error("Changelog is not supported for this backend")]
    NotSupported,
    #[error("Failed to run changelog command: {0}")]
    Spawn(String),
    #[error("Command exited with error (code {code}): {message}")]
    Exit { code: i32, message: String },
    #[error("No changelog information was returned")]
    Empty,
}

/// Fetch changelog / release-notes text for `packages` (pending update names)
/// from the given backend. Returns `Err(ChangelogError::NotSupported)` for
/// backends where changelog fetching is not implemented.
///
/// This function is fully async and unprivileged. It must be called from a
/// background task (e.g. `crate::ui::spawn_background_async`), NOT from the
/// GTK main thread.
pub async fn fetch_changelog(
    kind: BackendKind,
    packages: &[String],
) -> Result<String, ChangelogError> { ... }
```

**Per-backend dispatch** inside `fetch_changelog`:

- `BackendKind::Apt` → `fetch_apt(packages)`
- `BackendKind::Dnf` → `fetch_dnf(packages)`
- `BackendKind::Pacman` → `fetch_pacman(packages)`
- `BackendKind::Zypper` → `fetch_zypper(packages)`
- `BackendKind::Flatpak` → `fetch_flatpak(packages)`
- `BackendKind::Homebrew` → `fetch_homebrew(packages)`
- `BackendKind::Fwupd` → `fetch_fwupd()`
- `BackendKind::Nix` → `Err(ChangelogError::NotSupported)`

### 3.2 UpdateRow Changes (`src/ui/update_row.rs`)

**New fields** added to `UpdateRow`:
```rust
changelog_button: gtk::Button,
changelog_spinner: gtk::Spinner,
packages_cache: Rc<RefCell<Vec<String>>>,
backend_kind: BackendKind,
```

**Behaviour:**
- `new()` accepts `kind: BackendKind` as an additional parameter.
- A `gtk::Button` labelled `"View Changelog"` with icon `"dialog-information-symbolic"` is
  created.
- It is conditionally hidden (`set_visible(false)`) for `BackendKind::Nix`.
- The button is wrapped in an `adw::ActionRow` (title = `""`, add_suffix = button) and added
  as the last child row via `self.row.add_row(&changelog_action_row)`. This row is **not**
  tracked in `pkg_rows` (it must persist across `set_packages` calls).
- The changelog action row is only visible when the package list is non-empty
  (`set_packages` shows/hides it with the expander).
- On click: captures `packages_cache` and `backend_kind`; calls
  `crate::ui::spawn_background_async` → `changelog::fetch_changelog(kind, &packages)` →
  sends result back via `async_channel` → received on GTK main thread →
  opens `adw::AlertDialog` with scrollable content.

### 3.3 Changelog Dialog

An `adw::AlertDialog` is used (already in scope; used for metered/battery/snapshot dialogs
in `window.rs`):

```rust
let dialog = adw::AlertDialog::builder()
    .heading("Changelog")
    .body("")           // body left empty; content is the extra child
    .build();

let text_view = gtk::TextView::builder()
    .editable(false)
    .cursor_visible(false)
    .wrap_mode(gtk::WrapMode::Word)
    .monospace(true)
    .margin_top(8)
    .margin_bottom(8)
    .margin_start(8)
    .margin_end(8)
    .build();
text_view.buffer().set_text(&changelog_text);

let scrolled = gtk::ScrolledWindow::builder()
    .child(&text_view)
    .min_content_height(300)
    .max_content_height(500)
    .hscrollbar_policy(gtk::PolicyType::Never)
    .build();

dialog.set_extra_child(Some(&scrolled));
dialog.add_response("close", "Close");
dialog.set_default_response(Some("close"));
dialog.set_close_response("close");
dialog.present(Some(parent_widget));
```

The `parent_widget` is obtained by calling `self.row.root()` and downcasting to
`gtk::Widget`.

---

## 4. Per-Backend Implementation Details

### 4.1 APT (`fetch_apt`)

```
tokio::process::Command::new("apt-cache")
    .args(["show", "--no-all-versions"])
    .args(&packages[..packages.len().min(20)])
    .output()
    .await
```

Parse the stdout as UTF-8 text. Return the raw output (apt-cache show produces readable
key-value blocks). If the output is empty after trim, return `Err(ChangelogError::Empty)`.

### 4.2 DNF (`fetch_dnf`)

```
tokio::process::Command::new("dnf")
    .args(["updateinfo", "info", "--updates"])
    .output()
    .await
```

Exit code 0 = success; exit code 1 = error. Check stdout; if it contains only whitespace or
"No advisory information available", return `Err(ChangelogError::Empty)`. Otherwise return
the trimmed stdout.

### 4.3 Pacman (`fetch_pacman`)

```
tokio::process::Command::new("pacman")
    .args(["-Si"])
    .args(&packages[..packages.len().min(10)])
    .output()
    .await
```

Return trimmed stdout. Label the dialog heading "Package Info" for Pacman/Zypper to set
expectations correctly (not a changelog). This is done in the UI layer by checking the kind.

### 4.4 Zypper (`fetch_zypper`)

```
tokio::process::Command::new("zypper")
    .args(["info"])
    .args(&packages[..packages.len().min(10)])
    .env("LANG", "C")
    .env("LC_ALL", "C")
    .output()
    .await
```

Return trimmed stdout. Same "Package Info" label as Pacman.

### 4.5 Flatpak (`fetch_flatpak`)

Two-step:

1. Get remote mappings:
   ```
   flatpak list --app --columns=application,origin
   ```
   Parse into a `HashMap<String, String>` (app_id → remote_name).

2. For each pending app (cap 5, limited due to individual network calls):
   ```
   flatpak remote-info --log <remote> <app_id>
   ```
   Concatenate results with `"\n---\n"` separators.

If inside the Flatpak sandbox, prefix both commands with `flatpak-spawn --host` using the
same `build_flatpak_cmd` helper already in `flatpak.rs`. Import or replicate this logic in
`changelog.rs` (prefer a shared helper — see §6).

### 4.6 Homebrew (`fetch_homebrew`)

```
tokio::process::Command::new("brew")
    .args(["info", "--json=v2"])
    .args(&packages[..packages.len().min(10)])
    .output()
    .await
```

Parse the JSON output:
```json
{ "formulae": [{ "desc": "...", "homepage": "...", "name": "..." }] }
```

Format as:
```
<name>
  Description: <desc>
  Homepage:    <homepage>
```

If JSON parsing fails, fall back to returning the raw stdout text.

### 4.7 fwupd (`fetch_fwupd`)

```
tokio::process::Command::new("fwupdmgr")
    .args(["get-updates", "--json"])
    .output()
    .await
```

Parse the JSON (same as `parse_fwupd_updates` in `fwupd.rs`). Extract per-device fields:
```json
"Name", "Version", "Releases[0].Summary", "Releases[0].Description"
```

Format as human-readable text blocks per device. If `Devices` is empty or all descriptions
are empty, return `Err(ChangelogError::Empty)`.

---

## 5. Implementation Steps (Ordered, File-by-File)

### Step 1: `src/changelog.rs` (new file)

1. Create `src/changelog.rs`.
2. Define `ChangelogError` using `thiserror`.
3. Implement `pub async fn fetch_changelog(kind: BackendKind, packages: &[String]) -> Result<String, ChangelogError>`.
4. Implement private async helpers: `fetch_apt`, `fetch_dnf`, `fetch_pacman`, `fetch_zypper`,
   `fetch_flatpak`, `fetch_homebrew`, `fetch_fwupd`.
5. For Flatpak, implement a private `is_in_flatpak_sandbox() -> bool` (can call
   `crate::backends::flatpak::is_running_in_flatpak()` directly) and a helper to build
   the `flatpak-spawn --host` prefix when needed.

### Step 2: `src/main.rs` (or `src/lib.rs` if applicable) — register the module

Add `mod changelog;` to `src/main.rs`.

### Step 3: `src/ui/update_row.rs` — add changelog button

1. Add `backend_kind: BackendKind` field.
2. Add `changelog_row: adw::ActionRow` field (the persistent footer row).
3. Add `packages_cache: Rc<RefCell<Vec<String>>>` field.
4. Update `UpdateRow::new(backend, on_skip_changed, on_retry)` signature — it already
   receives `&dyn Backend`, so `backend.kind()` is available without signature change.
5. In `new()`:
   - Create the `changelog_row` with `adw::ActionRow`.
   - Create `changelog_button = gtk::Button::from_icon_name("dialog-information-symbolic")`:
     - Set label: `"View Changelog"` (use `set_label` or build with label).
     - Use `gtk::Button::builder().label("View Changelog").build()` for clarity.
     - Set `valign(gtk::Align::Center)`.
   - Create `changelog_spinner = gtk::Spinner::builder().visible(false).build()`.
   - Add `changelog_button` and `changelog_spinner` as suffixes to `changelog_row`.
   - Call `self.row.add_row(&changelog_row)` **after** the `UpdateRow` struct is partially
     initialised (or use a `post_init` step). Since `add_row` is called on `self.row`, do it
     in `new()` before returning.
   - Hide `changelog_row` initially (`changelog_row.set_visible(false)`) — shown only
     when packages are set.
   - Hide for `BackendKind::Nix`: `if kind == BackendKind::Nix { changelog_row.set_visible(false); changelog_button.set_sensitive(false); }` — but also just never show it at all for Nix.
6. Wire the button click with `glib::clone!`:
   - Capture `packages_cache` (clone of the `Rc<RefCell<Vec<String>>>`), `backend_kind`.
   - Disable the button, show spinner.
   - Spawn background via `crate::ui::spawn_background_async`.
   - In background: call `crate::changelog::fetch_changelog(kind, &packages).await`.
   - Send result back via `async_channel::bounded(1)`.
   - On GTK main thread: hide spinner, re-enable button, open dialog (see §3.3).
   - The parent widget for `dialog.present()` is obtained from
     `changelog_button.root().and_downcast::<gtk::Widget>()`. Use `changelog_button`
     directly as the parent — `adw::AlertDialog::present` accepts `Option<&impl IsA<gtk::Widget>>`.
7. In `set_packages()`:
   - Update `packages_cache` with the new package list.
   - Show/hide `changelog_row`: `self.changelog_row.set_visible(!packages.is_empty() && kind != BackendKind::Nix)`.

### Step 4: `src/ui/mod.rs` — verify `spawn_background_async` is accessible

Check that `crate::ui::spawn_background_async` (or `super::spawn_background_async`) is
accessible from `update_row.rs`. It is defined in `src/ui/mod.rs` or `src/ui/window.rs`.
If it is in `window.rs`, move it to `mod.rs` as a `pub(super)` or `pub(crate)` function.

Signature expected:
```rust
pub fn spawn_background_async<F, Fut>(f: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static;
```

This function is already used in `window.rs`. Confirm its exact location and export level
before Step 3.

### Step 5: `src/ui/window.rs` — no changes required

`UpdateRow::new()` in `window.rs` already passes `backend` (a `&dyn Backend`). Since
`backend.kind()` is called inside `new()`, the call sites do not change.

### Step 6: Verify `src/main.rs` module registration

Confirm `mod changelog;` is present.

---

## 6. New Dependencies

**No new Cargo dependencies required.**

- `thiserror` is already in `Cargo.toml` (version `"2"`).
- `serde_json` is already present for fwupd JSON parsing.
- `tokio::process::Command` is already used throughout backends.
- `adw::AlertDialog` is already used in `window.rs`.
- `gtk::TextView`, `gtk::ScrolledWindow` are part of `gtk4` crate already imported.

---

## 7. Risks & Mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| `apt-cache show` hangs or is very slow on large package lists | Low | Cap at 20 packages; use `tokio::time::timeout(Duration::from_secs(15), ...)` around the command |
| `dnf updateinfo` slow on cold cache | Medium | No package cap needed (single command); apply 30-second timeout |
| `flatpak remote-info --log` makes multiple network calls | Medium | Cap at 5 apps; inform user in dialog heading ("may require network") |
| `fwupdmgr` unavailable / daemon not running | Low | `list_available()` already handles this; `fetch_fwupd` mirrors same error handling |
| `brew info --json=v2` fails on Linuxbrew with some casks | Low | Fall back to raw stdout on JSON parse error |
| `pacman -Si` fails when a package is AUR-only (not in sync db) | Medium | Ignore per-package exit code; collect all output lines regardless |
| Dialog displayed before fetch completes (race) | Low | Button disabled during fetch; channel bounded(1) ensures sequential delivery |
| `adw::AlertDialog::set_extra_child` not available in libadwaita v1.5 | Must check | `set_extra_child` was added in libadwaita 1.5 — project has `features = ["v1_5"]` so it IS available. Confirmed safe. |
| `gtk::TextView` monospace in dark mode readability | Low | No CSS override needed; system monospace font handles this |
| `packages_cache` out of sync with `pkg_rows` after re-check clears rows | Medium | `set_packages()` always updates `packages_cache` before showing button; never use stale cache |
| Changelog button visible during update-in-progress state | Low | Button is in the expander (collapsed by default during update); no additional guard needed |
| Command injection via package names passed to shell | N/A — NOT applicable | Commands use `tokio::process::Command::args()` array form, never shell string interpolation. Package names from `list_available()` are already validated by parsing. |

### 7.1 Timeout Handling

All async commands in `changelog.rs` must be wrapped with `tokio::time::timeout`:

```rust
use tokio::time::{timeout, Duration};

let out = timeout(
    Duration::from_secs(30),
    tokio::process::Command::new("cmd").args(&[...]).output()
)
.await
.map_err(|_| ChangelogError::Spawn("timed out".to_string()))?
.map_err(|e| ChangelogError::Spawn(e.to_string()))?;
```

### 7.2 `adw::AlertDialog` heading per BackendKind

In the UI glue (button click handler), set the dialog heading based on kind:
- APT, DNF, Flatpak, fwupd, Homebrew → `"Changelog"`
- Pacman, Zypper → `"Package Info"`

### 7.3 Flatpak Sandbox Detection

Reuse `crate::backends::flatpak::is_running_in_flatpak()` directly in `changelog.rs` rather
than duplicating the path check.

---

## 8. Module Registration Checklist

- [ ] `mod changelog;` added to `src/main.rs`
- [ ] `use crate::changelog;` added in `src/ui/update_row.rs`
- [ ] `spawn_background_async` is `pub(crate)` or accessible from `update_row.rs`
- [ ] `BackendKind` derive `PartialEq` already present — confirmed in `mod.rs`
  (`#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]`)

---

## 9. Exact `UpdateRow::new` Signature — No Change Required

```rust
pub fn new(
    backend: &dyn Backend,
    on_skip_changed: impl Fn() + 'static,
    on_retry: impl Fn() + 'static,
) -> Self
```

`backend.kind()` is available inside `new()` without changing callers. Store it as
`backend_kind: BackendKind` in the struct.

---

## 10. File Summary

| File | Action | Description |
|------|--------|-------------|
| `src/changelog.rs` | **Create** | Async changelog fetching, `ChangelogError`, per-backend dispatch |
| `src/main.rs` | **Edit** | Add `mod changelog;` |
| `src/ui/update_row.rs` | **Edit** | Add `backend_kind`, `changelog_row`, `packages_cache` fields; wire button click; update `set_packages` |
| `src/ui/mod.rs` | **Edit (if needed)** | Expose `spawn_background_async` at `pub(crate)` if it is currently private in `window.rs` |

No changes to `src/backends/*.rs`, `Cargo.toml`, `meson.build`, or any other file.
