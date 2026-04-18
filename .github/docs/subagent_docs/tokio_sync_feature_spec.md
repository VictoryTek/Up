# Tokio Sync Feature — Specification

## Current State

The tokio dependency in `Cargo.toml` (line 15) is:

```toml
tokio = { version = "1", features = ["rt", "macros", "io-util", "process", "fs", "sync"] }
```

The `"sync"` feature was added in commit `e06b1c7` ("Update Cargo.toml", 2026-04-17)
to fix the build error described below. Prior to that commit the features were:

```toml
tokio = { version = "1", features = ["rt", "macros", "io-util", "process", "fs"] }
```

Resolved tokio version in `Cargo.lock`: **1.50.0**.

## Problem

The `tokio::sync` module (specifically `tokio::sync::Mutex`) is used in two source
files. Without the `sync` feature enabled, the module is private and compilation fails:

```
error[E0603]: module `sync` is private
  --> src/ui/window.rs:210:50
    |
210 | ...  Some(Arc::new(tokio::sync::Mutex::new(s)))
    |                           ^^^^  ----- struct `Mutex` is not publicly re-exported
    |                           |
    |                           private module
```

The `tokio::sync` usage was introduced in commits around the "front-load sudo
authentication" feature (`fe43032`, `a4b7d40`, `4c0213d`) but the corresponding
`"sync"` feature flag was not added to `Cargo.toml` at that time.

## Affected Files

| File | Usage |
|------|-------|
| `src/runner.rs` (line 7) | `use tokio::sync::Mutex;` — imported at module level |
| `src/ui/window.rs` (line 196) | `let shell: Option<Arc<tokio::sync::Mutex<PrivilegedShell>>>` — type annotation |
| `src/ui/window.rs` (line 210) | `Some(Arc::new(tokio::sync::Mutex::new(s)))` — construction |

## Solution

Add `"sync"` to the tokio features list in `Cargo.toml`:

```diff
-tokio = { version = "1", features = ["rt", "macros", "io-util", "process", "fs"] }
+tokio = { version = "1", features = ["rt", "macros", "io-util", "process", "fs", "sync"] }
```

**Status: Already applied** in commit `e06b1c7`.

No `Cargo.lock` update is required — Cargo feature flags do not change the lock
file when the crate version remains the same. The Nix build (`flake.nix`) uses
`cargoLock = { lockFile = ./Cargo.lock; }` which vendors the tokio 1.50.0 crate;
the sync module is present in the vendored source and is gated only by the feature
flag in `Cargo.toml`.

## Risk Assessment

**Risk: Minimal**

- This is a feature-flag-only change; no new crate is added to the dependency tree.
- `tokio::sync` is a well-established, stable module (Mutex, RwLock, channels, etc.).
- The tokio crate is already pulled at version 1.50.0; enabling `sync` only
  compiles the additional module code that is already present in the vendored source.
- No API surface change, no new transitive dependencies.
- Build size impact: negligible.

## Notes

- The `Cargo.lock` was **not** updated alongside commit `e06b1c7`. This is correct
  behavior — feature changes don't alter the lock file.
- The stale subagent docs (`security_low_batch1_spec.md` line 41,
  `security_low_batch1_review.md` line 35) incorrectly state that `tokio::sync` is
  "not used" — those docs predate the privileged shell feature and are now outdated.
