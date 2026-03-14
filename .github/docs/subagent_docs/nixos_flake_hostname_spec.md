# NixOS Flake Hostname Explicit Targeting — Specification

**Feature:** Explicit `--flake /etc/nixos#<hostname>` with runtime hostname detection  
**Spec Author:** Research & Specification Agent  
**Date:** 2026-03-14  
**Status:** Ready for Implementation  
**Supersedes / Extends:** `nixos_upgrade_spec.md` (the previously implemented NixOS upgrade base)

---

## 1. Current State Analysis

### 1.1 Relevant files

| File | Role |
|---|---|
| `src/upgrade.rs` | Core upgrade logic — `upgrade_nixos()`, `detect_nixos_config_type()` |
| `src/ui/upgrade_page.rs` | GTK4 upgrade page — System Information group, Config Type row |

### 1.2 Current `upgrade_nixos()` (src/upgrade.rs)

```rust
fn upgrade_nixos(tx: &async_channel::Sender<String>) {
    let config_type = detect_nixos_config_type();
    match config_type {
        NixOsConfigType::LegacyChannel => {
            // ...
            run_streaming_command("sudo", &["nix-channel", "--update"], tx);
            run_streaming_command("pkexec", &["nixos-rebuild", "switch", "--upgrade"], tx);
        }
        NixOsConfigType::Flake => {
            // ...
            run_streaming_command("sudo", &["nix", "flake", "update", "/etc/nixos"], tx);
            run_streaming_command(
                "pkexec",
                &["nixos-rebuild", "switch", "--flake", "/etc/nixos"],
                tx,
            );
        }
    }
}
```

### 1.3 Current upgrade_page.rs — Config Type row

```rust
if distro_info.id == "nixos" {
    let config_type = upgrade::detect_nixos_config_type();
    let config_label = match config_type {
        upgrade::NixOsConfigType::Flake => "Flake-based (/etc/nixos/flake.nix)",
        upgrade::NixOsConfigType::LegacyChannel => "Channel-based (/etc/nixos/configuration.nix)",
    };
    let config_row = adw::ActionRow::builder()
        .title("NixOS Config Type")
        .subtitle(config_label)
        .build();
    // ...
}
```

### 1.4 Two existing bugs identified during research

| Bug | Location | Description |
|---|---|---|
| **Bug A**: Wrong `nix flake update` syntax | `upgrade.rs` line ~283 | `nix flake update /etc/nixos` passes `/etc/nixos` as a positional **input name** argument, not the flake path. Modern Nix treats positional args to `nix flake update` as input names. This command would fail trying to find a flake input named `/etc/nixos`. |
| **Bug B**: No explicit hostname attr | `upgrade.rs` | `nixos-rebuild switch --flake /etc/nixos` omits the `#<hostname>` attribute selector. While nixos-rebuild auto-detects the hostname, this is fragile and gives the user no confirmation of which NixOS configuration will be built. |

---

## 2. Research Summary (6 Sources)

### Source 1 — NixOS Wiki: Flake output schema

URL: `https://wiki.nixos.org/wiki/Flakes`

The authoritative NixOS flake output schema includes the following comment directly in the schema:

```nix
# Used with `nixos-rebuild switch --flake .#<hostname>`
# nixosConfigurations."<hostname>".config.system.build.toplevel must be a derivation
nixosConfigurations."<hostname>" = {};
```

**Key findings:**
- `nixosConfigurations."<hostname>"` is the documented flake output structure for NixOS system configurations.
- The `#<hostname>` attribute selector is the **intended** way to select a configuration.
- The hostname convention is universal — nearly all NixOS flake configurations use the system hostname as the attribute key.
- Attribute names can technically be anything; `hostname` is the strong convention, not a technical requirement.

---

### Source 2 — NixOS Manual: Changing the Configuration / Upgrading

URL: `https://nixos.org/manual/nixos/stable/index.html#sec-upgrading`

**Key findings:**
- The manual shows `nixos-rebuild switch --upgrade` for channel-based upgrades.
- For flake-based systems, `nixos-rebuild switch --flake /path/to/flake#attr` is the documented pattern.
- `nixos-rebuild` documentation states: when `#attr` is omitted, the tool auto-selects using the system's short hostname (`hostname -s`). This is DOCUMENTED behavior, not incidental — the tool reads `/etc/hostname` or invokes `hostname` to determine the attribute name.
- The auto-detection is reliable on standard NixOS systems but can break if the flake attribute name doesn't match the running hostname (e.g., config renamed to a descriptive label, multi-config flakes).
- The `--upgrade` flag incorporates `nix-channel --update`; there is no `--upgrade` equivalent for flake-based configs.

---

### Source 3 — Nix Reference Manual: `nix flake update`

URL: `https://nix.dev/manual/nix/2.25/command-ref/new-cli/nix3-flake-update`

This is the most impactful research finding for this spec.

**Synopsis:**
```
nix flake update [option...] inputs...
```

**Description (verbatim from docs):**
> Unlike other `nix flake` commands, `nix flake update` takes a list of **names of inputs** to update as its positional arguments and operates on the flake **in the current directory**. You can pass a different flake-url with `--flake` to override that default.

**Correct examples from the manual:**
```bash
# Update ALL inputs of a flake in a DIFFERENT directory:
nix flake update --flake ~/repos/another

# Note in the manual:
# When trying to refer to a flake in a subdirectory, write ./another instead of another.
# Otherwise Nix will try to look up the flake in the registry.
```

**Confirmed bug in current code:**
```bash
# WRONG — /etc/nixos is treated as an input name to look up, not a flake path
nix flake update /etc/nixos

# CORRECT — use --flake flag to specify the flake directory
nix flake update --flake /etc/nixos
```

The current implementation passes `/etc/nixos` as a positional argument, which modern Nix (2.19+) interprets as an input name. This would log an error like `error: flake '/etc/nixos' does not have an input named '/etc/nixos'` and exit with a non-zero code.

---

### Source 4 — NixOS Manual: `networking.hostName` option

URL: `https://nixos.org/manual/nixos/stable/index.html` (IPv4 Configuration section)

The NixOS manual shows hostname configuration:

```nix
{ networking.hostName = "cartman"; }
```

**Key findings:**
- The hostname is set via `networking.hostName` in NixOS configuration.
- Default hostname on a fresh install is `nixos`; it's set at system activation time.
- The hostname written to the kernel (`/proc/sys/kernel/hostname`) matches `networking.hostName`.
- `/etc/hostname` on NixOS is managed by the activation script and contains the value of `networking.hostName`.
- The runtime hostname (from `gethostname()` syscall, `/proc/sys/kernel/hostname`) is the authoritative source for what `nixos-rebuild` would auto-detect.

**Convention for flake attribute names:**
The NixOS flake convention (observed across thousands of public configurations on GitHub, Sourcehut, Codeberg, etc.) is that `nixosConfigurations."<attr>"` uses the hostname set in `networking.hostName`. Users like the spec's submitter (whose hostname is `vexos`) name their configuration to match their hostname.

---

### Source 5 — Rust standard library: hostname detection options

From `docs.rs/hostname` and Rust stdlib knowledge:

**Three viable methods for getting hostname in Rust without adding a dependency:**

1. **Read `/proc/sys/kernel/hostname`** (Linux-only, no dependency):
   ```rust
   std::fs::read_to_string("/proc/sys/kernel/hostname")
       .map(|h| h.trim().to_owned())
       .unwrap_or_else(|_| "nixos".to_string())
   ```
   - Reads the kernel hostname directly.
   - On NixOS this is always set via the activation script from `networking.hostName`.
   - No trailing newline concern if `.trim()` is applied.
   - No subprocess spawn. Pure Rust. No added dependency.
   - **This is the recommended approach for this project** — NixOS is Linux-only, no new Cargo dependency.

2. **`hostname` crate** (`hostname::get()`) — calls `gethostname()` POSIX syscall:
   - Not currently in `Cargo.toml`.
   - Would require adding `hostname = "0.4"` to dependencies.
   - Returns `OsString` which needs conversion: `hostname::get()?.to_string_lossy().into_owned()`.
   - Adds a compile-time dependency for a one-liner that `/proc/sys/kernel/hostname` provides directly.
   - **Not recommended** — unnecessary dependency for a feature available via filesystem.

3. **`std::process::Command::new("hostname").output()`**:
   - Spawns a subprocess; simpler than it looks.
   - Works but is slower and requires `hostname` to be in PATH.
   - **Not recommended** — subprocess spawn for what is a simple file read.

**Recommendation:** Method 1 (`/proc/sys/kernel/hostname`). It is canonical, zero-dependency, and Linux-idiomatic.

---

### Source 6 — NixOS privilege model: `sudo` vs `pkexec` for GUI apps

From NixOS wiki (Polkit page), NixOS manual, and the previous review `nixos_upgrade_review.md`:

**Summary of constraints:**

| Escalation method | Works in GUI? | For `nixos-rebuild`? | Notes |
|---|---|---|---|
| `pkexec <cmd>` | ✅ (with active DE auth agent) | ⚠️ Partial | No pre-shipped polkit rule for `nixos-rebuild`. Will prompt but may fail `Not authorized` without a user-written polkit rule. |
| `sudo <cmd>` | ❌ (no TTY) | ✅ if cached/NOPASSWD | Fails with `sudo: no tty present` unless credentials cached or `NOPASSWD` configured. |
| `pkexec sh -c "cmd1 && cmd2"` | ✅ | ⚠️ PATH restricted | `pkexec` PATH is limited; `sh` is at `/bin/sh` on NixOS. `nix` and `nixos-rebuild` are in Nix store paths not in pkexec's default PATH. |

**Current implementation status:** The existing code already uses this mixed `sudo`/`pkexec` pattern and the previous review classified the `sudo`-without-TTY issue as RECOMMENDED (not CRITICAL). The fix being specified here does NOT change the privilege escalation strategy — that is out of scope.

**Scope boundary:** This spec only addresses the hostname detection and explicit `#hostname` attribute. The privilege escalation approach (the `sudo` fallback) remains as-is, consistent with previous decisions.

---

## 3. Problem Definition

The current `upgrade_nixos()` implementation has two related problems:

### Problem 1: Wrong `nix flake update` API usage (Bug A — functional bug)

```rust
// Current code (WRONG in modern Nix):
run_streaming_command("sudo", &["nix", "flake", "update", "/etc/nixos"], tx);
```

`/etc/nixos` is being passed as a positional argument, which modern Nix treats as an input name. This command fails in practice on systems with Nix 2.19+.

**Correct form:**
```rust
run_streaming_command("sudo", &["nix", "flake", "update", "--flake", "/etc/nixos"], tx);
```

### Problem 2: No explicit flake attribute (Bug B — robustness and transparency)

```rust
// Current code (omits #hostname):
run_streaming_command(
    "pkexec",
    &["nixos-rebuild", "switch", "--flake", "/etc/nixos"],
    tx,
);
```

While `nixos-rebuild` auto-detects the hostname when `#attr` is omitted, this is:
- **Fragile**: breaks if the flake attribute doesn't match the running hostname.
- **Opaque**: the user cannot confirm which configuration will be built.
- **Not how the user runs the command manually**: the user's stated command explicitly passes `#vexos`.

**Target behavior:**
```rust
let hostname = detect_system_hostname();
// emits: "nixos-rebuild switch --flake /etc/nixos#vexos"
run_streaming_command(
    "pkexec",
    &["nixos-rebuild", "switch", "--flake", &format!("/etc/nixos#{hostname}")],
    tx,
);
```

### Problem 3: UI lacks flake target clarity

The current Config Type row subtitle is `"Flake-based (/etc/nixos/flake.nix)"` — it does not show which `nixosConfigurations` attribute will be selected. The user cannot confirm from the UI that the correct machine's config will be built.

**Target:** Show `"Flake-based (/etc/nixos#vexos)"` (where `vexos` is resolved at display time).

---

## 4. Proposed Solution Architecture

### 4.1 New function: `detect_system_hostname()` (in `src/upgrade.rs`)

```rust
/// Returns the current system hostname by reading /proc/sys/kernel/hostname.
/// Falls back to "nixos" if the file is unreadable.
/// On NixOS this always reflects networking.hostName as set in the configuration.
pub fn detect_system_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|h| h.trim().to_owned())
        .unwrap_or_else(|_| "nixos".to_owned())
}
```

**Design rationale:**
- `/proc/sys/kernel/hostname` is a Linux kernel pseudo-file that returns the currently active hostname.
- On NixOS it is set during system activation from `networking.hostName`.
- Reading this file is equivalent to `hostname -s` (short form only, no domain).
- No new Cargo dependency; no subprocess spawn.
- The function is `pub` because `upgrade_page.rs` needs to call it for the UI subtitle.

**Fallback:** "nixos" — the NixOS default hostname. This is the most likely value on an unconfigured system; it is better than panicking or displaying an empty string.

**Edge cases:**
- Hostname with hyphens (e.g., `my-desktop`) — valid NixOS attribute name, no special handling needed.
- Hostname may not match the flake attribute name if the user used a different name. This is documented as a risk (Section 7.3).
- Empty hostname — fallback handles this.
- `/proc` not mounted — extremely unlikely on a running NixOS system; fallback handles this.

---

### 4.2 Updated `upgrade_nixos()` (in `src/upgrade.rs`)

```rust
fn upgrade_nixos(tx: &async_channel::Sender<String>) {
    let config_type = detect_nixos_config_type();

    match config_type {
        NixOsConfigType::LegacyChannel => {
            let _ = tx.send_blocking("Detected: legacy channel-based NixOS config".into());
            let _ = tx.send_blocking("Step 1: Updating NixOS channel...".into());
            run_streaming_command("sudo", &["nix-channel", "--update"], tx);
            let _ = tx.send_blocking("Step 2: Rebuilding NixOS (switch --upgrade)...".into());
            run_streaming_command("pkexec", &["nixos-rebuild", "switch", "--upgrade"], tx);
        }
        NixOsConfigType::Flake => {
            let hostname = detect_system_hostname();
            let flake_target = format!("/etc/nixos#{hostname}");

            let _ = tx.send_blocking("Detected: flake-based NixOS config".into());
            let _ = tx.send_blocking(
                format!("Step 1: Updating flake inputs in /etc/nixos..."),
            );
            // FIX (Bug A): use --flake flag, not positional argument
            run_streaming_command(
                "sudo",
                &["nix", "flake", "update", "--flake", "/etc/nixos"],
                tx,
            );
            let _ = tx.send_blocking(
                format!("Step 2: Rebuilding NixOS (switch --flake {flake_target})..."),
            );
            // FIX (Bug B): pass explicit #hostname attribute
            run_streaming_command(
                "pkexec",
                &["nixos-rebuild", "switch", "--flake", &flake_target],
                tx,
            );
        }
    }
}
```

**Key changes from current code:**
1. `nix flake update /etc/nixos` → `nix flake update --flake /etc/nixos` (Bug A fix).
2. `nixos-rebuild switch --flake /etc/nixos` → `nixos-rebuild switch --flake /etc/nixos#<hostname>` (Bug B fix).
3. Log messages updated to show the resolved flake target.

**Note on `run_streaming_command` and owned Strings:**
The `&flake_target` borrow works because `flake_target` lives for the duration of the call. The function signature `fn run_streaming_command(program: &str, args: &[&str], tx: ...)` requires `&str` args. Since `flake_target` is a heap-allocated `String`, `&flake_target` coerces to `&str` correctly.

---

### 4.3 Updated Config Type row in `src/ui/upgrade_page.rs`

```rust
if distro_info.id == "nixos" {
    let config_type = upgrade::detect_nixos_config_type();
    let config_label = match config_type {
        upgrade::NixOsConfigType::Flake => {
            let hostname = upgrade::detect_system_hostname();
            format!("Flake-based (/etc/nixos#{})", hostname)
        }
        upgrade::NixOsConfigType::LegacyChannel => {
            "Channel-based (/etc/nixos/configuration.nix)".to_owned()
        }
    };
    let config_row = adw::ActionRow::builder()
        .title("NixOS Config Type")
        .subtitle(&config_label)
        .build();
    config_row.add_prefix(&gtk::Image::from_icon_name("emblem-system-symbolic"));
    info_group.add(&config_row);
}
```

**Design notes:**
- `detect_system_hostname()` is called at page construction time (inside `UpgradePage::build()`).
- This runs on the GTK main thread. Reading `/proc/sys/kernel/hostname` is a tiny file read — negligible cost, no async required.
- `format!()` allocates a `String`; the `&config_label` borrow is passed to `.subtitle()` which converts to `&str`.
- The `"to_owned()"` on the legacy branch ensures both arms produce the same type (`String`), allowing `let config_label: String = match ...`.

---

## 5. Files to be Modified

| File | Changes |
|---|---|
| `src/upgrade.rs` | Add `detect_system_hostname()` function; update `upgrade_nixos()` with Bug A fix and Bug B fix |
| `src/ui/upgrade_page.rs` | Update NixOS config type row to show resolved `#hostname` |

**No changes required to:**
- `Cargo.toml` — no new dependencies.
- `src/backends/nix.rs` — handles per-user Nix profile updates, unrelated.
- `src/app.rs`, `src/main.rs`, `src/runner.rs` — unaffected.
- `meson.build`, `flake.nix`, `data/` — unaffected.

---

## 6. Exact Implementation Steps

### Step 1: Add `detect_system_hostname()` to `src/upgrade.rs`

Insert after `detect_nixos_config_type()` (approximately line 31):

```rust
/// Returns the current system hostname by reading /proc/sys/kernel/hostname.
/// Falls back to "nixos" if the file is unreadable.
pub fn detect_system_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|h| h.trim().to_owned())
        .unwrap_or_else(|_| "nixos".to_owned())
}
```

### Step 2: Fix `upgrade_nixos()` in `src/upgrade.rs`

Replace the entire `NixOsConfigType::Flake` arm:

**Before:**
```rust
NixOsConfigType::Flake => {
    let _ = tx.send_blocking("Detected: flake-based NixOS config".into());
    let _ = tx.send_blocking("Updating flake inputs in /etc/nixos...".into());
    run_streaming_command(
        "sudo",
        &["nix", "flake", "update", "/etc/nixos"],
        tx,
    );
    let _ = tx.send_blocking("Rebuilding NixOS (switch --flake)...".into());
    run_streaming_command(
        "pkexec",
        &["nixos-rebuild", "switch", "--flake", "/etc/nixos"],
        tx,
    );
}
```

**After:**
```rust
NixOsConfigType::Flake => {
    let hostname = detect_system_hostname();
    let flake_target = format!("/etc/nixos#{hostname}");

    let _ = tx.send_blocking("Detected: flake-based NixOS config".into());
    let _ = tx.send_blocking("Step 1: Updating flake inputs in /etc/nixos...".into());
    run_streaming_command(
        "sudo",
        &["nix", "flake", "update", "--flake", "/etc/nixos"],
        tx,
    );
    let _ = tx.send_blocking(
        format!("Step 2: Rebuilding NixOS (switch --flake {flake_target})..."),
    );
    run_streaming_command(
        "pkexec",
        &["nixos-rebuild", "switch", "--flake", &flake_target],
        tx,
    );
}
```

### Step 3: Update Config Type row in `src/ui/upgrade_page.rs`

Replace the NixOS-specific config row block:

**Before:**
```rust
if distro_info.id == "nixos" {
    let config_type = upgrade::detect_nixos_config_type();
    let config_label = match config_type {
        upgrade::NixOsConfigType::Flake => "Flake-based (/etc/nixos/flake.nix)",
        upgrade::NixOsConfigType::LegacyChannel => "Channel-based (/etc/nixos/configuration.nix)",
    };
    let config_row = adw::ActionRow::builder()
        .title("NixOS Config Type")
        .subtitle(config_label)
        .build();
    config_row.add_prefix(&gtk::Image::from_icon_name("emblem-system-symbolic"));
    info_group.add(&config_row);
}
```

**After:**
```rust
if distro_info.id == "nixos" {
    let config_type = upgrade::detect_nixos_config_type();
    let config_label = match config_type {
        upgrade::NixOsConfigType::Flake => {
            let hostname = upgrade::detect_system_hostname();
            format!("Flake-based (/etc/nixos#{})", hostname)
        }
        upgrade::NixOsConfigType::LegacyChannel => {
            "Channel-based (/etc/nixos/configuration.nix)".to_owned()
        }
    };
    let config_row = adw::ActionRow::builder()
        .title("NixOS Config Type")
        .subtitle(&config_label)
        .build();
    config_row.add_prefix(&gtk::Image::from_icon_name("emblem-system-symbolic"));
    info_group.add(&config_row);
}
```

---

## 7. Risks and Edge Cases

### 7.1 `nix flake update --flake /etc/nixos` requires Nix to support `--flake` flag

**Risk:** The `--flake` option for `nix flake update` was added in Nix 2.19 (late 2023). On older NixOS installations (NixOS 23.05 or earlier with Nix < 2.19), this flag may not exist and `nix flake update --flake /etc/nixos` would fail with `error: unrecognized option '--flake'`.

**Mitigation:** NixOS users running a flake-based config are very likely on a recent NixOS version that ships Nix 2.19+. The app already assumes flakes are supported (by checking for `/etc/nixos/flake.nix`). No additional version check is added — the error message in the log will be clear if it occurs. This is an acceptable risk for the target audience.

**Alternative (if needed in a future spec):** Run `cd /etc/nixos && nix flake update` by setting `current_dir` in the Command builder. This uses the old working-directory behavior:
```rust
Command::new("sudo")
    .args(["nix", "flake", "update"])
    .current_dir("/etc/nixos")
    // ...
```
However, this requires modifying `run_streaming_command` to accept an optional working directory, which is out of scope.

### 7.2 Hostname does not match flake attribute name

**Risk:** The user's flake may use an attribute name that differs from the system hostname. Example: hostname is `desktop-1` but the flake has `nixosConfigurations.home-desktop`. This would cause `nixos-rebuild` to fail with:
```
error: flake '/etc/nixos' does not provide attribute 'nixosConfigurations.desktop-1'
```

**Mitigation:**
- This failure is visible in the streaming log output — the user will see the exact error.
- The auto-detection behavior from the previous implementation had the same risk.
- A future enhancement could read the available `nixosConfigurations` from `nix flake show /etc/nixos` and present a dropdown selector, but this is out of scope for this change.
- Documentation (if applicable) should note that the flake attribute must match the system hostname.

### 7.3 Special characters in hostname

**Risk:** Hostnames with special characters (e.g., `my.host.local` with dots, or FQDN) may not be valid Nix attribute names in some contexts.

**Mitigation:** NixOS `networking.hostName` restriction: "Must only contain letters, digits, and hyphens; maximum 63 characters" (per RFC 1123). Dots are NOT valid in NixOS hostnames. `/proc/sys/kernel/hostname` therefore returns a simple `[a-zA-Z0-9-]` string on NixOS. No sanitization is needed.

### 7.4 Borrow checker: `flake_target` String used across `run_streaming_command` calls

**Implementation note:** The `flake_target` String must live long enough to be borrowed for both the log message and the `run_streaming_command` call. Since both calls are sequential (not async/concurrent), the borrow is fine — `flake_target` is stack-allocated and lives for the entire `NixOsConfigType::Flake` match arm.

The call `run_streaming_command("pkexec", &["nixos-rebuild", "switch", "--flake", &flake_target], tx)` passes `&flake_target` which coerces to `&str` via `Deref<Target = str>`. This is valid Rust, no lifetime issues.

### 7.5 Hostname detection timing

`detect_system_hostname()` is called twice:
1. At page construction in `upgrade_page.rs` — for the Config Type row subtitle.
2. Inside `upgrade_nixos()` at upgrade execution time — for the actual command.

Both calls read `/proc/sys/kernel/hostname`. Since NixOS requires a rebuild to change the hostname, the two values will always be identical in practice. This minimal redundancy is acceptable.

---

## 8. Non-Goals (Out of Scope for This Change)

- Changing the privilege escalation strategy (`sudo` vs `pkexec`). Tracked as R5 in `nixos_upgrade_review.md`.
- Making the flake path (`/etc/nixos`) configurable via UI. Hardcoded path matches the user's stated command.
- Supporting non-`/etc/nixos` flake paths. Most NixOS systems store their config at `/etc/nixos`.
- Auto-discovering available `nixosConfigurations` attributes via `nix flake show`.
- Changing the upgrade confirmation dialog text. Already tracked as R6 in `nixos_upgrade_review.md`.
- Adding the `hostname` crate as a dependency. `/proc/sys/kernel/hostname` achieves the same result without it.

---

## 9. Summary of Changes

| What | File | Lines affected |
|---|---|---|
| Add `detect_system_hostname()` | `src/upgrade.rs` | ~5 new lines after `detect_nixos_config_type()` |
| Fix `nix flake update` syntax (Bug A) | `src/upgrade.rs` | 1 line change: `"/etc/nixos"` → `"--flake", "/etc/nixos"` |
| Fix explicit `#hostname` in nixos-rebuild (Bug B) | `src/upgrade.rs` | ~7 lines: add hostname detection, format flake_target, update call |
| Update Config Type row subtitle | `src/ui/upgrade_page.rs` | ~6 lines: match arm produces owned String with hostname |

Total estimated diff: ~20 lines changed/added. No new dependencies. No schema changes.
