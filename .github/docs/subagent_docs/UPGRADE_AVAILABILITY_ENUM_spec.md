# UPGRADE_AVAILABILITY_ENUM — Specification

MASTER_PLAN item 13, part A only (user decision). Source: ARCH M4 (first bullet).
Parts B (typed `PrivilegedShell`/`BackendError`) and C (history result enum)
are explicitly out of scope for this pass.

## Problem

`upgrade::check_upgrade_available()` returns a human-readable `String`
("Yes — Fedora 42 is available", "No — …", "Could not check …", "Upgrades are
disabled …"). The UI decides whether to enable the "Start Upgrade" button with:

```rust
let is_available = result_msg.starts_with("Yes");
```

Any wording change — or running these strings through gettext, which other
code already does — silently flips the upgrade gate.

## Current state

- `check_upgrade_available(&DistroInfo) -> String`
  (`src/upgrade/version.rs:21`), dispatching to `check_ubuntu_upgrade` /
  `check_fedora_upgrade` / `check_opensuse_upgrade` / `check_nixos_upgrade`,
  each `-> String`.
- Single caller: `src/ui/upgrade_page.rs:471-487` — sends the string over an
  `async_channel::<String>`, parses `.starts_with("Yes")`, and sets the row
  subtitle to the raw string.
- Re-exported at `src/upgrade/mod.rs:12`.
- `UbuntuUpgradeInfo` (already an enum) stays as the internal Ubuntu
  parse-result type; only the public boundary type changes.

## Design

New enum in `src/upgrade/version.rs`:

```rust
/// Outcome of an upgrade-availability check for the running distro.
#[derive(Debug, Clone)]
pub enum UpgradeAvailability {
    /// A newer release is available and the upgrade path is open.
    Available(String),
    /// No upgrade is offered (none released, released-but-not-promoted, …).
    NotAvailable(String),
    /// Upgrades are administratively disabled (Ubuntu `Prompt=never`).
    Disabled(String),
    /// The check could not be completed (network/parse error, unsupported distro).
    Unknown(String),
}

impl UpgradeAvailability {
    pub fn is_available(&self) -> bool { matches!(self, Self::Available(_)) }
    pub fn message(&self) -> &str {
        match self {
            Self::Available(m) | Self::NotAvailable(m)
            | Self::Disabled(m) | Self::Unknown(m) => m,
        }
    }
}
```

`check_upgrade_available` and the four `check_*` helpers return
`UpgradeAvailability`. Message text is preserved verbatim (minus the leading
"Yes — " / "No — " markers, which become the variant); variant mapping:

| Current string | Variant |
|---|---|
| "Yes — …" (all distros, tool fallback) | `Available` |
| "No — … not yet released / not available / released-not-promoted" | `NotAvailable` |
| `UbuntuUpgradeInfo::NotAvailable` | `NotAvailable` |
| "Upgrades are disabled in /etc/update-manager/release-upgrades" | `Disabled` |
| "Could not check …", "Could not parse …", `CheckFailed` | `Unknown` |
| "Not supported for this distribution" | `Unknown` |

UI (`upgrade_page.rs`): channel becomes `async_channel::<UpgradeAvailability>`;

```rust
if let Ok(result) = rx.recv().await {
    *upgrade_available.borrow_mut() = result.is_available();
    upgrade_available_row.set_subtitle(result.message());
    (*recompute_state)();
}
```

`src/upgrade/mod.rs` re-exports `UpgradeAvailability` alongside
`check_upgrade_available`.

## Dependencies / Context7

None. Internal type change. Not required.

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| Message wording drift changes UI text | Strings copied verbatim; only the "Yes/No —" prefix is dropped (it was UI-facing noise). A follow-up may drop or reshape the prefix wording safely now that gating no longer depends on it. |
| A `check_*` branch mapped to the wrong variant | Table above enumerates every current return; unit tests assert `is_available()` for representative cases of each helper via the pure paths that don't need network (unsupported distro, unparseable version, `Prompt=never`). |
| Other callers | Grep confirms exactly one caller. |

## Success criteria

- No `.starts_with("Yes")` (or `"No"`) anywhere; the gate is
  `UpgradeAvailability::is_available()`.
- New unit tests: unsupported distro → `Unknown`; `next_*` parse failure paths
  → `Unknown`; (Ubuntu `Prompt=never` covered if reachable without I/O).
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo build`,
  `cargo test` clean; `scripts/preflight.sh` exits 0.
