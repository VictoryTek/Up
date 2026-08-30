# FIX_PACKAGE_COUNTING — Specification

MASTER_PLAN item 22 — package-count miscounting for APT selective updates and
DNF/generic prerequisite checks. Source: BUGS M5 & M6 (also ARCH L7).

## Bugs

### M6 — `upgrade/check.rs::check_packages_up_to_date`

Counts every non-empty stdout line that doesn't start with `"Listing"`. For
`dnf check-update` the output has a `"Last metadata expiration check: …"`
header and blank-separated section labels (`Obsoleting Packages`, …), all
counted as pending packages → an up-to-date Fedora system reports
"N packages need updating first" and the distro-upgrade button is blocked.

### M5 — `backends/os_package_manager.rs::count_apt_upgraded`

Scans for a line containing `"upgraded"` and parses the leading integer. Fails
two ways: (a) a localised / reworded summary line silently yields 0; (b) after
`apt-get install --only-upgrade` apt can print `"0 upgraded, …"` while dpkg
still configured newer versions → the per-row "N updated" figure shows 0 /
"Up to date" after a successful partial upgrade.

## Fixes

### M6

`check_packages_up_to_date` now selects a parser alongside the command and
counts with it:

| distro | command | counter |
|---|---|---|
| ubuntu | `apt list --upgradable` | `parse_apt_list_upgradable(s).len()` |
| fedora | `dnf check-update` | `parse_dnf_list_upgrades(s).len()` |
| opensuse-leap | `zypper list-updates` | `parse_zypper_list_updates(s).len()` |

These `pub(crate)` parsers in `src/backends/os_package_manager.rs` are already
unit-tested and correctly skip headers / section labels.

### M5

1. Force `LC_ALL=C` on the APT `run_update` and `run_selected_update` shell
   commands so the summary line stays English.
2. `count_apt_upgraded`:
   - Primary: match `<int> upgraded[,]` (the number must be immediately
     followed by the `upgraded` token — a bare leading integer on some other
     line no longer matches). Trust the summary only when it is `> 0`.
   - Fallback (no summary, or `0 upgraded`): count distinct packages in dpkg
     `"Setting up <name> …"` lines.

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| Fallback over-counts (dependencies configured) | Only used when the summary is absent or `0`; a slightly-high non-zero beats a wrong `0`. Primary path (summary `> 0`) is unchanged for the common case. |
| Behaviour change for a genuine no-op `apt upgrade` | No "Setting up" lines and `0 upgraded` → still returns `0`. Covered by the existing `test_count_apt_upgraded_zero`. |
| `LC_ALL=C` affecting apt behaviour | Only affects message language; already used elsewhere (`estimate_size`, `check.rs`). |

## Success criteria

- New tests: dnf metadata-only output → 0 pending; dnf real updates counted;
  `count_apt_upgraded` fallback on "Setting up" lines; bare-leading-integer
  line ignored; non-zero summary still wins.
- Existing `count_apt_upgraded` tests still pass.
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo build`,
  `cargo test` clean; `scripts/preflight.sh` exits 0.
