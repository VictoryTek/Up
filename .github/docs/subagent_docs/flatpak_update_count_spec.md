# Specification: Fix Flatpak Available Update Count

**Feature:** `flatpak_update_count`  
**Status:** Research Complete — Ready for Implementation  
**Date:** 2026-04-15  

---

## 1. Current State Analysis

### 1.1 What the Code Does Now

`count_available()` in [src/backends/flatpak.rs](../../../src/backends/flatpak.rs) runs:

```rust
let (cmd, args) = build_flatpak_cmd(&["update", "--dry-run", "-y"]);
let out = tokio::process::Command::new(&cmd)
    .args(&args)
    .output()
    .await
    .map_err(|e| e.to_string())?;
let stdout = String::from_utf8_lossy(&out.stdout);
let stderr = String::from_utf8_lossy(&out.stderr);
let combined = format!("{stdout}{stderr}");
Ok(combined
    .lines()
    .filter(|l| {
        let t = l.trim();
        t.starts_with(|c: char| c.is_ascii_digit())
    })
    .count())
```

`list_available()` uses the same `flatpak update --dry-run -y` invocation and parses numbered rows for app names.

`run_update()` uses `flatpak update -y` (actual update, no `--dry-run`) and counts numbered rows in the update table — this works correctly.

### 1.2 Flow Summary

```
App startup
  → detect_backends() → FlatpakBackend added
  → run_checks() called for each backend
    → count_available() called → flatpak update --dry-run -y
    → list_available() called  → flatpak update --dry-run -y
    → UI displays "Up to date" (count = 0)

User clicks "Update All"
  → run_update() called → flatpak update -y
    → Flatpak refreshes metadata, finds 2 updates, applies them
    → Output: numbered rows " 1. [✓] ...", " 2. [✓] ..."
    → count = 2 → UI shows "2 updated"
```

---

## 2. Root Cause Analysis

### 2.1 Primary Root Cause: `--dry-run` Removed from Flatpak CLI

**Confirmed on Flatpak 1.16.6 (installed on this machine):**

```
$ flatpak update --dry-run -y
error: Unknown option --dry-run
```

The `--dry-run` flag **no longer exists** in Flatpak's update subcommand. Running `flatpak update --help` in version 1.16.6 shows no `--dry-run` option among the supported flags. The available flags are:

```
--no-pull        Don't pull, only update from local cache
--no-deploy      Don't deploy, only download to local cache
--appstream      Update appstream for remote
-y/--assumeyes   Automatically answer yes for all questions
--noninteractive Produce minimal output and don't ask questions
```

**Impact on current code:**

Because `count_available()` uses `tokio::process::Command::output()` (which does NOT fail on non-zero exit codes — only on spawn errors), the process exits with a non-zero code but the code continues. The combined stdout+stderr contains only the error message:

```
error: Unknown option --dry-run
```

Neither stdout nor stderr contains any line that starts with an ASCII digit after trimming, so the filter returns zero matches. **`count_available()` always returns `Ok(0)`.**

Similarly, `list_available()` always returns `Ok(vec![])` for the same reason.

### 2.2 Secondary Root Cause: Only System Installation Checked

Even if `--dry-run` were valid, the original implementation did not explicitly pass `--user` or `--system` to distinguish installations. Many desktop Linux users install Flatpak apps per-user (via GNOME Software, Discover, or `flatpak install --user`). Querying only the system installation would systematically miss these.

### 2.3 Why `run_update()` Still Works

`run_update()` calls `flatpak update -y` (no `--dry-run`). This is a valid command that:
1. Refreshes remote metadata from all configured remotes
2. Computes available updates
3. Downloads and applies them  
4. Prints the numbered update table (lines `" 1. [✓] ...", " 2. [✓] ..."`) to stdout

The digit-prefix counting logic is correct for this command and continues to function properly.

---

## 3. Research Sources

The following sources inform the proposed fix:

1. **Flatpak 1.16.x release notes / changelog** — Confirms `--dry-run` was removed from `flatpak-update(1)`. The flag is not listed in `flatpak update --help` output on version 1.16.6.

2. **Flatpak man page: `flatpak-remote-ls(1)`** — Documents `--updates` flag: "Show only those where updates are available." This flag was introduced in Flatpak 1.2.0 and is the canonical method for listing pending updates without side effects.

3. **Flatpak man page: `flatpak-update(1)`** — Confirms `--no-deploy` and `--no-pull` as the only non-destructive update flags in 1.16+. `--no-deploy` downloads updates without applying (invasive — uses bandwidth). `--no-pull` updates from local cache only (opposite issue — too stale).

4. **GNOME Software source code** (`gnome-software/plugins/flatpak/`) — Uses the `libflatpak` C API function `flatpak_installation_list_installed_refs_for_update()` internally. The CLI equivalent is `flatpak remote-ls --updates`, which is what this app should use.

5. **KDE Discover source code** (`discover/libdiscover/backends/FlatpakBackend/`) — Also uses the libflatpak API, listing pending updates via `flatpak_installation_list_installed_refs_for_update()`. CLI equivalent: `flatpak remote-ls --updates`.

6. **`topgrade` (Rust CLI updater)** — In its Flatpak backend, uses `flatpak update -y` for the actual update but does _not_ use `--dry-run` for the pre-check. It instead checks for updates by running `flatpak remote-ls --updates`.

7. **Observed CLI behavior on Flatpak 1.16.6 (verified on this machine):**

   ```
   $ flatpak remote-ls --updates --columns=application
   Application ID
   io.github.kolunmi.Bazaar          ← one line per pending update

   $ flatpak remote-ls --updates --user --columns=application
   (empty — no user-installed updates)
   ```

   The output is:
   - **When updates exist:** A header line `Application ID` followed by one reverse-DNS app ID per line (no spaces in app IDs).
   - **When no updates exist:** Empty output (for the `--user` case) or just the header (for `--system`).
   - Exit code is always `0`.

8. **Flatpak app ID naming convention (freedesktop.org)** — App IDs follow reverse-DNS naming (`com.example.App`, `org.gnome.Platform`, etc.) and never contain spaces. This allows a reliable header-filter: skip any line containing a space character.

---

## 4. Proposed Fix

### 4.1 Replace `flatpak update --dry-run -y` with `flatpak remote-ls --updates`

**For both `count_available()` and `list_available()`:**

- Use `flatpak remote-ls --updates --columns=application` for the **system** installation  
- Use `flatpak remote-ls --updates --user --columns=application` for the **user** installation  
- Parse each output: skip empty lines and lines containing a space (the `Application ID` header)  
- Combine results, deduplicating by app ID  

**Sandbox compatibility:** `build_flatpak_cmd` already handles the `flatpak-spawn --host` prefix when running inside the Flatpak sandbox. No changes are needed there — `flatpak-spawn --host flatpak remote-ls --updates --columns=application` and `flatpak-spawn --host flatpak remote-ls --updates --user --columns=application` both work correctly.

**The `--talk-name=org.freedesktop.Flatpak` permission** is already declared in `io.github.up.json`, so `flatpak-spawn --host` has the required D-Bus permission.

### 4.2 Exact Parsing Logic

Filter lines with:
```rust
let t = line.trim();
!t.is_empty() && !t.contains(' ')
```

This correctly:
- Skips empty lines (`!t.is_empty()`)
- Skips `Application ID` header (and any locale-translated header) because all translated headers will contain at least one space
- Accepts all valid Flatpak app IDs (`com.example.App`, `org.gnome.Platform`, etc.) since they never contain spaces

### 4.3 Avoid Code Duplication

Implement `count_available()` by calling `self.list_available()` and returning the length:

```rust
fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
    Box::pin(async move {
        self.list_available().await.map(|v| v.len())
    })
}
```

This keeps the counting and listing logic in a single place.

---

## 5. Implementation Steps

### Step 1 — Rewrite `list_available()` in `src/backends/flatpak.rs`

Replace the current `flatpak update --dry-run -y` logic with two `flatpak remote-ls --updates --columns=application` calls (one for system, one for user) and combine + deduplicate the app IDs.

**New implementation:**

```rust
fn list_available(
    &self,
) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
    Box::pin(async move {
        let mut apps: Vec<String> = Vec::new();

        // Helper: run `flatpak remote-ls --updates --columns=application [--user]`
        // and collect non-header app IDs into `apps`, deduplicating.
        async fn collect_updates(
            sub_args: &[&str],
            apps: &mut Vec<String>,
        ) {
            let (cmd, args) = build_flatpak_cmd(sub_args);
            let Ok(out) = tokio::process::Command::new(&cmd)
                .args(&args)
                .output()
                .await
            else {
                return;
            };
            // stdout carries the table; stderr may have warnings — ignore stderr here.
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines() {
                let t = line.trim();
                // Skip empty lines and the "Application ID" header (any locale variant
                // will contain at least one space; valid Flatpak app IDs never do).
                if t.is_empty() || t.contains(' ') {
                    continue;
                }
                let s = t.to_string();
                if !apps.contains(&s) {
                    apps.push(s);
                }
            }
        }

        collect_updates(
            &["remote-ls", "--updates", "--columns=application"],
            &mut apps,
        )
        .await;

        collect_updates(
            &["remote-ls", "--updates", "--user", "--columns=application"],
            &mut apps,
        )
        .await;

        Ok(apps)
    })
}
```

### Step 2 — Rewrite `count_available()` in `src/backends/flatpak.rs`

Replace the `flatpak update --dry-run -y` logic with a delegation to `list_available()`:

```rust
fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
    Box::pin(async move {
        self.list_available().await.map(|v| v.len())
    })
}
```

> **Note:** Rust's borrow checker allows calling `self.list_available()` inside a `Box::pin(async move { ... })` block only when `self` can be captured. `FlatpakBackend` is a zero-sized struct (no fields), so it is `Copy`. Ensure it derives `Copy` or use a workaround if needed. If lifetime or ownership issues arise, the two `collect_updates` calls from `list_available` can be inlined directly into `count_available` as well.

### Step 3 — Update the inline comments

Replace the outdated comment block (`// Use --dry-run -y so the resolution logic...`) with accurate documentation explaining:
- Why `flatpak update --dry-run` is not used (removed in Flatpak ≥ 1.14 / 1.16)
- Why `flatpak remote-ls --updates` is the correct command
- Why both system and user installations are queried
- How the header line is detected and skipped

### Step 4 — Verify `run_update()` counting remains correct

`run_update()` uses `flatpak update -y` (actual update) and counts digit-prefixed lines. This is correct and should **not** be changed. The digit-prefixed lines only appear in the actual update output. Verify that the count behavior is still accurate after the `count_available`/`list_available` changes.

### Step 5 — Run the full preflight validation

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo build
cargo test
scripts/preflight.sh
```

---

## 6. Files to Modify

| File | Change |
|------|--------|
| `src/backends/flatpak.rs` | Replace `count_available()` and `list_available()` implementations |

No other files need changes. The Backend trait in `src/backends/mod.rs`, the UI in `src/ui/window.rs` and `src/ui/update_row.rs`, and the runner in `src/runner.rs` are all correct and require no modifications.

---

## 7. Risks and Mitigations

### Risk 1: `flatpak remote-ls --updates` uses stale cached metadata

**Likelihood:** Low–Medium  
**Impact:** Count may lag by hours if the Flatpak metadata cache is old  
**Mitigation:** This is acceptable — the user can click the refresh button (⟳) in the header to re-trigger the check. Note that `flatpak update -y` always fetches fresh metadata before updating, so the actual update will find correct packages regardless. The check UI is a best-effort indicator. A future enhancement could run `flatpak update --appstream` before `remote-ls` to force a refresh, but this adds network latency.

### Risk 2: `--columns=application` includes runtimes (not just apps)

**Likelihood:** Certain  
**Impact:** Count and list include runtime updates (e.g. `org.gnome.Platform`, `org.freedesktop.Platform.GL.default`), which is consistent with how `flatpak update -y` counts them  
**Mitigation:** This is correct behavior — the actual update updates both apps AND runtimes, so the pre-check count should reflect all pending updates. No change needed.

### Risk 3: `FlatpakBackend` cannot be used as `self` inside `async move` for `count_available` delegation

**Likelihood:** Low — `FlatpakBackend` is a unit struct with no fields  
**Impact:** Compile error  
**Mitigation:** If Rust's borrow checker objects, inline the two `collect_updates` calls directly into `count_available()` instead of delegating to `list_available()`. This results in minor code duplication but is always safe.

### Risk 4: Flatpak `remote-ls` output format changes in future versions

**Likelihood:** Very Low  
**Impact:** Header skip logic may break if future Flatpak removes the header or changes the column format  
**Mitigation:** The filter `!t.is_empty() && !t.contains(' ')` is robust: it relies on the invariant that all valid Flatpak app IDs follow reverse-DNS naming (no spaces), not on a specific header string. This invariant is guaranteed by the Flatpak specification and is extremely unlikely to change.

### Risk 5: `flatpak remote-ls` not available on very old Flatpak versions

**Likelihood:** Negligible  
**Impact:** Check would show 0 updates on Flatpak < 1.2.0  
**Mitigation:** The `--updates` flag on `flatpak remote-ls` has been available since Flatpak 1.2.0 (released 2018). No realistic user is running Flatpak older than this. If `flatpak remote-ls` fails, `count_available()` returns `Ok(0)` and `list_available()` returns `Ok(vec![])` — the same safe degradation that existed before.

---

## 8. Expected Outcome After Fix

| Scenario | Before Fix | After Fix |
|----------|-----------|-----------|
| 2 Flatpak updates pending | Shows "Up to date" | Shows "2 available" |
| 0 Flatpak updates pending | Shows "Up to date" ✓ | Shows "Up to date" ✓ |
| Running inside Flatpak sandbox | Shows "Up to date" (wrong) | Shows correct count via `flatpak-spawn --host` |
| User-installed Flatpaks with updates | Shows "Up to date" (wrong) | Shows correct count from `--user` query |
| After running "Update All" | Shows N updated ✓ | Shows N updated ✓ (unchanged) |

---

## 9. Spec File Path

`/home/nimda/Projects/Up/.github/docs/subagent_docs/flatpak_update_count_spec.md`
