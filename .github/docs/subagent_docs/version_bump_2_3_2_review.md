# Version Bump to 2.3.2 — Review

## Changed

- `Cargo.toml` — `version = "2.3.1"` → `"2.3.2"`.
- `Cargo.lock` — `up` package regenerated to `2.3.2` (via `cargo build`).
- `data/io.github.up.metainfo.xml` — new
  `<release version="2.3.2" date="2026-08-29">` entry prepended, matching the
  existing `<description translate="no"><p>…</p></description>` structure.
- `releases/2.3.2.md` — created, matching the section style of
  `releases/2.3.1.md` / `2.3.0.md`.

## Not changed (derive from Cargo.toml automatically)

- `flake.nix` — `version = (builtins.fromTOML (readFile ./Cargo.toml)).package.version`.
- `meson.build` — greps the version out of `Cargo.toml`.
- About dialog — `env!("CARGO_PKG_VERSION")`.

## Validation

| Check | Result |
|---|---|
| `cargo build` | PASS (`up v2.3.2`) |
| XML well-formed (`ElementTree`) | PASS |
| `scripts/preflight.sh` | exit 0 (`appstreamcli` / `desktop-file-validate` / `cargo-audit` not installed locally — skipped, CI covers them) |
| No stray `2.3.1` outside the metainfo release history | confirmed |

## Release-notes scope

Covers everything merged since 2.3.1 (MASTER_PLAN items 10–26): changelog
viewer, size estimate + low-disk banner, Preferences/plugin manager, error
details dialog, single-prompt distro upgrade, Fedora prereq / unsupported-distro
/ flake-NixOS / APT-count fixes, gettext wiring, `serde_yml` → `yaml_serde`,
and the internal executor / event-loop / dead-code cleanups.
