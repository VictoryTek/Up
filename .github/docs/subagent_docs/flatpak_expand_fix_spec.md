# Specification: Fix Flatpak Expander Row Not Showing Updates

**Feature name**: `flatpak_expand_fix`  
**Date**: 2026-04-14  
**Status**: Ready for Implementation

---

## 1. Current State Analysis

### 1.1 Widget Construction (`src/ui/update_row.rs`)

`UpdateRow::new()` creates an `adw::ExpanderRow` for each backend:

```rust
let row = adw::ExpanderRow::builder()
    .title(backend.display_name())
    .subtitle(backend.description())
    .build();
```

No `enable_expansion` is set; it defaults to `true`. This means the expand
arrow (chevron) is **always visible** regardless of whether any child rows
have been added.

`set_packages(&[String])` adds child `adw::ActionRow` widgets via
`self.row.add_row(&pkg_row)`. When called with an empty slice it returns
early without adding any children, but **does not disable the expand
arrow**.

### 1.2 Check Flow (`src/ui/window.rs`)

The `run_checks` closure calls both methods on a background thread and
delivers results to the GTK main loop:

```rust
super::spawn_background_async(move || async move {
    let count = backend_clone.count_available().await;
    let list  = backend_clone.list_available().await;
    let _ = tx.send((count, list)).await;
});
if let Ok((count_result, list_result)) = rx.recv().await {
    match count_result {
        Ok(count) => {
            row.set_status_available(count);      // shows "N available"
            *total_ref.borrow_mut() += count;
        }
        Err(msg) => row.set_status_unknown(&msg),
    }
    match list_result {
        Ok(packages) => row.set_packages(&packages), // adds child rows
        Err(_)       => row.set_packages(&[]),
    }
}
```

### 1.3 Flatpak Backend (`src/backends/flatpak.rs`)

Both `count_available()` and `list_available()` run
`flatpak update --dry-run` (or `flatpak-spawn --host flatpak update
--dry-run` inside the sandbox). They share the first filter — lines whose
trimmed content starts with an ASCII digit — which matches numbered update
entries in the output.

**`count_available`** — just counts matching lines. No text extraction.
Returns the correct value.

**`list_available`** — after the digit filter, it extracts the app name
using `split(']').nth(1)`:

```rust
.filter_map(|l| {
    l.trim()
        .split(']')
        .nth(1)          // ← assumes ']' is present in each line
        .unwrap_or("")
        .split_whitespace()
        .next()
        .map(|s| s.to_string())
})
```

---

## 2. Root Cause

### 2.1 Assumed vs. Actual Flatpak Output Format

The parser assumes the **legacy bracket-checkmark format**:

```
 1. [✓] com.example.App  stable  u  flathub  50.1 MB
```

However, **modern Flatpak** (1.6+, the version shipped on all current
major distributions — Fedora, Ubuntu 22.04+, Arch, Debian Bookworm, Mint)
uses a **plain tabular format without brackets**:

```
        ID                                               Branch         Op         Remote         Download
 1.     com.example.App                                  stable         u          flathub        50.1 MB
 2.     org.gnome.Platform/x86_64/46                     46             u          flathub       900.0 MB
```

When `list_available()` processes a modern output line:

```
" 1.     com.example.App  stable  u  flathub  50.1 MB"
```

- After `trim()` → `"1.     com.example.App  stable  u  flathub  50.1 MB"`
- `split(']')` produces a single element (no `]` in the string)
- `.nth(1)` returns `None`
- `filter_map` discards the line
- The method silently returns `Ok(vec![])` for **every** update

### 2.2 Resulting UI Symptom

| Step | What happens |
|------|--------------|
| `count_available()` returns `N` | `set_status_available(N)` → row shows **"N available"** |
| `list_available()` returns `[]` | `set_packages(&[])` → **zero child rows** added |
| `AdwExpanderRow` default state | `enable-expansion: true` → **expand arrow always visible** |
| User clicks arrow | `expanded` toggles `false → true` → **arrow rotates** |
| No children present | **Nothing appears** — expander looks broken |

### 2.3 Secondary Issue: Arrow Shown with Zero Children

Because `enable-expansion` defaults to `true` and `set_packages()` never
sets it to `false`, the expand arrow is shown even when there are genuinely
zero updates ("Up to date" state). This is misleading.

### 2.4 Potential Tertiary Issue: stdout-only Capture

Both `count_available()` and `list_available()` read only `out.stdout`.
Certain Flatpak builds or older versions write the update table to stderr.
In that scenario, even the fixed `list_available()` parser would receive an
empty string and return no packages while `count_available()` would return
zero (hiding the arrow). This is a separate concern from the primary bug
but should be fixed for robustness.

---

## 3. Proposed Fix

### 3.1 Overview

Two files require changes:

| File | Change |
|------|--------|
| `src/backends/flatpak.rs` | Fix `list_available()` parser; combine stdout+stderr in both check methods |
| `src/ui/update_row.rs` | Control `enable-expansion` from `set_packages()` |

---

### 3.2 `src/backends/flatpak.rs` — Fix `list_available()` Parser

#### Current code (broken)

```rust
fn list_available(
    &self,
) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
    Box::pin(async move {
        let (cmd, args) = build_flatpak_cmd(&["update", "--dry-run"]);
        let out = tokio::process::Command::new(&cmd)
            .args(&args)
            .output()
            .await
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        // Lines format: " 1. [✓] com.app.Name  stable  u  flathub  1.0 MB"
        // Extract app ID from between ']' and first whitespace.
        Ok(text
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.starts_with(|c: char| c.is_ascii_digit())
            })
            .filter_map(|l| {
                l.trim()
                    .split(']')
                    .nth(1)
                    .unwrap_or("")
                    .split_whitespace()
                    .next()
                    .map(|s| s.to_string())
            })
            .filter(|s| !s.is_empty())
            .collect())
    })
}
```

#### Replacement code (fixed)

```rust
fn list_available(
    &self,
) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
    Box::pin(async move {
        let (cmd, args) = build_flatpak_cmd(&["update", "--dry-run"]);
        let out = tokio::process::Command::new(&cmd)
            .args(&args)
            .output()
            .await
            .map_err(|e| e.to_string())?;
        // Combine stdout and stderr: some Flatpak versions write the table
        // to stderr, so reading only stdout would miss all updates.
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let combined = format!("{stdout}{stderr}");
        // Lines are either the modern format (no brackets):
        //   " 1.     com.example.App  stable  u  flathub  50.1 MB"
        // or the legacy bracket format (Flatpak < 1.6):
        //   " 1. [✓] com.example.App  stable  u  flathub  50.1 MB"
        Ok(combined
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.starts_with(|c: char| c.is_ascii_digit())
            })
            .filter_map(|l| {
                let t = l.trim();
                // Strip the leading "N." number prefix (handles 1–N digit numbers).
                let rest = t
                    .trim_start_matches(|c: char| c.is_ascii_digit())
                    .trim_start_matches(['.', '\t', ' ']);
                // Skip optional "[✓]" / "[i]" bracket marker (legacy Flatpak).
                let name_part = if rest.starts_with('[') {
                    rest.splitn(2, ']').nth(1).unwrap_or("").trim()
                } else {
                    rest
                };
                let name = name_part.split_whitespace().next()?;
                if name.is_empty() {
                    None
                } else {
                    Some(name.to_string())
                }
            })
            .collect())
    })
}
```

#### Also update `count_available()` to combine stdout+stderr

Replace:

```rust
let text = String::from_utf8_lossy(&out.stdout);
Ok(text
    .lines()
    ...
```

With:

```rust
let stdout = String::from_utf8_lossy(&out.stdout);
let stderr = String::from_utf8_lossy(&out.stderr);
let combined = format!("{stdout}{stderr}");
Ok(combined
    .lines()
    ...
```

This keeps `count_available()` and `list_available()` consistent in what
they read, preventing a split-brain state where one returns N and the other
silently returns 0 due to a stream mismatch.

---

### 3.3 `src/ui/update_row.rs` — Control `enable-expansion` in `set_packages()`

`AdwExpanderRow::set_enable_expansion(false)` hides the expand chevron and
disables toggling. Call it when the packages list is empty; call it with
`true` when packages exist. This gives correct visual feedback regardless
of the backend:

#### Current code

```rust
pub fn set_packages(&self, packages: &[String]) {
    // Remove previously added package rows to avoid duplicates on re-check.
    {
        let mut tracked = self.pkg_rows.borrow_mut();
        for pkg_row in tracked.drain(..) {
            self.row.remove(&pkg_row);
        }
    }
    if packages.is_empty() {
        return;
    }
    ...
}
```

#### Replacement code

```rust
pub fn set_packages(&self, packages: &[String]) {
    // Remove previously added package rows to avoid duplicates on re-check.
    {
        let mut tracked = self.pkg_rows.borrow_mut();
        for pkg_row in tracked.drain(..) {
            self.row.remove(&pkg_row);
        }
    }
    // Hide the expand arrow when there is nothing to expand.
    self.row.set_enable_expansion(!packages.is_empty());
    if packages.is_empty() {
        return;
    }
    const MAX_PACKAGES: usize = 50;
    let display_count = packages.len().min(MAX_PACKAGES);
    let mut tracked = self.pkg_rows.borrow_mut();
    for pkg in &packages[..display_count] {
        let pkg_row = adw::ActionRow::builder().title(pkg.as_str()).build();
        self.row.add_row(&pkg_row);
        tracked.push(pkg_row);
    }
    if packages.len() > MAX_PACKAGES {
        let remaining = packages.len() - MAX_PACKAGES;
        let more_row = adw::ActionRow::builder()
            .title(format!("\u{2026} and {remaining} more").as_str())
            .build();
        self.row.add_row(&more_row);
        tracked.push(more_row);
    }
}
```

The only addition is the single line:

```rust
self.row.set_enable_expansion(!packages.is_empty());
```

This is placed **after** clearing old rows and **before** the early-return
guard. It is called on every invocation of `set_packages`, so the
`enable-expansion` state always tracks the current package list accurately
on any re-check cycle.

---

## 4. Files to Modify

| File | Section | Change Description |
|------|---------|--------------------|
| `src/backends/flatpak.rs` | `FlatpakBackend::list_available()` | Replace `split(']').nth(1)` parser with digit-strip + optional-bracket parser; combine stdout+stderr |
| `src/backends/flatpak.rs` | `FlatpakBackend::count_available()` | Combine stdout+stderr for consistency |
| `src/ui/update_row.rs` | `UpdateRow::set_packages()` | Add `self.row.set_enable_expansion(!packages.is_empty())` |

No changes are required to `window.rs`, `mod.rs`, `app.rs`, `runner.rs`,
`upgrade_page.rs`, or any backend besides `flatpak.rs`.

---

## 5. Implementation Steps

1. Open `src/backends/flatpak.rs`.
2. In `count_available()`: replace the single `let text = String::from_utf8_lossy(&out.stdout);` line with the three-line stdout+stderr combine block.
3. In `list_available()`: replace the comment and the entire `filter_map` closure (lines using `split(']')`) with the new multi-format parser; update the comment above describing the line formats; replace the stdout-only `let text = ...` with the stdout+stderr combine block.
4. Open `src/ui/update_row.rs`.
5. In `set_packages()`: insert `self.row.set_enable_expansion(!packages.is_empty());` directly after the block that removes old `pkg_rows`, before the `if packages.is_empty() { return; }` guard.
6. Run `cargo build` to confirm compilation.
7. Run `cargo clippy -- -D warnings` to confirm no new warnings.
8. Run `cargo fmt --check` to confirm formatting.

---

## 6. Risks and Edge Cases

### 6.1 Flatpak Output Format Variations

| Scenario | Handled? |
|----------|----------|
| Modern format, no brackets (Flatpak 1.6+) | ✅ Fixed by new parser |
| Legacy bracket-checkmark format (`[✓]`) | ✅ Kept by bracket-skip branch |
| Tab-separated columns instead of spaces | ✅ `trim_start_matches` strips tabs; `split_whitespace` handles tabs |
| Multi-digit update numbers (`10.`, `100.`) | ✅ `trim_start_matches(digit)` handles any digit count |
| Header/footer lines (e.g. "Looking for updates…") | ✅ Filtered out by digit-start guard |
| Line starting with digit but no app name after stripping | ✅ Returns `None` from `filter_map` via the `is_empty` check |
| Updates table on stderr only | ✅ Fixed by combining stdout+stderr |

### 6.2 `set_enable_expansion` State on Re-Check

When the user triggers a new check cycle, `set_status_checking()` is
called, which shows the spinner and "Checking..." label. The
`enable_expansion` state is **not reset** during checking — the arrow
remains in whatever state it was from the last check. This is acceptable:
if the previous check found packages, the expander stays open; if there
were none, the arrow stays hidden. On completion, `set_packages()` will
always set the definitive state.

If a future UX iteration wants to collapse and hide the arrow during
re-checks, `set_status_checking()` can be updated to add:

```rust
self.row.set_enable_expansion(false);
self.row.set_expanded(false);
```

This is **not included** in the current fix to keep the change minimal and
focused.

### 6.3 No Impact on Other Backends

`set_packages()` is called for all backends (APT, DNF, Pacman, Zypper,
Homebrew, Nix). The `set_enable_expansion` change benefits all backends:
any backend that returns an empty package list (e.g. Nix, which always
returns `Ok(vec![])`) will now correctly hide its expand arrow instead of
showing a non-functional chevron.

### 6.4 `adw::ExpanderRow::set_enable_expansion` API Availability

`enable-expansion` is a stable property of `AdwExpanderRow` available
since libadwaita 1.0. The project targets libadwaita 0.7 (Rust crate) with
feature flag `v1_5`, and the GTK runtime requirement is libadwaita ≥ 1.0.
The method is available in the Rust bindings as
`adw::prelude::ExpanderRowExt::set_enable_expansion()` (re-exported via
`adw::prelude::*`, which is already imported in `update_row.rs`).

### 6.5 `flatpak update --dry-run` Exit Code

On some systems or Flatpak versions, `--dry-run` exits with a non-zero
code even when there are pending updates, or when nothing needs updating.
Both `list_available()` and `count_available()` use
`tokio::process::Command::output()` and check only the parsed text — they
do **not** fail on non-zero exit codes (they return the parsed data from
stdout/stderr). This means a non-zero exit code is silently ignored, which
is the correct behaviour for a read-only preflight check.

---

## 7. Summary

The bug is caused by a **parser mismatch** in `FlatpakBackend::list_available()`:
the code assumes `[✓]` bracket markers that are absent from modern Flatpak
output. `count_available()` returns the correct count (N) but
`list_available()` returns an empty list, causing `set_packages(&[])` to
add zero children to the `AdwExpanderRow`. Because `enable-expansion`
defaults to `true`, the expand arrow is always shown. Clicking it toggles
the row's `expanded` state (arrow animates) but reveals nothing.

The fix requires **three targeted changes** across two files:

1. Update the `filter_map` in `list_available()` to handle modern
   Flatpak output (strip numeric prefix, optionally skip bracket marker).
2. Combine stdout+stderr in both `count_available()` and
   `list_available()` to capture output regardless of the Flatpak version's
   stream routing.
3. Add `self.row.set_enable_expansion(!packages.is_empty())` in
   `set_packages()` to hide the expand arrow when there are no child rows.
