# Review: Remove the unwired D-Bus daemon (item 4, folds in item 5)

Spec: `.github/docs/subagent_docs/daemon_removal_spec.md`

Modified/removed files:
- Removed: `daemon/` (whole crate, 9 files)
- Removed: `src/dbus_client.rs`
- Removed: `data/io.github.up.Daemon.conf`
- Removed: `data/io.github.up.Daemon.service`
- Modified: `Cargo.toml` (workspace members, dropped `zbus`/`tokio-util`/`futures-util`)
- Modified: `meson.build` (daemon build target, libexecdir, D-Bus policy install, systemd system unit install)
- Modified: `flake.nix` (postInstall daemon section)
- Modified: `data/io.github.up.policy` (removed 4 daemon-only actions, reworded stale "legacy"/"transition period" wording on the 2 actions actually in effect)
- Modified: `src/plugins/backend.rs` (stale doc comment referencing daemon routing)
- Modified: `scripts/preflight.sh` (removed Step 3b `cargo build -p up-daemon`, discovered failing during Phase 6 — direct consequence of the crate deletion, not scope creep)
- Regenerated: `Cargo.lock` (workspace member removed, 3 deps dropped)

## Findings

1. **Specification Compliance** — All steps in the spec executed: crate
   deleted, dead client deleted, workspace/deps trimmed, meson/flake
   packaging trimmed, policy file trimmed while preserving the two
   actually-live `pkexec.*` actions (verified they're matched by
   `org.freedesktop.policykit.exec.path`, not caller-supplied action ID)
   and the two `ALLOWED_POLKIT_PREFIXES`-referenced actions
   (`update.system`, `cleanup.system`) still used by plugin descriptor
   validation (`src/plugins/validate.rs:22`).
2. **Best Practices** — Deletion was surgical: only daemon-specific
   packaging blocks removed, unrelated blocks (`dbus` buildInput for
   GTK/glib, systemd user-unit machinery for the `--check` timer) left
   intact.
3. **Consistency** — Follows the master plan's own item-4 delete-path
   description (daemon crate + packaging + policy) exactly.
4. **Maintainability** — Removes ~1300 lines of unreachable code (daemon
   crate + dead client) and simplifies `meson.build`/`flake.nix`. Cleaned
   up one stale doc comment in `src/plugins/backend.rs` that described
   the now-removed daemon-routing option, and reworded polkit action
   descriptions that called themselves "(legacy pkexec path)" — they are
   now simply the path.
5. **Completeness** — Verified via `grep -rln "daemon\|dbus_client"`
   across `.rs`/`.toml`/`.build`/`.nix` files that all remaining hits are
   unrelated (fwupd's own system daemon, Determinate Nix's `nixd`
   daemon) — no orphaned references to the deleted crate remain.
   `README.md` and `po/POTFILES.in` had no daemon references to begin
   with (checked in Phase 1).
6. **Performance** — N/A (no runtime behavior change to existing paths;
   removes a background systemd service and D-Bus policy registration
   that was never invoked).
7. **Security** — Net reduction in attack surface: a root-privileged,
   always-running D-Bus service reachable by any local user
   (`context="default"` in the now-deleted `Daemon.conf`) is removed.
   Verified the two retained `pkexec.*` polkit actions are the ones
   actually exercised by `src/runner.rs` and `src/upgrade/execute.rs` —
   removing them would have broken all privileged operations, so they
   were deliberately kept.
8. **API Currency** — N/A, no external library integration.
9. **Build Validation:**

   `cargo build`:
   ```
   Compiling up v2.2.0 (/home/nimda/Projects/Up)
   Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.55s
   ```

   `cargo clippy -- -D warnings`:
   ```
   Checking up v2.2.0 (/home/nimda/Projects/Up)
   Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.91s
   ```
   No warnings — confirms no orphaned imports/dead code from the
   deletion (single-crate workspace compiles clean).

   `cargo fmt --check`: clean, no diff.

   `cargo test`:
   ```
   test result: ok. 106 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
   ```
   (unchanged from pre-removal baseline — daemon crate had no shared
   test dependency on the root crate).

## Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 100% | A |
| Functionality | 100% | A |
| Code Quality | 100% | A |
| Security | 100% | A |
| Performance | 100% | A |
| Consistency | 100% | A |
| Build Success | 100% | A |

**Overall Grade: A (100%)**

## Result: PASS
