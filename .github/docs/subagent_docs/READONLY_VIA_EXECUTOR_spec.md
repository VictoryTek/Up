# READONLY_VIA_EXECUTOR — Specification

MASTER_PLAN item 11 (scope: "list_available + estimate_size" — user decision).
Source: ARCH M2.

## Problem

`src/executor.rs` defines `CommandExecutor` "enabling dependency injection and
test doubles", and `run_update` uses it — but `list_available()`,
`estimate_size()`, and `count_available()` spawn `tokio::process::Command`
directly in every backend. Those paths cannot be exercised with `MockExecutor`
and are the parse-heavy, regression-prone parts of each backend.

Out of scope (per user decision): sync detection probes
(`is_nixos`/`is_vexos`/`is_determinate_nix`/`os_package_manager::detect`/flatpak
sandbox probes — need a separate sync `SystemProber`); the `nix profile upgrade`
/ `nix-env` streaming-during-update issue (M2c); `run_cleanup` internal reads.

## Current state

- `CommandExecutor::run()` streams every line as `BackendEvent::LogLine` and
  returns `Err(BackendError)` on non-zero exit — wrong semantics for a
  read-only probe (probes need stdout on failure, must tolerate non-zero exit,
  and should not flood the log panel during a check cycle).
- `Backend::list_available(&self)`, `estimate_size(&self)`,
  `count_available(&self)` take no runner.
- Only two real call sites: `src/check.rs:24` and `src/ui/window.rs:805-806`.
  **No existing test calls these three methods** (only the `mod.rs` default
  impl and those two sites) — low blast radius.
- Direct-spawn sites to migrate: `os_package_manager.rs` (APT/DNF/pacman/zypper
  list + APT/DNF/zypper estimate), `flatpak.rs` (`flatpak_remote_ls_updates`,
  estimate loop), `fwupd.rs` (list + estimate), `homebrew.rs` (list),
  `plugins/backend.rs` (list + estimate), `nix.rs` (determinate `version`,
  `nix-env --dry-run`, `nixos_flake_dry_run_check`).

## Design

### 1. New `probe` method + `ProbeOutput` (`src/executor.rs`)

```rust
pub struct ProbeOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
    pub spawned: bool,   // false => process could not be spawned
}
impl ProbeOutput {
    pub fn ok(&self) -> bool { self.spawned && self.code == Some(0) }
}

pub trait CommandExecutor: Send + Sync {
    fn run<'a>(...) -> ...;                       // unchanged

    /// Run a read-only probe command. Captures stdout+stderr, never treats a
    /// non-zero exit as an error, and does NOT stream to the log panel.
    /// `env` entries are applied to the child's environment.
    fn probe<'a>(
        &'a self,
        program: &'a str,
        args: &'a [&'a str],
        env: &'a [(&'a str, &'a str)],
    ) -> Pin<Box<dyn Future<Output = ProbeOutput> + Send + 'a>>;
}
```

### 2. `SystemExecutor` (`src/executor.rs`) — real non-streaming impl

Zero-field unit struct. `run()` = spawn + capture + `Err` on non-zero
(sufficient for the read-only call sites, which never call `run()`).
`probe()` = `tokio::process::Command` spawn, apply `env`, `.output()`,
map to `ProbeOutput` (`spawned:false` on spawn error).

Used at the two call sites that lack a `BackendEvent` channel.

### 3. `CommandRunner::probe` (`src/runner.rs`)

Non-streaming capture (same as `SystemExecutor::probe`). Keeps the two impls
behaviourally identical; the orchestrator already holds a `CommandRunner` and
may call `probe` for future work, but this pass does not add such calls.

### 4. `MockExecutor::probe` (`src/executor.rs` test_utils)

Consumes the existing FIFO `responses` queue and records the call in `calls`:
- `Ok(s)`  → `ProbeOutput { stdout: s, code: Some(0), spawned: true, .. }`
- `Err(BackendError::Exit { code, message })` → `ProbeOutput { stdout: "",
  stderr: message, code: Some(code), spawned: true }`
- `Err(BackendError::Spawn(_))` → `spawned: false`

Add `MockExecutor::with_probe(stdout, code)` helper for the "non-zero exit but
usable stdout" cases (dnf, fwupd).

### 5. Trait signature change (`src/backends/mod.rs`)

```rust
fn list_available<'a>(&'a self, runner: &'a dyn CommandExecutor)
    -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + 'a>>;
fn estimate_size<'a>(&'a self, runner: &'a dyn CommandExecutor)
    -> Pin<Box<dyn Future<Output = Option<u64>> + Send + 'a>>;
fn count_available<'a>(&'a self, runner: &'a dyn CommandExecutor)
    -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + 'a>>;
```
Default `count_available` delegates to `self.list_available(runner)`.

### 6. Call sites

`check.rs` and `window.rs`: construct `crate::executor::SystemExecutor` once
and pass `&executor` into `count_available` / `list_available`.

### 7. Per-backend migration

Each `tokio::process::Command::new(P).args(A)[.env(...)].output().await`
becomes `runner.probe(P, &A, ENV).await` then branch on `.spawned` / `.code` /
`.stdout` / `.stderr`, preserving each site's existing exit-code handling
(dnf `Some(1)` vs `Some(100)`, fwupd `code == 2`, zypper "ignore status", etc.).

- `flatpak.rs`: `flatpak_remote_ls_updates(scope)` and the estimate loop gain a
  `runner: &dyn CommandExecutor` parameter; `build_flatpak_cmd` output is
  passed to `probe` (sandbox wrapping preserved).
- `nix.rs`: `list_available` gains the runner param; route the
  `determinate-nixd version`, `nix-env -u --dry-run`, and
  `nixos_flake_dry_run_check` spawns through `probe`.
  `nixos_flake_tempdir_check` keeps its direct `Command` (needs `current_dir`
  + filesystem copies — outside the `probe` abstraction); documented in a
  comment.
- `plugins/backend.rs`: pass `cmd.environment` (as `&[(&str,&str)]`) to `probe`.

## Dependencies

None added. No Context7 needed (internal abstraction, no external API change).

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| Exit-code semantics differ per site | Each migration preserves the site's existing `status.code()` branch; `ProbeOutput.code` carries it. |
| `probe` env handling drops a var a plugin relied on | `env` param threads `cmd.environment` through verbatim. |
| Behaviour change from streaming (log spam) | `probe` deliberately does not stream — matches today's behaviour for checks (they don't stream now). |
| Two `probe` impls drift | `SystemExecutor::probe` and `CommandRunner::probe` share identical bodies; small helper `fn spawn_probe(program, args, env)` in `executor.rs` used by both. |
| Wide diff | No call sites beyond the two known; `cargo build` + full `cargo test` gate every step. |

## Success criteria

- New unit tests: at least one `MockExecutor`-driven test for APT, DNF,
  zypper, pacman, flatpak, fwupd, homebrew, and plugin `list_available`
  (and an `estimate_size` test for APT + a plugin) proving the parse path is
  now injectable.
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo build`,
  `cargo test` all clean.
- `scripts/preflight.sh` exits 0.
- No `tokio::process::Command` / `std::process::Command` left in
  `list_available` / `estimate_size` / `count_available` bodies except the
  documented `nixos_flake_tempdir_check`.
