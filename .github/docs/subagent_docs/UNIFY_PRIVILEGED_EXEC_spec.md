# UNIFY_PRIVILEGED_EXEC — Specification

MASTER_PLAN item 12 (scope: "Full: async + PrivilegedShell" — user decision).
Source: ARCH M3.

## Problem

Two independent privileged-execution stacks:

- **Update / cleanup / cache-bypass** — async `PrivilegedShell` + `CommandRunner`
  (`src/runner.rs`), authenticates once per run, streams output via
  `BackendEvent`.
- **Distro upgrade** — synchronous `run_command_sync` (`src/runner.rs:456-508`,
  a blocking re-implementation of `CommandRunner::run`) called from
  `std::thread::spawn` in `src/upgrade/execute.rs`. Spawns a **separate
  `pkexec` process per step**: legacy-NixOS upgrade prompts twice, Fedora up
  to four times.

`run_command_sync` is used only by `execute.rs`.

## Design

### `execute_upgrade` becomes async and runs through a shared runner

```rust
pub(crate) async fn execute_upgrade(
    distro: &DistroInfo,
    tx: &async_channel::Sender<String>,      // narrative log lines
    runner: &dyn CommandExecutor,            // command execution + output streaming
) -> Result<(), String>
```

Every `run_command_sync("pkexec", &[ARGS], tx)` becomes:

```rust
runner.run("pkexec", &[ARGS]).await.is_ok()
```

`CommandRunner::run` already routes any `"pkexec"` invocation through the
pre-authenticated `PrivilegedShell` when one is present, so all steps in a
single upgrade share **one polkit prompt**. Command output streams through the
runner's `BackendEvent` channel; narrative lines ("Downloading upgrade
packages…") continue to go through `tx`.

`tx.send_blocking(...)` in the async body becomes `tx.send(...).await`. The
Ubuntu `/var/log/dist-upgrade/main.log` tail helper keeps its own
`std::thread::spawn` + `send_blocking` (it is a passive file follower, not a
command runner).

### Fedora reboot step

`dnf system-upgrade reboot` triggers `systemctl reboot`; systemd SIGTERMs the
GUI (and the privileged shell) before the command returns. It now runs through
the shared runner and a non-`Ok` result is treated as expected:

```rust
let _ = tx.send("Triggering upgrade reboot…".into()).await;
let _ = runner.run("pkexec", &["dnf", "system-upgrade", "reboot"]).await;
Ok(())
```

This removes the last extra `pkexec` — Fedora now prompts **once**.

### New entry point `run_upgrade` (`src/upgrade/execute.rs`)

```rust
pub async fn run_upgrade(
    distro: &DistroInfo,
    log_tx: &async_channel::Sender<String>,
) -> Result<(), String>
```

Owns the privileged-session lifecycle, mirroring `orchestrator::run_cache_bypass`:

1. `PrivilegedShell::new().await` — on `Err`, push the message to `log_tx` and
   return `Err`.
2. Wrap in `Arc<tokio::sync::Mutex<_>>`.
3. `(be_tx, be_rx) = async_channel::unbounded::<BackendEvent>()`.
4. Forwarder task: `BackendEvent::LogLine(_, line)` → `log_tx.send(line)`.
5. `CommandRunner::new(be_tx.clone(), upgrade_kind(&distro.id), Some(shell))`.
6. `let res = execute_upgrade(distro, log_tx, &runner).await;`
7. `drop(be_tx)`, await forwarder, `shell.lock().await.close().await`.
8. return `res`.

`upgrade_kind(distro_id)` maps ubuntu/mint/pop/… → `Apt`, fedora → `Dnf`,
opensuse-leap → `Zypper`, nixos → `Nix`, else `Apt`. Used only to tag log
events; no new `BackendKind` variant is added.

### UI call site (`src/ui/upgrade_page.rs`, ~332-376)

Replace the `std::thread::spawn(|| execute_upgrade(&distro2, &tx_clone))` block
with:

```rust
let (log_tx, log_rx) = async_channel::unbounded::<String>();
let (result_tx, result_rx) = async_channel::bounded::<Result<(), String>>(1);
let distro2 = distro.clone();
crate::ui::spawn_background_async(move || async move {
    let outcome = upgrade::run_upgrade(&distro2, &log_tx).await;
    drop(log_tx);
    let _ = result_tx.send(outcome).await;
});
while let Ok(line) = log_rx.recv().await { log_panel.append_line(&line); }
let outcome = result_rx.recv().await.unwrap_or_else(|_| Err("…".into()));
// unchanged: button.set_sensitive(true); reboot dialog / error line
```

### Delete `run_command_sync`

Remove `src/runner.rs::run_command_sync` (and its now-unused `use` items /
helper text) once `execute.rs` no longer references it.

### `upgrade/mod.rs`

Re-export `run_upgrade` instead of `execute_upgrade`
(`execute_upgrade` stays `pub(crate)` for the module test).

## Dependencies / Context7

None. Internal refactor, no external API changes. Not required.

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| Upgrade paths have no integration tests; real behaviour is distro-specific | Keep each step's command args byte-for-byte identical; only the transport changes. Preserve step ordering and the exact narrative strings. |
| `PrivilegedShell` treats mid-command shell death as an error | Already handled for NixOS activation markers; the Fedora reboot step explicitly ignores a non-`Ok` result. Legacy-NixOS `nixos-rebuild switch --upgrade` benefits from the existing `is_nixos_activation_success` fallback (previously `run_command_sync` returned `false` → spurious failure). |
| `/usr/bin/env PATH=… nix …` args were designed for a fresh `pkexec` | Inside the already-root shell the same argv runs correctly; `env` still sets PATH. No `sh -c` involved. |
| Auth-cancel UX | `PrivilegedShell::new()` `Err` is surfaced to the log panel and returned as `Err`, then the existing "Upgrade failed: …" line renders. Button re-enabled in the unchanged tail of the UI closure. |
| Output ordering (narrative vs streamed) | Single async task; `tx.send().await` before `runner.run().await` preserves order. Forwarder and narrative share one `log_tx`. |

## Success criteria

- No `run_command_sync` anywhere; `execute.rs` has no `std::process`/`pkexec`
  spawn except the passive Ubuntu log-tail file reader.
- `execute_upgrade` module test updated to async, still asserts the
  unsupported-distro `Err`.
- One new unit test: `run_upgrade`/`execute_upgrade` for an unsupported distro
  returns `Err` without touching the runner (via `MockExecutor`).
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo build`,
  `cargo test` all clean; `scripts/preflight.sh` exits 0.
