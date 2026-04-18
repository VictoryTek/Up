# Review: Move System Info to Update Tab & Conditionally Hide Upgrade Tab

**Feature Name:** `ui_sysinfo_upgrade_tab`  
**Date:** 2026-04-17  
**Reviewer:** QA Subagent  
**Status:** PASS  

---

## 1. Build Results

| Check | Command | Result |
|---|---|---|
| **Build** | `nix develop --command cargo build` | ✅ `Finished dev profile [unoptimized + debuginfo] target(s) in 0.10s` |
| **Clippy** | `nix develop --command cargo clippy -- -D warnings` | ✅ No warnings |
| **Format** | `nix develop --command cargo fmt --check` | ✅ No diffs |
| **Tests** | `nix develop --command cargo test` | ✅ 12 passed, 0 failed, 0 ignored |

---

## 2. Specification Compliance

### 2.1 Step 1 — `UpgradePageInit` struct in `upgrade.rs`
- **PASS.** The struct is present, correctly placed, and exported as pub.
- Derives: `#[derive(Debug, Clone)]`. The main spec body (Step 1) specifies `Debug, Clone`. The appendix code sketch shows `Debug, Clone, Serialize, Deserialize`. The implementation chose the minimal correct derivation — `UpgradePageInit` is only ever sent through an in-process async channel and never needs serialization. This is more correct than the appendix sketch.
- Fields `distro: DistroInfo` and `nixos_extra: Option<(NixOsConfigType, String)>` match exactly.

### 2.2 Step 2 — `UpgradePage::build()` Refactor
- **PASS.** All spec changes implemented:
  - Signature changed from `-> gtk::Box` to `-> (gtk::Box, async_channel::Sender<upgrade::UpgradePageInit>)` ✓
  - Internal `spawn_background_async` detection block removed ✓
  - `info_group` title changed to `"Upgrade Status"` ✓
  - Distribution row removed from `info_group` ✓
  - Current Version row removed from `info_group` ✓
  - `upgrade_available_row` retained ✓
  - Conditional NixOS Config Type row logic preserved and correct ✓
  - `flake_banner` revealed on Flake detection ✓
  - `distro_info_state` stored from `init.distro` ✓
  - `nixos_config_type` stored from `init.nixos_extra` ✓
  - Check button enabled and auto-triggered when `upgrade_supported` ✓
  - Upgrade availability async check spawned when supported ✓
  - Returns `(page_box, init_tx)` ✓

### 2.3 Step 3 — System Info Section in `build_update_page()`
- **PASS.** All spec changes implemented:
  - `sys_info_group` (`adw::PreferencesGroup`, title "System Information") added ✓
  - `distro_row` (title "Distribution", subtitle "Loading…", prefix icon "computer-symbolic") ✓
  - `version_row` (title "Current Version", subtitle "Loading…") ✓
  - Inserted **after** `status_label` and **before** `backends_group` ✓
  - Return type changed to `(gtk::Box, Rc<dyn Fn()>, adw::ActionRow, adw::ActionRow)` ✓

### 2.4 Step 4 — Detection Lifted to `UpWindow::build()`
- **PASS.** All spec changes implemented:
  - `build_update_page()` destructured into 4-tuple ✓
  - `UpgradePage::build()` destructured into `(upgrade_widget, upgrade_init_tx)` ✓
  - `upgrade_stack_page` retained from `add_titled_with_icon` return ✓
  - `upgrade_stack_page.set_visible(false)` immediately after add (prevents any flash for non-upgradeable distros — matches recommended approach from spec §8.1) ✓
  - Single `async_channel::bounded::<(DistroInfo, Option<(NixOsConfigType, String)>)>(1)` detection channel ✓
  - `spawn_background_async` runs `detect_distro()`, `detect_nixos_config_type()`, `detect_hostname()` off GTK thread ✓
  - GTK callback populates `sysinfo_distro_row` and `sysinfo_version_row` ✓
  - GTK callback gates `upgrade_stack_page.set_visible(info.upgrade_supported)` ✓
  - **Positive deviation from spec Step 4 literal code:** Init forwarded to upgrade page only when `info.upgrade_supported` is true. This directly implements the mitigation recommended in spec §8 Risk Row 5 ("only send init when `upgrade_supported = true`"). The upgrade page's `init_rx` channel simply never fires for unsupported distros — no channel leak, no panic, no wasted work.

### 2.5 Step 5 — Upgrade Page Callers Updated
- **PASS.** The old `let upgrade_page = UpgradePage::build()` bare-`gtk::Box` call is gone. All call sites updated.

### 2.6 Steps 6 & 7 — Imports and Destructuring
- **PASS.** `use crate::upgrade` already covers `UpgradePageInit` via path. Destructuring updated to 4-tuple at the single call site.

### 2.7 Step 8 — Upgrade Page Logic Completeness
- **PASS.** All downstream callbacks in `upgrade_page.rs` remain intact and correct.

---

## 3. Detailed Findings

### 3.1 Best Practices

- **Channel sizing:** Detection channel is `bounded(1)` — correct for a one-shot event. Upgrade-check channel is `unbounded` for log streaming — appropriate.
- **Closure capture discipline:** GTK widgets captured by clone (GObject reference-counted handles, cheap). No `Arc` on GTK-thread-only data.
- **`glib::spawn_future_local`** used for all GTK main-loop async tasks. `spawn_background_async` used for all blocking I/O. No blocking on GTK thread.
- **`Rc<RefCell<_>>`** for all shared state that stays on the GTK thread. Appropriate use.
- **`downgrade()`/`upgrade()`** used for the window reference in the `about_action` closure — correct weak-reference pattern.
- **`let _ = ... .send(...).await`** for all one-shot channel sends — correctly discards `SendError` if the receiver has already been dropped (e.g., window closed before detection finishes).

### 3.2 Functionality

Both features are correctly and fully implemented:

1. **System information on Update tab:** `distro_row` ("Distribution") and `version_row` ("Current Version") are populated asynchronously after `detect_distro()` completes. They appear between `status_label` and `backends_group`. All users see this, not just those using the Upgrade tab.

2. **Upgrade tab hidden for unsupported distros:** `upgrade_stack_page.set_visible(false)` is called immediately, preventing any UI flash. It is set to `true` only when `upgrade_supported` is confirmed. For Arch, Manjaro, Tumbleweed, unknown distros etc., the tab never appears.

### 3.3 Code Quality

- No unused variables, no dead code paths in the critical path.
- `#[allow(dead_code)]` on `CheckMsg` enum in `upgrade_page.rs` — the `Error` variant is actually used (matched in the `check_rx.recv()` loop), so this allow attribute is harmless but slightly overly broad. It was present before this refactor and is not introduced by these changes. Not a regression.
- Naming is clear: `sysinfo_distro_row`, `sysinfo_version_row`, `upgrade_init_tx`, `upgrade_stack_page`, `init_rx` — all self-documenting.
- No redundant clones; GObject handles are cheap.

### 3.4 Security

- **Pango markup injection:** `glib::markup_escape_text(&raw_hostname)` is used before embedding hostname into the NixOS Config Type row subtitle. ✓
- **No unsafe code** in any modified file. ✓
- **`validate_hostname`** defensive check in `upgrade.rs` guards flake attribute construction. ✓
- No shell string interpolation anywhere in the modified code paths. ✓

### 3.5 Performance

- Distro detection runs exactly **once** per app startup (previously it ran inside `UpgradePage::build()` separately, potentially before the window appeared). Single shared detection eliminates duplicated work.
- Bounded channel of capacity 1 for the detection result — no unbounded accumulation.
- GTK thread never blocks.

### 3.6 Consistency

The implementation perfectly mirrors existing project patterns:
- `adw::PreferencesGroup` + `adw::ActionRow` for structured info display.
- `spawn_background_async` + `glib::spawn_future_local` for the async/GTK bridge.
- `Rc<dyn Fn()>` for shared callbacks (same as the existing `run_checks` closure).
- Return-value-based API design matching `build_update_page()` conventions.

---

## 4. Score Table

| Category | Score | Grade |
|---|---|---|
| Specification Compliance | 98% | A |
| Best Practices | 97% | A |
| Functionality | 100% | A+ |
| Code Quality | 96% | A |
| Security | 100% | A+ |
| Performance | 100% | A+ |
| Consistency | 99% | A+ |
| Build Success | 100% | A+ |

**Overall Grade: A+ (99%)**

---

## 5. Minor Notes (Non-Blocking)

1. **`UpgradePageInit` derives:** Implementation uses `#[derive(Debug, Clone)]`; the spec appendix code sketch shows `#[derive(Debug, Clone, Serialize, Deserialize)]`. The implementation is correct — the struct is never serialized; the appendix sketch was over-specified. No action needed.

2. **`upgrade_available_row` "Not supported" branch:** Inside `upgrade_page.rs`, the init callback still contains the `else` branch for `upgrade_supported = false`, setting subtitle to "Not supported for this distribution yet". Since `init` is now only sent when `upgrade_supported = true`, this branch is logically dead. It is harmless defensive code and does not affect correctness. Could be cleaned up in a future pass.

3. **`#[allow(dead_code)]` on `CheckMsg`:** Pre-existing attribute. Not introduced by this change.

---

## 6. Verdict

**PASS**

All specification requirements are met. Both features — system info on the update tab and conditional upgrade tab visibility — are correctly implemented. All four build checks pass with zero errors, zero warnings, and zero formatting diffs. The implementation includes a positive improvement over the literal spec code (guarding `upgrade_init_tx.send` with `if info.upgrade_supported`), which aligns with the spec's own risk mitigations. No issues requiring refinement.
