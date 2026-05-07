# Stragglers Specification

**Feature:** Three small remaining fixes (Zypper locale, Flatpak remote-ls, glib::clone audit)
**Date:** 2026-05-07
**Spec author:** Research subagent

---

## 1. Current State Analysis

The codebase is a GTK4/libadwaita Linux desktop application written in Rust (Edition 2021). The three items below are low-severity improvements identified in a previous review pass.

### Files examined

| File | Purpose |
|---|---|
| `src/backends/os_package_manager.rs` | APT, DNF, Pacman, Zypper backends |
| `src/backends/flatpak.rs` | Flatpak backend |
| `src/ui/window.rs` | Main application window |
| `src/ui/upgrade_page.rs` | Upgrade tab |
| `src/ui/update_row.rs` | Per-backend update row widget |
| `src/ui/log_panel.rs` | Terminal output panel widget |
| `src/ui/reboot_dialog.rs` | Reboot prompt dialog |
| `Cargo.toml` | Dependency manifest |

---

## 2. Problem Definitions & Solutions

---

### Item A — Zypper `count_zypper_upgraded` locale sensitivity

#### Problem

`count_zypper_upgraded` (file `src/backends/os_package_manager.rs`, around line 335) counts lines containing the string `"done"` in the output of `zypper update -y`. Example zypper output in English:

```
Retrieving package htop.rpm (1/2)...done
Retrieving package curl.rpm (2/2)...done
```

In a non-English locale (e.g., German) zypper emits `"erledigt"` instead of `"done"`, and similar variations exist in other languages. The counter returns **0** in those locales, making the update count appear as "0 updated" even when packages were installed.

The `run_update` for `ZypperBackend` runs the command through `pkexec sh -c "zypper refresh && zypper update -y"`. `pkexec` strips caller-supplied environment variables for security, so locale variables must be injected **inside the shell command string** — not via `.env()` on the outer `tokio::process::Command`.

#### Exact location

File: `src/backends/os_package_manager.rs`

Approximate lines 275–284 (inside `ZypperBackend::run_update`):

```rust
            match runner
                .run(
                    "pkexec",
                    &["sh", "-c", "zypper refresh && zypper update -y"],
                )
                .await
```

#### Required change

Replace the shell command string to set `LANG=C LC_ALL=C` before each zypper invocation:

```rust
            match runner
                .run(
                    "pkexec",
                    &["sh", "-c", "LANG=C LC_ALL=C zypper refresh && LANG=C LC_ALL=C zypper update -y"],
                )
                .await
```

No changes are needed to `count_zypper_upgraded` itself — the fix is upstream in the command.

#### Risks

- **Low.** Setting locale variables in the shell command string is a standard and safe pattern for forcing English output from locale-aware CLI tools.
- `pkexec` runs the shell as root; the locale variable is set by the shell itself, not inherited from the calling process's environment. This is intentional and correct.
- Existing unit tests (`test_count_zypper_upgraded_some`, `test_count_zypper_upgraded_none`) already use English-format output and will continue to pass unchanged.

---

### Item B — Flatpak `list_available`: replace `update --no-deploy` with `remote-ls --updates`

#### Problem

`FlatpakBackend::list_available` currently runs:

```
flatpak update --no-deploy -y --user --columns=application
```

`flatpak update --no-deploy` initiates the full update protocol — it contacts every configured remote, downloads repository metadata (OSTree summary files), and resolves the full dependency graph before aborting without committing. On slow connections or with many remotes this can take several seconds and saturates the network, yet it runs on every background availability check.

`flatpak remote-ls --updates --columns=application` is the purpose-built command for listing available updates. It performs a lightweight metadata fetch per remote and is significantly faster.

#### Exact location

File: `src/backends/flatpak.rs`

Approximate lines 118–139 (inside `FlatpakBackend::list_available`):

**Comment block to replace (lines 119–126):**
```rust
            // Use `flatpak update --no-deploy -y --user --columns=application` to detect
            // pending updates without applying them. The `--columns=application` flag
            // ensures one application ID per line for predictable parsing.
            // The `--user` flag is intentional: the `--system` variant triggers a polkit
            // prompt on every background check, which is poor UX. System Flatpak installs
            // are uncommon on desktop systems, so only user installations are checked here.
            let (cmd, args) = build_flatpak_cmd(&[
                "update",
                "--no-deploy",
                "-y",
                "--user",
                "--columns=application",
            ]);
```

**Error message to replace (line ~137):**
```rust
                return Err(format!("flatpak update --no-deploy failed: {stderr}"));
```

#### Required changes

1. Replace the comment and `build_flatpak_cmd` call:

```rust
            // Use `flatpak remote-ls --updates --user --columns=application` to list
            // pending updates without initiating the full update protocol.
            // `--columns=application` ensures one application ID per line for predictable
            // parsing. The `--user` flag is intentional: the `--system` variant triggers a
            // polkit prompt on every background check, which is poor UX. System Flatpak
            // installs are uncommon on desktop systems, so only user installations are
            // checked here.
            let (cmd, args) = build_flatpak_cmd(&[
                "remote-ls",
                "--updates",
                "--user",
                "--columns=application",
            ]);
```

2. Replace the error message string:

```rust
                return Err(format!("flatpak remote-ls --updates failed: {stderr}"));
```

#### Parser compatibility

`parse_flatpak_updates` / `parse_flatpak_app_line` already handle:
- Lines containing a reverse-DNS application ID → included
- Lines equal to `"application"` or `"Application"` (column header) → skipped
- Empty / whitespace-only lines → skipped
- Duplicate IDs → deduplicated

`flatpak remote-ls --updates --columns=application` emits exactly this format (header `Application` followed by one app ID per line), so **no parser changes are required**.

#### Scope preservation

The existing `--user` flag is preserved intentionally. The justification from the original code comment applies equally here: querying the system installation triggers a polkit prompt on every background check, which degrades UX. System Flatpak installs are rare on desktop systems.

#### Risks

- **Low.** `flatpak remote-ls --updates` is a stable, documented command present since Flatpak 0.9.x.
- Removes the `-y` flag (which suppresses prompts for the `update` subcommand but is meaningless for `remote-ls`). This is correct.
- The existing unit tests (`test_parse_flatpak_updates_*`) operate on already-parsed strings and are unaffected by the command change.

---

### Item C — `glib::clone!` macro audit across UI files

#### Findings

**All five UI files were audited.** The codebase has **already been migrated** to `glib::clone!` for all GTK signal handler closures and `glib::spawn_future_local` calls.

| File | Signal handler clone style | Action required |
|---|---|---|
| `src/ui/window.rs` | `glib::clone!(#[weak]…, #[strong]…, => …)` throughout | None |
| `src/ui/upgrade_page.rs` | `glib::clone!(#[weak]…, #[strong]…, => …)` throughout | None |
| `src/ui/update_row.rs` | No closures passed to signal handlers | None |
| `src/ui/log_panel.rs` | No GTK signal handlers; uses `text_view.downgrade()` + `Rc::clone` inline, not before a handler closure | None |
| `src/ui/reboot_dialog.rs` | `connect_response` uses a plain `move` closure (no captures to clone) | None |

#### Remaining manual-clone patterns (intentionally NOT replaced)

Two closures in the codebase use the manual `let x = x.clone();` block pattern immediately before a closure literal:

1. **`run_checks: Rc<dyn Fn()>`** in `src/ui/window.rs` (~line 383):
   ```rust
   let run_checks: Rc<dyn Fn()> = {
       let rows = rows.clone();
       let detected = detected.clone();
       // … more clones …
       Rc::new(move || { … })
   };
   ```

2. **`recompute_state: Rc<dyn Fn()>`** in `src/ui/upgrade_page.rs` (~line 143):
   ```rust
   let recompute_state: Rc<dyn Fn()> = {
       let upgrade_btn = upgrade_button.clone();
       // … more clones …
       Rc::new(move || { … })
   };
   ```

Both are shared logic closures stored in `Rc<dyn Fn()>` — they are **not** passed directly to a GTK signal handler or `glib::spawn_future_local`. Per the task specification, these must **not** be changed.

`glib::clone!` cannot be applied to `Rc::new(move || { … })` because `Rc<T>` does not implement `glib::clone::Downgrade`, and the macro is designed for use with GTK signal handlers and async futures, not for creating shared closures.

#### Conclusion for Item C

**No implementation work required.** The codebase is already compliant with the `glib::clone!` requirement for all in-scope patterns.

---

## 3. Implementation Steps

### Step 1 — Item A (Zypper locale fix)

Edit `src/backends/os_package_manager.rs`:

In `ZypperBackend::run_update`, change the `sh -c` argument string from:
```
"zypper refresh && zypper update -y"
```
to:
```
"LANG=C LC_ALL=C zypper refresh && LANG=C LC_ALL=C zypper update -y"
```

### Step 2 — Item B (Flatpak remote-ls)

Edit `src/backends/flatpak.rs`:

In `FlatpakBackend::list_available`:
1. Replace the comment block and `build_flatpak_cmd` call (see exact replacements in §2.B above).
2. Replace the error format string (see §2.B above).

### Step 3 — Item C

No implementation work required.

---

## 4. Dependencies

No new crates or external dependencies are introduced. All changes are in-place edits to existing functions.

---

## 5. Verification

After implementation, run:

```bash
cargo build          # must compile without errors
cargo clippy -- -D warnings  # must produce no warnings
cargo fmt --check    # must produce no diffs
cargo test           # existing tests must pass
```

Key tests that exercise the changed code:
- `test_count_zypper_upgraded_some` / `_none` — unchanged; still pass with English-format strings
- `test_parse_flatpak_updates_*` — unchanged; operate on pre-parsed strings
- `test_flatpak_run_update_*` — unchanged; the `run_update` path is not modified
