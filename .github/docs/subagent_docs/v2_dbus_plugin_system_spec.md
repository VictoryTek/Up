# v2.0 Specification: D-Bus Backend Service & Plugin Discovery System

> Generated: May 11, 2026 — Research & Specification Phase

---

## Table of Contents

1. [Current State Analysis](#current-state-analysis)
2. [Problem Definition](#problem-definition)
3. [Proposed Solution Architecture](#proposed-solution-architecture)
4. [D-Bus Service Design](#d-bus-service-design)
5. [Polkit Policy Design](#polkit-policy-design)
6. [Backend Plugin System](#backend-plugin-system)
7. [Implementation Steps](#implementation-steps)
8. [Dependencies](#dependencies)
9. [Migration Strategy](#migration-strategy)
10. [Security Analysis](#security-analysis)
11. [Risks and Mitigations](#risks-and-mitigations)
12. [Testing Strategy](#testing-strategy)

---

## 1. Current State Analysis

### 1.1 Privileged Execution (Current)

**File:** `src/runner.rs` — `PrivilegedShell`

The current architecture spawns a persistent `pkexec /bin/sh` process for all privileged operations:

```
User clicks "Update All"
  → PrivilegedShell::new() spawns `pkexec /bin/sh`
  → User authenticates once via polkit agent
  → All privileged commands piped to stdin of the root shell
  → Output parsed via stdout sentinel tokens (___UP_RC_<session_id>_<exit_code>___)
  → Shell closed when all backends complete
```

**Problems with this approach:**
- The sentinel pattern (`___UP_RC_...`) is parsed from the command's own stdout stream — any subprocess printing a matching line could spoof exit codes
- A single `pkexec /bin/sh` grants **full root shell access** — not scoped to specific operations
- No cancellation mechanism for in-flight commands (stdin EOF kills the shell but not the running command)
- No audit logging of which specific commands were executed with root privilege
- The current polkit policy annotates `/bin/sh` as the executable path — any `pkexec /bin/sh` invocation matches this action, not just Up's
- No systemd integration or automatic cleanup on crash

### 1.2 Backend Architecture (Current)

**Files:** `src/backends/mod.rs`, `src/backends/os_package_manager.rs`, etc.

The backend system is a compile-time, hardcoded set:

```rust
pub enum BackendKind {
    Apt, Dnf, Pacman, Zypper, Flatpak, Homebrew, Nix, Fwupd,
}

pub trait Backend: Send + Sync {
    fn kind(&self) -> BackendKind;
    fn display_name(&self) -> &str;
    fn run_update<'a>(&'a self, runner: &'a dyn CommandExecutor) -> Pin<Box<...>>;
    fn needs_root(&self) -> bool { false }
    fn list_available(&self) -> Pin<Box<...>> { ... }
    // ... other methods
}
```

Detection in `detect_backends()`:
```rust
pub fn detect_backends() -> Vec<Arc<dyn Backend>> {
    // Hardcoded checks using `which::which("apt")`, etc.
}
```

**Problems:**
- Adding a new backend (e.g., `apk`, `xbps`, `eopkg`, `swupd`) requires modifying core source code
- `BackendKind` enum must be extended, requiring changes to every `match` site
- Community contributors cannot add backends without understanding and building the entire project
- No mechanism for distro vendors to ship their own backend definitions
- Output parsing logic is tightly coupled to each backend struct

### 1.3 Current Polkit Policy

**File:** `data/io.github.up.policy`

Two actions defined:
- `io.github.up.pkexec.update` — annotates `/bin/sh` for package updates
- `io.github.up.pkexec.upgrade` — annotates `/usr/bin/env` for distribution upgrades

Both use `auth_admin_keep` for active sessions. The NOTE in the policy file explicitly states: *"This action matches any caller of `pkexec /bin/sh`, not only Up. True per-application scoping requires a D-Bus backend service."*

---

## 2. Problem Definition

### 2.1 Security Concerns

| Issue | Severity | Description |
|-------|----------|-------------|
| Overly broad privilege | Critical | `pkexec /bin/sh` grants unrestricted root shell; not scoped to specific package management operations |
| Sentinel spoofing | High | A malicious package post-install script could print `___UP_RC_...` tokens to manipulate exit code parsing |
| No audit trail | High | Root commands are not logged to systemd journal or audit subsystem — impossible to trace what ran as root |
| Shared polkit action | Medium | Any process calling `pkexec /bin/sh` triggers the same polkit action; a compromised process could piggyback on Up's cached credential |

### 2.2 Architectural Concerns

| Issue | Severity | Description |
|-------|----------|-------------|
| No cancellation | High | Cannot cancel a running `apt upgrade` without killing the entire root shell; no SIGINT/SIGTERM forwarding |
| Hardcoded backends | Medium | Community cannot extend without forking; distro-specific backends require core changes |
| Monolithic privilege | Medium | All backends share one root shell — a Flatpak bug could theoretically access the root shell meant for APT |
| No progress reporting | Low | Stdout line counting is the only progress indicator; no structured progress protocol |

### 2.3 Desired Outcomes

1. **Principle of least privilege** — each operation type gets its own polkit action; the daemon only executes pre-approved command patterns
2. **Proper cancellation** — cancel in-flight operations via D-Bus method call, with graceful SIGTERM→SIGKILL escalation
3. **Audit logging** — all privileged operations logged to systemd journal with caller PID, UID, and action ID
4. **Extensibility** — community/vendor backends loadable from YAML descriptors without recompilation
5. **Backward compatibility** — graceful fallback to `pkexec` path when D-Bus service is unavailable (e.g., development, testing)

---

## 3. Proposed Solution Architecture

### 3.1 High-Level Design

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                          Up (GTK4 Frontend)                                  │
│                                                                             │
│  ┌──────────────────┐     ┌─────────────────────────┐                      │
│  │ UpdateOrchestrator│     │ PluginRegistry          │                      │
│  │                  │     │ (loads YAML descriptors) │                      │
│  └────────┬─────────┘     └──────────┬──────────────┘                      │
│           │                           │                                      │
│           ▼                           ▼                                      │
│  ┌──────────────────────────────────────────────────┐                       │
│  │ UpDaemonProxy (zbus client)                       │                       │
│  │ io.github.up.Daemon on system bus                 │                       │
│  └──────────────────────┬───────────────────────────┘                       │
└─────────────────────────┼───────────────────────────────────────────────────┘
                          │ D-Bus (system bus)
                          ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                     up-daemon (Privileged Service)                           │
│                     Runs as root via systemd activation                      │
│                                                                             │
│  ┌──────────────────────────────────────────────────────────────────┐       │
│  │ D-Bus Interface: io.github.up.Daemon1                             │       │
│  │                                                                   │       │
│  │ Methods:                                                          │       │
│  │   RunUpdate(backend_id: str) → (success: bool, output: str)      │       │
│  │   RunCleanup(backend_id: str) → (success: bool, output: str)     │       │
│  │   RunUpgrade(distro: str, variant: str) → (success: bool)        │       │
│  │   CreateSnapshot(tool: str) → (success: bool, desc: str)         │       │
│  │   Cancel(operation_id: str) → ()                                  │       │
│  │   ListOperations() → Vec<OperationInfo>                           │       │
│  │                                                                   │       │
│  │ Signals:                                                          │       │
│  │   OperationProgress(op_id: str, line: str)                        │       │
│  │   OperationComplete(op_id: str, success: bool, msg: str)          │       │
│  │                                                                   │       │
│  │ Properties:                                                       │       │
│  │   Version: str                                                    │       │
│  │   ActiveOperations: Vec<str>                                      │       │
│  └──────────────────────────────────────────────────────────────────┘       │
│                                                                             │
│  ┌────────────────┐  ┌────────────────┐  ┌────────────────────────┐        │
│  │PolkitAuthority │  │CommandAllowlist│  │ Audit Logger (journal) │        │
│  └────────────────┘  └────────────────┘  └────────────────────────┘        │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 3.2 Component Responsibilities

| Component | Responsibility |
|-----------|---------------|
| **Up (frontend)** | UI, plugin registry, D-Bus client proxy, fallback to pkexec |
| **up-daemon** | Privileged command execution, polkit auth checking, audit logging, cancellation |
| **Plugin Registry** | Loads YAML descriptors, validates schemas, provides `Backend` implementations |
| **Polkit Policy** | Defines scoped actions per operation type |
| **systemd unit** | Socket-activated daemon lifecycle, idle timeout, resource limits |

### 3.3 Daemon Lifecycle

1. **Activation:** systemd D-Bus activation — daemon starts on first method call to `io.github.up.Daemon1`
2. **Idle timeout:** 60 seconds after last active operation; daemon exits cleanly
3. **Crash recovery:** systemd `Restart=on-failure` with 5s delay
4. **Resource limits:** `MemoryMax=256M`, `CPUQuota=50%` via systemd unit

---

## 4. D-Bus Service Design

### 4.1 D-Bus Interface Definition (XML)

```xml
<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<node name="/io/github/up/Daemon">
  <interface name="io.github.up.Daemon1">

    <!-- Run the update command for a specific backend -->
    <method name="RunUpdate">
      <arg name="backend_id" type="s" direction="in"/>
      <arg name="operation_id" type="s" direction="out"/>
    </method>

    <!-- Run the cleanup/maintenance command for a specific backend -->
    <method name="RunCleanup">
      <arg name="backend_id" type="s" direction="in"/>
      <arg name="operation_id" type="s" direction="out"/>
    </method>

    <!-- Run a distribution upgrade -->
    <method name="RunUpgrade">
      <arg name="distro_id" type="s" direction="in"/>
      <arg name="variant" type="s" direction="in"/>
      <arg name="operation_id" type="s" direction="out"/>
    </method>

    <!-- Create a pre-update snapshot -->
    <method name="CreateSnapshot">
      <arg name="tool" type="s" direction="in"/>
      <arg name="operation_id" type="s" direction="out"/>
    </method>

    <!-- Cancel a running operation -->
    <method name="Cancel">
      <arg name="operation_id" type="s" direction="in"/>
      <arg name="success" type="b" direction="out"/>
    </method>

    <!-- List currently active operations -->
    <method name="ListOperations">
      <arg name="operations" type="a(ssb)" direction="out"/>
      <!-- Array of (operation_id, backend_id, is_cancellable) -->
    </method>

    <!-- Signal: a line of output from an operation -->
    <signal name="OperationOutput">
      <arg name="operation_id" type="s"/>
      <arg name="line" type="s"/>
    </signal>

    <!-- Signal: an operation has completed -->
    <signal name="OperationComplete">
      <arg name="operation_id" type="s"/>
      <arg name="success" type="b"/>
      <arg name="exit_code" type="i"/>
      <arg name="summary" type="s"/>
    </signal>

    <!-- Properties -->
    <property name="Version" type="s" access="read"/>
    <property name="ActiveOperationCount" type="u" access="read"/>
    <property name="IdleTimeoutSecs" type="u" access="read"/>

  </interface>
</node>
```

### 4.2 zbus Service Implementation Pattern

```rust
use zbus::{connection, interface, fdo, object_server::SignalEmitter};
use tokio::sync::broadcast;

struct UpDaemon {
    operations: Arc<Mutex<HashMap<String, OperationHandle>>>,
    command_allowlist: CommandAllowlist,
}

#[interface(name = "io.github.up.Daemon1")]
impl UpDaemon {
    async fn run_update(
        &self,
        backend_id: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<String> {
        // 1. Verify polkit authorization for io.github.up.update.<backend_id>
        // 2. Validate backend_id against allowlist
        // 3. Spawn privileged command
        // 4. Return operation_id
        // 5. Stream output via OperationOutput signal
        todo!()
    }

    async fn cancel(&self, operation_id: &str) -> fdo::Result<bool> {
        // Send SIGTERM to process group, escalate to SIGKILL after 10s
        todo!()
    }

    #[zbus(signal)]
    async fn operation_output(
        emitter: &SignalEmitter<'_>,
        operation_id: &str,
        line: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn operation_complete(
        emitter: &SignalEmitter<'_>,
        operation_id: &str,
        success: bool,
        exit_code: i32,
        summary: &str,
    ) -> zbus::Result<()>;

    #[zbus(property)]
    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    #[zbus(property)]
    fn active_operation_count(&self) -> u32 {
        self.operations.lock().unwrap().len() as u32
    }
}
```

### 4.3 zbus Client Proxy (in frontend)

```rust
use zbus::{proxy, Connection};

#[proxy(
    interface = "io.github.up.Daemon1",
    default_service = "io.github.up.Daemon",
    default_path = "/io/github/up/Daemon"
)]
trait UpDaemon {
    async fn run_update(&self, backend_id: &str) -> zbus::Result<String>;
    async fn run_cleanup(&self, backend_id: &str) -> zbus::Result<String>;
    async fn run_upgrade(&self, distro_id: &str, variant: &str) -> zbus::Result<String>;
    async fn create_snapshot(&self, tool: &str) -> zbus::Result<String>;
    async fn cancel(&self, operation_id: &str) -> zbus::Result<bool>;

    #[zbus(signal)]
    async fn operation_output(&self, operation_id: String, line: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn operation_complete(
        &self,
        operation_id: String,
        success: bool,
        exit_code: i32,
        summary: String,
    ) -> zbus::Result<()>;

    #[zbus(property)]
    fn version(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn active_operation_count(&self) -> zbus::Result<u32>;
}
```

### 4.4 systemd Service Files

**`data/io.github.up.Daemon.service`:**
```ini
[Unit]
Description=Up System Update Daemon
Documentation=https://github.com/VictoryTek/Up
After=dbus.service

[Service]
Type=dbus
BusName=io.github.up.Daemon
ExecStart=/usr/libexec/up-daemon
User=root

# Security hardening
NoNewPrivileges=no
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
SystemCallFilter=@system-service @process @network-io

# Resource limits
MemoryMax=256M
CPUQuota=50%

# Idle timeout — daemon exits after 60s of inactivity
# (implemented in application code via tokio timeout)

# Restart on crash
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

**`data/io.github.up.Daemon.conf`** (D-Bus system bus policy):
```xml
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <!-- Only root can own this name -->
  <policy user="root">
    <allow own="io.github.up.Daemon"/>
  </policy>

  <!-- Any user in the 'wheel' or 'sudo' group can call methods -->
  <policy context="default">
    <allow send_destination="io.github.up.Daemon"
           send_interface="io.github.up.Daemon1"/>
    <allow send_destination="io.github.up.Daemon"
           send_interface="org.freedesktop.DBus.Properties"/>
    <allow send_destination="io.github.up.Daemon"
           send_interface="org.freedesktop.DBus.Introspectable"/>
  </policy>
</busconfig>
```

### 4.5 Cancellation Mechanism

```rust
struct OperationHandle {
    operation_id: String,
    backend_id: String,
    child: Option<tokio::process::Child>,
    cancel_token: CancellationToken,
    started_at: std::time::Instant,
}

impl OperationHandle {
    /// Cancel with graceful shutdown: SIGTERM → 10s wait → SIGKILL
    async fn cancel(&mut self) -> bool {
        self.cancel_token.cancel();
        if let Some(child) = &self.child {
            let pid = child.id();
            if let Some(pid) = pid {
                // Send SIGTERM to process group
                unsafe { libc::kill(-(pid as i32), libc::SIGTERM); }

                // Wait up to 10s for graceful exit
                let timeout = tokio::time::sleep(Duration::from_secs(10));
                tokio::select! {
                    _ = child.wait() => return true,
                    _ = timeout => {
                        // Escalate to SIGKILL
                        unsafe { libc::kill(-(pid as i32), libc::SIGKILL); }
                        return true;
                    }
                }
            }
        }
        false
    }
}
```

### 4.6 Audit Logging

All privileged operations logged via `log` crate → systemd journal:

```rust
use log::{info, warn};

fn audit_log(caller_uid: u32, caller_pid: u32, action: &str, backend: &str, args: &[&str]) {
    info!(
        target: "up-daemon::audit",
        "AUDIT: uid={} pid={} action={} backend={} cmd={:?}",
        caller_uid, caller_pid, action, backend, args
    );
}
```

Journal fields visible via `journalctl -u io.github.up.Daemon`:
```
May 11 10:15:03 host up-daemon[1234]: AUDIT: uid=1000 pid=5678 action=update backend=apt cmd=["apt", "update", "&&", "apt", "upgrade", "-y"]
```

### 4.7 Command Allowlist

The daemon does NOT accept arbitrary commands. It maintains a strict allowlist of permitted command patterns per backend:

```rust
struct CommandAllowlist {
    entries: HashMap<String, Vec<AllowedCommand>>,
}

struct AllowedCommand {
    program: String,
    args_pattern: Vec<ArgPattern>,
    environment: HashMap<String, String>,
}

enum ArgPattern {
    Literal(String),
    /// Backend-specified argument validated against regex
    Variable { name: String, regex: String },
}
```

Built-in backends have their allowlists compiled in. Plugin backends declare commands in their YAML descriptor, and the daemon validates each command against the declared pattern before execution.

---

## 5. Polkit Policy Design

### 5.1 Scoped Action Hierarchy

```
io.github.up
├── io.github.up.update          (run package updates)
│   ├── io.github.up.update.system   (APT/DNF/Pacman/Zypper — needs root)
│   ├── io.github.up.update.firmware (fwupd — own polkit, just passes through)
│   └── io.github.up.update.plugin   (plugin-defined backends needing root)
├── io.github.up.cleanup         (run maintenance/cleanup)
│   └── io.github.up.cleanup.system  (autoremove, clean cache)
├── io.github.up.upgrade         (distribution upgrade)
│   └── io.github.up.upgrade.system  (major version upgrade)
├── io.github.up.snapshot        (create pre-update snapshots)
│   └── io.github.up.snapshot.create
└── io.github.up.cancel          (cancel running operations)
    └── io.github.up.cancel.operation
```

### 5.2 Policy File

**`data/io.github.up.policy`** (replaces current file):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE policyconfig PUBLIC
    "-//freedesktop//DTD PolicyKit Policy Configuration 1.0//EN"
    "http://www.freedesktop.org/standards/PolicyKit/1/policyconfig.dtd">
<policyconfig>

  <vendor>Up — System Updater</vendor>
  <vendor_url>https://github.com/VictoryTek/Up</vendor_url>
  <icon_name>io.github.up</icon_name>

  <!-- Update system packages (APT, DNF, Pacman, Zypper, Nix) -->
  <action id="io.github.up.update.system">
    <description>Update system packages</description>
    <message>Up needs administrator privileges to update system packages.</message>
    <defaults>
      <allow_any>auth_admin</allow_any>
      <allow_inactive>auth_admin</allow_inactive>
      <allow_active>auth_admin_keep</allow_active>
    </defaults>
  </action>

  <!-- Update plugin-managed backends -->
  <action id="io.github.up.update.plugin">
    <description>Update packages via plugin backend</description>
    <message>Up needs administrator privileges to update packages via a plugin backend.</message>
    <defaults>
      <allow_any>auth_admin</allow_any>
      <allow_inactive>auth_admin</allow_inactive>
      <allow_active>auth_admin_keep</allow_active>
    </defaults>
  </action>

  <!-- Cleanup / maintenance operations -->
  <action id="io.github.up.cleanup.system">
    <description>Run system cleanup and maintenance</description>
    <message>Up needs administrator privileges to remove unused packages.</message>
    <defaults>
      <allow_any>auth_admin</allow_any>
      <allow_inactive>auth_admin</allow_inactive>
      <allow_active>auth_admin_keep</allow_active>
    </defaults>
  </action>

  <!-- Distribution upgrade -->
  <action id="io.github.up.upgrade.system">
    <description>Upgrade the system to a new release</description>
    <message>Up needs administrator privileges to perform a system upgrade.</message>
    <defaults>
      <allow_any>auth_admin</allow_any>
      <allow_inactive>auth_admin</allow_inactive>
      <allow_active>auth_admin</allow_active>
    </defaults>
  </action>

  <!-- Snapshot creation -->
  <action id="io.github.up.snapshot.create">
    <description>Create a system snapshot</description>
    <message>Up needs administrator privileges to create a system snapshot.</message>
    <defaults>
      <allow_any>auth_admin</allow_any>
      <allow_inactive>auth_admin</allow_inactive>
      <allow_active>auth_admin_keep</allow_active>
    </defaults>
  </action>

  <!-- Cancel an in-progress operation -->
  <action id="io.github.up.cancel.operation">
    <description>Cancel a running update operation</description>
    <message>Up needs privileges to cancel an in-progress system operation.</message>
    <defaults>
      <allow_any>auth_admin</allow_any>
      <allow_inactive>auth_admin</allow_inactive>
      <allow_active>yes</allow_active>
    </defaults>
  </action>

</policyconfig>
```

### 5.3 Polkit Authorization in Daemon

The daemon checks polkit before executing any privileged operation:

```rust
use zbus::Connection;

async fn check_polkit_auth(
    connection: &Connection,
    caller_name: &str,
    action_id: &str,
) -> Result<bool, zbus::Error> {
    let proxy = zbus::fdo::PolicyKit1Proxy::builder(connection)
        .destination("org.freedesktop.PolicyKit1")?
        .path("/org/freedesktop/PolicyKit1/Authority")?
        .build()
        .await?;

    // CheckAuthorization with AllowUserInteraction flag
    // so the polkit agent presents the auth dialog
    let subject = polkit_subject_from_bus_name(caller_name);
    let result = proxy.check_authorization(
        &subject,
        action_id,
        &HashMap::new(),  // details
        1,  // AllowUserInteraction
        "",  // cancellation_id
    ).await?;

    Ok(result.is_authorized)
}
```

---

## 6. Backend Plugin System

### 6.1 Plugin Discovery Paths

Following XDG Base Directory specification (`$XDG_DATA_DIRS`):

| Path | Purpose | Priority |
|------|---------|----------|
| `/usr/share/up/backends.d/` | Distribution-shipped backends | Low (default) |
| `/usr/local/share/up/backends.d/` | Locally-installed backends | Medium |
| `$XDG_DATA_HOME/up/backends.d/` | User-installed backends (non-privileged only) | High |
| `/etc/up/backends.d/` | Sysadmin overrides/disables | Highest |

Discovery order: files in higher-priority directories override same-named files in lower-priority ones. A file named `disabled` (zero-byte) in `/etc/up/backends.d/` disables a backend.

### 6.2 YAML Descriptor Schema

**Schema version:** `1`

```yaml
# /usr/share/up/backends.d/apk.yaml
---
schema_version: 1

# Unique identifier — must match filename (without .yaml)
id: apk

# Human-readable metadata
display_name: "APK"
description: "Alpine Linux packages"
icon_name: "system-software-install-symbolic"

# Detection: how to determine if this backend is available
detection:
  # All conditions must be true for the backend to be active
  binary: "apk"                    # Required binary in PATH
  os_id: ["alpine"]               # Optional: match ID= from /etc/os-release
  file_exists: null                # Optional: path that must exist

# Privilege model
privilege:
  needs_root: true
  polkit_action: "io.github.up.update.system"  # or "io.github.up.update.plugin"

# Commands — each is a named operation
commands:
  update:
    # Command executed for the 'update' operation
    # Supports variable interpolation: {{packages}} for package list
    program: "apk"
    args: ["upgrade", "--no-interactive"]
    environment:
      LANG: "C"
      LC_ALL: "C"
    # Parser for counting updated packages
    parser:
      type: "regex_count"
      pattern: "^\\(\\d+/\\d+\\) Upgrading"

  list_available:
    program: "apk"
    args: ["version", "-l", "<"]
    parser:
      type: "line_field"
      field_index: 0
      separator: " "
      skip_lines: 0  # header lines to skip

  cleanup:
    program: "apk"
    args: ["cache", "clean"]
    parser:
      type: "line_count"
      pattern: "^Removing"

  estimate_size:
    program: "apk"
    args: ["upgrade", "--simulate"]
    parser:
      type: "size_regex"
      pattern: "After this operation, (\\d+) .iB"
      unit_group: 1

# Capabilities — declares what this backend supports
capabilities:
  update: true
  list_available: true
  cleanup: true
  estimate_size: true
  count_available: true  # derived from list_available

# Metadata for the plugin system
metadata:
  author: "Alpine Linux Community"
  version: "1.0.0"
  min_up_version: "2.0.0"
  license: "GPL-3.0-or-later"
```

### 6.3 Additional Example: XBPS (Void Linux)

```yaml
# /usr/share/up/backends.d/xbps.yaml
---
schema_version: 1
id: xbps
display_name: "XBPS"
description: "Void Linux packages"
icon_name: "system-software-install-symbolic"

detection:
  binary: "xbps-install"
  os_id: ["void"]

privilege:
  needs_root: true
  polkit_action: "io.github.up.update.system"

commands:
  update:
    program: "xbps-install"
    args: ["-Syu"]
    environment:
      LANG: "C"
    parser:
      type: "regex_count"
      pattern: "^\\S+ \\S+ -> \\S+"

  list_available:
    program: "xbps-install"
    args: ["-Mun"]
    parser:
      type: "line_field"
      field_index: 0
      separator: " "
      skip_lines: 0

  cleanup:
    program: "xbps-remove"
    args: ["-Oo"]
    parser:
      type: "line_count"
      pattern: "^Removing"

capabilities:
  update: true
  list_available: true
  cleanup: true
  estimate_size: false
  count_available: true

metadata:
  author: "Void Linux Community"
  version: "1.0.0"
  min_up_version: "2.0.0"
  license: "GPL-3.0-or-later"
```

### 6.4 Parser Types

The plugin system supports these parser types for extracting structured data from command output:

| Parser Type | Description | Parameters |
|-------------|-------------|------------|
| `regex_count` | Count lines matching a regex | `pattern` |
| `line_count` | Count lines matching a prefix/pattern | `pattern` |
| `line_field` | Extract field from each line | `field_index`, `separator`, `skip_lines` |
| `size_regex` | Extract size value from regex capture group | `pattern`, `unit_group` |
| `json_path` | Extract value from JSON output | `path` |
| `exit_code` | Use exit code as the result | `success_codes: [0]`, `update_code: 100` |

### 6.5 Plugin Registry (Rust Implementation)

```rust
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Schema version for plugin descriptors
const CURRENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize)]
pub struct PluginDescriptor {
    pub schema_version: u32,
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub icon_name: String,
    pub detection: DetectionConfig,
    pub privilege: PrivilegeConfig,
    pub commands: CommandSet,
    pub capabilities: CapabilitySet,
    pub metadata: PluginMetadata,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DetectionConfig {
    pub binary: String,
    #[serde(default)]
    pub os_id: Vec<String>,
    pub file_exists: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PrivilegeConfig {
    pub needs_root: bool,
    pub polkit_action: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CommandSet {
    pub update: Option<CommandDef>,
    pub list_available: Option<CommandDef>,
    pub cleanup: Option<CommandDef>,
    pub estimate_size: Option<CommandDef>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CommandDef {
    pub program: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub environment: HashMap<String, String>,
    pub parser: ParserDef,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum ParserDef {
    #[serde(rename = "regex_count")]
    RegexCount { pattern: String },
    #[serde(rename = "line_count")]
    LineCount { pattern: String },
    #[serde(rename = "line_field")]
    LineField {
        field_index: usize,
        separator: String,
        #[serde(default)]
        skip_lines: usize,
    },
    #[serde(rename = "size_regex")]
    SizeRegex { pattern: String, unit_group: usize },
    #[serde(rename = "json_path")]
    JsonPath { path: String },
    #[serde(rename = "exit_code")]
    ExitCode {
        #[serde(default = "default_success_codes")]
        success_codes: Vec<i32>,
        update_code: Option<i32>,
    },
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CapabilitySet {
    pub update: bool,
    pub list_available: bool,
    #[serde(default)]
    pub cleanup: bool,
    #[serde(default)]
    pub estimate_size: bool,
    #[serde(default)]
    pub count_available: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PluginMetadata {
    pub author: String,
    pub version: String,
    pub min_up_version: String,
    pub license: String,
}

/// Scan all plugin directories and return validated descriptors
pub fn discover_plugins() -> Vec<PluginDescriptor> {
    let dirs = plugin_search_dirs();
    let mut seen: HashMap<String, PluginDescriptor> = HashMap::new();

    // Iterate in reverse priority (lowest first) so higher-priority overrides
    for dir in dirs.iter().rev() {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "yaml" || e == "yml").unwrap_or(false) {
                    match load_and_validate(&path) {
                        Ok(desc) => { seen.insert(desc.id.clone(), desc); }
                        Err(e) => log::warn!("Skipping plugin {:?}: {}", path, e),
                    }
                }
            }
        }
    }

    // Check /etc/up/backends.d/ for disabled files
    let etc_dir = PathBuf::from("/etc/up/backends.d");
    seen.retain(|id, _| {
        !etc_dir.join(format!("{}.disabled", id)).exists()
    });

    seen.into_values().collect()
}

fn plugin_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // XDG_DATA_HOME (user plugins — non-privileged only)
    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(data_home).join("up/backends.d"));
    } else if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/up/backends.d"));
    }

    // XDG_DATA_DIRS (system plugins)
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':') {
        dirs.push(PathBuf::from(dir).join("up/backends.d"));
    }

    dirs
}
```

### 6.6 PluginBackend: Backend Trait Implementation

```rust
use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::executor::CommandExecutor;

/// A `Backend` implementation constructed from a YAML plugin descriptor.
pub struct PluginBackend {
    descriptor: PluginDescriptor,
}

impl Backend for PluginBackend {
    fn kind(&self) -> BackendKind {
        // Plugins use BackendKind::Plugin(id) — requires extending the enum
        // OR we use a dynamic approach (see Migration section)
        BackendKind::Plugin(self.descriptor.id.clone())
    }

    fn display_name(&self) -> &str {
        &self.descriptor.display_name
    }

    fn description(&self) -> &str {
        &self.descriptor.description
    }

    fn icon_name(&self) -> &str {
        &self.descriptor.icon_name
    }

    fn needs_root(&self) -> bool {
        self.descriptor.privilege.needs_root
    }

    fn run_update<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            let Some(cmd) = &self.descriptor.commands.update else {
                return UpdateResult::Skipped("No update command defined".into());
            };

            let args: Vec<&str> = cmd.args.iter().map(|s| s.as_str()).collect();
            let result = if self.descriptor.privilege.needs_root {
                // Route through privileged path
                let mut full_args = vec![cmd.program.as_str()];
                full_args.extend(&args);
                runner.run("pkexec", &full_args).await
            } else {
                runner.run(&cmd.program, &args).await
            };

            match result {
                Ok(output) => {
                    let count = apply_parser(&cmd.parser, &output);
                    UpdateResult::Success { updated_count: count }
                }
                Err(e) => UpdateResult::Error(e),
            }
        })
    }

    // ... similar implementations for list_available, cleanup, estimate_size
}
```

### 6.7 Plugin Validation Rules

Before a plugin is loaded, the following checks are performed:

1. **Schema version** — must equal `CURRENT_SCHEMA_VERSION`
2. **ID format** — lowercase alphanumeric + hyphens only, max 32 chars
3. **Binary exists** — `which::which(detection.binary)` succeeds
4. **OS match** — if `os_id` is non-empty, current OS must match
5. **No path traversal** — `program` field must not contain `..` or start with `/` (resolved via PATH)
6. **No shell metacharacters** — `args` must not contain `;`, `|`, `&`, `$`, backticks, `>`
7. **Polkit action** — must be one of the allowed action prefixes (`io.github.up.update.*`, `io.github.up.cleanup.*`)
8. **Version compatibility** — `min_up_version` <= current Up version

---

## 7. Implementation Steps

### Phase 1: D-Bus Daemon (Core)

| Step | Action | Files |
|------|--------|-------|
| 1.1 | Create `up-daemon` binary crate | `daemon/Cargo.toml`, `daemon/src/main.rs` |
| 1.2 | Implement D-Bus interface with `zbus` `#[interface]` | `daemon/src/interface.rs` |
| 1.3 | Implement polkit authorization checking | `daemon/src/auth.rs` |
| 1.4 | Implement command execution with process group mgmt | `daemon/src/executor.rs` |
| 1.5 | Implement cancellation with SIGTERM/SIGKILL | `daemon/src/cancel.rs` |
| 1.6 | Implement audit logging to journal | `daemon/src/audit.rs` |
| 1.7 | Implement idle timeout and graceful shutdown | `daemon/src/lifecycle.rs` |
| 1.8 | Implement command allowlist | `daemon/src/allowlist.rs` |
| 1.9 | Create systemd service file | `data/io.github.up.Daemon.service` |
| 1.10 | Create D-Bus system bus configuration | `data/io.github.up.Daemon.conf` |
| 1.11 | Update polkit policy with scoped actions | `data/io.github.up.policy` |

### Phase 2: Frontend D-Bus Client

| Step | Action | Files |
|------|--------|-------|
| 2.1 | Add `zbus` dependency to frontend | `Cargo.toml` |
| 2.2 | Create D-Bus proxy module | `src/dbus_client.rs` |
| 2.3 | Implement `DaemonExecutor` (implements `CommandExecutor` via D-Bus) | `src/dbus_client.rs` |
| 2.4 | Add fallback detection (use D-Bus if daemon available, else pkexec) | `src/orchestrator.rs` |
| 2.5 | Wire signal streaming to `OrchestratorEvent` channel | `src/orchestrator.rs` |
| 2.6 | Update `CancelHandle` to use D-Bus `Cancel` method | `src/orchestrator.rs` |

### Phase 3: Plugin System

| Step | Action | Files |
|------|--------|-------|
| 3.1 | Add `serde_yml` and `regex` dependencies | `Cargo.toml` |
| 3.2 | Create plugin descriptor types | `src/plugins/descriptor.rs` |
| 3.3 | Implement plugin discovery and loading | `src/plugins/discovery.rs` |
| 3.4 | Implement plugin validation | `src/plugins/validate.rs` |
| 3.5 | Implement parser engine | `src/plugins/parser.rs` |
| 3.6 | Implement `PluginBackend` (Backend trait for plugins) | `src/plugins/backend.rs` |
| 3.7 | Extend `BackendKind` to support dynamic plugins | `src/backends/mod.rs` |
| 3.8 | Integrate plugin backends into `detect_backends()` | `src/backends/mod.rs` |
| 3.9 | Ship built-in YAML descriptors for existing backends | `data/backends.d/` |
| 3.10 | Create example community plugin descriptors | `examples/plugins/` |

### Phase 4: Integration

| Step | Action | Files |
|------|--------|-------|
| 4.1 | Update meson.build to install daemon, service files, bus config | `meson.build` |
| 4.2 | Update flake.nix to build and package daemon | `flake.nix` |
| 4.3 | Update preflight.sh to build daemon crate | `scripts/preflight.sh` |
| 4.4 | Add integration tests for D-Bus interface | `daemon/tests/` |
| 4.5 | Add plugin loading tests with sample YAML | `src/plugins/tests/` |
| 4.6 | Update documentation | `README.md`, `.github/docs/` |

### New Files to Create

```
daemon/
├── Cargo.toml
├── src/
│   ├── main.rs           # Daemon entry point, connection builder
│   ├── interface.rs      # #[interface] implementation
│   ├── auth.rs           # Polkit authorization
│   ├── executor.rs       # Privileged command execution
│   ├── cancel.rs         # Cancellation logic
│   ├── audit.rs          # Journal audit logging
│   ├── lifecycle.rs      # Idle timeout, shutdown
│   └── allowlist.rs      # Command allowlist validation
src/
├── dbus_client.rs        # Frontend D-Bus proxy + DaemonExecutor
├── plugins/
│   ├── mod.rs
│   ├── descriptor.rs     # YAML schema types
│   ├── discovery.rs      # Directory scanning
│   ├── validate.rs       # Security validation
│   ├── parser.rs         # Output parser engine
│   └── backend.rs        # PluginBackend impl
data/
├── io.github.up.Daemon.service    # systemd unit
├── io.github.up.Daemon.conf       # D-Bus bus policy
├── backends.d/
│   ├── apk.yaml          # Alpine Linux (example)
│   └── xbps.yaml         # Void Linux (example)
examples/
└── plugins/
    ├── eopkg.yaml        # Solus (example)
    └── swupd.yaml        # Clear Linux (example)
```

### Existing Files to Modify

| File | Change |
|------|--------|
| `Cargo.toml` | Add workspace members, add `zbus`, `serde_yml`, `regex` deps |
| `src/backends/mod.rs` | Extend `BackendKind` with `Plugin(String)` variant; integrate plugin discovery |
| `src/orchestrator.rs` | Add D-Bus path; use `DaemonExecutor` when available |
| `src/runner.rs` | Mark `PrivilegedShell` as deprecated; keep for fallback |
| `data/io.github.up.policy` | Replace with scoped actions |
| `meson.build` | Install daemon binary, service files, bus config, plugin descriptors |
| `flake.nix` | Build daemon; install systemd/dbus files |
| `scripts/preflight.sh` | Add daemon build step |

---

## 8. Dependencies

### 8.1 New Dependencies (Context7-verified)

| Crate | Version | Purpose | Verified |
|-------|---------|---------|----------|
| `zbus` | `5` (latest: 5.15.0) | D-Bus interface (daemon + client) | ✅ Context7 `/z-galaxy/zbus` — 241 snippets, High reputation, score 92.35 |
| `serde_yml` | `0.0.12` | YAML plugin descriptor parsing | ✅ Context7 `/websites/rs_crate_serde_yml` — 100 snippets, High reputation, score 62.2 |
| `regex` | `1` | Plugin output parsers | Standard, well-known |
| `tokio-util` | `0.7` | CancellationToken for operation cancellation | Part of tokio ecosystem |
| `event-listener` | `5` | Required by zbus for blocking→async bridging | zbus dependency |
| `libc` | `0.2` | Process group signaling (SIGTERM/SIGKILL) | Standard FFI |

### 8.2 Cargo.toml Changes (Frontend)

```toml
[dependencies]
# ... existing deps ...
zbus = { version = "5", default-features = false, features = ["tokio"] }
serde_yml = "0.0.12"
regex = "1"
tokio-util = { version = "0.7", features = ["rt"] }
```

### 8.3 Daemon Cargo.toml

```toml
[package]
name = "up-daemon"
version = "2.0.0"
edition = "2021"
description = "Privileged backend service for Up system updater"
license = "GPL-3.0-or-later"

[dependencies]
zbus = { version = "5", default-features = false, features = ["tokio"] }
tokio = { version = "1", features = ["rt-multi-thread", "process", "signal", "time", "sync", "io-util", "macros"] }
tokio-util = { version = "0.7", features = ["rt"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
log = "0.4"
env_logger = "0.11"
libc = "0.2"
thiserror = "2"
```

### 8.4 Workspace Configuration

Convert to a Cargo workspace:

```toml
# Root Cargo.toml
[workspace]
members = [".", "daemon"]

[package]
name = "up"
# ... rest of existing config
```

---

## 9. Migration Strategy

### 9.1 Phased Rollout

| Phase | Version | D-Bus Daemon | pkexec Fallback | Plugin System |
|-------|---------|--------------|-----------------|---------------|
| Phase A | v2.0-alpha | Optional (if installed) | Primary path | Loaded but not privileged |
| Phase B | v2.0-beta | Primary path | Fallback | Full integration |
| Phase C | v2.0 stable | Required | Removed | Stable API |

### 9.2 Fallback Detection (Phase A/B)

```rust
/// Determine the execution strategy for privileged operations.
pub async fn detect_execution_mode() -> ExecutionMode {
    // Try to connect to the daemon
    match Connection::system().await {
        Ok(conn) => {
            let proxy = UpDaemonProxy::new(&conn).await;
            match proxy {
                Ok(p) => {
                    // Verify daemon is responsive
                    if p.version().await.is_ok() {
                        return ExecutionMode::Daemon(conn);
                    }
                }
                Err(_) => {}
            }
        }
        Err(_) => {}
    }

    // Fallback to legacy pkexec path
    ExecutionMode::LegacyPkexec
}

pub enum ExecutionMode {
    Daemon(Connection),
    LegacyPkexec,
}
```

### 9.3 BackendKind Extension

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendKind {
    // Built-in (existing)
    Apt,
    Dnf,
    Pacman,
    Zypper,
    Flatpak,
    Homebrew,
    Nix,
    Fwupd,
    // Dynamic (plugins)
    Plugin(String),
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // ... existing variants ...
            Self::Plugin(id) => write!(f, "{}", id),
        }
    }
}
```

### 9.4 Backward Compatibility

- Built-in backends continue to work unchanged via the `CommandRunner` + `PrivilegedShell` path
- The D-Bus path is an additional execution mode, not a replacement during transition
- Plugin backends that don't require root can operate without the daemon
- The daemon ships as a separate binary; distributions can package it independently

---

## 10. Security Analysis

### 10.1 Threat Model

| Threat | Mitigation |
|--------|-----------|
| Malicious plugin descriptor injecting arbitrary commands | Command allowlist validation; no shell metacharacters in args; program resolved via PATH only |
| TOCTOU between polkit check and command execution | Atomic check-then-execute within single async task; no yielding between auth and spawn |
| Spoofed D-Bus caller | System bus policy restricts callers; polkit verifies caller PID/UID at auth time |
| Plugin descriptor path traversal | Programs must not contain `..` or start with `/`; validated before execution |
| Denial of service via flooding daemon | Rate limiting on D-Bus methods; max concurrent operations (default: 4) |
| Privilege escalation via environment injection | Environment variables in plugins are a strict allowlist (only LANG, LC_ALL, DEBIAN_FRONTEND, etc.) |
| Plugin descriptor tampering | System plugin paths (`/usr/share/`) require root to modify; user plugins cannot request root privilege |

### 10.2 Security Boundaries

```
┌─────────────────────────────────────────┐
│ Unprivileged (user session)             │
│                                         │
│  ┌──────────────────┐                   │
│  │ Up GTK Frontend   │                   │
│  │ (uid=1000)       │                   │
│  └────────┬─────────┘                   │
│           │ D-Bus system bus             │
└───────────┼─────────────────────────────┘
            │ polkit authorization check
┌───────────┼─────────────────────────────┐
│ Privileged (root)                       │
│           ▼                             │
│  ┌──────────────────┐                   │
│  │ up-daemon        │                   │
│  │ (uid=0)          │                   │
│  │                  │                   │
│  │ Allowlist check  │                   │
│  │ Audit log        │                   │
│  │ Execute command  │                   │
│  └──────────────────┘                   │
└─────────────────────────────────────────┘
```

### 10.3 Plugin Security Constraints

1. **User-installed plugins** (in `$XDG_DATA_HOME`) are **never granted root privilege** — `needs_root: true` is silently downgraded to `false` for user-path plugins
2. **System plugins** (in `/usr/share/` or `/usr/local/share/`) may request root
3. Plugin programs are resolved via `PATH` — no absolute paths allowed (prevents pointing at attacker-controlled binaries)
4. Plugin `args` are passed as discrete argv entries — never concatenated into a shell command
5. Environment variables are restricted to a safe allowlist:
   - `LANG`, `LC_ALL`, `LC_MESSAGES` — locale control
   - `DEBIAN_FRONTEND` — APT non-interactive mode
   - `HOME`, `PATH` — inherited from daemon
   - All others rejected at validation time

---

## 11. Risks and Mitigations

| Risk | Impact | Probability | Mitigation |
|------|--------|-------------|-----------|
| zbus API instability between major versions | Build breakage | Low (zbus 5.x is stable) | Pin to `5.x`; use `default-features = false` to minimize surface |
| Daemon not installed on target system | Feature unavailable | Medium (new dependency) | Graceful fallback to pkexec; document installation requirements |
| polkit agent not available (headless server) | Auth fails | Low | Fallback to `pkttyagent`; document requirement |
| Plugin YAML parsing vulnerabilities (billion laughs, etc.) | DoS | Low | `serde_yml` limits depth; cap file size to 64KB; validate before deserializing |
| Incorrect cancellation leaves system in inconsistent state | Data corruption | Medium | Always complete atomic operations (e.g., dpkg configure) before killing; cancel between packages, not during |
| Daemon crash during operation | Orphan root processes | Low | Process group management; systemd `KillMode=control-group` cleans children |
| Plugin regex ReDoS | CPU exhaustion | Low | Use `regex` crate (guarantees linear time); add 1s timeout per parse |
| Breaking change to BackendKind serialization (history.jsonl) | History display broken | Medium | Use `#[serde(untagged)]` or string fallback for unknown variants |

---

## 12. Testing Strategy

### 12.1 Unit Tests

| Component | Test Approach |
|-----------|---------------|
| Plugin descriptor parsing | Load sample YAML files; verify all fields parsed correctly |
| Plugin validation | Test rejection of invalid descriptors (path traversal, metacharacters, missing fields) |
| Parser engine | Test each parser type against captured command output fixtures |
| Command allowlist | Verify allowed/rejected command patterns |
| Polkit auth module | Mock polkit responses; verify behavior on allow/deny/cancel |

### 12.2 Integration Tests

| Test | Approach |
|------|----------|
| D-Bus interface | Start daemon on session bus (for testing); call methods via proxy; verify signals |
| Cancellation | Start a long-running command; call Cancel; verify SIGTERM sent and operation reports Cancelled |
| Plugin discovery | Create temp directories with YAML files; verify detection and loading |
| End-to-end update | Mock package manager commands; verify full flow from D-Bus call to completion signal |

### 12.3 Test Infrastructure

```rust
#[cfg(test)]
mod tests {
    // Use session bus for testing (no root required)
    async fn test_connection() -> Connection {
        connection::Builder::session()
            .unwrap()
            .name("io.github.up.Daemon.Test")
            .unwrap()
            .serve_at("/io/github/up/Daemon", TestDaemon::new())
            .unwrap()
            .build()
            .await
            .unwrap()
    }
}
```

### 12.4 CI Considerations

- Daemon tests require `dbus-daemon` running (available in most CI images)
- Use `dbus-run-session` wrapper for isolated testing
- Plugin tests are pure filesystem operations — no special requirements
- Polkit tests mock the authority interface — no real polkit agent needed

---

## Research Sources

1. **zbus documentation** — https://z-galaxy.github.io/zbus/ (D-Bus service/client patterns in Rust)
2. **Polkit architecture** — https://wiki.archlinux.org/title/Polkit (action definitions, authorization rules)
3. **XDG Base Directory Specification** — https://specifications.freedesktop.org/basedir-spec/latest/ (plugin path conventions)
4. **PackageKit architecture** — https://www.freedesktop.org/software/PackageKit/ (privileged D-Bus daemon pattern for package management)
5. **systemd D-Bus activation** — https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html (BusName= socket activation)
6. **fwupd D-Bus service** — https://github.com/fwupd/fwupd (real-world privileged D-Bus daemon with polkit; same pattern)
7. **GNOME Software / KDE Discover** — PackageKit clients using D-Bus for privileged operations
8. **serde_yml crate** — https://crates.io/crates/serde_yml (YAML parsing in Rust, fork of serde-yaml)
9. **Flatpak portal pattern** — D-Bus interfaces for sandboxed app communication with host

---

## Summary of Design Decisions

1. **System bus D-Bus service** (not session bus) — because the daemon runs as root; matches PackageKit/fwupd pattern
2. **systemd D-Bus activation** (not always-running) — daemon starts on demand, exits after 60s idle; no resource waste
3. **zbus v5 with tokio feature** — eliminates background threads; integrates cleanly with existing tokio runtime
4. **YAML for plugin descriptors** (not TOML, not JSON) — most readable for the community; `serde_yml` is mature
5. **Command allowlist** (not arbitrary execution) — daemon only runs pre-validated command patterns; plugins cannot inject arbitrary commands
6. **Graceful fallback** — existing pkexec path preserved during transition; Up remains functional without daemon installed
7. **Scoped polkit actions** — fine-grained authorization; sysadmins can allow updates but deny upgrades via polkit rules
8. **Process group cancellation** — SIGTERM to process group ensures child processes (dpkg, rpm) also receive the signal
9. **Plugin paths follow XDG** — standard, predictable, overridable by sysadmins
10. **User plugins cannot escalate** — only system-installed plugins may request root; prevents privilege escalation via user-writable directories
