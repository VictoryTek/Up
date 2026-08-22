# Spec: Wire up `up --check` CLI argument handling

## Current state analysis

- `src/main.rs:23-28` calls `env_logger::init()` then unconditionally
  constructs `UpApplication` and calls `app.run()`, handing all process
  arguments to GTK/`GApplication`. GTK rejects the unrecognized `--check`
  flag and the process exits non-zero.
- `src/check.rs` is a complete, already-correct implementation of
  `run_check()`: detects backends, counts pending updates, compares against
  a stamp file (`$XDG_CACHE_HOME/up/last-check-count`), and fires a
  `notify-send` desktop notification when the count changed and is
  non-zero. It is declared as a module (`mod check;` in `main.rs:5`) but
  `run_check()` has zero callers, so it's dead code (module carries
  `#![allow(dead_code)]`).
- `check.rs::run_check()` calls `env_logger::init()` itself (line 15) —
  this duplicates `main()`'s own call. `env_logger::init()` panics if a
  global logger is already installed, so both cannot run unconditionally
  in the same process.
- `data/io.github.up-check.service.in` (a systemd oneshot unit) already
  runs `@BINDIR@/up --check`, and `data/io.github.up-check.timer` fires it
  daily. This packaging is already in place and unaffected by this change.
- `gio::resources_register_include!` in `main.rs:24` registers the
  GResource bundle (icons, CSS, UI templates) needed only by the GTK app,
  not by the headless check path.

## Problem definition

`main()` never inspects `std::env::args()`, so `up --check` (invoked daily
by the systemd timer) fails every time instead of running the background
update check.

## Proposed solution

1. In `main()`, after the single `env_logger::init()` call, inspect
   `std::env::args()` for a `--check` flag. If present, call
   `check::run_check()` and return `gtk::glib::ExitCode::SUCCESS` without
   touching GTK/GResources.
2. Otherwise (normal GUI path), keep existing behavior: register
   GResources, build `UpApplication`, call `app.run()`.
3. Remove the redundant `env_logger::init()` call from
   `check::run_check()` (main.rs now owns the single init).
4. Remove `#![allow(dead_code)]` from `src/check.rs` now that it has a
   real caller, per master plan item 15's incremental-removal guidance.

## Implementation steps

1. Edit `src/main.rs`:
   - Keep `env_logger::init()` as the first line of `main()`.
   - Add an early-return branch: if `std::env::args().any(|a| a ==
     "--check")`, call `check::run_check()` and return
     `gtk::glib::ExitCode::SUCCESS`.
   - Move `gio::resources_register_include!` below that branch so it only
     runs for the GUI path.
2. Edit `src/check.rs`:
   - Remove the `env_logger::init()` call inside `run_check()`.
   - Remove the `#![allow(dead_code)]` module attribute.

## Dependencies

None — no new crates, no Context7 lookup needed (pure internal wiring, no
external library integration).

## Configuration changes

None. `data/io.github.up-check.service.in` and `.timer` already invoke
`up --check` correctly; no packaging changes needed.

## Risks and mitigations

- **Risk:** `env_logger::init()` double-init panic if both `main()` and
  `check.rs` call it. **Mitigation:** remove the duplicate call in
  `check.rs`, keep the single call in `main()`.
- **Risk:** Removing `#![allow(dead_code)]` from `check.rs` could surface
  new dead-code warnings if any helper in the file is still unused after
  wiring. **Mitigation:** verify via `cargo clippy -- -D warnings` in
  Phase 3; `run_check()` exercises every private helper in the file
  (`stamp_file_path`, `read_stamp`, `write_stamp`, `send_notification`),
  so no warnings are expected.
- **Risk:** Argument parsing is naive (`args().any(|a| a == "--check")`)
  and doesn't reject unknown flags or support `--help`. **Mitigation:**
  out of scope — the spec only requires making the documented `--check`
  flag work, matching what the systemd unit invokes. No other flags are
  used anywhere in the codebase or packaging.
