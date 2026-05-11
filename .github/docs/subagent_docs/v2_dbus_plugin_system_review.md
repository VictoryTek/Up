# Review: v2.0 D-Bus Backend Service & Plugin Discovery System

> Reviewer: QA Subagent | Date: May 11, 2026

---

## Build Validation

| Check | Result |
|-------|--------|
| `cargo check -p up-daemon` | ✅ PASS (5 dead_code warnings — all expected for new API surface) |
| Frontend crate (`cargo check -p up`) | ⚠️ Cannot validate on Windows (GTK4 deps) — code-level review only |
| Syntax errors in frontend code | ✅ None found (code follows established patterns) |
| Meson build file syntax | ✅ Valid |
| Nix flake syntax | ✅ Valid |
| Polkit policy XML | ✅ Well-formed |
| D-Bus bus config XML | ✅ Well-formed |
| Plugin YAML descriptors | ✅ Valid schema conformance |

---

## Issues Found

### CRITICAL

#### C1: Plugin discovery priority inversion (security impact)

**File:** `src/plugins/discovery.rs`, line 24

```rust
for dir in dirs.iter().rev() {
```

The `plugin_search_dirs()` function returns directories ordered lowest→highest priority:
`[system_dirs..., user_dir, /etc/up/backends.d]`

Using `.rev()` iterates highest-priority first (/etc, then user, then system). Since `HashMap::insert` overwrites, the **last** insert wins — meaning **system** (lowest priority) directories override `/etc` (highest priority).

**Impact:** Sysadmins cannot override plugin definitions via `/etc/up/backends.d/`. If a system plugin and an /etc override have the same ID, the system version wins instead of the admin override. This violates the XDG override semantics specified in Section 6.1 of the spec.

**Fix:** Remove `.rev()` so iteration proceeds lowest→highest, allowing higher-priority directories to overwrite:
```rust
for dir in &dirs {
```

---

#### C2: Completed operations never removed from HashMap (resource leak)

**File:** `daemon/src/executor.rs` + `daemon/src/interface.rs`

After `spawn_operation` completes (the spawned tokio task finishes), the `OperationHandle` remains in `self.operations` indefinitely. Over the daemon's lifetime (or under repeated D-Bus-activation cycles), this causes:

1. **Memory leak** — unbounded growth of the HashMap
2. **Incorrect `active_operation_count`** property — reports completed operations as active
3. **`list_operations`** returns stale entries

**Fix:** After the spawned task completes and emits `OperationComplete`, remove the entry from the operations map. Pass an `Arc<Mutex<HashMap>>` clone into the spawned task and call `.remove(&operation_id)` at the end of execution.

---

### RECOMMENDED

#### R1: `cancel()` method lacks polkit authorization check

**File:** `daemon/src/interface.rs`, `cancel()` method (line ~222)

The spec defines `io.github.up.cancel.operation` as a polkit-protected action (Section 5.1), and the policy file includes it. However, the daemon's `cancel()` method does not perform a polkit check — any D-Bus caller can cancel any operation.

**Impact:** Any unprivileged process on the system bus could cancel in-progress system updates. The D-Bus bus policy allows all callers to invoke methods; polkit is the authorization boundary.

**Fix:** Add `#[zbus(header)]` and `#[zbus(connection)]` parameters to `cancel()` and check `io.github.up.cancel.operation` before proceeding.

---

#### R2: No timeout on DaemonExecutor signal loop (client hang risk)

**File:** `src/dbus_client.rs`, `run()` method (the `loop { tokio::select! { ... } }`)

If the daemon crashes or the D-Bus connection drops mid-operation, the client will loop forever waiting for `OperationComplete`. The streams will never yield another item (they'll remain pending indefinitely on a dead connection).

**Fix:** Add a connection-alive check or a reasonable timeout (e.g., 30 minutes for package operations) with `tokio::time::timeout` wrapping the select loop, returning a `BackendError::Spawn("Daemon connection lost")` on timeout.

---

#### R3: `audit::log_operation_complete` is never called

**File:** `daemon/src/executor.rs`

The function `audit::log_operation_complete` exists in `audit.rs` but is never invoked from the executor after an operation finishes. The spec (Section 4.6) requires both START and COMPLETE audit entries.

**Fix:** Call `audit::log_operation_complete(&op_id, overall_success, overall_exit_code)` at the end of the spawned task in `executor.rs`, after emitting the `OperationComplete` signal.

---

#### R4: Missing `ReadWritePaths` in systemd service for package management

**File:** `data/io.github.up.Daemon.service`

The unit has `ProtectSystem=strict` which mounts the filesystem read-only except for `/dev`, `/proc`, `/sys`. Package managers (apt, dnf, pacman) need write access to `/var`, `/usr`, `/etc`, and `/opt`. Without `ReadWritePaths=`, all update commands will fail with permission errors.

**Fix:** Add:
```ini
ReadWritePaths=/var /usr /etc /opt /tmp
```
Or alternatively, change to `ProtectSystem=full` (protects `/usr` and `/boot` only) since the daemon *needs* to modify system packages.

---

#### R5: `xbps-install -Syu` lacks `--yes` flag for non-interactive execution

**File:** `data/backends.d/xbps.yaml`

The XBPS update command `xbps-install -Syu` will prompt for confirmation interactively. Since the daemon runs with `Stdio::null()` on stdin, this will cause the command to hang or fail.

**Fix:** Change args to `["-Syu", "--yes"]` or the equivalent `["-Sy", "-u", "--yes"]`.

---

### INFORMATIONAL

#### I1: Size unit conflation in parser.rs

**File:** `src/plugins/parser.rs`, `detect_size_multiplier()`

The function treats "GB" and "GiB" identically (both use 1024^3). Strictly, GB = 10^9 and GiB = 2^30. This causes ~7% size overestimation for decimal units. Low priority since this is only used for UI display estimates.

---

#### I2: DaemonExecutor `run()` ignores `_program` and `_args` parameters

**File:** `src/dbus_client.rs`

The `CommandExecutor::run()` implementation ignores the program/args parameters and uses `self.backend_id` to tell the daemon which operation to run. This is architecturally correct (the daemon has its own allowlist), but the trait interface is slightly mismatched. A `run_backend()` method on DaemonExecutor would be more explicit. No fix needed — this is a design trade-off for maintaining the `CommandExecutor` trait.

---

#### I3: No upgrade commands registered in allowlist

**File:** `daemon/src/allowlist.rs`

The `upgrade_commands` HashMap is empty — `run_upgrade()` will always return `InvalidArgs`. This is expected per the spec's phased rollout (Phase B/C) and noted in the implementation plan.

---

#### I4: Dead code warnings in daemon crate

5 warnings for unused code — all expected for a newly created API whose callers haven't been fully wired up yet. No action required.

---

#### I5: `file_exists` detection field not validated for path traversal

**File:** `src/plugins/validate.rs`

The validator checks the `binary` and `args` fields for path traversal, but doesn't validate `detection.file_exists`. A plugin could probe arbitrary paths (e.g., `/etc/shadow`). This is read-only (existence check), so it leaks no file content, only presence/absence. Low risk.

---

#### I6: `futures-util` added to frontend Cargo.toml

The `futures-util` crate is used in `dbus_client.rs` for `StreamExt`. This is appropriate and well-established in the Tokio ecosystem.

---

## Specification Compliance Matrix

| Spec Requirement | Status | Notes |
|-----------------|--------|-------|
| D-Bus interface `io.github.up.Daemon1` | ✅ Implemented | All methods, signals, properties present |
| Polkit authorization per operation | ✅ Implemented | Missing only for `cancel()` (R1) |
| Command allowlist | ✅ Implemented | Built-in backends defined; plugin registration API ready |
| Audit logging | ⚠️ Partial | START logged; COMPLETE never called (R3) |
| Cancellation (SIGTERM→SIGKILL) | ✅ Implemented | Process group signaling works correctly |
| Idle timeout (60s) | ✅ Implemented | Poll-based check every 5s |
| systemd service file | ⚠️ Needs fix | ProtectSystem=strict too restrictive (R4) |
| D-Bus bus configuration | ✅ Correct | Root owns, default policy allows calls |
| Polkit policy (scoped actions) | ✅ Implemented | All 6 actions + 2 legacy actions |
| Plugin YAML schema | ✅ Implemented | All parser types, capabilities, metadata |
| Plugin discovery (XDG paths) | ❌ Priority bug | `.rev()` inverts override semantics (C1) |
| Plugin validation (security) | ✅ Implemented | Shell metachar, path traversal, env allowlist, polkit prefix |
| PluginBackend trait impl | ✅ Implemented | Delegates to CommandExecutor correctly |
| Frontend D-Bus client | ✅ Implemented | Fallback detection, signal streaming, proxy |
| Graceful fallback to pkexec | ✅ Implemented | `detect_execution_mode()` probes daemon |
| BackendKind::Plugin variant | ✅ Implemented | Display, Serialize, Deserialize all handled |
| Plugin deduplication | ✅ Implemented | Built-in backends not shadowed by same-ID plugins |
| Resource limits (256M, 50% CPU) | ✅ Implemented | systemd MemoryMax/CPUQuota |
| Workspace Cargo.toml | ✅ Correct | `members = [".", "daemon"]` |
| Meson install targets | ✅ Correct | Daemon, service, conf, plugins all installed |
| Nix flake packaging | ✅ Correct | Daemon binary, service files, D-Bus conf installed |
| Preflight script | ✅ Updated | Step 3b builds daemon crate |

---

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 88% | B+ |
| Best Practices | 90% | A- |
| Functionality | 82% | B |
| Code Quality | 93% | A |
| Security | 85% | B+ |
| Performance | 88% | B+ |
| Consistency | 95% | A |
| Build Success | 95% | A |

**Overall Grade: B+ (90%)**

---

## Verdict: **NEEDS_REFINEMENT**

### Rationale

Two CRITICAL issues must be resolved before approval:

1. **C1 (Plugin discovery priority inversion)** — This is a security-relevant correctness bug. Admin overrides in `/etc/up/backends.d/` will be silently ignored, and lower-priority system plugins will win. Single-character fix (remove `.rev()`).

2. **C2 (Operations never cleaned up)** — The daemon will report incorrect state and leak memory. Under D-Bus activation patterns (start→work→idle-exit→restart), this is partially mitigated by the 60s exit, but during heavy use the operations map will grow unboundedly and `active_operation_count` / `list_operations` will return stale data.

### RECOMMENDED fixes strongly advised:

- **R4 (ProtectSystem=strict)** is likely to cause all update operations to fail at runtime. This should be treated as borderline-CRITICAL since the daemon literally cannot perform its primary function without filesystem write access.
- **R1 (cancel polkit check)** — any bus client can cancel operations without authorization.
- **R3 (audit completion)** — incomplete audit trail violates spec.

---

## Files Requiring Changes

| File | Issues |
|------|--------|
| `src/plugins/discovery.rs` | C1 |
| `daemon/src/executor.rs` | C2, R3 |
| `daemon/src/interface.rs` | C2 (operation map cleanup), R1 |
| `data/io.github.up.Daemon.service` | R4 |
| `data/backends.d/xbps.yaml` | R5 |
| `src/dbus_client.rs` | R2 |
