# Specification: Add `--refresh` to `nixos-rebuild switch` in the NixOS Flake Update Path

**Feature:** `nixos_rebuild_refresh`  
**File:** `.github/docs/subagent_docs/nixos_rebuild_refresh_spec.md`  
**Date:** 2026-05-13  
**Status:** Ready for Implementation

---

## 1. Current State Analysis

### Affected file
`src/backends/nix.rs`

### Exact code — flake-based NixOS `run_update` branch

Lines ~455–480 (the `is_nixos() && is_nixos_flake()` branch):

```rust
// Single pkexec invocation so polkit only prompts once.
// pkexec resets PATH, so we restore the NixOS binary paths
// explicitly via `env PATH=...` before invoking sh.
// config_name is validated by validate_flake_attr (ASCII
// alphanumeric / hyphen / underscore / dot only), so it is
// safe to interpolate into the shell command string.
let cmd = format!(
    "stdbuf -oL -eL \
     nix --extra-experimental-features 'nix-command flakes' \
     flake update --flake /etc/nixos && \
     stdbuf -oL -eL \
     nixos-rebuild switch --flake /etc/nixos#{} --print-build-logs",
    config_name
);
```

### Existing tests covering this branch

No unit tests cover the flake NixOS branch in `run_update`. The comment in the test module explicitly documents this:

> "The NixOS flake, NixOS channel, and Determinate Nix run_update branches each begin with OS-detection (is_nixos, is_nixos_flake, is_determinate_nix) that reads /run/current-system, /etc/os-release, /nix/receipt.json etc., making them impossible to exercise in unit tests without a SystemProber abstraction."

### Other `nixos-rebuild` usages in `nix.rs`

The non-flake NixOS branch uses:
```rust
"stdbuf -oL -eL nix-channel --update && \
 stdbuf -oL -eL nixos-rebuild switch --print-build-logs",
```
This branch is **not affected** by this change (channels do not use the flake eval cache).

---

## 2. Problem Definition

### Root cause

Nix maintains two distinct caching layers:

1. **Download / tarball cache** (`~/.cache/nix/tarballs/`, controlled by `tarball-ttl`, default 3600 s): Caches archives fetched from URLs. When a flake input points to a URL (e.g. a rolling `nixos-unstable` tarball or a GitHub archive), Nix will serve the cached copy until the TTL expires, regardless of whether the upstream content has changed.

2. **Eval cache** (SQLite, `~/.cache/nix/eval-cache-v*/`): Caches evaluation results keyed by the flake's locked revision hash. A genuinely new `flake.lock` should miss the eval cache, but corrupted or edge-case entries can cause stale results.

### Trigger sequence

```
nix flake update --flake /etc/nixos   # updates flake.lock
                                       # writes new locked revisions to disk
nixos-rebuild switch --flake /etc/nixos#<config>  ← runs here
                                       # evaluates the flake
                                       # but Nix's download cache still serves
                                       # the old tarballs for recently-fetched inputs
```

### Known failure modes

| Scenario | Effect without `--refresh` |
|---|---|
| Force-pushed branch (`nixos-unstable` receives a hard reset) | `flake.lock` shows new `lastModified` but Nix re-uses cached tarball → build references the old closure |
| Rolling channel tarball mutated in place | Same-URL tarball cache hit → stale evaluation |
| `tarball-ttl` has not elapsed after `flake update` | Nix trusts the cached download; rebuild uses pre-update content |
| Corrupted narinfo/tarball cache | Build fails or produces wrong output |

Without `--refresh`, users must manually re-run:
```sh
sudo nixos-rebuild switch --flake /etc/nixos#<config> --refresh --print-build-logs
```

### Why `--refresh` on `nixos-rebuild switch`, not on `nix flake update`

`nix flake update` **writes** the lock file from the registry/network, it does not use a download cache for the final lock content. Adding `--refresh` there has no meaningful effect on the subsequent build step.

The download cache is consumed during **evaluation and building** — i.e. during `nixos-rebuild switch`. That is where `--refresh` must be placed.

---

## 3. Research Sources

1. **Nix 2.24 Reference Manual — `nix` global options**  
   `https://nix.dev/manual/nix/2.24/command-ref/new-cli/nix`  
   > `--refresh`: "Consider all previously downloaded files out-of-date."

2. **Nix 2.28 Reference Manual — `conf-eval-cache`**  
   `https://nix.dev/manual/nix/stable/command-ref/conf-file#conf-eval-cache`  
   > "Whether to use the flake evaluation cache. Certain commands won't have to evaluate when invoked for the second time with a particular version of a flake."  
   Confirms the eval cache is separate from the download cache, and is keyed per flake version.

3. **Nix 2.28 Reference Manual — `conf-tarball-ttl`**  
   Same source as above:  
   > "The number of seconds a downloaded tarball is considered fresh. … Setting the TTL to 0 forces Nix to always check if the tarball is up to date."  
   `--refresh` is the per-invocation equivalent of `tarball-ttl = 0`.

4. **Nix 2.18 Reference Manual — `nix flake update` options**  
   `https://nix.dev/manual/nix/2.18/command-ref/new-cli/nix3-flake-update`  
   Confirms `--refresh` is in the "Miscellaneous global options" section, available on all `nix` subcommands since the experimental new CLI (Nix ≥ 2.4 with `nix-command` feature).

5. **Nix 2.24 Reference Manual — `nix build` options**  
   `https://nix.dev/manual/nix/2.24/command-ref/new-cli/nix3-build`  
   `--refresh` listed under "Miscellaneous global options". `nix build` is what `nixos-rebuild switch` invokes internally for flake-based systems. Confirms the flag is forwarded.

6. **NixOS Manual (25.11) — Changing the Configuration**  
   `https://nixos.org/manual/nixos/stable/index.html#sec-nixos-rebuild`  
   Documents `nixos-rebuild switch` usage and confirms it accepts extra Nix global flags. The section on network problems documents `--option use-binary-caches false` as an example of passing Nix global options through `nixos-rebuild`, confirming the flag-forwarding behaviour.

---

## 4. Proposed Solution

### Exact one-line diff

```diff
-     nixos-rebuild switch --flake /etc/nixos#{} --print-build-logs",
+     nixos-rebuild switch --flake /etc/nixos#{} --refresh --print-build-logs",
```

### Full `format!` string before and after

**Before:**
```rust
let cmd = format!(
    "stdbuf -oL -eL \
     nix --extra-experimental-features 'nix-command flakes' \
     flake update --flake /etc/nixos && \
     stdbuf -oL -eL \
     nixos-rebuild switch --flake /etc/nixos#{} --print-build-logs",
    config_name
);
```

**After:**
```rust
let cmd = format!(
    "stdbuf -oL -eL \
     nix --extra-experimental-features 'nix-command flakes' \
     flake update --flake /etc/nixos && \
     stdbuf -oL -eL \
     nixos-rebuild switch --flake /etc/nixos#{} --refresh --print-build-logs",
    config_name
);
```

### Flag placement rationale

`--refresh` is placed immediately after the flake specifier and before `--print-build-logs`. Both are global Nix options; ordering has no semantic effect. This placement groups them naturally as "evaluation/build modifiers" before the output-format flag.

---

## 5. Nix Version Compatibility

| Nix version | `--refresh` supported? | Notes |
|---|---|---|
| Nix 2.0–2.3 (legacy CLI) | N/A | `nixos-rebuild` uses `nix-build`; `--refresh` not applicable via new CLI |
| Nix 2.4–2.17 (nix-command experimental) | Yes | `--refresh` available as global option on all new-CLI subcommands |
| Nix 2.18–2.28 (current stable) | Yes | Same; `--all` for `nix profile upgrade` added in 2.18; `--refresh` pre-existing |
| Determinate Nix (any bundled version) | Yes | Ships Nix 2.x internally; `--refresh` supported |

**The existing code already requires `nix-command flakes` experimental features, constraining the minimum to Nix 2.4.** `--refresh` has been present since that baseline. No version guard is needed.

---

## 6. Side Effects and Performance Implications

| Concern | Assessment |
|---|---|
| **Build time increase** | Minimal. `--refresh` causes Nix to send conditional HTTP requests (`ETag`/`If-Modified-Since`) to check tarballs. For most GitHub-sourced flake inputs, content-addressed paths in `/nix/store` are already present — no re-download occurs. |
| **Network requirement** | `--refresh` requires network access for tarball re-checks. Systems offline during rebuild may see failures. However: (a) the immediately-preceding `nix flake update` already requires network, so offline rebuilds are not a regression; (b) Nix's `--offline` mode can be used if needed. |
| **Eval cache bypass** | `--refresh` does **not** disable the eval cache. If stale eval cache results are also suspected, `--option eval-cache false` would additionally be required. That is out of scope for this change. |
| **Non-flake branch** | Not affected. The `nix-channel --update && nixos-rebuild switch` path does not use flake evaluation caching. No change is needed there. |
| **Idempotency** | Adding `--refresh` does not change the correctness contract. A fully up-to-date system will still produce no changes; `--refresh` only ensures Nix does not use stale inputs when changes exist. |

---

## 7. Risks and Mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| `--refresh` rejected by an ancient Nix version | Very Low | Minimum Nix version already implied by `nix-command flakes` requirement (Nix 2.4+). Flag present since 2.4. |
| Slightly longer rebuild on slow connections | Low | Effect is conditional HTTP requests, not full re-downloads; accepted trade-off for correctness. |
| Breaking existing tests | None | No tests cover this branch (documented in the test module). The string change has no effect on the MockExecutor-based test suite. |

---

## 8. Implementation Steps

1. **Open** `src/backends/nix.rs`.

2. **Locate** the `format!` call in the `run_update` method, inside the `is_nixos() && is_nixos_flake()` branch (around line 471):
   ```rust
   nixos-rebuild switch --flake /etc/nixos#{} --print-build-logs",
   ```

3. **Replace** that line with:
   ```rust
   nixos-rebuild switch --flake /etc/nixos#{} --refresh --print-build-logs",
   ```

4. **Verify** the full `format!` string reads:
   ```rust
   let cmd = format!(
       "stdbuf -oL -eL \
        nix --extra-experimental-features 'nix-command flakes' \
        flake update --flake /etc/nixos && \
        stdbuf -oL -eL \
        nixos-rebuild switch --flake /etc/nixos#{} --refresh --print-build-logs",
       config_name
   );
   ```

5. **Run** `cargo build` to confirm the change compiles without error.

6. **Run** `cargo clippy -- -D warnings` to confirm no new warnings.

7. **Run** `cargo fmt --check` to confirm no formatting changes required.

8. **Run** `cargo test` to confirm all existing tests pass (no regressions; this branch has no unit tests, so the test suite is unaffected).

---

## 9. Files Modified

| File | Change |
|---|---|
| `src/backends/nix.rs` | Add `--refresh` flag to `nixos-rebuild switch` in the flake-based NixOS `run_update` branch |

No other files require modification. No new dependencies. No configuration changes.
