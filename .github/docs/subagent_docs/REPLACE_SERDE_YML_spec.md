# REPLACE_SERDE_YML — Specification

MASTER_PLAN item 21 — replace `serde_yml 0.0.12` with a maintained YAML parser.
Source: ARCH M13.

## Problem

`serde_yml 0.0.12` (an unmaintained fork of `serde_yaml` by a third party,
pulling `libyml` which has carried soundness concerns / RUSTSEC attention) is
used to parse semi-trusted plugin descriptor YAML in
`src/plugins/discovery.rs`.

## Options considered (Context7 + crates.io)

| Crate | Latest | Status |
|---|---|---|
| `serde_yaml_ng` | 0.10.0 (May 2024) | community fork, last release ~2 yrs old |
| `serde-saphyr` | active | newer API, not a `serde_yaml` drop-in |
| **`yaml_serde`** | **0.10.7 (Aug 2026)** | **"serde_yaml maintained by The YAML Organization"; 3 releases in the last month; `serde_yaml`-compatible `from_str` API** |

Chosen: **`yaml_serde 0.10`**. It is the official continuation under The YAML
Organization, actively released, and API-compatible (`yaml_serde::from_str`).
Its parser dependency `libyaml-rs` is a c2rust transpile of upstream libyaml
(same lineage as the original `serde_yaml`'s `unsafe-libyaml`, YAML-org
backed) — not the flagged `libyml` fork.

## Change

- `Cargo.toml`: `serde_yml = "0.0.12"` → `yaml_serde = "0.10"`.
- `src/plugins/discovery.rs`: `serde_yml::from_str` → `yaml_serde::from_str`
  (identical signature; `PluginDescriptor` derives already unchanged).
- `src/plugins/descriptor.rs`: doc comment updated.
- `Cargo.lock` regenerated — `serde_yml`, `libyml` drop out; `yaml_serde`,
  `libyaml-rs` come in.
- New test: `shipped_descriptors_parse` deserializes every shipped descriptor
  (`data/backends.d/*.yaml`, `examples/plugins/*.yaml`) with `yaml_serde`.

The Nix flake uses `cargoLock = { lockFile = ./Cargo.lock; }` (no vendor
hash), so the regenerated lock is picked up automatically.

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| Behavioural difference vs. `serde_yml` on the real descriptor format | New `shipped_descriptors_parse` test covers all four shipped/example descriptors; `nix flake check` builds clean. |
| `libyaml-rs` soundness | Same transpile approach and YAML-org stewardship as upstream `serde_yaml`; strictly better than the abandoned `serde_yml`/`libyml`. |
| Offline build | `nix flake check` fetched `yaml_serde 0.10.7` successfully in this environment. |

## Success criteria

- No `serde_yml` in `Cargo.toml` / `Cargo.lock` / `src/`.
- `shipped_descriptors_parse` passes.
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo build`,
  `cargo test` clean; `scripts/preflight.sh` exits 0 (incl. `nix flake check`).
