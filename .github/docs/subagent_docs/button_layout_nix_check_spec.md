# Spec: Update Button Layout Fix + Cancel Button + NixOS VexOS Check Fix

## Current State Analysis

### Issue 1: Update Button Cut in Half (UI Layout Bug)

**File**: `src/ui/window.rs` — `build_update_page()`

The `update_button` widget is appended to `content_box`, which is nested inside:
`page_box > scrolled (vexpand=true) > clamp > content_box`

The `log_panel.expander` is appended **directly to `page_box`** (outside the scrolled area).

When the log panel expands, GTK reduces the height of `scrolled` to accommodate the
log panel. Because `content_box` is inside `scrolled`, the Update All button (at the
bottom of `content_box`) becomes clipped when the scrolled area shrinks too small to
show all content — resulting in the button being cut in half at the visible boundary.

**Fix**: Move `update_button` (and the new cancel button) out of `content_box` and
into `page_box` as a fixed footer between `scrolled` and `log_panel.expander`.

### Issue 2: Cancel Button Not Available

**File**: `src/ui/window.rs` — update_button `connect_clicked` handler

`orchestrator.run_all(event_tx)` returns a `CancelHandle` (defined in
`src/orchestrator.rs`) but the return value is **discarded** with no binding:
```rust
orchestrator.run_all(event_tx);  // CancelHandle dropped immediately
```

`CancelHandle::cancel()` closes the privileged shell stdin to stop the update gracefully.

**Fix**: 
- Add a `cancel_button` widget (label "Cancel", `.pill` CSS class)
- Create a `Rc<Cell<Option<CancelHandle>>>` to hold the handle during an update
- Capture `CancelHandle` from `run_all()` and store it in the cell
- Show cancel_button when update starts, hide when it ends
- Connect cancel_button to call `handle.cancel()`

### Issue 3: NixOS/VexOS Check Returns "Up to Date" Incorrectly

**File**: `src/backends/nix.rs` — `NixBackend::list_available()`

For flake-based NixOS (including VexOS), `list_available()` calls
`nixos_flake_changed_inputs()` which:
1. Tries `nix flake update --dry-run /etc/nixos` (Nix ≥ 2.19)
2. Falls back to temp-dir copy + compare flake.lock nodes

This only detects **flake input changes** (e.g., new nixpkgs commit in the lock file).
However, VexOS uses `vexos-update` which:
- Runs `nix flake update` (updates inputs)
- Runs `nixos-rebuild switch` (rebuilds system configuration)

A rebuild can be necessary even when inputs haven't changed (e.g., the current 
running system profile is behind the latest derivation, or the system was never
switched after a previous update). The flake lock comparison misses this case.

**Evidence**: Screenshot shows "All packages available in binary cache — applying
update... building the system configuration..." despite the pre-check saying 0 updates.

**Fix for VexOS**: In `list_available()`, when `is_vexos()` is true, skip the
flake lock comparison and return `vec!["NixOS system".to_string()]` unconditionally.
This correctly enables the Update All button for VexOS systems, since `vexos-update`
always performs a potentially-updating rebuild.

For non-VexOS standard NixOS flake systems, the existing check is retained (it is
accurate for standard flake-managed systems where rebuilding without input changes
is typically a no-op).

---

## Proposed Solution Architecture

### window.rs changes

#### 1. Move buttons to fixed footer

Remove from `content_box`:
```rust
// REMOVE:
content_box.append(&update_button);
```

Add between `scrolled` and `log_panel.expander` in `page_box`:
```rust
let footer_box = gtk::Box::builder()
    .orientation(gtk::Orientation::Horizontal)
    .halign(gtk::Align::Center)
    .spacing(12)
    .margin_top(8)
    .margin_bottom(8)
    .build();

let cancel_button = gtk::Button::builder()
    .label("Cancel")
    .css_classes(vec!["pill"])
    .visible(false)
    .build();

footer_box.append(&cancel_button);
footer_box.append(&update_button);
page_box.append(&footer_box);
// then log panel...
page_box.append(&log_panel.expander);
```

#### 2. Cancel button wiring

Add near `updating` state:
```rust
let cancel_handle: Rc<Cell<Option<CancelHandle>>> = Rc::new(Cell::new(None));
```

In the update_button click handler, after `orchestrator.run_all(event_tx)`:
```rust
let handle = orchestrator.run_all(event_tx);
cancel_handle.set(Some(handle));
cancel_button.set_visible(true);
```

At the end of the update (after AllFinished and cleanup):
```rust
cancel_handle.set(None);
cancel_button.set_visible(false);
```

Connect cancel_button:
```rust
cancel_button.connect_clicked(glib::clone!(
    #[strong]
    cancel_handle,
    move |btn| {
        if let Some(handle) = cancel_handle.take() {
            handle.cancel();
        }
        btn.set_sensitive(false);
    }
));
```

Note: `Rc<Cell<Option<CancelHandle>>>` requires `CancelHandle: Default` or we use
`RefCell<Option<CancelHandle>>` instead since `CancelHandle` doesn't implement `Default`.

**Use `Rc<RefCell<Option<CancelHandle>>>`** for the handle storage.

Also need to import `CancelHandle`:
```rust
use crate::orchestrator::{OrchestratorEvent, UpdateOrchestrator, CancelHandle};
```
(add `CancelHandle` to the existing import if not already present)

### nix.rs changes

In `list_available()`, modify the NixOS flake branch:

```rust
// Before:
if is_nixos() && is_nixos_flake() {
    nixos_flake_changed_inputs().await
}

// After:
if is_nixos() && is_nixos_flake() {
    if is_vexos() {
        // VexOS uses vexos-update which always rebuilds the system.
        // The flake lock comparison cannot determine if a rebuild is needed
        // without running the actual update command (which requires root).
        // Always report the system as having a pending update so the user
        // can trigger vexos-update to let it decide what to do.
        Ok(vec!["NixOS system".to_string()])
    } else {
        nixos_flake_changed_inputs().await
    }
}
```

---

## Implementation Steps

1. **src/ui/window.rs**:
   a. Add `CancelHandle` to the `use crate::orchestrator::` import
   b. Create `cancel_button` widget alongside `update_button`
   c. Create `Rc<RefCell<Option<CancelHandle>>>` state
   d. Remove `content_box.append(&update_button)`
   e. Create `footer_box`, append `cancel_button` and `update_button`, add to `page_box`
   f. In `update_button connect_clicked`: capture handle, store in cell, show cancel_button
   g. In update completion: clear handle, hide cancel_button
   h. Connect `cancel_button` to `handle.cancel()`
   i. In update_button closure, ensure all early-return paths also hide cancel_button

2. **src/backends/nix.rs**:
   a. In `list_available()`, add VexOS check before calling `nixos_flake_changed_inputs()`

---

## Dependencies

No new external dependencies required.

## Risks and Mitigations

- **Risk**: `CancelHandle` uses interior mutability with `Arc<Mutex<...>>`. Calling
  `cancel()` from the GTK thread is safe per the existing design.
- **Risk**: The cancel_button might remain visible if an early-return path is missed.
  Mitigation: audit all return paths in the update_button closure.
- **Risk**: VexOS always showing "1 available" might confuse users who just updated.
  Mitigation: The row will show "1 available" with label "NixOS system" which is 
  informative. After running the update, if vexos-update finds nothing to do, it
  exits successfully with 0 store operations and the count shows "Up to date (0)".
