# Spec: Remove the unwired D-Bus daemon (item 4)

## Decision

User decision (2026-08-22): **remove** the daemon rather than wire it in.
Rationale: the GUI already works end-to-end via direct `pkexec` calls
through `PrivilegedShell`/`CommandRunner` (`src/runner.rs`); the daemon
adds a privileged root D-Bus service with its own allowlist/audit/cancel
machinery that nothing calls, a client (`src/dbus_client.rs`) that isn't
even in the module tree, and a `run_upgrade` D-Bus method whose command
table is permanently empty. Keeping an unreachable root-privileged service
installed and packaged is a larger liability than deleting it. This also
unblocks item 5 (unused `zbus`/`futures-util`/`tokio-util` root deps) and
turns items 14/37/41/44 (daemon-internal bugs) moot, and simplifies item 3
(plugin backend privilege fix) to a contained `pkexec`-routing change.

## Current state analysis

- `daemon/` — full crate (`up-daemon` binary), 1102 lines across
  `main.rs`, `interface.rs`, `allowlist.rs`, `auth.rs`, `audit.rs`,
  `cancel.rs`, `executor.rs`, `lifecycle.rs`. Implements a
  polkit-authenticated `io.github.up.Daemon1` D-Bus interface
  (`run_update`, `run_cleanup`, `run_upgrade`, `create_snapshot`,
  `cancel`, `list_operations`, etc.) with a command allowlist, audit
  logging, and idle-lifecycle shutdown.
- `src/dbus_client.rs` — 221-line zbus client for the same interface.
  **Not referenced anywhere** in `src/main.rs`'s module tree (`grep -rn
  "dbus_client" src/main.rs src/app.rs` returns nothing) — dead, doesn't
  even compile as part of the binary.
- Root `Cargo.toml`: `[workspace] members = [".", "daemon"]`, plus
  `zbus`, `futures-util`, `tokio-util` dependencies used only by
  `dbus_client.rs` (confirmed via grep — no other `src/` file references
  `zbus::`, `futures_util`, or `tokio_util`). This is exactly item 5's
  scope; folding it into this same deletion since it's the same edit.
- `daemon/Cargo.toml` — separate crate manifest with its own `zbus`,
  `tokio`, `tokio-util`, `serde`, `uuid`, etc. deps; removed wholesale
  with the crate directory.
- Packaging surfaces that reference the daemon:
  - `meson.build`: `cargo_build_daemon` custom_target (lines ~46-58),
    the `libexecdir` variable (used only for the daemon), the "D-Bus
    daemon configuration" `install_data('data/io.github.up.Daemon.conf'
    ...)` block, and the "Systemd system units (daemon)"
    `install_data('data/io.github.up.Daemon.service' ...)` block +
    `systemd_system_unit_dir` lookup.
  - `flake.nix`: `postInstall`'s "D-Bus daemon" section (moves
    `up-daemon` binary to `$out/libexec`, installs `.Daemon.service` and
    `.Daemon.conf`). `cargoBuildFlags = [ "--workspace" ]` will still
    build correctly once `daemon` is removed from the workspace members
    (it'll just build the single remaining crate).
  - `data/io.github.up.Daemon.conf` (D-Bus system policy) and
    `data/io.github.up.Daemon.service` (systemd system unit) — daemon-only
    packaging files, deleted outright.
  - `data/io.github.up.policy` (polkit actions) contains a **mix** of
    actions:
    - Daemon-only actions checked via `daemon/src/auth.rs`'s
      `check_polkit()` (D-Bus `CheckAuthorization` calls): none of these
      action IDs are passed to `CheckAuthorization` anywhere once the
      daemon is deleted — **except** `io.github.up.update.system`, which
      is also referenced as the `polkit_action` value in plugin
      descriptors (`data/backends.d/{apk,xbps}.yaml`,
      `examples/plugins/{eopkg,swupd}.yaml`) and validated against
      `ALLOWED_POLKIT_PREFIXES` in `src/plugins/validate.rs:22`. That
      field is descriptor metadata only — nothing currently calls
      polkit's `CheckAuthorization` with it (that only happened inside
      the now-deleted daemon) — but it is validated/parsed at plugin-load
      time, so the action ID string must remain declared in the `.policy`
      file for `pkexec`'s own PolicyKit lookup to have *a* matching
      action if a plugin's `needs_root` path ever routes through
      `pkexec` (which is exactly what item 3 will do next). **Keep**
      `io.github.up.update.system` and `io.github.up.cleanup.system`
      (the two prefixes `ALLOWED_POLKIT_PREFIXES` allows) — actually,
      per investigation below, `pkexec` does not consult these action IDs
      at all (see next paragraph), so keeping them is for descriptor
      validation purposes only, no functional coupling to remove.
    - The "Legacy actions retained for backward compatibility... used by
      the pkexec fallback path" — `io.github.up.pkexec.update` and
      `io.github.up.pkexec.upgrade` — these carry
      `org.freedesktop.policykit.exec.path` annotations
      (`/bin/sh`, `/usr/bin/env`). This is how `pkexec` resolves which
      polkit action applies to a given invocation: by matching the
      resolved executable path against an action's
      `policykit.exec.path` annotation, **not** by the caller supplying
      an action ID. These two are what's actually in effect today for
      every `pkexec sh ...` / `pkexec env ...` call in `src/runner.rs`
      and `src/upgrade/execute.rs`. They must stay — they were never
      daemon-only despite the "during the transition period" comment.
  - `flake.nix` `buildInputs`/`nativeBuildInputs` (`dbus` package) stays —
    still needed for GTK/glib D-Bus session-bus usage elsewhere in the
    app (unrelated to the daemon) and to keep `pkg-config` happy for the
    `dbus` system dependency some GTK components probe for.

## Problem definition

A fully-built, installed, root-privileged D-Bus daemon exists and is
packaged (systemd unit, D-Bus policy, polkit actions) but has zero
callers in the shipped GUI binary. Its allowlist has already drifted from
what the GUI runs, and `run_upgrade` can never succeed (empty command
table). Per the master plan, this needs resolving before item 3 (plugin
privilege bug) and item 9 (selective updates) can be finalized cleanly.

## Proposed solution

Delete the daemon crate, its dead GUI-side client, and all packaging that
references it. Trim `data/io.github.up.policy` to only the action IDs
still in effect: the two `pkexec.*` legacy/exec-path actions (actually
live) and the `io.github.up.update.system` / `io.github.up.cleanup.system`
descriptor-validation action IDs referenced by plugin YAML
(`ALLOWED_POLKIT_PREFIXES`). Remove the three now-fully-unused root
dependencies (`zbus`, `futures-util`, `tokio-util`) — this is item 5,
folded into the same change since it's the identical edit (delete the
only file that used them).

## Implementation steps

1. Delete `daemon/` directory (whole crate).
2. Delete `src/dbus_client.rs`.
3. Edit root `Cargo.toml`:
   - `[workspace] members = ["."]` (drop `"daemon"`).
   - Remove `zbus`, `tokio-util`, `futures-util` dependency lines.
4. Edit `meson.build`:
   - Remove the `cargo_build_daemon` custom_target block and the
     `libexecdir` variable (only consumer).
   - Remove the "D-Bus daemon configuration" `install_data(...Daemon.conf)`
     block.
   - Remove the "Systemd system units (daemon)" block
     (`systemd_system_unit_dir` lookup + `install_data(...Daemon.service)`).
5. Delete `data/io.github.up.Daemon.conf` and
   `data/io.github.up.Daemon.service`.
6. Edit `data/io.github.up.policy`: remove the daemon-only action blocks
   (`io.github.up.update.plugin`, `io.github.up.upgrade.system`,
   `io.github.up.snapshot.create`, `io.github.up.cancel.operation`) —
   these are never checked by anything once the daemon is gone. Keep
   `io.github.up.update.system`, `io.github.up.cleanup.system` (still
   referenced by `ALLOWED_POLKIT_PREFIXES` / plugin descriptors) and both
   `io.github.up.pkexec.*` legacy actions (actually the live `pkexec`
   path). Update the stale "Legacy actions retained... during the
   transition period (v1.x → v2.x)" comment since there's no longer a
   daemon path to transition away from — reword to state these are the
   actions actually used by the `pkexec` invocations in `runner.rs` /
   `upgrade/execute.rs`.
7. Edit `flake.nix`: remove the "D-Bus daemon" block from `postInstall`
   (`mkdir -p $out/libexec`, the `mv $out/bin/up-daemon ...` line, and the
   two `install -Dm644 data/io.github.up.Daemon.*` lines).

## Dependencies

None — pure deletion, no new libraries, no Context7 lookup applicable.

## Configuration changes

Packaging-only changes described above (meson, flake, polkit policy,
systemd unit removal). No runtime config schema changes.

## Risks and mitigations

- **Risk:** Removing the two "legacy" `pkexec.*` policy actions by
  mistake would break every privileged operation (they're what `pkexec`
  actually matches on today). **Mitigation:** explicitly keep them;
  verified via `org.freedesktop.policykit.exec.path` annotations that
  they're the live path, not daemon-only.
- **Risk:** Removing `io.github.up.update.system` /
  `.cleanup.system` could break plugin descriptor validation
  (`ALLOWED_POLKIT_PREFIXES` in `src/plugins/validate.rs`) if any code
  path cross-checks the `.policy` file contents at runtime.
  **Mitigation:** confirmed `ALLOWED_POLKIT_PREFIXES` is a compile-time
  Rust constant checked against the YAML descriptor string, not read from
  `io.github.up.policy` — so the `.policy` file doesn't gate this at
  runtime either way, but keeping the entries preserves the documented
  action namespace for future `pkexec`-with-explicit-action wiring (item
  3) without another packaging change.
- **Risk:** `flake.nix`'s `cargoBuildFlags = [ "--workspace" ]` might
  behave oddly with a single-member workspace. **Mitigation:** a
  workspace with one member is valid Cargo; `--workspace` just builds
  that one member. Verify via `nix flake check` in Phase 6.
- **Risk:** Orphaned references elsewhere (README, po/POTFILES.in) —
  checked, none found.
- **Risk:** `daemon` binary path removal breaks a NixOS system unit some
  user already has installed. **Mitigation:** out of scope for source
  removal; this is a packaging-version concern for release notes, not
  code correctness.
