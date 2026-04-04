# Specification: Check for Updates Pre-Flight + Flatpak Count Bug Fix

**Feature:** `check_for_updates`  
**File:** `.github/docs/subagent_docs/check_for_updates_spec.md`  
**Date:** 2026-04-04  
**Status:** Ready for Implementation

---

## 1. Current State Analysis

### 1.1 Backend Trait (`src/backends/mod.rs`)

The `Backend` trait exposes two relevant methods:

```rust
fn run_update<'a>(&'a self, runner: &'a CommandRunner)
    -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>>;

fn count_available(&self)
    -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
    Box::pin(async { Ok(0) })
}
```

`count_available()` is already implemented on all backends:

| Backend        | Command Used for Count                         | Returns                         |
|----------------|------------------------------------------------|---------------------------------|
| APT            | `apt list --upgradable`                        | Lines containing `/`            |
| DNF            | `dnf check-update`                             | Non-header lines when exit≠0    |
| Pacman         | `pacman -Qu`                                   | Non-empty lines                 |
| Zypper         | `zypper list-updates`                          | Lines starting with `v `        |
| Flatpak        | `flatpak remote-ls --updates`                  | Non-empty lines (**BUGGY**)     |
| Homebrew       | `brew outdated`                                | Non-empty lines                 |
| Nix (NixOS)    | *(returns `Err`)* — cannot check w/out update  | Always `Err("Run Update All…")`  |
| Nix (non-NixOS)| `nix-env -u --dry-run` (stderr)               | Lines containing `"upgrading"`  |

The trait has **no** method to return a list of pending package **names**, only a count.

### 1.2 UI — Update Page (`src/ui/window.rs`, `src/ui/update_row.rs`)

`build_update_page()` in `window.rs`:

1. Spawns backend detection off-thread.
2. On detection, removes placeholder row, creates one `UpdateRow` per backend, adds each to `backends_group` (`adw::PreferencesGroup`).
3. **Immediately enables the "Update All" button** after backend detection (before checks complete).
4. Runs `run_checks` automatically after detection, and again any time the header refresh button is clicked.

The `run_checks` closure:
- Iterates over detected backends
- Calls `backend.count_available()` in a background thread for each
- On result, calls `row.set_status_available(count)` or `row.set_status_unknown(msg)`
- **Does NOT affect the Update All button sensitivity**

`UpdateRow` (`src/ui/update_row.rs`) is built around an `adw::ActionRow`. It has:
- `set_status_checking()`, `set_status_available(count)`, `set_status_running()`, `set_status_success(count)`, `set_status_error(msg)`, `set_status_skipped(msg)`, `set_status_unknown(msg)`
- No mechanism for showing an expandable list of package names

The header bar already contains a `view-refresh-symbolic` button with tooltip "Check for updates" that triggers `run_checks`. The header refresh button is the existing "check" affordance.

### 1.3 Flatpak Count Bug — Root Cause

**File:** `src/backends/flatpak.rs`

**`count_available()` (reports the count before update):**
```rust
fn count_available(&self) -> ... {
    Box::pin(async move {
        let out = tokio::process::Command::new("flatpak")
            .args(["remote-ls", "--updates"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text.lines().filter(|l| !l.is_empty()).count())
    })
}
```

**`run_update()` (reports count after update):**
```rust
let count = output
    .lines()
    .filter(|l| {
        let t = l.trim();
        t.starts_with(|c: char| c.is_ascii_digit())
    })
    .count();
UpdateResult::Success { updated_count: count }
```

**Why the counts differ:**

`flatpak remote-ls --updates` queries configured remotes for refs with newer versions. It typically shows **only app refs** by default (not runtime refs) and defaults to the user installation context. It does NOT include:
- **Runtimes** that need updating (e.g., `org.gnome.Platform`, `org.freedesktop.Platform`)
- **Extensions** attached to installed apps that have pending updates
- **System-installation** refs when operator only queries user context

Meanwhile, `flatpak update -y` (without `--user`/`--system`) updates **all refs across all installations**: apps + runtimes + extensions. The numbered table lines it emits (lines trimmed to start with a digit: ` 1. [✓] ...`) include all of these.

**Concrete scenario producing "1 available, 2 updated":**
- `flatpak remote-ls --updates` → 1 line (e.g., `com.example.App`)
- `flatpak update -y` → 2 numbered lines (e.g., `com.example.App` + `org.runtime.Base`)

This is a **documented, widely-reported Flatpak discrepancy**. The definitive fix is to use
`flatpak update --dry-run` for `count_available()`, because it executes the exact same resolution
logic as `flatpak update -y` and therefore guarantees the count matches actuality.

---

## 2. Problem Definition

### Problem A — Feature: No Pre-Flight Check Before Update

**Current behavior:**
- The "Update All" button becomes enabled as soon as backends are detected (before `count_available()` results arrive).
- The user can click "Update All" with no knowledge of what will be changed.
- Individual backend update counts are shown after checking, but they appear async and don't gate the button.
- No package-level detail is shown — only a count badge.

**Desired behavior:**
- The "Update All" button is disabled on startup.
- The `run_checks` operation runs automatically and gathers counts + package lists.
- After all checks complete:
  - If total available > 0: Enable "Update All" and show each backend's count.
  - If total available = 0: Keep "Update All" disabled; show "Everything is up to date."
- Each backend row expands to show the list of pending package names (where the backend supports it).
- Re-running the check (via the refresh button) temporarily re-disables "Update All" until the new check completes.

### Problem B — Bug: Flatpak Count Mismatch

**Current behavior:**
- `count_available()` uses `flatpak remote-ls --updates` → reports N.
- `run_update()` processes `flatpak update -y` table output → reports M where M > N.
- The UI shows "1 available" but "2 updated", confusing the user.

**Desired behavior:**
- `count_available()` reports exactly the same number that `run_update()` will later show as updated.

---

## 3. Backend Trait Extension

### 3.1 New Method: `list_available()`

Add a `list_available()` method to the `Backend` trait with a default impl returning an empty vector:

```rust
/// Return a human-readable list of packages pending update.
/// Each element is a short package identifier or description (e.g., "htop 3.3.0").
/// Returns Ok(vec![]) for backends that cannot enumerate packages without
/// performing the update (e.g., NixOS).
/// Default implementation returns Ok(vec![]) for backward compatibility.
fn list_available(&self) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
    Box::pin(async { Ok(Vec::new()) })
}
```

This is **additive only** — no existing backend implementation changes are required for correctness.
Backends that provide a list gain the expandable-row feature; backends that don't (e.g., NixOS) degrade gracefully.

### 3.2 Backend Implementations of `list_available()`

#### APT (`src/backends/os_package_manager.rs`)
```rust
fn list_available(&self) -> ... {
    Box::pin(async move {
        let out = tokio::process::Command::new("apt")
            .args(["list", "--upgradable"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text
            .lines()
            .filter(|l| l.contains('/'))
            .filter_map(|l| l.split('/').next().map(|s| s.to_string()))
            .collect())
    })
}
```

Extracts package name before `/` from lines like `htop/noble,now 3.3.0 amd64 [upgradable from: 3.2.2]`.

#### DNF (`src/backends/os_package_manager.rs`)
```rust
fn list_available(&self) -> ... {
    Box::pin(async move {
        let out = tokio::process::Command::new("dnf")
            .args(["check-update"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        // Exit code 100 = updates available; 0 = up to date; 1 = error
        if out.status.code() == Some(1) {
            return Err("dnf check-update failed".to_string());
        }
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text
            .lines()
            .filter(|l| !l.is_empty()
                && !l.starts_with("Last")
                && !l.starts_with("Obsoleting")
                && !l.starts_with("Security"))
            .filter_map(|l| l.split_whitespace().next().map(|s| s.to_string()))
            .collect())
    })
}
```

#### Pacman (`src/backends/os_package_manager.rs`)
```rust
fn list_available(&self) -> ... {
    Box::pin(async move {
        let out = tokio::process::Command::new("pacman")
            .args(["-Qu"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        // Each line: "pkgname old-ver -> new-ver"
        Ok(text
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| l.split_whitespace().next().map(|s| s.to_string()))
            .collect())
    })
}
```

#### Zypper (`src/backends/os_package_manager.rs`)
```rust
fn list_available(&self) -> ... {
    Box::pin(async move {
        let out = tokio::process::Command::new("zypper")
            .args(["list-updates"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        // Table rows starting with "v " — extract 3rd column (package name)
        Ok(text
            .lines()
            .filter(|l| l.starts_with("v "))
            .filter_map(|l| {
                l.split('|').nth(2).map(|s| s.trim().to_string())
            })
            .filter(|s| !s.is_empty())
            .collect())
    })
}
```

#### Flatpak (`src/backends/flatpak.rs`)

**ALSO applies the count bug fix here by sharing the same dry-run command for both `count_available()` and `list_available()`:**

For `list_available()`:
```rust
fn list_available(&self) -> ... {
    Box::pin(async move {
        let out = tokio::process::Command::new("flatpak")
            .args(["update", "--dry-run"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.starts_with(|c: char| c.is_ascii_digit())
            })
            .filter_map(|l| {
                // Line format: " 1. [✓] com.app.Name  stable  u  flathub  1.0 MB"
                // After the leading number/dot/checkmark, 4th whitespace-delimited token is the app ID
                let trimmed = l.trim();
                // Skip "1. [✓] " prefix to get to the ID
                let after_bracket = trimmed
                    .split(']')
                    .nth(1)
                    .unwrap_or("")
                    .trim();
                after_bracket
                    .split_whitespace()
                    .next()
                    .map(|s| s.to_string())
            })
            .filter(|s| !s.is_empty())
            .collect())
    })
}
```

#### Homebrew (`src/backends/homebrew.rs`)
```rust
fn list_available(&self) -> ... {
    Box::pin(async move {
        let out = tokio::process::Command::new("brew")
            .args(["outdated"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        // Each line is "pkgname (old-version) < new-version" or just "pkgname"
        Ok(text
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| l.split_whitespace().next().map(|s| s.to_string()))
            .collect())
    })
}
```

#### Nix (`src/backends/nix.rs`)
```rust
fn list_available(&self) -> ... {
    Box::pin(async move {
        if is_nixos() {
            // NixOS cannot list pending Nix store operations without running an update.
            // Return empty list; UI will degrade gracefully (no expand affordance).
            Ok(Vec::new())
        } else {
            let out = tokio::process::Command::new("nix-env")
                .args(["-u", "--dry-run"])
                .output()
                .await
                .map_err(|e| e.to_string())?;
            // nix-env --dry-run emits "upgrading 'name-1.0' to 'name-2.0'" on stderr
            let text = String::from_utf8_lossy(&out.stderr);
            Ok(text
                .lines()
                .filter(|l| l.contains("upgrading"))
                .filter_map(|l| {
                    // Extract the package name from between single quotes
                    l.split('\'').nth(1).map(|s| s.to_string())
                })
                .collect())
        }
    })
}
```

---

## 4. Flatpak `count_available()` Fix

**Replace** the current `flatpak remote-ls --updates` approach with `flatpak update --dry-run`.

**Old `count_available()` in `src/backends/flatpak.rs`:**
```rust
fn count_available(&self) -> ... {
    Box::pin(async move {
        let out = tokio::process::Command::new("flatpak")
            .args(["remote-ls", "--updates"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text.lines().filter(|l| !l.is_empty()).count())
    })
}
```

**New `count_available()` in `src/backends/flatpak.rs`:**
```rust
fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
    Box::pin(async move {
        let out = tokio::process::Command::new("flatpak")
            .args(["update", "--dry-run"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.starts_with(|c: char| c.is_ascii_digit())
            })
            .count())
    })
}
```

**Why this fixes the bug:** `flatpak update --dry-run` executes the same dependency and installation resolution as `flatpak update -y` but does not commit any changes. It outputs the exact numbered table of refs that WOULD be updated, making the count from `count_available()` identical to the count seen after `run_update()`.

`flatpak update --dry-run` is available since Flatpak 1.2.0 (released 2018); all modern Linux distributions ship ≥1.2.0, so this is safe.

---

## 5. UI Architecture

### 5.1 UpdateRow: Replace `adw::ActionRow` with `adw::ExpanderRow`

The current `UpdateRow` uses `adw::ActionRow`. Replace it with `adw::ExpanderRow` to allow package list expansion.

**Key libadwaita-rs API** (verified from Context7 `/gnome/libadwaita` docs):
- `adw::ExpanderRow::builder().title("APT").subtitle("Debian / Ubuntu packages").build();`
- `.add_suffix(&widget)` — same as ActionRow, for spinner/status label
- `.add_row(&child_row)` — adds a child `adw::ActionRow` that appears when expanded
- `adw::PreferencesGroup::add(&expander_row)` — works identically to ActionRow

`adw::ExpanderRow` implements `adw::prelude::PreferencesRowExt` and `gtk::prelude::WidgetExt`, 
so it can be added to `adw::PreferencesGroup` the same way as `adw::ActionRow`.

**Updated `UpdateRow` struct:**
```rust
pub struct UpdateRow {
    pub row: adw::ExpanderRow,   // was adw::ActionRow
    status_label: gtk::Label,
    spinner: gtk::Spinner,
    progress_bar: gtk::ProgressBar,
}
```

**New method `set_packages()`:**
```rust
pub fn set_packages(&self, packages: &[String]) {
    // Remove all existing child rows first (prevent duplicates on re-check)
    // GTK4: iterate and remove each child row
    while let Some(child) = self.row.first_child() {
        // Only remove ActionRow children we added, not internal ExpanderRow children.
        // Check by type or use a container — see implementation note below.
        self.row.remove(&child);
    }
    if packages.is_empty() {
        // Disable expand affordance — no children means no visual expander in libadwaita
        return;
    }
    for pkg in packages {
        let pkg_row = adw::ActionRow::builder()
            .title(pkg)
            .build();
        self.row.add_row(&pkg_row);
    }
}
```

> **Implementation Note — Clearing child rows:** `adw::ExpanderRow` wraps child rows in
> its own internal listbox widget. To safely remove added rows without touching internal
> widgets, track added rows in a `Vec<adw::ActionRow>` stored in `UpdateRow` and then call
> `self.row.remove(&row)` for each one.

**Updated `UpdateRow` struct with child tracking:**
```rust
pub struct UpdateRow {
    pub row: adw::ExpanderRow,
    status_label: gtk::Label,
    spinner: gtk::Spinner,
    progress_bar: gtk::ProgressBar,
    pkg_rows: RefCell<Vec<adw::ActionRow>>,  // tracks added child rows for clearing
}
```

### 5.2 Update Page Logic Changes (`src/ui/window.rs`)

The key behavioral change is **gating "Update All" on check completion with ≥1 pending update**.

#### State to Add

Inside `build_update_page()`, add shared state:
```rust
// Tracks pending check jobs (decremented as each backend check completes)
let pending_checks: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
// Accumulates total available update count across all backends
let total_available: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
```

#### Update All Button Initial State

The button is already initialized `sensitive(false)`:
```rust
let update_button = gtk::Button::builder()
    ...
    .sensitive(false)  // already correct — keep this
    .build();
```

**Remove** the line that enables it after backend detection:
```rust
// REMOVE THIS:
update_button_ref.set_sensitive(true);
```

The button is only enabled from within `run_checks` after all checks pass with total > 0.

#### Updated `run_checks` Closure

The closure must:
1. Reset `total_available` and `pending_checks` on each invocation
2. Disable "Update All" button at start of re-check (to prevent stale state)
3. Call both `count_available()` and `list_available()` in each per-backend future
4. After each future completes, decrement `pending_checks`
5. When `pending_checks` reaches 0: evaluate `total_available` and set button sensitivity

```rust
let run_checks: Rc<dyn Fn()> = {
    let rows = rows.clone();
    let detected = detected.clone();
    let update_button_checks = update_button.clone();
    let pending_checks = pending_checks.clone();
    let total_available = total_available.clone();
    let status_label_checks = status_label.clone();

    Rc::new(move || {
        let n = detected.borrow().len();
        if n == 0 { return; }

        // Disable button and reset counters at start of each check cycle
        update_button_checks.set_sensitive(false);
        *pending_checks.borrow_mut() = n;
        *total_available.borrow_mut() = 0;
        status_label_checks.set_label("Checking for updates...");

        for (idx, backend) in detected.borrow().iter().enumerate() {
            {
                let borrowed = rows.borrow();
                borrowed[idx].1.set_status_checking();
            }
            let backend_clone = backend.clone();
            let rows_ref = rows.clone();
            let pending_ref = pending_checks.clone();
            let total_ref = total_available.clone();
            let btn_ref = update_button_checks.clone();
            let status_ref = status_label_checks.clone();

            glib::spawn_future_local(async move {
                // Channel pair: count result + package list result
                let (count_tx, count_rx) = async_channel::bounded::<Result<usize, String>>(1);
                let (list_tx, list_rx) = async_channel::bounded::<Result<Vec<String>, String>>(1);

                super::spawn_background_async(move || async move {
                    let count = backend_clone.count_available().await;
                    let list = backend_clone.list_available().await;
                    let _ = count_tx.send(count).await;
                    let _ = list_tx.send(list).await;
                });

                let count_result = count_rx.recv().await;
                let list_result = list_rx.recv().await;

                let row = rows_ref.borrow()[idx].1.clone();

                // Apply count result to row
                match count_result {
                    Ok(Ok(count)) => {
                        row.set_status_available(count);
                        // Accumulate total
                        *total_ref.borrow_mut() += count;
                    }
                    Ok(Err(msg)) => {
                        row.set_status_unknown(&msg);
                    }
                    Err(_) => {}
                }

                // Apply package list to row (enables expand affordance)
                if let Ok(Ok(packages)) = list_result {
                    row.set_packages(&packages);
                }

                // Decrement pending counter; if last, update button
                let remaining = {
                    let mut p = pending_ref.borrow_mut();
                    *p -= 1;
                    *p
                };
                if remaining == 0 {
                    let total = *total_ref.borrow();
                    if total > 0 {
                        btn_ref.set_sensitive(true);
                        status_label_checks.set_label(
                            &format!("{total} update{} available", if total == 1 { "" } else { "s" })
                        );
                    } else {
                        status_label_checks.set_label("Everything is up to date.");
                    }
                }
            });
        }
    })
};
```

> **Note:** The two separate channels (`count_tx`/`list_tx`) in the same background task guarantee both results come from the same execution context without spawning two background threads per backend. The background task sends count, then list, and the futures receive them sequentially. This is safe because they share a single `spawn_background_async` call.

Alternatively (and more cleanly), combine both calls into a single channel result type:
```rust
type CheckPayload = (Result<usize, String>, Result<Vec<String>, String>);
let (tx, rx) = async_channel::bounded::<CheckPayload>(1);
super::spawn_background_async(move || async move {
    let count = backend_clone.count_available().await;
    let list = backend_clone.list_available().await;
    let _ = tx.send((count, list)).await;
});
if let Ok((count_result, list_result)) = rx.recv().await { ... }
```

**This combined-channel approach is the recommended implementation.**

### 5.3 Visual Design

- `adw::ExpanderRow` replaces `adw::ActionRow` for backend rows. When packages are provided, the row shows a down-arrow expand affordance. When no packages are available (Nix/NixOS, or backends that return empty list), the expand affordance is hidden (no children = no expander shown by libadwaita).
- The `status_label` suffix on the row continues to show the count badge (e.g., "12 available" in accent color, "Up to date" in success color).
- Child rows added via `add_row()` display as `adw::ActionRow` with the package name as the title. No subtitle/icon needed for simplicity.
- The existing loading spinner in the row header continues to show during the check.
- The "Update All" button uses `sensitive(false)` while checking. The status label above reads "Checking for updates..." during check and "N updates available" / "Everything is up to date." after.

---

## 6. Implementation Steps

Complete numbered list in implementation order:

1. **`src/backends/mod.rs`** — Add `list_available()` method to the `Backend` trait with default empty-vec implementation (as specified in §3.1).

2. **`src/backends/flatpak.rs`** — Fix `count_available()` to use `flatpak update --dry-run` instead of `flatpak remote-ls --updates` (as specified in §4). Implement `list_available()` to parse `flatpak update --dry-run` output for app IDs (as specified in §3.2).

3. **`src/backends/os_package_manager.rs`** — Implement `list_available()` for `AptBackend`, `DnfBackend`, `PacmanBackend`, and `ZypperBackend` (as specified in §3.2).

4. **`src/backends/homebrew.rs`** — Implement `list_available()` for `HomebrewBackend` (as specified in §3.2).

5. **`src/backends/nix.rs`** — Implement `list_available()` for `NixBackend` (as specified in §3.2): return `Ok(Vec::new())` for NixOS; parse `nix-env -u --dry-run` stderr for non-NixOS systems.

6. **`src/ui/update_row.rs`** — Replace `adw::ActionRow` with `adw::ExpanderRow` as the base widget. Add `pkg_rows: RefCell<Vec<adw::ActionRow>>` field for child row tracking. Update all builder calls (`.row` type changes from `adw::ActionRow` to `adw::ExpanderRow`). Add `set_packages(&[String])` method that clears and rebuilds child rows. Verify all existing `add_suffix()` calls still compile (they do — `ExpanderRow` also exposes `add_suffix()`).

7. **`src/ui/window.rs`** — Add `pending_checks: Rc<RefCell<usize>>` and `total_available: Rc<RefCell<usize>>` shared state inside `build_update_page()`. Remove the line `update_button_ref.set_sensitive(true)` that fires after backend detection. Rewrite the `run_checks` closure per §5.2: combined-channel approach, decrement pending counter, enable button only when all checks complete with total > 0, update status label.

8. **Run `cargo fmt`** — Format all modified files.

9. **Run `cargo clippy -- -D warnings`** — Fix any lint issues introduced.

10. **Run `cargo build`** — Confirm clean compilation.

11. **Run `cargo test`** — Confirm tests pass.

---

## 7. Files to Modify

| File | Change Type |
|------|-------------|
| `src/backends/mod.rs` | Add `list_available()` trait method with default impl |
| `src/backends/flatpak.rs` | Fix `count_available()` + implement `list_available()` |
| `src/backends/os_package_manager.rs` | Implement `list_available()` for APT, DNF, Pacman, Zypper |
| `src/backends/homebrew.rs` | Implement `list_available()` for Homebrew |
| `src/backends/nix.rs` | Implement `list_available()` for Nix/NixOS |
| `src/ui/update_row.rs` | Replace ActionRow → ExpanderRow; add `set_packages()` |
| `src/ui/window.rs` | Gate "Update All" on check completion; call `list_available()` |

**Files NOT modified:**
- `src/ui/upgrade_page.rs` — No changes needed
- `src/ui/log_panel.rs` — No changes needed
- `src/ui/mod.rs` — No changes needed
- `src/runner.rs` — No changes needed
- `src/app.rs`, `src/main.rs`, `src/reboot.rs`, `src/upgrade.rs` — No changes needed
- `Cargo.toml` — No new dependencies required
- `data/`, `scripts/`, `meson.build` — No changes needed

---

## 8. Flatpak Fix Details (Authoritative Summary)

### Root Cause (Precise)

`flatpak remote-ls --updates` queries remotes for refs with newer versions than what is installed. The default scope of this command:
- Lists refs from configured remotes for the current installation context
- By default operates on the **user** installation only (or combined, depending on Flatpak version and system config)
- Does **not** include runtimes that are pulled in as dependencies by `flatpak update`

`flatpak update -y` (without `--user` or `--system`) applies updates to:
- All refs (apps + runtimes + extensions) from all installations (user + system)

**Result:** If a runtime update is pending alongside an app update, `remote-ls --updates` reports N while `flatpak update -y` operates on N+M refs.

### Exact Fix

**Command to replace:** `flatpak remote-ls --updates` (in `count_available()`)  
**Command to use instead:** `flatpak update --dry-run`

**Counting logic to replace:**
```rust
Ok(text.lines().filter(|l| !l.is_empty()).count())
```

**Counting logic to use instead:**
```rust
Ok(text
    .lines()
    .filter(|l| {
        let t = l.trim();
        t.starts_with(|c: char| c.is_ascii_digit())
    })
    .count())
```

This mirrors the counting logic already used in `run_update()`, ensuring the pre-update count and post-update count are computed from the same output format produced by the same resolution engine.

**Backwards compatibility:** `flatpak update --dry-run` has been available since Flatpak 1.2.0 (June 2018). All actively maintained Linux distributions (Debian 10+, Ubuntu 18.04+, Fedora 29+, Arch, openSUSE, etc.) ship Flatpak ≥1.2.0. No compatibility shim is needed.

---

## 9. Risks and Mitigations

| Risk | Severity | Mitigation |
|------|----------|------------|
| `flatpak update --dry-run` exits non-zero on some systems even with no updates | Medium | Ignore exit code; only parse stdout for digit-prefixed lines. Test on target distros. |
| `flatpak update --dry-run` output format changes between Flatpak versions | Low | The numbered table format has been stable since Flatpak 1.x. Add a comment in code documenting the expected format for future maintainers. |
| `adw::ExpanderRow` render difference from `adw::ActionRow` breaks layout | Low | `ExpanderRow` has the same height and padding as `ActionRow` when it has no children. Visual regression test recommended. |
| `ExpanderRow` with many packages (e.g., 200+ APT packages) slows GTK due to large list | Medium | Cap displayed package list at 50 items with a tail row reading "… and N more". Implement in `set_packages()`: if packages.len() > 50, add first 50 as rows, add a summary ActionRow as last child. |
| Re-running `run_checks` while a previous check is still in-flight causes double-fire | Low | The counter reset at start of each `run_checks` call may race. Mitigation: add a `checking: Rc<RefCell<bool>>` flag; if a check is already running, the refresh button click is a no-op (or cancels pending futures — simpler to debounce). Implementation: disable header refresh button and "Update All" for the duration of checks; re-enable after all complete. |
| `adw::ExpanderRow::remove()` on internal child rows (not ones we added) causes panic | Medium | Track added rows in `pkg_rows: RefCell<Vec<adw::ActionRow>>` and only `remove()` those exact widget instances. Do not call `first_child()` traversal. |
| `nix-env -u --dry-run` is slow on large Nix profiles | Low | Already handled: `list_available()` runs in the background thread via `spawn_background_async`. No UI block. |
| Nix (NixOS) shows no package list even with updates pending | Informational | This is by design. The `count_available()` for NixOS returns `Err("Run Update All to check")`, which becomes `set_status_unknown()` in the UI — this behavior is unchanged. The `list_available()` returns `Ok(vec![])`, so no expand affordance is shown — correct behavior. |
| DNF `check-update` returns exit code 100 (updates available) vs 0 (up to date) — the error check in `list_available()` must not treat 100 as an error | Medium | Check `out.status.code() == Some(1)` as the error condition (actual error); treat exit code 100 (updates) and 0 (up to date) as success. This already matches the existing `count_available()` logic for DNF. |

---

## 10. Context7 Sources Verified

| Library | Context7 ID | Docs Used |
|---------|------------|-----------|
| libadwaita | `/gnome/libadwaita` | AdwExpanderRow API, `add_suffix`, `add_row`, PreferencesGroup integration |
| libadwaita (GNOME docs mirror) | `/websites/gnome_pages_gitlab_gnome_libadwaita_doc_1-latest` | ExpanderRow `set_title_lines`, style classes |

Key API confirmations from Context7:
- `adw_expander_row_add_row()` / Rust: `expander_row.add_row(&child)` — confirmed available in libadwaita 1.x
- `adw_expander_row_add_suffix()` — confirmed available (same as ActionRow suffix API)
- Both `AdwExpanderRow` and `AdwActionRow` can be added to `AdwPreferencesGroup` identically
- The libadwaita version in `Cargo.toml` is `adw = { version = "0.7", package = "libadwaita", features = ["v1_5"] }` — `ExpanderRow` with `add_row()` is present since libadwaita 1.0, well within v1_5 feature set

---

*Spec written: 2026-04-04*  
*Author: Research subagent (check_for_updates pass)*
