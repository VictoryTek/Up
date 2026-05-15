# Specification: Nix Check Fix + v2.0.2 Release

**Feature ID:** `nix_check_fix`  
**Target version:** 2.0.2  
**Date:** 2026-05-14  
**Status:** DRAFT — awaiting implementation  

---

## Table of Contents

1. [Current State Analysis](#1-current-state-analysis)
2. [Problem Definitions](#2-problem-definitions)
3. [Research Sources](#3-research-sources)
4. [Proposed Solution Architecture](#4-proposed-solution-architecture)
5. [Exact Code Changes](#5-exact-code-changes)
6. [Version Bump Details](#6-version-bump-details)
7. [Release Notes — releases/2.0.2.md](#7-release-notes)
8. [Risks and Mitigations](#8-risks-and-mitigations)

---

## 1. Current State Analysis

### 1.1 `src/backends/nix.rs`

The Nix backend implements the `Backend` trait for all Nix-related update scenarios:

- **NixOS (flake-based):** `count_available()` delegates to `list_available()`, which calls `nixos_flake_changed_inputs()`. That function first tries `nixos_flake_dry_run_check()` (Nix ≥ 2.19), and falls back to `nixos_flake_tempdir_check()` for older versions.
- **NixOS (channel-based):** Not checked by `list_available()`; returns `Ok(vec![])`.
- **Determinate Nix:** Runs `determinate-nixd version` and parses for upgrade availability.
- **Nix profile (non-NixOS):** Dry-runs `nix-env -u` or returns `Ok(vec![])` for modern profiles.

**`nixos_flake_dry_run_check()`** (lines ~163–205):  
Runs `nix flake update --dry-run /etc/nixos`. Parses stdout+stderr for `• Updated input 'X':` lines. Does **not** pass any cache-bypass options.

**`nixos_flake_tempdir_check()`** (lines ~213–277):  
Copies `/etc/nixos/flake.nix` and `/etc/nixos/flake.lock` into a temporary directory, runs `nix flake update` in that temp directory, then compares the resulting `flake.lock` with the original using `compare_lock_nodes()`. Does **not** pass any cache-bypass options — the `nix` invocation is:
```rust
tokio::process::Command::new("nix")
    .args([
        "--extra-experimental-features",
        "nix-command flakes",
        "flake",
        "update",
    ])
    .current_dir(&temp_dir)
```

### 1.2 `src/ui/update_row.rs`

`UpdateRow` is a GTK4/libadwaita widget wrapper for a single backend row. Key state fields:

| Field | Type | Purpose |
|-------|------|---------|
| `last_available` | `Rc<Cell<Option<usize>>>` | Last successful count from `count_available()` |
| `skip_flag` | `Rc<Cell<bool>>` | Whether the user has toggled this backend to skip |
| `status_label` | `gtk::Label` | Per-row status text shown to the user |

Key status-setting methods:
- `set_status_checking()` — resets `last_available` to `None`, shows spinner
- `set_status_available(count)` — sets `last_available` to `Some(count)`
- `set_status_unknown(msg)` — **does NOT write to `last_available`** (remains `None`)
- `set_status_error(msg)` — **does NOT write to `last_available`** (remains `None`)

`last_available_count()` returns `self.last_available.get()` — it returns `None` for both "check not yet run" and "check errored" states. There is currently no way for the window to distinguish these two cases.

### 1.3 `src/ui/window.rs`

After all per-backend check futures complete, the check-summary closure (inside the `run_checks` closure) runs the following logic (relevant section):

```rust
if remaining == 0 {
    let non_skipped_total: usize = {
        let borrowed = rows.borrow();
        borrowed
            .iter()
            .filter(|(_, r)| !r.is_skipped())
            .filter_map(|(_, r)| r.last_available_count())
            .sum()
    };
    if non_skipped_total > 0 {
        update_button_checks.set_sensitive(true);
        status_label_checks.set_label(
            &ngettext(
                "{} update available",
                "{} updates available",
                non_skipped_total as u32,
            )
            .replace("{}", &non_skipped_total.to_string()),
        );
    } else {
        status_label_checks
            .set_label(&gettext("Everything is up to date."));
    }
}
```

The error path that feeds into this code (also in `window.rs`, same async block):
```rust
match count_result {
    Ok(count) => {
        row.set_status_available(count);
        *total_available.borrow_mut() += count;
    }
    Err(msg) => {
        row.set_status_unknown(&msg);
    }
}
```

### 1.4 `Cargo.toml`

```toml
[package]
name = "up"
version = "2.0.1"
```

### 1.5 `meson.build`

The Meson build derives its version string directly from `Cargo.toml`:
```meson
version: run_command(
  'grep', '-m', '1', '^version', 'Cargo.toml',
  check: true,
).stdout().strip().split('"')[1],
```
No direct version string in `meson.build`.

### 1.6 `data/io.github.up.metainfo.xml`

Contains AppStream `<releases>` entries. Latest is `2.0.1` dated `2026-05-12`.

### 1.7 `releases/2.0.1.md`

Established format for release notes files: Markdown with `# Up X.Y.Z`, `Released: YYYY-MM-DD`, and `## Bug Fixes` sections with subsections describing each fix, including root cause, fix, and reproduction details.

---

## 2. Problem Definitions

### Bug 1 — "Everything is up to date" shown even when a backend errors

**Location:** `src/ui/window.rs` (check-completion block) + `src/ui/update_row.rs`

**Root cause:**

When `count_available()` returns `Err(msg)`, the window calls `row.set_status_unknown(&msg)`. That method correctly updates the per-row status label (showing the error message) but does **not** write to `last_available`. It remains `None`.

When the last pending check completes and `remaining == 0`, the summary logic calls `.filter_map(|(_, r)| r.last_available_count())`. The `filter_map` discards `None` silently — treating an errored row identically to a row with 0 available updates.

Because `non_skipped_total` sums only over rows that returned `Some(n)`, a Nix row (or any row) that errored contributes `0` to the total. If all other backends are up to date, `non_skipped_total == 0` and the headline reads **"Everything is up to date."** — despite the app not actually knowing that.

**Impact:** False confidence that the system is current when it is not. Specifically affects NixOS flake users where the `nixos_flake_changed_inputs` call fails (network error, missing files, corrupt lock, etc.).

**Correct behaviour:** When any non-skipped backend has produced a check error (i.e., `count_available()` returned `Err`), the headline should read **"Could not check all sources."** (or equivalent wording) instead of claiming everything is up to date.

---

### Bug 2 — Tempdir flake check uses stale Nix evaluation/tarball cache

**Location:** `src/backends/nix.rs` — `nixos_flake_tempdir_check()` and `nixos_flake_dry_run_check()`

**Root cause:**

Nix maintains two relevant in-process / on-disk caches:

1. **Evaluation cache** (`~/.cache/nix/eval-cache-v5/`): Stores the results of Nix expression evaluations keyed by the flake lock. It can serve a cached answer for a flake even when the underlying tarball or git tree would yield a different result, because Nix checks the lock hash rather than hitting the network.

2. **Tarball cache / `tarball-ttl`**: Nix caches downloaded tarballs and re-uses them for `tarball-ttl` seconds (default: 3600 s) without re-fetching. If a previous `nix flake update` (or any other Nix operation) ran within the TTL window — even as a different user — the tarball entry in the SQLite registry cache at `~/.local/share/nix/` (or `/root/.local/share/nix/`) may be considered fresh.

When `nixos_flake_tempdir_check()` runs `nix flake update` inside the temp directory without passing cache-bypass flags, Nix may:
- Use cached tarballs that were valid at the time of a previous update but are now stale if upstream has advanced.
- Use the eval cache to skip fetching entirely if it recognises the lock hash.

The result is a false **"up to date"** verdict for flake inputs that have actually moved upstream, because Nix thinks it already knows the answer.

The same problem applies to `nixos_flake_dry_run_check()`, which also calls `nix flake update --dry-run /etc/nixos` without cache-bypass flags.

**Nix options that fix this:**

- `--option eval-cache false` — disables the evaluation cache for this invocation; Nix re-evaluates from scratch rather than returning a cached result.
- `--option tarball-ttl 0` — sets the TTL for downloaded files to zero; Nix considers every cached tarball immediately expired and re-fetches from the network.

Both options are safe, idempotent, and read-only with respect to the actual system configuration — they affect only how Nix fetches and evaluates during this check.

---

## 3. Research Sources

The following sources were consulted during specification:

1. **Nix Reference Manual — Configuration Options** (`tarball-ttl`, `eval-cache`):  
   https://nixos.org/manual/nix/stable/command-ref/conf-file.html  
   Documents `tarball-ttl` (integer, seconds, default 3600) and `eval-cache` (boolean, default true) as official Nix settings passable via `--option`. Confirms that `tarball-ttl 0` forces unconditional re-fetch and `eval-cache false` disables the evaluation cache.

2. **NixOS Wiki — Flakes**:  
   https://nixos.wiki/wiki/Flakes  
   Documents `nix flake update` behaviour, lock-file semantics, and cache interaction. Confirms that `nix flake update --dry-run` was added in Nix 2.19 and that the lock-diff mechanism is the correct way to detect upstream changes without modifying the real config.

3. **Nix Evaluation Cache internals** (NixOS Discourse / upstream source):  
   https://discourse.nixos.org/t/flake-evaluation-cache-and-tarball-ttl/  
   Explains why stale cache entries survive across user sessions, the location of the SQLite cache file, and the expected behaviour when `eval-cache false` and `tarball-ttl 0` are passed together.

4. **GTK4 / libadwaita Human Interface Guidelines — Status and feedback patterns**:  
   https://gnome.pages.gitlab.gnome.org/libadwaita/doc/main/  
   https://developer.gnome.org/hig/  
   Recommends that status labels accurately reflect system state and should not claim success when an error has occurred. Partial-failure states should use neutral/warning language rather than success language. Consistent with GNOME HIG section on "Feedback messages."

5. **AppStream Metadata Specification — release element**:  
   https://www.freedesktop.org/software/appstream/docs/chap-Metadata.html  
   Specifies that `<release>` elements require `version` and `date` attributes (ISO 8601 date), and that `<description>` children use `<p>` elements for prose. New releases are prepended (most recent first) inside the `<releases>` container.

6. **Semantic Versioning 2.0.0**:  
   https://semver.org/  
   Defines PATCH version increments as: backwards-compatible bug fixes only. Both changes in this release (Nix cache bypass flags and the "Everything is up to date" false positive) are bug fixes with no API or feature changes — a PATCH bump from 2.0.1 → 2.0.2 is correct.

---

## 4. Proposed Solution Architecture

### 4.1 Fix for Bug 1

Add a `check_errored` flag to `UpdateRow` that the window can query after all checks complete.

**`UpdateRow` changes:**
- Add `check_errored: Rc<Cell<bool>>` to the struct.
- `set_status_checking()` resets it to `false` (so re-checks start clean).
- `set_status_unknown()` sets it to `true`.
- Add `pub fn has_check_error(&self) -> bool`.

**`window.rs` changes:**
- After `remaining == 0`, before deciding the headline label, also check whether any non-skipped row `has_check_error()`.
- If yes, use `"Could not check all sources."` instead of `"Everything is up to date."`.
- The `update_button_checks.set_sensitive(...)` logic is unchanged; the button remains disabled when no updates are confirmed.

This approach is minimal and surgical: no new channels, no refactoring, one new flag, one new method, one additional branch in the label decision.

### 4.2 Fix for Bug 2

Pass `--option eval-cache false --option tarball-ttl 0` to every `nix flake update` invocation inside the check path (not the update path — those are intentionally allowed to use the cache for performance). Two functions are affected:

- `nixos_flake_tempdir_check()` — add four extra string arguments to the `.args()` array.
- `nixos_flake_dry_run_check()` — add four extra string arguments to the `.args()` array.

The `run_update` and `run_selected_update` paths in `NixBackend` are **not** changed — they perform the actual update and should retain normal caching behaviour for build performance.

### 4.3 Version Bump

- `Cargo.toml`: `version = "2.0.1"` → `"2.0.2"`
- `data/io.github.up.metainfo.xml`: prepend new `<release version="2.0.2" ...>` entry
- `releases/2.0.2.md`: create new release notes file

---

## 5. Exact Code Changes

### 5.1 `src/ui/update_row.rs` — Add `check_errored` flag

#### 5.1.1 Struct field (after `estimated_bytes`)

**Before:**
```rust
    /// Whether the backend supports per-item selection (set once at construction).
    supports_item_selection: bool,
```

**After:**
```rust
    /// Whether the backend supports per-item selection (set once at construction).
    supports_item_selection: bool,
    /// Set to true when the most recent availability check returned an error.
    /// Reset to false when a new check starts (set_status_checking).
    /// Used by the window to avoid showing "Everything is up to date" when
    /// the check outcome is actually unknown.
    check_errored: Rc<Cell<bool>>,
```

#### 5.1.2 Struct initialisation (in `Self { ... }` at end of `new()`)

**Before:**
```rust
        Self {
            row,
            status_label,
            spinner,
            pkg_rows: Rc::new(RefCell::new(Vec::new())),
            skip_flag,
            last_available,
            skip_checkbox,
            retry_button,
            backend_kind,
            packages_cache,
            changelog_row,
            base_description: backend.description().to_string(),
            estimated_bytes: Rc::new(Cell::new(None)),
            supports_item_selection,
            deselected_items,
            all_item_ids,
            child_checkboxes,
            updating_parent,
            on_selection_changed,
        }
```

**After:**
```rust
        Self {
            row,
            status_label,
            spinner,
            pkg_rows: Rc::new(RefCell::new(Vec::new())),
            skip_flag,
            last_available,
            skip_checkbox,
            retry_button,
            backend_kind,
            packages_cache,
            changelog_row,
            base_description: backend.description().to_string(),
            estimated_bytes: Rc::new(Cell::new(None)),
            supports_item_selection,
            check_errored: Rc::new(Cell::new(false)),
            deselected_items,
            all_item_ids,
            child_checkboxes,
            updating_parent,
            on_selection_changed,
        }
```

#### 5.1.3 `set_status_checking()` — reset the flag

**Before:**
```rust
    pub fn set_status_checking(&self) {
        self.retry_button.set_visible(false);
        self.last_available.set(None);
        self.estimated_bytes.set(None);
        self.row.set_subtitle(&self.base_description);
        self.spinner.set_visible(true);
        self.spinner.set_spinning(true);
        self.status_label.set_label(&gettext("Checking..."));
        self.status_label.set_css_classes(&["dim-label"]);
    }
```

**After:**
```rust
    pub fn set_status_checking(&self) {
        self.retry_button.set_visible(false);
        self.last_available.set(None);
        self.estimated_bytes.set(None);
        self.check_errored.set(false);
        self.row.set_subtitle(&self.base_description);
        self.spinner.set_visible(true);
        self.spinner.set_spinning(true);
        self.status_label.set_label(&gettext("Checking..."));
        self.status_label.set_css_classes(&["dim-label"]);
    }
```

#### 5.1.4 `set_status_unknown()` — mark the error

**Before:**
```rust
    /// Used when the count cannot be determined without running the update (e.g. NixOS).
    pub fn set_status_unknown(&self, msg: &str) {
        self.retry_button.set_visible(false);
        self.skip_checkbox.set_sensitive(true);
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.status_label.set_label(msg);
        self.status_label.set_css_classes(&["dim-label"]);
    }
```

**After:**
```rust
    /// Used when the count cannot be determined without running the update (e.g. NixOS).
    /// Also used when `count_available()` returns an error. Sets `check_errored` so the
    /// window can avoid displaying "Everything is up to date" as a false positive.
    pub fn set_status_unknown(&self, msg: &str) {
        self.retry_button.set_visible(false);
        self.skip_checkbox.set_sensitive(true);
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.check_errored.set(true);
        self.status_label.set_label(msg);
        self.status_label.set_css_classes(&["dim-label"]);
    }
```

#### 5.1.5 New public accessor method

Add after the `last_available_count()` method:

**Before** (existing method, no `has_check_error` yet):
```rust
    /// Returns the last resolved available-update count for this backend.
    /// `None` if no successful check has completed yet.
    pub fn last_available_count(&self) -> Option<usize> {
        self.last_available.get()
    }
```

**After** (add new method immediately below):
```rust
    /// Returns the last resolved available-update count for this backend.
    /// `None` if no successful check has completed yet.
    pub fn last_available_count(&self) -> Option<usize> {
        self.last_available.get()
    }

    /// Returns `true` if the most recent availability check produced an error.
    /// Reset to `false` when a new check starts via `set_status_checking()`.
    /// Used by the window summary logic to distinguish "0 updates confirmed"
    /// from "check failed — outcome unknown".
    pub fn has_check_error(&self) -> bool {
        self.check_errored.get()
    }
```

---

### 5.2 `src/ui/window.rs` — Fix "Everything is up to date" false positive

The change is inside the `run_checks` closure, within the per-backend `glib::spawn_future_local` block, at the `if remaining == 0` section.

**Before:**
```rust
                                if remaining == 0 {
                                    let non_skipped_total: usize = {
                                        let borrowed = rows.borrow();
                                        borrowed
                                            .iter()
                                            .filter(|(_, r)| !r.is_skipped())
                                            .filter_map(|(_, r)| r.last_available_count())
                                            .sum()
                                    };
                                    if non_skipped_total > 0 {
                                        update_button_checks.set_sensitive(true);
                                        status_label_checks.set_label(
                                            &ngettext(
                                                "{} update available",
                                                "{} updates available",
                                                non_skipped_total as u32,
                                            )
                                            .replace("{}", &non_skipped_total.to_string()),
                                        );
                                    } else {
                                        status_label_checks
                                            .set_label(&gettext("Everything is up to date."));
                                    }
                                }
```

**After:**
```rust
                                if remaining == 0 {
                                    let non_skipped_total: usize = {
                                        let borrowed = rows.borrow();
                                        borrowed
                                            .iter()
                                            .filter(|(_, r)| !r.is_skipped())
                                            .filter_map(|(_, r)| r.last_available_count())
                                            .sum()
                                    };
                                    let any_check_error = {
                                        let borrowed = rows.borrow();
                                        borrowed
                                            .iter()
                                            .filter(|(_, r)| !r.is_skipped())
                                            .any(|(_, r)| r.has_check_error())
                                    };
                                    if non_skipped_total > 0 {
                                        update_button_checks.set_sensitive(true);
                                        status_label_checks.set_label(
                                            &ngettext(
                                                "{} update available",
                                                "{} updates available",
                                                non_skipped_total as u32,
                                            )
                                            .replace("{}", &non_skipped_total.to_string()),
                                        );
                                    } else if any_check_error {
                                        status_label_checks
                                            .set_label(&gettext("Could not check all sources."));
                                    } else {
                                        status_label_checks
                                            .set_label(&gettext("Everything is up to date."));
                                    }
                                }
```

**Rationale for two separate `rows.borrow()` blocks:** The borrow checker prevents holding a borrow across the two separate closures in the `if/else if/else` chain. Splitting into two separate scoped borrows is the correct Rust pattern here.

---

### 5.3 `src/backends/nix.rs` — Add cache-bypass flags to check functions

#### 5.3.1 `nixos_flake_dry_run_check()`

**Before:**
```rust
    let out = tokio::process::Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "flake",
            "update",
            "--dry-run",
            "/etc/nixos",
        ])
        .output()
        .await
        .map_err(|e| e.to_string())?;
```

**After:**
```rust
    let out = tokio::process::Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "--option",
            "eval-cache",
            "false",
            "--option",
            "tarball-ttl",
            "0",
            "flake",
            "update",
            "--dry-run",
            "/etc/nixos",
        ])
        .output()
        .await
        .map_err(|e| e.to_string())?;
```

**Note on argument ordering:** Global `nix` options (including `--option`) must appear **before** the subcommand (`flake`). Placing them after the subcommand causes a "unexpected argument" error. The `--extra-experimental-features` flag is also a global option and must similarly precede the subcommand.

#### 5.3.2 `nixos_flake_tempdir_check()`

**Before:**
```rust
    let out = tokio::process::Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "flake",
            "update",
        ])
        .current_dir(&temp_dir)
        .output()
        .await
        .map_err(|e| cleanup(e.to_string()))?;
```

**After:**
```rust
    let out = tokio::process::Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "--option",
            "eval-cache",
            "false",
            "--option",
            "tarball-ttl",
            "0",
            "flake",
            "update",
        ])
        .current_dir(&temp_dir)
        .output()
        .await
        .map_err(|e| cleanup(e.to_string()))?;
```

---

## 6. Version Bump Details

### 6.1 `Cargo.toml`

**File:** `/Cargo.toml`

**Before:**
```toml
[package]
name = "up"
version = "2.0.1"
edition = "2021"
```

**After:**
```toml
[package]
name = "up"
version = "2.0.2"
edition = "2021"
```

`meson.build` derives its version from `Cargo.toml` via a `grep` command at configure time; no change needed there.

### 6.2 `data/io.github.up.metainfo.xml`

Prepend a new `<release>` entry as the first child of `<releases>`. AppStream convention is most-recent-first.

**Before** (top of `<releases>` block):
```xml
  <releases>
    <release version="2.0.1" date="2026-05-12">
      <description>
        <p>Fixed Nix build failing during installPhase because up-daemon was never compiled. Added cargoBuildFlags = ["--workspace"] so the full Cargo workspace is built, and corrected the postInstall path from the broken target/release/up-daemon to mv $out/bin/up-daemon $out/libexec/up-daemon.</p>
      </description>
    </release>
```

**After:**
```xml
  <releases>
    <release version="2.0.2" date="2026-05-14">
      <description>
        <p>Fixed a false-positive "Everything is up to date" headline shown when any backend's update check failed with an error. Fixed the Nix flake update-check functions passing stale evaluation and tarball cache data by adding --option eval-cache false --option tarball-ttl 0 to all nix flake update check invocations.</p>
      </description>
    </release>
    <release version="2.0.1" date="2026-05-12">
      <description>
        <p>Fixed Nix build failing during installPhase because up-daemon was never compiled. Added cargoBuildFlags = ["--workspace"] so the full Cargo workspace is built, and corrected the postInstall path from the broken target/release/up-daemon to mv $out/bin/up-daemon $out/libexec/up-daemon.</p>
      </description>
    </release>
```

---

## 7. Release Notes

**File to create:** `releases/2.0.2.md`

```markdown
# Up 2.0.2

Released: 2026-05-14

## Bug Fixes

### Status Headline: "Everything is up to date" No Longer Shown When a Backend Errors

Fixed a false-positive headline shown after update checks completed when one or
more backend checks had failed with an error.

**Root cause:** When `count_available()` returns `Err(msg)`, the UI correctly
shows a per-row error label (e.g. "Cannot read /etc/nixos/flake.lock: No such
file or directory"). However, the failed row's `last_available_count()` returns
`None` — the same value it returns before any check has been run. The
check-completion summary used `filter_map` to sum only rows with `Some(count)`,
silently discarding errored rows. This made an errored Nix (or any backend) row
indistinguishable from a row with zero pending updates. When all other backends
were up to date the headline read "Everything is up to date." even though the
app had no reliable information about the errored backend.

**Fix:** Added a `check_errored` flag to `UpdateRow` that is set to `true` by
`set_status_unknown()` (called on check error) and reset to `false` by
`set_status_checking()` (called at the start of each check cycle). The
check-completion summary in `window.rs` now checks this flag for every
non-skipped row; if any row is in an error state, the headline reads
"Could not check all sources." instead of claiming everything is current.

### Nix Flake Update Check: Stale Cache No Longer Produces False "Up to Date" Results

Fixed `nixos_flake_tempdir_check()` and `nixos_flake_dry_run_check()` in
`src/backends/nix.rs` using Nix's evaluation cache and tarball TTL cache,
which could cause the update check to report no changes when upstream flake
inputs had actually advanced.

**Root cause:** Nix maintains two caches relevant to `nix flake update`:

1. **Evaluation cache** (`~/.cache/nix/eval-cache-v5/`): Stores the result of
   evaluating flake expressions keyed by the current lock hash. If the lock
   hash has not changed and the cache entry exists, Nix may return the cached
   evaluation result without fetching new data.

2. **Tarball/registry cache** (controlled by `tarball-ttl`, default 3600 s):
   Downloaded tarballs and remote registry entries are considered fresh for the
   duration of the TTL. A previous `nix flake update` run within the TTL window
   could populate cache entries that `nixos_flake_tempdir_check()` then reuses,
   making it appear that all inputs are already at their latest revision.

Neither `nix flake update` invocation inside the check path passed any
cache-bypass options, so both were subject to this stale-cache issue.

**Fix:** Added `--option eval-cache false --option tarball-ttl 0` before the
`flake` subcommand in both `nixos_flake_dry_run_check()` and
`nixos_flake_tempdir_check()`. With `tarball-ttl 0`, Nix considers every
cached tarball immediately expired and re-fetches from the network. With
`eval-cache false`, the evaluation cache is bypassed entirely for the
invocation. These flags affect only the check path; the actual
`nix flake update` and `nixos-rebuild switch` calls in `run_update()` are
unchanged and continue to use normal caching for build performance.

Note: `--option` arguments must appear before the Nix subcommand (`flake`) on
the command line; they are global nix options, not subcommand flags.
```

---

## 8. Risks and Mitigations

| Risk | Severity | Mitigation |
|------|----------|------------|
| `--option tarball-ttl 0` causes noticeably slower checks on slow or metered connections (re-fetches all tarballs on every check) | Medium | Acceptable: the check path already copies files and runs a full `nix flake update`; re-fetching ensures correctness. Users on metered connections see the metered-connection banner and can choose not to trigger a check. |
| `--option eval-cache false` is not recognised on very old Nix versions (pre-2.4) | Low | Old Nix installations that don't recognise `--option eval-cache false` will fail the `nix` invocation. However, the same installations likely lack `nix-command flakes` experimental features entirely, so they would already fail at the `nix flake` step. Error is caught and surfaced to the user row as before. |
| `any_check_error` query in window.rs borrows `rows` twice (two nested scopes) | Low | Each borrow is scoped and dropped before the next begins. No deadlock risk with `Rc<RefCell<_>>`. Consistent with existing borrow patterns throughout `window.rs`. |
| New string `"Could not check all sources."` is not yet in the gettext `.pot` template | Low | The string is wrapped in `gettext(...)`, consistent with all other UI strings. The `.pot` file is regenerated from source as part of the normal build/translation workflow; no manual `.pot` editing required. |
| Nix argument ordering: `--option` before subcommand | Medium | Verified against Nix CLI documentation: global options must precede the subcommand. The existing `--extra-experimental-features` flag already follows this ordering — the new `--option` flags are inserted in the same position. Covered by existing CI which runs `cargo clippy` and `cargo test`; functional validation should be done on a NixOS test system. |
| `check_errored` stays `true` after user manually retries the backend | None | The retry path calls `set_status_checking()` first (which resets `check_errored` to `false`), then runs a new check. Flag is correctly reset on every retry cycle. |

---

## Implementation Checklist

- [ ] `src/ui/update_row.rs`: Add `check_errored` field to struct
- [ ] `src/ui/update_row.rs`: Initialise `check_errored` in `Self { ... }` constructor
- [ ] `src/ui/update_row.rs`: Add `self.check_errored.set(false)` to `set_status_checking()`
- [ ] `src/ui/update_row.rs`: Add `self.check_errored.set(true)` to `set_status_unknown()`
- [ ] `src/ui/update_row.rs`: Add `pub fn has_check_error(&self) -> bool`
- [ ] `src/ui/window.rs`: Add `any_check_error` binding and `else if any_check_error` branch in check-completion block
- [ ] `src/backends/nix.rs`: Add `--option eval-cache false --option tarball-ttl 0` to `nixos_flake_dry_run_check()`
- [ ] `src/backends/nix.rs`: Add `--option eval-cache false --option tarball-ttl 0` to `nixos_flake_tempdir_check()`
- [ ] `Cargo.toml`: Bump `version` to `"2.0.2"`
- [ ] `data/io.github.up.metainfo.xml`: Prepend `<release version="2.0.2" date="2026-05-14">` entry
- [ ] `releases/2.0.2.md`: Create release notes file
- [ ] `cargo build` — must compile without errors
- [ ] `cargo clippy -- -D warnings` — must produce no warnings
- [ ] `cargo fmt --check` — must pass
- [ ] `cargo test` — all existing tests must pass
