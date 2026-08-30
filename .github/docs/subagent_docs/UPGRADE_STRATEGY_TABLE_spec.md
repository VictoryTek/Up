# UPGRADE_STRATEGY_TABLE — Specification

MASTER_PLAN item 17 — `upgrade_supported` and `execute_upgrade` disagree about
supported distros. Source: ARCH M8.

## Problem

Three different lists of "supported" distros:

1. `detect.rs::detect_distro` — `upgrade_supported` true for ubuntu, linuxmint,
   pop, elementary, zorin, fedora, opensuse-leap, debian, nixos, rhel, centos,
   plus any `ID_LIKE` containing ubuntu or debian.
2. `execute.rs::execute_upgrade` — only ubuntu, fedora, opensuse-leap, nixos
   have an implemented path; everything else returns "not yet supported".
3. `version.rs::check_upgrade_available` — same four as (2), different message.

A Mint / Debian / CentOS user passes the `upgrade_supported` gate, runs the
prerequisite checks, presses "Start Upgrade", and only then learns it was
never implemented.

## Design

One source of truth in `src/upgrade/detect.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeStrategy { Ubuntu, Fedora, OpenSuseLeap, NixOs }

impl UpgradeStrategy {
    pub fn for_distro(id: &str) -> Option<Self> {
        match id {
            "ubuntu" => Some(Self::Ubuntu),
            "fedora" => Some(Self::Fedora),
            "opensuse-leap" => Some(Self::OpenSuseLeap),
            "nixos" => Some(Self::NixOs),
            _ => None,
        }
    }
}
```

- `detect_distro`: `upgrade_supported = UpgradeStrategy::for_distro(&id).is_some()`
  — the whole ID/ID_LIKE match and the now-orphan `id_like` binding are removed.
- `execute_upgrade`: dispatch on `UpgradeStrategy::for_distro(&distro.id)`.
- `check_upgrade_available`: dispatch on the same.
- `upgrade_kind` (log-tag helper, added in item 12): also derived from it.

Net effect: the claimed-but-unimplemented distros (debian, linuxmint, pop,
elementary, zorin, rhel, centos, ID_LIKE matches) are no longer reported as
supported — the upgrade page will correctly say "not supported for this
distribution yet" instead of leading the user to a dead end.

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| Regression: an Ubuntu-derivative that *did* work loses support | `execute_upgrade` already matched `id == "ubuntu"` exactly, so those derivatives always hit the "not implemented" path — this only stops the UI from pretending otherwise. Re-adding them belongs with a real per-derivative implementation. |
| Missed call site | Grep confirms the three lists are the only dispatch points; `upgrade_kind` folded in too. |

## Success criteria

- One list; `detect.rs`, `execute.rs`, `version.rs` all derive from
  `UpgradeStrategy::for_distro`.
- New tests: strategy table maps the four implemented distros and rejects
  debian/linuxmint/centos/arch; `detect_distro().upgrade_supported` always
  equals `for_distro(id).is_some()`.
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo build`,
  `cargo test` clean; `scripts/preflight.sh` exits 0.
