# Specification: Conditional Tab Bar Visibility Based on Upgrade Support

**Feature:** `tab_visibility`  
**Date:** 2026-04-27  
**Status:** Ready for Implementation

---

## 1. Current State Analysis

### 1.1 UI Widget Hierarchy

The application window is built in `src/ui/window.rs` → `UpWindow::build()`.

The tab UI is implemented with:
- **`adw::ViewStack`** — a stacking container where each child is a named page
- **`adw::ViewSwitcherBar`** — a bottom bar that renders tab buttons for each visible page in the ViewStack

The widget tree (relevant portion):

```
adw::ApplicationWindow
  └── gtk::Box (vertical, `main_box`)
        ├── adw::HeaderBar (`header`)
        ├── adw::ViewStack (`view_stack`)
        │     ├── StackPage "update" → gtk::Box (update page)
        │     └── StackPage "upgrade" → gtk::Box (upgrade page)
        └── adw::ViewSwitcherBar (`view_switcher_bar`)  ← always visible
```

The ViewSwitcherBar is constructed with `reveal(true)`:

```rust
// src/ui/window.rs (approximately lines 91–95)
let view_switcher_bar = adw::ViewSwitcherBar::builder()
    .stack(&view_stack)
    .reveal(true)
    .build();
```

### 1.2 Upgrade Support Detection

Detection is performed in `src/upgrade.rs` → `detect_distro()`, which parses `/etc/os-release` and sets `DistroInfo::upgrade_supported`:

| Distro ID(s) | `upgrade_supported` |
|---|---|
| `ubuntu`, `linuxmint`, `pop`, `elementary`, `zorin` | `true` |
| `fedora` | `true` |
| `opensuse-leap` | `true` |
| `debian` | `true` |
| `nixos` | `true` |
| `rhel`, `centos` | `true` |
| Any distro with `ID_LIKE` containing `ubuntu` or `debian` | `true` |
| `arch`, `manjaro`, `void`, `alpine`, `gentoo`, and all others | `false` |

### 1.3 Current Tab Gating Logic

In `UpWindow::build()`, a background thread detects the distro and sends the result over an `async_channel`. On the GTK main thread a `glib::spawn_future_local` closure receives the result and currently does:

```rust
// src/ui/window.rs (approximately lines 71–88)
glib::spawn_future_local(async move {
    if let Ok((info, nixos_extra)) = detect_rx.recv().await {
        // 1. Populate system info rows
        sysinfo_distro_row.set_subtitle(&info.name);
        sysinfo_version_row.set_subtitle(&info.version);

        // 2. Gate upgrade tab visibility
        if !info.upgrade_supported {
            upgrade_stack_page.set_visible(false);   // ← hides the page
        }

        // 3. Forward to upgrade page init channel
        if info.upgrade_supported {
            let init = upgrade::UpgradePageInit { distro: info, nixos_extra };
            let _ = upgrade_init_tx.send(init).await;
        }
    }
});
```

### 1.4 Execution Order Issue

`view_switcher_bar` is **declared after** the async setup block:

```
lines 47–89:  spawn_background_async { ... }  ← detects distro
              glib::spawn_future_local { ... } ← captures upgrade_stack_page (no view_switcher_bar)
lines 91–95:  let view_switcher_bar = ...      ← declared too late to capture
lines 124–127: main_box.append(&view_switcher_bar)
```

Because `view_switcher_bar` does not exist at the point where the closure is defined, it cannot currently be referenced inside the async result handler.

### 1.5 NixOS vs Ubuntu Behaviour

- **Ubuntu** (`upgrade_supported = true`): both "Update" and "Upgrade" tabs are visible. The Upgrade page connects its `init_rx` channel and auto-runs checks.
- **NixOS** (`upgrade_supported = true`): identical tab structure; the Upgrade page additionally adds a "NixOS Config Type" row and may show a flake advisory banner.
- **Arch/Void/Alpine/etc.** (`upgrade_supported = false`): the `upgrade_stack_page.set_visible(false)` call hides the Upgrade page in the ViewStack. **However, the `ViewSwitcherBar` remains visible with `reveal(true)`,** rendering a single-tab bar at the bottom of the window — unnecessary chrome.

---

## 2. Problem Definition

On distros that do **not** support upgrades (Arch, Manjaro, Void, Alpine, Gentoo, etc.), the application currently:

1. Hides the "Upgrade" `StackPage` — ✅ correct
2. Leaves `ViewSwitcherBar` visible with `reveal(true)` — ❌ **problem**

This means non-upgrade-distro users see:

```
┌────────────────────────────────────┐
│  🔄  Up                      ⋮    │  ← HeaderBar
├────────────────────────────────────┤
│                                    │
│  [update page content]             │
│                                    │
├────────────────────────────────────┤
│  [  📦 Update  ]                   │  ← ViewSwitcherBar — pointless single tab
└────────────────────────────────────┘
```

The expected behaviour:

```
┌────────────────────────────────────┐
│  🔄  Up                      ⋮    │  ← HeaderBar
├────────────────────────────────────┤
│                                    │
│  [update page content]             │
│                                    │
└────────────────────────────────────┘
```

---

## 3. Proposed Solution Architecture

### 3.1 Approach

The `view_switcher_bar` variable must be:
1. **Declared before** the async setup block so it can be captured by the `glib::spawn_future_local` closure.
2. **Cloned** for the closure (GTK4-rs widgets are `glib::Object` reference-counted, so `.clone()` is a cheap ref-count bump).
3. **Hidden** inside the `if !info.upgrade_supported` branch via `set_reveal(false)`.

`AdwViewSwitcherBar::set_reveal(false)` is the correct API call — it is the property the builder sets to `true` and it produces a smooth animated hide. Using `set_visible(false)` would also work and removes the widget from the layout entirely, but since the detection happens at startup (before the user sees anything), either is acceptable. `set_reveal(false)` is preferred as it is the semantic "this bar has nothing to switch between" state.

### 3.2 Decision: `set_reveal(false)` vs `set_visible(false)`

| Method | Effect | Pros | Cons |
|---|---|---|---|
| `set_reveal(false)` | Animates bar out; preserves layout slot momentarily | Semantic match to `reveal` property; smooth | Leaves a tiny empty slot during animation (invisible on startup) |
| `set_visible(false)` | Immediately removes widget from layout | Zero space consumed | Less idiomatic for this widget |

**Decision: use `set_reveal(false)`** — matches the property used at construction time; no visible difference at startup since detection completes before first paint in practice.

### 3.3 No Other Components to Change

- `src/upgrade.rs` — no change needed; `upgrade_supported` logic is already correct
- `src/ui/upgrade_page.rs` — no change needed; it only initialises when `upgrade_supported = true`
- `src/backends/` — no change needed
- `src/app.rs` / `src/main.rs` — no change needed

---

## 4. Exact Files to Modify

### 4.1 `src/ui/window.rs`

**Change 1 — Move `view_switcher_bar` declaration before the async setup block**

Current position (approximately line 91, after the async block):
```rust
let view_switcher_bar = adw::ViewSwitcherBar::builder()
    .stack(&view_stack)
    .reveal(true)
    .build();
```

Move this declaration to **before** the `super::spawn_background_async` block (i.e., immediately after `upgrade_stack_page` is obtained, around line 45).

**Change 2 — Clone `view_switcher_bar` into the `glib::spawn_future_local` closure**

The closure currently captures: `detect_rx`, `sysinfo_distro_row`, `sysinfo_version_row`, `upgrade_stack_page`, `upgrade_init_tx`.

Add a clone before the closure:
```rust
let view_switcher_bar_for_detect = view_switcher_bar.clone();
```

Then move `view_switcher_bar_for_detect` into the closure.

**Change 3 — Hide the bar when upgrade is not supported**

Inside the closure, in the `if !info.upgrade_supported` branch:
```rust
if !info.upgrade_supported {
    upgrade_stack_page.set_visible(false);
    view_switcher_bar_for_detect.set_reveal(false);  // ← ADD THIS LINE
}
```

---

## 5. Implementation Steps

1. Open `src/ui/window.rs`.
2. Locate the `view_switcher_bar` builder declaration (currently after the async block, approximately line 91).
3. **Cut** the entire `let view_switcher_bar = adw::ViewSwitcherBar::builder()...build();` statement.
4. **Paste** it immediately after the line that obtains `upgrade_stack_page` (i.e., after `let upgrade_stack_page = view_stack.add_titled_with_icon(...);`, approximately line 43).
5. Before the `super::spawn_background_async(...)` call, add:
   ```rust
   let view_switcher_bar_for_detect = view_switcher_bar.clone();
   ```
6. In the `glib::spawn_future_local` async closure, update the `if !info.upgrade_supported` block to:
   ```rust
   if !info.upgrade_supported {
       upgrade_stack_page.set_visible(false);
       view_switcher_bar_for_detect.set_reveal(false);
   }
   ```
7. Verify no other references to `view_switcher_bar` are broken (it is used in `main_box.append(&view_switcher_bar)` which still works because the original binding is retained).
8. Run `cargo build` to confirm compilation.
9. Run `cargo clippy -- -D warnings` to confirm no lint warnings.
10. Run `cargo fmt --check` to confirm formatting.

---

## 6. Dependencies

No new dependencies are required. `AdwViewSwitcherBar::set_reveal()` is available in libadwaita-rs v0.7 (already a project dependency).

---

## 7. Risks and Mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| `view_switcher_bar` referenced before `view_stack` is ready | Low | `view_stack` is declared at the top of `build()`; moving `view_switcher_bar` declaration down from line 91 to ~line 45 still keeps it after `view_stack` is created |
| Clone invalidated before closure fires | None | GTK4-rs widgets are `glib::Object` with strong ref-counting; clone is valid until all clones are dropped |
| `set_reveal(false)` leaves empty vertical space | Very low | Detection completes in milliseconds; any animation artefact is invisible at startup |
| Regression on upgrade-supported distros | None | The `set_reveal(false)` call is gated exclusively inside `if !info.upgrade_supported`; upgrade-supported paths are untouched |
| Arch/Manjaro users expect single Update tab with no bar | Resolved by this change | The ViewSwitcherBar is hidden; the update page fills the full window height |

---

## 8. Acceptance Criteria

- [ ] On Ubuntu (upgrade supported): two tabs ("Update", "Upgrade") are visible; `ViewSwitcherBar` is shown.
- [ ] On NixOS (upgrade supported): two tabs are visible; `ViewSwitcherBar` is shown.
- [ ] On Arch/Manjaro/Void/Alpine/Gentoo (upgrade NOT supported): no tab bar visible; update page fills the window.
- [ ] `cargo build` succeeds with zero errors.
- [ ] `cargo clippy -- -D warnings` produces zero warnings.
- [ ] `cargo fmt --check` passes.
