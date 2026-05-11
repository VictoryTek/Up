# Final Review: v2.0 D-Bus Backend Service & Plugin Discovery System

> Reviewer: Re-Review Subagent | Date: May 11, 2026

---

## Refinement Verification

### C1 — Plugin discovery priority inversion ✅ CONFIRMED FIXED

**File:** `src/plugins/discovery.rs`, line 23

```rust
for dir in dirs.iter() {
```

The `.rev()` has been removed. Iteration now proceeds in the order returned by `plugin_search_dirs()`: system dirs first (lowest priority), then user dir, then `/etc` (highest priority). Since `HashMap::insert` overwrites existing keys, later inserts from higher-priority directories correctly override earlier ones. XDG override semantics are now correct.

---

### C2 — Operations never removed from HashMap ✅ CONFIRMED FIXED

**File:** `daemon/src/interface.rs`

Each of `run_update`, `run_cleanup`, `run_upgrade`, and `create_snapshot` now spawns a cleanup task after inserting the operation handle:

```rust
let ops_ref = self.operations.clone();
let cleanup_id = op_id.clone();
tokio::spawn(async move {
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let mut ops = ops_ref.lock().await;
        if let Some(handle) = ops.get(&cleanup_id) {
            if handle.join_handle.as_ref().map_or(true, |h| h.is_finished()) {
                ops.remove(&cleanup_id);
                break;
            }
        } else {
            break;
        }
    }
});
```

The polling approach with 1-second intervals ensures completed operations are cleaned up reliably. The `is_finished()` check on the `JoinHandle` correctly detects task completion. Memory leak resolved.

---

### R1 — cancel() missing polkit authorization ✅ CONFIRMED FIXED

**File:** `daemon/src/interface.rs`, `cancel()` method (approx. line 308)

The cancel method now includes:
- `#[zbus(header)]` and `#[zbus(connection)]` parameters
- Polkit check for `io.github.up.cancel.operation` action
- Returns `AccessDenied` if authorization fails

```rust
async fn cancel(
    &self,
    operation_id: &str,
    #[zbus(header)] header: zbus::message::Header<'_>,
    #[zbus(connection)] connection: &zbus::Connection,
) -> fdo::Result<bool> {
    let sender = header.sender().ok_or_else(|| { ... })?.to_string();
    let action = "io.github.up.cancel.operation";
    if !auth::check_polkit(connection, &sender, action).await...? {
        return Err(fdo::Error::AccessDenied("Authorization denied".into()));
    }
    ...
}
```

This aligns with the spec (Section 5.1) and the existing polkit policy file.

---

### R3 — audit::log_operation_complete never called ✅ CONFIRMED FIXED

**File:** `daemon/src/executor.rs`, line ~175

```rust
// Audit log the completion
crate::audit::log_operation_complete(&op_id, overall_success, overall_exit_code);
```

Called immediately after computing the final summary and before emitting the `OperationComplete` D-Bus signal. Audit trail now has both START and COMPLETE entries per the spec (Section 4.6).

---

### R4 — ProtectSystem=strict blocks package managers ✅ CONFIRMED FIXED

**File:** `data/io.github.up.Daemon.service`

```ini
ProtectSystem=false
```

Changed from `strict` to `false`. Since the daemon runs as root and executes package managers that need to write to `/usr`, `/var`, `/etc`, and `/opt`, this is the correct trade-off. The service retains other hardening directives (`ProtectHome=yes`, `PrivateTmp=yes`, `ProtectKernelTunables=yes`, etc.) that don't interfere with package management.

---

## New Issues Introduced by Fixes

### None Critical

The cleanup task approach (polling every 1 second) is slightly less elegant than a callback/watcher pattern, but is correct, bounded (breaks out of loop when operation finishes or is removed), and has negligible overhead given the low frequency of update operations. No new bugs introduced.

### Minor Observation (Informational)

The cleanup tasks hold an `Arc<Mutex<HashMap>>` reference and poll once per second. In the worst case (4 concurrent operations), this means 4 tasks each acquiring the lock for microseconds every second — well within acceptable bounds for a system daemon. No action needed.

---

## Build Status

| Check | Result |
|-------|--------|
| `cargo check -p up-daemon` | ✅ PASS (3 dead_code warnings — expected for `register_plugin`, `is_challenge`, `remaining`) |
| Code review | ✅ No new issues introduced |
| Spec alignment | ✅ All fixed items now match specification |

---

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 95% | A |
| Best Practices | 90% | A- |
| Functionality | 95% | A |
| Code Quality | 90% | A- |
| Security | 92% | A- |
| Performance | 88% | B+ |
| Consistency | 93% | A |
| Build Success | 100% | A+ |

**Overall Grade: A- (93%)**

---

## Remaining Non-Blocking Items (from original review, not in scope for this refinement)

- **R2** (client timeout on daemon disconnect) — deferred, not a daemon-side issue
- **R5** (xbps `--yes` flag) — minor, plugin descriptor concern
- **I1–I6** — all informational, no action required

---

## Verdict

## ✅ APPROVED

All 2 CRITICAL and 3 RECOMMENDED issues have been verified as correctly resolved. No new issues were introduced by the fixes. The daemon crate compiles successfully. The implementation aligns with the specification.
