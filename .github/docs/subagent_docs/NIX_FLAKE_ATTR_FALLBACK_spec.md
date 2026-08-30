# NIX_FLAKE_ATTR_FALLBACK — Specification

MASTER_PLAN item 10 (partial — "Flake-attr fallback only" scope chosen by the user).

## Problem definition

`src/backends/nix.rs::resolve_nixos_flake_attr()` resolves the
`nixosConfigurations` attribute name **only** by reading
`/etc/nixos/vexos-variant`. On any plain flake-based NixOS system that file
does not exist, so the function returns:

> "Cannot read /etc/nixos/vexos-variant: ... If this is a VexOS system,
> ensure the variant file was created during system configuration."

This breaks `NixBackend::run_update()` (standard flake path),
`NixBackend::run_selected_update()`, and `upgrade/execute.rs` (Flake arm) for
every non-VexOS flake user — they can never rebuild.

`UpdateResult::CacheMiss` vendor coupling is explicitly **out of scope** for
this change (per user decision) and is left untouched.

## Current state

- `resolve_nixos_flake_attr()` — `src/backends/nix.rs:96-115`, sync, returns
  `Result<String, String>`. Single source of truth; called from
  `run_update` (`nix.rs:525`), `run_selected_update` (`nix.rs:770`), and
  `upgrade/execute.rs:303`.
- `validate_flake_attr()` — `src/backends/nix.rs:67-84`, ASCII
  alphanumeric / `-` / `_` / `.`, max 253 chars.
- `upgrade::detect_hostname()` — `src/upgrade/detect.rs:36-41`, reads
  `/proc/sys/kernel/hostname`, falls back to `"nixos"`.
- Flatpak host probing pattern: `flatpak-spawn --host <cmd>` used by the
  `is_*` helpers in `nix.rs`.

## Proposed solution

Extend `resolve_nixos_flake_attr()` with a fallback chain. No new
dependencies. No API changes — same signature, same call sites.

Resolution order:

1. **`/etc/nixos/vexos-variant`** (unchanged) — if the file exists and is
   non-empty, validate and return its contents. This keeps VexOS behaviour
   byte-for-byte identical and lets any user pin an explicit attribute.
2. **Auto-detect from the flake** — run (read-only, unprivileged):
   ```
   nix --extra-experimental-features 'nix-command flakes' \
       eval /etc/nixos#nixosConfigurations --apply builtins.attrNames --json
   ```
   Parse the JSON string array:
   - exactly one config  → validate and return it.
   - multiple configs    → return the one whose name equals the system
     hostname (`detect_hostname()`); if none matches, return an `Err`
     listing the available names and telling the user to create
     `/etc/nixos/vexos-variant` with the desired name.
   - zero configs / eval failure → `Err` with the underlying message.
3. If `nix eval` cannot be spawned at all → `Err`.

When running inside the Flatpak sandbox the `nix eval` invocation is routed
through `flatpak-spawn --host`, matching the existing `is_*` helpers.

### New private helpers in `nix.rs`

- `fn nixos_configuration_names() -> Result<Vec<String>, String>` — runs the
  `nix eval` command (flatpak-aware), parses the JSON array, returns the
  names. Pure-ish; the JSON parsing part is unit-testable via a split-out
  `fn parse_configuration_names(json: &str) -> Result<Vec<String>, String>`.
- Reuse `crate::upgrade::detect_hostname()` for the hostname (already
  `pub` and re-exported from `crate::upgrade`).

### Behavioural matrix after change

| System                              | Before            | After                              |
|-------------------------------------|-------------------|------------------------------------|
| VexOS (variant file present)        | works             | works (unchanged path)             |
| Flake NixOS, 1 config               | error             | auto-detected                      |
| Flake NixOS, N configs, host match  | error             | host config used                   |
| Flake NixOS, N configs, no match    | error             | clearer error listing the configs  |
| Non-flake NixOS                     | n/a (not called)  | n/a                                |

## Implementation steps

1. `src/backends/nix.rs`
   - Add `parse_configuration_names(json: &str) -> Result<Vec<String>, String>`.
   - Add `nixos_configuration_names() -> Result<Vec<String>, String>`
     (spawns `nix eval`, flatpak-aware, calls the parser).
   - Rewrite `resolve_nixos_flake_attr()` per the resolution order above.
   - Update the doc-comment on `resolve_nixos_flake_attr()`.
   - Add unit tests for `parse_configuration_names` (valid array, empty
     array, malformed JSON, non-string elements).
2. No changes required at call sites — signature is unchanged.
3. No changes to `data/`, packaging, or D-Bus (daemon crate no longer exists;
   workspace `members = ["."]`).

## Dependencies

None added. Uses `serde_json` (already a dependency) for parsing.
Context7: `nix eval --apply builtins.attrNames --json` confirmed against
`/nixos/nix` docs (`src/nix/eval.md`).

## Configuration changes

None. `/etc/nixos/vexos-variant` remains an optional explicit override.

## Risks and mitigations

| Risk | Mitigation |
|------|------------|
| `nix eval` on a large flake is slow / evaluates too much | `--apply builtins.attrNames` forces only the attr set keys; `nixosConfigurations` keys are cheap (no system build). Acceptable — this already runs interactively before a full rebuild. |
| `nix eval` needs network for locked inputs | Flake is already locked in `/etc/nixos`; eval of attr names does not fetch. If it fails, we return a clear error (no worse than today). |
| Hostname mismatch picks wrong config silently | Only used as a disambiguator when multiple configs exist; exact string match only; otherwise an explicit error is returned, never a guess. |
| Shell-injection via config name | All returned names pass `validate_flake_attr()` before use, exactly as today. |
| Flatpak sandbox lacks host `nix` | Routed via `flatpak-spawn --host`, same as sibling helpers. |

## Success criteria

- `parse_configuration_names` unit tests pass (single, multi, empty, malformed).
- `cargo build`, `cargo fmt --check`, `cargo clippy -- -D warnings`,
  `cargo test` all clean.
- `resolve_nixos_flake_attr()` still returns the variant-file contents
  verbatim when that file is present (existing VexOS tests / behaviour).
- Preflight (`scripts/preflight.sh`) exits 0.
