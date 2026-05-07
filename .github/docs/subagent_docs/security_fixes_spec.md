# Security Fixes — Specification
> Generated: May 6, 2026  
> Covers Section 3 / Section 5 items from `CODEBASE_ANALYSIS.md`

---

## 0. Overview

This specification covers all security issues from `CODEBASE_ANALYSIS.md` that are not yet
marked complete. Issues are addressed in severity order. All fixes target correctness and
defence-in-depth without restructuring the existing async architecture.

### Files to Be Modified

| File | Issues Addressed |
|------|-----------------|
| `src/runner.rs` | 5.1/3.1 (sentinel spoofing + arg injection), 3.2 (timeout), 5.2 (shell_quote) |
| `src/backends/flatpak.rs` | 5.3 (unsigned bundle), 5.4 (inline Python script) |
| `src/upgrade.rs` | 5.6 (pkexec sh -c interpolation) |
| `src/ui/log_panel.rs` | 5.8 (ANSI stripping) |
| `data/io.github.up.policy` | 5.7 (new file — polkit policy) |
| `meson.build` | 5.7 (install the policy file) |
| `Cargo.toml` | 3.2 (add `tokio/time` feature) |

### New Files

| File | Purpose |
|------|---------|
| `data/io.github.up.policy` | Scoped polkit actions for Up |

### New Dependencies

| Crate / Feature | Reason | Already present? |
|-----------------|--------|-----------------|
| `tokio` feature `"time"` | `tokio::time::timeout` for per-command deadline | No — must be added |

No new crates are required. All other fixes use the Rust standard library.

---

## 1. Issue 5.1 / 3.1 — Sentinel Spoofing & Arg Injection (HIGH)

### Current State

**File**: `src/runner.rs`

`PrivilegedShell::run_command` writes the following script to the root shell's stdin
(lines 100–114 approximately):

```rust
const RC_MARKER: &str = "___UP_RC_";

let cmd_line = args
    .iter()
    .map(|a| shell_quote(a))
    .collect::<Vec<_>>()
    .join(" ");

let script = format!("{cmd_line} 2>&1\necho '{RC_MARKER}'$?'___'\n");
```

The sentinel `___UP_RC_<N>___` is parsed from the **combined stdout stream** of the
command being run. Any subprocess that prints a matching line to its stdout (or stderr,
since `2>&1` merges them) will cause the reader loop to interpret that line as a successful
exit, truncating real output and misreporting the exit code.

Additionally, no validation is performed on the contents of `args`. An `arg` value
containing a literal `\n` character followed by arbitrary shell syntax would be
interpreted by the root `/bin/sh` as a second, separate shell command — arbitrary code
execution with root privileges.

**Affected code** (reader loop, lines ~120–143):

```rust
if let Some(rest) = trimmed.strip_prefix(RC_MARKER) {
    if let Some(code_str) = rest.strip_suffix("___") {
        let code: i32 = code_str.parse().unwrap_or(-1);
        if code == 0 {
            return Ok(full_output);
        }
        return Err(format!("Command exited with code {code}"));
    }
}
```

**Risk assessment**: Currently safe because all call sites pass compile-time `&str`
literals. However, the CODEBASE_ANALYSIS correctly notes this is "one refactor away from
disaster." The fix should be applied proactively.

### Proposed Fix

**Two independent hardening measures, both required:**

#### 1a — Randomised per-session sentinel

Add a `session_id: String` field to `PrivilegedShell`. Derive it at `new()` time from the
process ID and a nanosecond timestamp (no new dependency required):

```rust
pub struct PrivilegedShell {
    child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    reader: BufReader<tokio::process::ChildStdout>,
    /// Unique token included in every sentinel for this session.
    /// Prevents any subprocess from spoofing exit-code markers by guessing
    /// the fixed compile-time constant.
    session_id: String,
}
```

In `PrivilegedShell::new()`, before the first `write_all`, generate:

```rust
let pid = std::process::id();
let ts = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap_or_default()
    .subsec_nanos();
// session_id is something like "3f2a_1a2b3c4d" — opaque to any subprocess
let session_id = format!("{:x}_{:x}", pid, ts);
```

Remove `const RC_MARKER`. Replace every use with `self.session_id`-derived markers at
runtime:

```rust
// In run_command:
let rc_prefix = format!("___UP_RC_{}_", self.session_id);
let rc_suffix = "___";

let script = format!(
    "{cmd_line} 2>&1\nprintf '%s%d%s\\n' '{rc_prefix}' $? '{rc_suffix}'\n",
    rc_prefix = rc_prefix,
    rc_suffix = rc_suffix,
);
```

Reader loop check:

```rust
if let Some(rest) = trimmed.strip_prefix(&rc_prefix) {
    if let Some(code_str) = rest.strip_suffix(rc_suffix) {
        let code: i32 = code_str.parse().unwrap_or(-1);
        // ... same logic as before
    }
}
```

**Why `printf` instead of `echo`**: `printf '%s%d%s\n'` is immune to the args that some
`echo` implementations interpret specially (e.g., `-n`, `-e`). It also precisely formats
the exit-code integer without trailing whitespace issues.

#### 1b — Validate args for control characters

At the top of `run_command`, before constructing `cmd_line`, reject any argument that
contains `\n`, `\r`, or `\0`:

```rust
for arg in args {
    if arg.contains(['\n', '\r', '\0']) {
        return Err(format!(
            "Security: argument contains forbidden control character: {:?}",
            arg
        ));
    }
}
```

This prevents the injection vector where a newline in an argument is interpreted by the
root shell as a command separator.

### Risks and Edge Cases

- The `session_id` derivation uses non-cryptographic sources; collision probability is
  negligible in practice (32-bit PID × 30-bit nanosecond sub-second counter). For the
  purpose of defeating accidental or malicious output matching this is sufficient.
- `printf` is a POSIX shell built-in, available in `/bin/sh` on all supported distros.
- The existing `___UP_READY___` handshake in `new()` is a fixed string emitted by Up
  itself (`echo '___UP_READY___'`), not by any subprocess. It is not vulnerable to
  spoofing by user commands. It does not need to be randomised.

---

## 2. Issue 3.2 — No Per-Command Timeout / No pkexec Exit-Code Surfacing (MEDIUM)

### Current State

`PrivilegedShell::run_command` blocks in a `loop { read_line … }` with no deadline.
A stuck package manager (e.g., `apt` waiting on a dpkg lock indefinitely) will hang the
GTK UI refresh forever. Additionally, pkexec exit codes 126 (auth cancelled) and 127
(auth failed/binary not found) are not distinguished from generic failures in
`PrivilegedShell::new()`.

**Cargo.toml** does not include the `"time"` feature for tokio:

```toml
tokio = { version = "1", features = ["rt", "macros", "io-util", "process", "fs", "sync"] }
```

### Proposed Fix

#### 2a — Add `tokio/time` feature to Cargo.toml

```toml
tokio = { version = "1", features = ["rt", "macros", "io-util", "process", "fs", "sync", "time"] }
```

#### 2b — Wrap the `run_command` read loop in a timeout

Add a constant at the module level:

```rust
/// Maximum wall-clock time a single privileged command may run.
/// Commands that exceed this limit return an error; the shell is closed.
const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3600); // 1 hour
```

Wrap the read loop:

```rust
use tokio::time::timeout;

let result = timeout(COMMAND_TIMEOUT, async {
    let mut full_output = String::new();
    loop {
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .await
            .map_err(|e| format!("Failed to read output: {e}"))?;
        if n == 0 {
            return Err("Privileged shell closed unexpectedly".to_string());
        }
        // ... sentinel check and output forwarding unchanged ...
    }
})
.await;

match result {
    Ok(inner) => inner,
    Err(_elapsed) => {
        // Close the shell so it does not accumulate zombie state.
        self.close().await;
        Err(format!(
            "Command timed out after {} seconds",
            COMMAND_TIMEOUT.as_secs()
        ))
    }
}
```

#### 2c — Surface pkexec exit codes 126/127 as named errors

In `PrivilegedShell::new()`, when `n == 0` (shell exited before responding), the
existing code already calls `child.wait()`. Extend the error message to name the
specific pkexec exit codes:

```rust
let code = status.and_then(|s| s.code()).unwrap_or(-1);
let reason = match code {
    126 => "authentication was cancelled".to_string(),
    127 => "not authorised or pkexec not found".to_string(),
    _ => format!("exit code {code}"),
};
return Err(format!("pkexec failed: {reason}"));
```

### Risks and Edge Cases

- The 1-hour timeout is conservative. Very large distribution upgrades (NixOS flake
  rebuild, Ubuntu do-release-upgrade) can genuinely take more than 1 hour. The timeout
  is intended as a safety net for truly stuck commands (dpkg lock, network hang), not
  an operational deadline. Callers such as the upgrade page may want to increase or
  configure this in a future iteration.
- After a timeout `self.close()` is called, which drops stdin (sends EOF to the shell)
  and calls `child.wait()`. This is the correct clean-up path.

---

## 3. Issue 5.2 — `shell_quote` Unquoted Fast Path (MEDIUM→LOW)

### Current State

**File**: `src/runner.rs`, `fn shell_quote`

```rust
fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.bytes().all(|b| {
        b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'_' | b'/' | b'.' | b'=' | b':' | b'+' | b',')
    }) {
        s.to_string()      // ← FAST PATH: value returned unquoted
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}
```

The fast path returns the value without single-quote wrapping. This is safe for the
listed character set but creates an audit burden: any future maintainer who adds a new
safe character to the allow-list (or passes a value through a runtime variable whose
content they have not fully audited) loses the single-quote protection silently.

### Proposed Fix

Remove the fast path entirely. Always single-quote:

```rust
/// Quote a string for safe interpolation inside a POSIX shell command line.
///
/// Every value is wrapped in single quotes. Embedded single quotes are escaped
/// with the `'\''` idiom. This is unconditionally safe for all POSIX sh
/// implementations and removes the need to maintain a character allow-list.
fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}
```

### Risks and Edge Cases

- Performance: single-quoting all args adds ~2 characters per argument and one
  `String::replace` pass. The overhead is negligible compared to subprocess I/O.
- Correctness: single-quoting is universally accepted by POSIX `/bin/sh` and all
  shells used by Up's supported backends.
- No behavioural change for any current call site: all current call sites pass values
  from the safe character set, and single-quoting those produces an equivalent shell
  token.

---

## 4. Issue 5.3 — Unsigned Flatpak Bundle Self-Update (MEDIUM)

### Current State

**File**: `src/backends/flatpak.rs`, function `download_and_install_bundle`

The function downloads a `.flatpak` bundle from `https://github.com/VictoryTek/Up/releases/download/`
and installs it via `flatpak install --bundle --reinstall --user -y`. The only integrity
check is a URL prefix allowlist and a single-quote rejection. No checksum or GPG
signature is verified. A MITM attacker who can intercept the GitHub CDN response (or a
compromised GitHub account) can deliver a malicious bundle that gets silently installed
into the user's Flatpak user installation.

The Flathub OSTree distribution path — `flatpak update -y` — already performs GPG
signature verification at the OSTree transport layer. The GitHub direct-download path
provides no equivalent guarantee.

```rust
// SECURITY CONCERN: no checksum or signature check before install
runner
    .run("flatpak-spawn", &["--host", "bash", "-c", &*script])
    .await
```

### Proposed Fix

**Remove the GitHub direct-download self-update path entirely.**

The `download_and_install_bundle` function and the block that calls it inside
`FlatpakBackend::run_update` should be deleted. A `// SECURITY:` comment should
document why it was removed.

**Rationale**: The `flatpak update -y` call that already runs in `run_update` handles
OSTree-distributed updates (including Flathub) with full signature verification.
The GitHub-direct path was a workaround for the case where Up itself is not distributed
via a Flathub OSTree remote. Until GPG/minisign verification of the `.flatpak` bundle
is implemented, running unsigned code from a URL is not acceptable.

**Specific deletions**:

1. Delete the entire `fn download_and_install_bundle(…)` function.
2. In `FlatpakBackend::run_update`, delete the block:
   ```rust
   let github_self_updated = if !updated_self && is_running_in_flatpak() {
       match fetch_github_latest_release(runner).await { … }
   } else {
       false
   };
   ```
   Replace the block with `let github_self_updated = false;` (or remove the variable
   entirely and simplify the `if updated_self || github_self_updated` branch).
3. The function `fetch_github_latest_release` becomes dead code if no other caller
   remains. Delete it too, along with the constants `GITHUB_RELEASE_DOWNLOAD_PREFIX`.
   If a caller is added in the future for informational purposes (e.g., showing
   available version without installing), it can be restored with the 5.4 fix applied.
4. `GITHUB_REPO` constant can remain if referenced elsewhere; otherwise remove it.

Add the following comment in the `run_update` function where the GitHub check was:

```rust
// SECURITY: GitHub-direct self-update has been removed. Downloading and
// installing a Flatpak bundle without GPG/checksum verification is not
// acceptable. When Up is distributed via Flathub, `flatpak update -y` above
// handles self-updates via OSTree with full signature verification. A
// GitHub-direct path should only be re-added with minisign or GPG verification
// of the downloaded bundle against a key pinned in the source code.
```

### Risks and Edge Cases

- If Up is currently distributed via GitHub Releases `.flatpak` only (not Flathub),
  removing this path means Up will not self-update. This is acceptable: security must
  take precedence. The Flatpak packaging work (Section 6 of the analysis) should be
  completed and Flathub submission made before the self-update path is restored.
- The `updated_self` detection (OSTree path) continues to work correctly.
- `UpdateResult::SuccessWithSelfUpdate` can still be returned via the `updated_self`
  branch from OSTree.

---

## 5. Issue 5.4 — Inline Python Script via `format!` (MEDIUM)

### Current State

**File**: `src/backends/flatpak.rs`, function `fetch_github_latest_release`

The function constructs a multi-line Python one-liner via `format!` and passes it as:
- Outside sandbox: `python3 -c "<script>"`
- Inside sandbox: `flatpak-spawn --host bash -c "curl … | python3 -c \"<escaped script>\""

Interpolated values:
- `ver = env!("CARGO_PKG_VERSION")` — compile-time constant, safe
- `repo = GITHUB_REPO` — compile-time constant `"VictoryTek/Up"`, safe

**Current risk**: Low, because all interpolated values are compile-time constants.
**Future risk**: High — if either value is ever made a runtime variable (e.g., configurable
repo, dynamic version), shell injection is trivially achievable.

**Note**: If Issue 5.3 is implemented (removing `download_and_install_bundle` and the
GitHub check block), `fetch_github_latest_release` becomes dead code and should be
deleted. In that case, Issue 5.4 requires **no code change** — the vulnerable code
is gone. The remaining steps below are only relevant if the function is retained.

### Proposed Fix (if function is retained)

For the non-sandbox path, pass the Python script via stdin rather than `-c`:

```rust
// Instead of: runner.run("python3", &["-c", &*script]).await
// We need a runner method that writes to stdin. Until CommandRunner grows
// that capability, the safe interim is to validate the constants and document
// the constraint clearly.
```

Since `CommandRunner` does not currently support writing to stdin of a spawned process,
the minimum-viable fix is:

1. Extract `repo` and `ver` from the format string and pass them as positional arguments
   to the Python script via `sys.argv`, completely decoupled from the script body:

   ```python
   # Script body (no format! interpolation):
   import sys, urllib.request, json
   repo = sys.argv[1]   # passed as separate arg, never in script body
   ver  = sys.argv[2]
   r = urllib.request.urlopen(
       f'https://api.github.com/repos/{repo}/releases/latest', timeout=10)
   ...
   ```

   ```rust
   runner.run("python3", &["-c", PYTHON_SCRIPT, GITHUB_REPO, env!("CARGO_PKG_VERSION")]).await
   ```

   Where `PYTHON_SCRIPT` is a `const &str` (not produced by `format!`).

2. For the Flatpak sandbox path using `flatpak-spawn --host bash -c`, pass `repo` and
   `ver` as positional shell arguments (`$1`, `$2`) rather than interpolating them into
   the script body:

   ```bash
   bash -s -- 'VictoryTek/Up' '1.0.3' <<'EOF'
   repo=$1; ver=$2
   curl ... | python3 -c "import sys,json; ..."
   EOF
   ```

   Since `CommandRunner` does not support heredoc via stdin, a practical alternative is
   to use `bash -s` with arguments or build the command using `--` positional parameter
   separation.

   **Simplest practical approach for the sandbox path**: since `repo` and `ver` are
   compile-time constants with no shell metacharacters, add a compile-time assertion and
   a `// SAFETY:` comment documenting the invariant:

   ```rust
   // SAFETY: GITHUB_REPO and CARGO_PKG_VERSION are compile-time constants.
   // GITHUB_REPO = "VictoryTek/Up" (only alphanumeric, '/', '-')
   // CARGO_PKG_VERSION = "X.Y.Z" (only digits and '.')
   // Neither value contains any shell metacharacter. If either is ever made
   // a runtime variable, this MUST be rewritten to pass values out-of-band
   // (e.g., env vars or positional args) rather than interpolating into
   // the script body.
   const _: () = {
       // Verify no shell metacharacters exist in the repo slug.
       // (This is a compile-time check via const evaluation.)
       let b = GITHUB_REPO.as_bytes();
       let mut i = 0;
       while i < b.len() {
           let c = b[i];
           assert!(
               c.is_ascii_alphanumeric() || c == b'/' || c == b'-' || c == b'_',
               "GITHUB_REPO contains a character that is not safe for shell interpolation"
           );
           i += 1;
       }
   };
   ```

### Risks and Edge Cases

- If 5.3 is implemented, this issue disappears with the deleted code. Implement 5.3
  first; revisit 5.4 only if the function is retained for informational purposes.
- The compile-time assertion in the `const` block will emit a descriptive compile error
  if `GITHUB_REPO` is ever changed to include metacharacters.

---

## 6. Issue 5.6 — `pkexec sh -c` with Interpolated Values in `upgrade.rs` (MEDIUM)

### Current State

**File**: `src/upgrade.rs`, function `upgrade_nixos`

There are two `pkexec sh -c` call sites in `upgrade_nixos`:

**Site A — Line ~831** (LegacyChannel branch):
```rust
const NIX_PATH_EXPORT: &str =
    "export PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin:$PATH";

let channel_url = format!("https://nixos.org/channels/{}", next_channel);
let add_cmd = format!(
    "{NIX_PATH_EXPORT} && nix-channel --add {} nixos",
    channel_url
);
if !crate::runner::run_command_sync("pkexec", &["sh", "-c", &add_cmd], tx) {
```

**Site B — Line ~858** (Flake branch):
```rust
let cmd = format!("{NIX_PATH_EXPORT} && nix flake update --flake /etc/nixos");
if !crate::runner::run_command_sync("pkexec", &["sh", "-c", &cmd], tx) {
```

### Analysis of Each Site

**Site A**: `channel_url` is derived from `next_nixos_channel(&distro.version_id)` which
parses `version_id` as two `u32` values and formats them as `nixos-{u32}.{u32:02}`. The
resulting URL is always of the form `https://nixos.org/channels/nixos-NN.NN` — only
alphanumeric characters, hyphens, dots, and the fixed HTTPS URL prefix. **No shell
injection is possible via the current value**. However, the `sh -c` construction wrapping
it is architecturally undesirable.

**Site B**: `cmd` interpolates only the constant `NIX_PATH_EXPORT`. No runtime data.
This is safe but still unnecessary.

**Root cause**: Both sites use `sh -c` solely to execute `export PATH=…` before the
actual command. This can be eliminated using `pkexec /usr/bin/env PATH=…` instead.

### Proposed Fix

Replace `sh -c` with `pkexec /usr/bin/env PATH=<value> <cmd> <args…>` pattern. This
eliminates the shell entirely for these two sites. NixOS provides `/usr/bin/env` as a
FHS compatibility symlink.

Replace the `NIX_PATH_EXPORT` const with a `NIX_PATH` const:

```rust
/// Colon-separated PATH prepended for NixOS tool access under pkexec.
///
/// pkexec resets PATH to a minimal set, excluding NixOS-specific tool paths.
/// We set PATH explicitly via `/usr/bin/env` to avoid a shell wrapper.
const NIX_PATH: &str =
    "/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin";
```

**Site A — Rewrite**:
```rust
// Pass channel_url as a positional argument; no sh -c needed.
// /usr/bin/env sets PATH without requiring a shell.
let path_arg = format!("PATH={}", NIX_PATH);
if !crate::runner::run_command_sync(
    "pkexec",
    &["/usr/bin/env", &path_arg, "nix-channel", "--add", &channel_url, "nixos"],
    tx,
) {
    return Err(format!(
        "Failed to register NixOS channel {} (see log for details)",
        next_channel
    ));
}
```

**Site B — Rewrite**:
```rust
let path_arg = format!("PATH={}", NIX_PATH);
if !crate::runner::run_command_sync(
    "pkexec",
    &["/usr/bin/env", &path_arg, "nix", "flake", "update", "--flake", "/etc/nixos"],
    tx,
) {
    return Err(
        "Failed to update flake inputs in /etc/nixos (see log for details)".to_string(),
    );
}
```

**Also**: The nixos-rebuild sites in `upgrade_nixos` use:
```rust
// LegacyChannel — no interpolation, no sh -c, already clean:
run_command_sync("pkexec", &["nixos-rebuild", "switch", "--upgrade"], tx)

// Flake — already uses run_command_sync with positional args, already clean:
run_command_sync("pkexec", &["nixos-rebuild", "switch", "--flake", &flake_target], tx)
```
These are already correct. No change needed.

**NixOS FHS compatibility note**: `/usr/bin/env` is available on NixOS via the `FHS
compatibility layer` provided by nixpkgs' `usrBinEnv` activation script. If an unusual
NixOS configuration lacks it, `pkexec` will fail with a descriptive error. This is
preferable to the silent injection risk from `sh -c`. The existing `sh -c` fallback
behaviour (failure logged to `tx`) is preserved.

### Risks and Edge Cases

- `pkexec /usr/bin/env PATH=...` passes polkit's default `org.freedesktop.policykit.exec`
  rule (or our new `io.github.up.policy` action once shipped) since `/usr/bin/env` is
  the real executable being authorised. This is semantically equivalent to the existing
  `pkexec /bin/sh` from polkit's perspective.
- The `NIX_PATH` env var approach (`env PATH=…`) only prepends the extra paths. If
  `nix-channel` or `nix` resolves to an unexpected binary in those paths, that is a
  compromise of the host NixOS system — not something Up can defend against.

---

## 7. Issue 5.7 — Missing Polkit Policy File (MEDIUM)

### Current State

No `.policy` file exists anywhere in the project. When Up calls `pkexec /bin/sh`,
polkit falls back to the generic action `org.freedesktop.policykit.exec`. The polkit
authentication dialog shows no vendor name, no description, and no icon specific to Up.
Sysadmins cannot write polkit JS rules that target Up's operations by action ID.

With the fix from Issue 5.6, `/usr/bin/env` is also used in some paths. Both `/bin/sh`
and `/usr/bin/env` need a policy entry.

### Proposed Fix: New File `data/io.github.up.policy`

**Design principles**:
- Define two scoped action IDs in the `io.github.up` namespace.
- Both actions reference the actual executable paths used by `pkexec`.
- `allow_active`: `auth_admin_keep` — authenticate once, keep credential for session.
- `allow_inactive`: `auth_admin` — always prompt if session is inactive.
- `allow_any`: `auth_admin` — always require admin authentication from any session.
- Vendor info identifies Up specifically, so the user knows which application is
  requesting privilege escalation.

**Important limitation**: polkit action matching in `pkexec` is based on the path of the
executable being run (`/bin/sh` or `/usr/bin/env`). It is not possible to restrict the
action to calls made exclusively by Up using the XML policy format alone — that requires
a polkit JS rules file or a D-Bus backend service. The policy file below is therefore a
significant improvement over the status quo (informative dialog, auditable action IDs)
but is not a complete privilege scope boundary.

**File content** (`data/io.github.up.policy`):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE policyconfig PUBLIC
    "-//freedesktop//DTD PolicyKit Policy Configuration 1.0//EN"
    "http://www.freedesktop.org/standards/PolicyKit/1/policyconfig.dtd">
<policyconfig>

  <vendor>Up — System Updater</vendor>
  <vendor_url>https://github.com/VictoryTek/Up</vendor_url>
  <icon_name>io.github.up</icon_name>

  <!--
    Action used when Up runs system package-manager updates through its
    persistent pkexec shell session (pkexec /bin/sh).

    Authentication is required once per desktop session (auth_admin_keep).
    The credential is cached by polkit for the duration of the session so
    that updating multiple backends does not prompt repeatedly.

    NOTE: This action matches any caller of `pkexec /bin/sh`, not only Up.
    True per-application scoping requires a D-Bus backend service. This file
    is a significant improvement over the default org.freedesktop.policykit.exec
    action and enables sysadmin rule customisation via polkit JS rules.
  -->
  <action id="io.github.up.pkexec.update">
    <description>Update system packages</description>
    <description xml:lang="en">Update system packages</description>
    <message>Up needs administrator privileges to update system packages.</message>
    <message xml:lang="en">Up needs administrator privileges to update system packages.</message>
    <defaults>
      <allow_any>auth_admin</allow_any>
      <allow_inactive>auth_admin</allow_inactive>
      <allow_active>auth_admin_keep</allow_active>
    </defaults>
    <annotate key="org.freedesktop.policykit.exec.path">/bin/sh</annotate>
    <annotate key="org.freedesktop.policykit.exec.allow_gui">true</annotate>
  </action>

  <!--
    Action used when Up runs distribution upgrade commands via pkexec
    with /usr/bin/env (NixOS upgrade path, see upgrade.rs).

    Distribution upgrades are a more impactful operation than routine updates;
    they use the same authentication level but a distinct action ID so that
    sysadmins can apply stricter rules specifically for upgrades.
  -->
  <action id="io.github.up.pkexec.upgrade">
    <description>Upgrade the system distribution</description>
    <description xml:lang="en">Upgrade the system distribution</description>
    <message>Up needs administrator privileges to upgrade the system to a new release.</message>
    <message xml:lang="en">Up needs administrator privileges to upgrade the system to a new release.</message>
    <defaults>
      <allow_any>auth_admin</allow_any>
      <allow_inactive>auth_admin</allow_inactive>
      <allow_active>auth_admin_keep</allow_active>
    </defaults>
    <annotate key="org.freedesktop.policykit.exec.path">/usr/bin/env</annotate>
    <annotate key="org.freedesktop.policykit.exec.allow_gui">true</annotate>
  </action>

</policyconfig>
```

### `meson.build` change

Add after the existing `install_data` calls:

```meson
install_data('data/io.github.up.policy',
  install_dir: join_paths(datadir, 'polkit-1', 'actions'),
)
```

### `data/io.github.up.gresource.xml` — No change required

The polkit policy is an installed system file, not an application resource bundled into
the binary. It must NOT be added to the GResource bundle. The existing gresource.xml
only embeds the app icon.

### Installation path

Standard polkit policy installation: `$(datadir)/polkit-1/actions/io.github.up.policy`

e.g., `/usr/share/polkit-1/actions/io.github.up.policy`

### Flatpak note

When distributed as a Flatpak, the policy file must be placed in the host's polkit
actions directory, not the sandbox. This requires either:
- A Flatpak portal/permission system (preferred long-term)
- The Flatpak manifest's `--filesystem=host-os:/usr/share/polkit-1/actions:create`
  permission (not recommended for Flathub)
- Post-install documentation asking the user to copy the policy file

For the Flatpak distribution model, the policy file is primarily useful for native
package installs (APT/DNF/Pacman). Include it in the source tree and install it via
Meson; Flatpak packaging can revisit.

### Risks and Edge Cases

- The action `io.github.up.pkexec.update` annotates `/bin/sh` — the same binary used by
  many other applications. If polkit chooses this action for other callers of
  `pkexec /bin/sh`, those callers will see Up's dialog text. This is a known limitation.
  A sysadmin can override with a JS rules file. Full scoping requires D-Bus.
- The action `io.github.up.pkexec.upgrade` annotates `/usr/bin/env` which may not exist
  on all NixOS configurations. On systems where it is absent, pkexec will fail at exec
  time (not at polkit policy lookup time), which is handled gracefully by the
  existing error propagation in `run_command_sync`.

---

## 8. Issue 5.8 — ANSI Escape Sequences in LogPanel (LOW)

### Current State

**File**: `src/ui/log_panel.rs`, method `append_line`

```rust
pub fn append_line(&self, line: &str) {
    let buffer = self.text_view.buffer();
    let mut end = buffer.end_iter();
    buffer.insert(&mut end, line);
    buffer.insert(&mut end, "\n");
    // ...
}
```

Package managers such as `apt`, `dnf`, `flatpak`, and `nix` emit ANSI colour sequences
(`\x1b[32m`, `\x1b[0m`, etc.) in their output. These appear as garbled characters
(`←[32m`) in the GTK `TextBuffer` since GTK's text rendering interprets them as literal
text, not terminal control codes.

### Proposed Fix

Add a private `strip_ansi` function and call it in `append_line`. No new crates required.

```rust
/// Remove ANSI/VT100 escape sequences from `s`.
///
/// Handles:
/// - CSI sequences: ESC `[` followed by parameter bytes (`0x30–0x3F`),
///   intermediate bytes (`0x20–0x2F`), and a final byte (`0x40–0x7E`).
/// - Simple two-byte ESC sequences: ESC followed by any ASCII letter.
///
/// Any other byte sequence starting with ESC is passed through unchanged
/// rather than silently discarding legitimate content.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        // ESC seen — inspect next character.
        match chars.peek().copied() {
            Some('[') => {
                // CSI sequence: consume '[' and everything up to and including
                // the final byte (first byte in 0x40–0x7E range).
                chars.next(); // consume '['
                for ch in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&ch) {
                        break; // final byte consumed; sequence complete
                    }
                }
            }
            Some(ch) if ch.is_ascii_alphabetic() => {
                // Simple two-byte escape (e.g., ESC M for reverse index).
                chars.next(); // consume the letter
            }
            _ => {
                // Unrecognised; emit ESC as-is rather than silently dropping.
                out.push('\x1b');
            }
        }
    }
    out
}
```

Modify `append_line`:

```rust
pub fn append_line(&self, line: &str) {
    let clean = strip_ansi(line);
    let buffer = self.text_view.buffer();
    let mut end = buffer.end_iter();
    buffer.insert(&mut end, &clean);
    buffer.insert(&mut end, "\n");

    // Auto-scroll to bottom
    buffer.move_mark(&self.scroll_mark, &buffer.end_iter());
    self.text_view.scroll_mark_onscreen(&self.scroll_mark);
}
```

### Risks and Edge Cases

- The strip function is conservative: unrecognised escape sequences are passed through
  rather than dropped, preventing accidental loss of legitimate content.
- OSC sequences (ESC `]`), DCS sequences (ESC `P`), and other non-CSI sequences are not
  handled; they will pass through as raw bytes. These are rarely emitted by package
  managers. A future iteration can extend the function if needed.
- GTK's `TextBuffer` is not a terminal emulator and does not support cursor movement,
  bold, or colour styling. Stripping the codes is the correct approach; rendering them
  as Pango markup would be overkill and would require escaping all other text.
- `String::with_capacity(s.len())` pre-allocates a reasonable initial capacity; the
  actual output is always shorter or equal to the input.

---

## 9. Dependency Changes Summary

### `Cargo.toml`

Only one change is required:

```toml
# Before:
tokio = { version = "1", features = ["rt", "macros", "io-util", "process", "fs", "sync"] }

# After (add "time"):
tokio = { version = "1", features = ["rt", "macros", "io-util", "process", "fs", "sync", "time"] }
```

`tokio::time` is already a first-party tokio crate. Adding the feature does not introduce
any new external dependencies or increase the compiled binary meaningfully.

---

## 10. Implementation Order

Implement in the following order to minimise rework:

1. **5.3** — Delete GitHub self-update path in `flatpak.rs`  
   (Removes code; simplifies 5.4 from "fix" to "N/A if deleted")
2. **5.4** — Confirm dead code deletion or apply the `const` assertion if retained
3. **5.2** — Remove `shell_quote` fast path in `runner.rs` (one-line change)
4. **5.1** — Add session_id to `PrivilegedShell` and add arg validation
5. **3.2** — Add `tokio/time` to Cargo.toml; wrap `run_command` in timeout; improve
   pkexec exit code messages
6. **5.6** — Replace `pkexec sh -c` with `pkexec /usr/bin/env` in `upgrade.rs`
7. **5.7** — Create `data/io.github.up.policy`; update `meson.build`
8. **5.8** — Add `strip_ansi` to `log_panel.rs`

---

## 11. Verification Checklist

After implementation, the reviewer must confirm:

- [ ] `cargo build` succeeds with no errors
- [ ] `cargo clippy -- -D warnings` produces no warnings
- [ ] `cargo fmt --check` produces no diffs
- [ ] `cargo test` passes all tests
- [ ] `src/runner.rs`: `PrivilegedShell` has `session_id` field; `RC_MARKER` const removed
- [ ] `src/runner.rs`: `run_command` rejects args containing `\n`, `\r`, `\0`
- [ ] `src/runner.rs`: `run_command` uses randomised `rc_prefix` derived from `session_id`
- [ ] `src/runner.rs`: `run_command` wrapped in `tokio::time::timeout`
- [ ] `src/runner.rs`: pkexec 126/127 exit codes produce named error messages
- [ ] `src/runner.rs`: `shell_quote` fast path removed; always single-quotes
- [ ] `src/backends/flatpak.rs`: `download_and_install_bundle` deleted
- [ ] `src/backends/flatpak.rs`: `fetch_github_latest_release` deleted (if no other callers)
- [ ] `src/backends/flatpak.rs`: `GITHUB_RELEASE_DOWNLOAD_PREFIX` deleted (if no other callers)
- [ ] `src/backends/flatpak.rs`: `// SECURITY:` comment present explaining removal
- [ ] `src/upgrade.rs`: No `sh -c` with interpolated values; uses `pkexec /usr/bin/env`
- [ ] `src/ui/log_panel.rs`: `strip_ansi` function present; called in `append_line`
- [ ] `data/io.github.up.policy`: valid XML; two actions present; installs to correct path
- [ ] `meson.build`: `install_data` for `io.github.up.policy` in `polkit-1/actions`
- [ ] `Cargo.toml`: tokio features include `"time"`
