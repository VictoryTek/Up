# Check for Updates — Final Re-Review

**Date:** 2026-04-04  
**Reviewer:** Re-Review Subagent  
**Feature:** Check for Updates (availability count + package list display)

---

## Issues Verified

### R1 — Concurrent-check underflow (epoch/generation counter)

**Status: RESOLVED ✅**

An `Rc<Cell<u64>>` epoch counter (`check_epoch`) was added in `src/ui/window.rs`.

Evidence (`window.rs` lines ~224–278):
```rust
let check_epoch: Rc<Cell<u64>> = Rc::new(Cell::new(0));
// ...
check_epoch.set(check_epoch.get() + 1);
let my_epoch = check_epoch.get();
// ... inside each spawned future:
if epoch_ref.get() != my_epoch {
    return;
}
```

Each call to `run_checks` increments the epoch and captures `my_epoch`. Every
spawned async future captures `my_epoch` and discards its results if the epoch
has moved on by the time it completes. This correctly prevents stale futures from
a superseded check cycle from decrementing `pending_checks` below zero or
corrupting `total_available`.

---

### R2 — Stale packages on re-check (set_packages on error path)

**Status: RESOLVED ✅**

`set_packages(&[])` is now called on the error path for `list_result`.

Evidence (`window.rs` lines ~283–291):
```rust
match list_result {
    Ok(packages) => row.set_packages(&packages),
    Err(_) => row.set_packages(&[]),
}
```

`set_packages` in `update_row.rs` drains previously tracked child rows before
adding new ones, so calling it with an empty slice effectively clears any
package rows left over from a prior successful check.

---

## Build Validation

| Check | Result |
|-------|--------|
| `cargo build` | ✅ Finished — 0 errors, 0 warnings |
| `cargo test` | ✅ 9 tests passed, 0 failed |

---

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 95% | A |
| Functionality | 100% | A |
| Code Quality | 95% | A |
| Security | 100% | A |
| Performance | 95% | A |
| Consistency | 100% | A |
| Build Success | 100% | A |

**Overall Grade: A (98%)**

---

## Verdict

**APPROVED**

All issues from the initial review have been resolved. The implementation is
correct, the build succeeds, and all 9 tests pass. The code is ready to ship.
