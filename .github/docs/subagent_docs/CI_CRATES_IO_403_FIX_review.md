# CI crates.io 403 fix — Review

## Problem

CI (cold Nix build) failed fetching every crate:

```
trying https://crates.io/api/v1/crates/gtk4/0.9.7/download
curl: (22) The requested URL returned error: 403
error: cannot download crate-gtk4-0.9.7.tar.gz from any mirror
```

**Root cause:** crates.io now User-Agent-gates the legacy
`api/v1/crates/<name>/<version>/download` endpoint (returns 403 for
curl/blank UAs). The pinned nixpkgs (`nixos-25.05` @ `ac62194c`, frozen
2026-01-01) hard-codes that URL in
`pkgs/build-support/rust/import-cargo-lock.nix`. Bumping nixpkgs is the real
fix but is not possible in this environment (flake registry frozen at
2026-01-01). Verified with `curl`:

| URL | default UA | custom UA |
|---|---|---|
| `crates.io/api/v1/crates/serde/1.0.228/download` | **403** | 200 → `static.crates.io` |
| `static.crates.io/crates/serde/1.0.228/download` | **200** | 200 |

`static.crates.io` accepts the same path shape and does **not** UA-gate.

## Fix (`flake.nix`)

Build `cargoDeps` explicitly via `rustPlatform.importCargoLock` with
`extraRegistries` overriding the crates.io-index download URL to
`https://static.crates.io/crates` (the `//` merge in `import-cargo-lock.nix`
lets an extra registry key shadow the built-in). `importCargoLock` then also
emits a spurious `[source."https://github.com/rust-lang/crates.io-index"]`
block in `.cargo/config.toml` that cargo rejects ("source already defined by
`crates-io`"); an appended `sed` in an `overrideAttrs` `buildCommand` deletes
that block.

Fixed-output derivations are keyed on the sha256, not the URL, so every crate
store path is byte-identical — `--check` re-fetch of
`crate-gtk4-0.9.7.tar.gz` produced the same
`/nix/store/54zamwd…-crate-gtk4-0.9.7.tar.gz` the CI log referenced.

Marked with a comment to remove once nixpkgs is bumped past the
`static.crates.io` fetcher change.

## Validation

| Check | Result |
|---|---|
| `--check` re-fetch of gtk4 crate via new URL | PASS (474k, hash matched, same store path) |
| `cargoDeps` `.cargo/config.toml` | clean — no colliding `[source."…crates.io-index"]` block |
| `nix build .#packages.x86_64-linux.default` (cold `cargo-vendor-dir` + full compile) | **PASS** → `/nix/store/33v174mhg1f3c5fh5yxi6pih8smjwll1-up-2.3.2` |
| `nix flake check` | PASS |
| `cargo test` | 159 passed / 0 failed |
| `scripts/preflight.sh` | exit 0 |

## Result

PASS
