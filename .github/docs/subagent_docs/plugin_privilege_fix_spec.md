# Spec: Fix plugin backends with `needs_root: true` running unprivileged (item 3)

## Current state analysis

- `src/orchestrator.rs:96-117` checks `backends.iter().any(|(b, _)| b.needs_root())` and, if
  true, authenticates once via `PrivilegedShell::new()` (a pre-authed
  `pkexec sh` process), storing it in `shell`. Each backend then gets a
  `CommandRunner::new(be_tx, kind, shell.clone())` (orchestrator.rs:153).
- `src/runner.rs:298-307` (`CommandRunner::run`): only routes a command
  through the pre-authed `PrivilegedShell` **if the literal `program`
  argument is `"pkexec"`**. Otherwise it spawns `program` directly as the
  unprivileged current user (runner.rs:309-314).
- Every built-in backend that needs root calls
  `runner.run("pkexec", &[actual_program, ...actual_args])` (confirmed in
  `src/backends/os_package_manager.rs` and `src/backends/nix.rs` — e.g.
  `runner.run("pkexec", &[nixd_path.as_str(), "upgrade"])`, or with an
  `env` prefix for PATH/locale vars:
  `runner.run("pkexec", &["env", "LANG=C", ..., program, ...args])`).
- `src/plugins/backend.rs`'s `PluginBackend::run_update` (line 62, was
  line 62 pre-item-4) and `run_cleanup` (line 135) call
  `runner.run(&cmd.program, &args)` — the **raw plugin-declared program
  name**, never `"pkexec"`. So even though `needs_root()` (line 48-50,
  delegates to `descriptor.privilege.needs_root`) correctly reports
  `true` and triggers the orchestrator's one-time polkit auth, the actual
  command execution bypasses the elevated shell entirely and runs
  unprivileged — it fails with a permissions error after wasting the
  auth prompt. This matches the bug description exactly.
- All four shipped/example `needs_root: true` plugin descriptors
  (`data/backends.d/apk.yaml`, `xbps.yaml`,
  `examples/plugins/eopkg.yaml`, `swupd.yaml`) declare a non-empty
  `commands.update.environment` (e.g. `LANG: "C"`, `LC_ALL: "C"`) —
  required so their `regex_count` parsers match C-locale output. This
  means a correct fix must also apply `cmd.environment` when routing
  through `pkexec`, not just prepend `"pkexec"` — otherwise the parser
  would silently undercount (or read 0) once the privilege bug is fixed,
  because `CommandExecutor::run(program, args)` has no environment
  parameter and currently drops it unconditionally.
- `src/executor.rs:9-19` (`CommandExecutor` trait): `run(&self, program,
  args)` — no env parameter, confirmed.
- `src/runner.rs:116-137` (`PrivilegedShell::run_command`): builds a
  shell command line by `shell_quote`-ing each element of `args` and
  joining with spaces, then executes it inside the pre-authed root shell.
  So `runner.run("pkexec", &["env", "LANG=C", "xbps-install", "-Syu"])`
  is routed straight into the elevated shell as
  `env LANG=C xbps-install -Syu` — no nested shell parsing, no injection
  risk from metacharacters, since each arg becomes a literal argv element
  to `env`/the target program (not shell-interpolated). This matches the
  existing `pkexec env VAR=VAL ... program args` pattern already used in
  `src/backends/nix.rs` for PATH injection — reusing the same idiom keeps
  this consistent with master plan item 32's inventory rather than adding
  a third style.
- `src/plugins/validate.rs` already guards plugin descriptors at load
  time: `needs_root` plugins must come from a non-user-writable path
  (`is_user_path` check), `polkit_action` must match an allowlisted
  prefix, command `args` may not contain shell metacharacters or path
  traversal, `program` may not be an absolute path or contain `/`, and
  environment variable **keys** are restricted to a fixed allowlist
  (`LANG`, `LC_ALL`, `LC_MESSAGES`, `DEBIAN_FRONTEND`, `HOME`, `PATH`).
  This is why the plain `env KEY=VALUE program args` argv-list approach
  is safe without additional escaping — no shell re-parses these strings.

## Problem definition

Plugin backends declaring `needs_root: true` prompt the user for admin
authentication (because `needs_root()` correctly signals it to the
orchestrator) but then execute their actual update/cleanup command
directly as the unprivileged user, because `PluginBackend::run_update`
and `run_cleanup` never route through `"pkexec"`. The update always fails
with a permissions error after wasting an auth prompt.

## Proposed solution

Add a small private helper on `PluginBackend` that builds the command to
execute for a given `CommandDef`: when `descriptor.privilege.needs_root`
is true, prefix with `"pkexec"` (and, if the command declares any
`environment` entries, an `env KEY=VALUE ...` segment ahead of the
program) instead of calling `runner.run(&cmd.program, &args)` directly.
Use this helper from both `run_update` and `run_cleanup` (the two
privileged call sites); `list_available` and `estimate_size` are
correctly documented and implemented as always-unprivileged direct
spawns and are unaffected.

## Implementation steps

1. In `src/plugins/backend.rs`, add a private async helper:
   ```rust
   async fn run_command(
       &self,
       cmd: &super::descriptor::CommandDef,
       runner: &dyn CommandExecutor,
   ) -> Result<String, crate::backends::BackendError> {
       if self.descriptor.privilege.needs_root {
           let mut owned_args: Vec<String> = Vec::new();
           if !cmd.environment.is_empty() {
               owned_args.push("env".to_string());
               for (key, value) in &cmd.environment {
                   owned_args.push(format!("{key}={value}"));
               }
           }
           owned_args.push(cmd.program.clone());
           owned_args.extend(cmd.args.iter().cloned());
           let args: Vec<&str> = owned_args.iter().map(String::as_str).collect();
           runner.run("pkexec", &args).await
       } else {
           let args: Vec<&str> = cmd.args.iter().map(String::as_str).collect();
           runner.run(&cmd.program, &args).await
       }
   }
   ```
2. Replace the body of `run_update` (currently: build `args`, call
   `runner.run(&cmd.program, &args)`) with a call to
   `self.run_command(cmd, runner).await`.
3. Replace the equivalent block in `run_cleanup` the same way.
4. `list_available` and `estimate_size` are untouched — they are
   documented as always-unprivileged.

## Dependencies

None — no new crates, pure internal routing fix.

## Configuration changes

None. No descriptor schema changes; `environment` is already a declared,
validated field being newly *honored* on the privileged path rather than
silently dropped.

## Risks and mitigations

- **Risk:** Building `env KEY=VALUE` strings from HashMap entries is
  non-deterministic in iteration order across runs. **Mitigation:**
  order doesn't matter for `env`'s argv — each `KEY=VALUE` pair sets one
  variable independently; no ordering dependency between them.
- **Risk:** A future plugin environment value containing `=` breaks the
  `KEY=VALUE` split. **Mitigation:** N/A here — we're only *constructing*
  `KEY=VALUE` strings (key and value are already separate strings from
  the `HashMap<String, String>`), not parsing them, so this isn't a
  concern.
- **Risk:** Widening the blast radius by touching `list_available`/
  `estimate_size`. **Mitigation:** explicitly out of scope; not touched.
- **Risk:** `owned_args`/borrow lifetime — `runner.run` takes `&'a str`
  args tied to the `PluginBackend`'s `'a` lifetime in the trait method
  signature, but `owned_args` is a local `Vec<String>` created inside the
  async block. **Mitigation:** verify via `cargo build` — the `&str`
  slice borrows from `owned_args` which outlives the `.await` point
  within the same async block (it's not moved out), matching the pattern
  already used for `args` in the existing non-root branch.
