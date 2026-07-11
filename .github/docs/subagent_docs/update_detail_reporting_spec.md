# Spec: Accurate per-item detail in the section dropdown

## Current state analysis

The per-backend `adw::ExpanderRow` (`src/ui/update_row.rs`) shows two independent
pieces of information after an update run:

- **Status text** (`set_status_success(count)`, `update_row.rs:200-212`) — e.g. "3 updated".
  `count` is `UpdateResult::Success { updated_count }` / `SuccessWithSelfUpdate { updated_count }`
  (`src/backends/mod.rs:102-121`), a `usize` computed by each backend's `run_update()` by parsing
  its own command output (e.g. `count_nix_store_operations`, digit-line counting in
  `flatpak.rs:78-84`).
- **Dropdown list** (`set_packages(&[String])`, `update_row.rs:134-164`) — populated **only**
  from the pre-run "Check for updates" pass (`window.rs:747-783`, calling `list_available()`),
  never refreshed after the update actually runs (confirmed: no `set_packages` call exists near
  any `BackendFinished` handler in `window.rs`, lines ~544-577, ~939-974, ~1109-1130).

These two values come from unrelated code paths and are never reconciled:

- **NixOS**: `list_available()` (`nix.rs:614-679`) returns real flake input names for standard
  NixOS, but for VexOS (`nix.rs:618-628`) it discards the real `inputs` list and hardcodes
  `vec!["NixOS system"]` (`nix.rs:626`) when anything changed — this is the literal source of the
  vague "nixos system" entry. Separately, `updated_count` (`nix.rs:485-605`) comes from
  `count_nix_store_operations` (`nix.rs:126-141`), which sums Nix's own
  `"these N derivations will be built"` / `"these N paths will be fetched"` lines — a completely
  different quantity (store paths/derivations rebuilt) than "number of flake inputs changed".
  These two numbers have no reason to match even after fixing the placeholder.
- **Flatpak**: the dropdown list comes from `flatpak remote-ls --updates` executed during the
  earlier check pass (`flatpak.rs:117-159`). The count comes from parsing digit-prefixed lines in
  `flatpak update -y`'s own output, run later (`flatpak.rs:66-115`, count at 78-84). Because these
  are two separate `flatpak` invocations run at different times, the metadata can drift between
  them — explaining both the vague/mismatched list and the "said up to date but updated things" /
  "said updates available but updated nothing" reports.

`UpdateResult` (`src/backends/mod.rs:102-121`) currently carries only a `usize` count on its
`Success` / `SuccessWithSelfUpdate` variants. There is no channel for a backend to report *which*
items it actually acted on.

## Problem definition

1. The dropdown must show the actual items affected by the update that just ran, not a stale
   pre-check snapshot or a vague placeholder.
2. The displayed count and the number of dropdown entries must always agree, because both must
   originate from the same data.
3. Flatpak's apparent "lied about availability" behavior is a symptom of two independent flatpak
   invocations disagreeing — fixed by driving both the count and the list from the single
   `flatpak update -y` invocation that actually performs the change.

## Proposed solution architecture

Extend `UpdateResult::Success` and `UpdateResult::SuccessWithSelfUpdate` with a new field,
`updated_items: Vec<String>`, populated by each backend from the *same* output it already parses
for its count. The UI stops relying on the pre-check list once a run finishes and instead calls
`row.set_packages(&updated_items)` with this fresh, authoritative data.

Backends that cannot cheaply extract names keep returning `Vec::new()` — `set_packages` already
treats an empty slice as "hide the expand arrow" (`update_row.rs:142-147`), which is a strict
improvement over today's stale/vague entries (no information shown vs. wrong information shown).

### Per-backend changes

**NixOS (`src/backends/nix.rs`)**

- Add `parse_nix_build_items(output: &str) -> Vec<String>`: scans for the same
  `"these N derivations will be built:"` / `"these N paths will be fetched...:"` header lines
  already matched by `count_nix_store_operations`, then collects the indented `/nix/store/...`
  path lines that follow each header, stripping the `/nix/store/<hash>-` prefix and `.drv` suffix
  to produce a readable name (e.g. `firefox-128.0`).
- Redefine `count_nix_store_operations` in terms of this new function
  (`count_nix_store_operations(output) == parse_nix_build_items(output).len()`) so the count and
  list can never disagree for any Nix-driven rebuild path (VexOS, standard flake, legacy channel,
  non-NixOS profile upgrade).
- Update every `run_update()` branch that currently does
  `UpdateResult::Success { updated_count: count_nix_store_operations(&output) }` to also compute
  `let items = parse_nix_build_items(&output);` and set `updated_count: items.len(), updated_items: items`.
- `determinate-nixd` and legacy `nix-env -u` branches: keep their existing count logic, but where
  cheap (`nix-env -u` already parses `"upgrading '...'"` lines at `list_available` time — reuse the
  same parsing pattern against the real `run_update` output instead of dry-run output) populate
  `updated_items`; otherwise leave `updated_items: Vec::new()`.
- Fix the VexOS placeholder at `nix.rs:626` to return the real `inputs` list from
  `nixos_flake_changed_inputs()` instead of `vec!["NixOS system".to_string()]` — this is used by
  `list_available()` for the *pre-run* "N available" display, independent of the `run_update` fix
  above, and removes the vague placeholder from that code path too.

**Flatpak (`src/backends/flatpak.rs`)**

- In `run_update()` (lines 66-115), alongside the existing digit-line count (78-84), parse the
  application/runtime ID out of each matched digit-prefixed line (the token following the `N.`
  index and status glyph) to build `updated_items: Vec<String>`. Use `updated_items.len()` as
  `updated_count` instead of the separately-computed `count`, so the two values are always
  identical by construction.
- Apply the same change to `run_selected_update()` (lines 225-265), which duplicates the
  digit-line counting logic.
- Leave `list_available()` untouched — it remains correct for the pre-run "N available" display;
  the fix is scoped to what's shown *after* the run completes.

**Other backends (apt/dnf/pacman/zypper in `os_package_manager.rs`, `homebrew.rs`, `fwupd.rs`,
`src/plugins/backend.rs`, and the `run_cleanup` default in `mod.rs:190`)**

- No behavioral change. Add `updated_items: Vec::new()` to every existing
  `UpdateResult::Success { updated_count: ... }` / `SuccessWithSelfUpdate { updated_count: ... }`
  construction site (mechanical, ~20 call sites) so the crate compiles against the new field.
  These backends' pre-check lists already come from accurate dry-run enumeration, so the dropdown
  behavior for them is unchanged (it will simply stay empty post-run unless a future spec adds
  real parsing).

### UI changes (`src/ui/window.rs`, `src/ui/update_row.rs`)

- `update_row.rs`: no change needed — `set_packages` already exists with correct semantics.
- `window.rs` — in each of the three `OrchestratorEvent::BackendFinished` match sites
  (~line 549 primary "Update All" flow, ~line 944 Retry flow, ~line 1113 cache-bypass flow), when
  matching `UpdateResult::Success { updated_count, updated_items }` or
  `UpdateResult::SuccessWithSelfUpdate { updated_count, updated_items }`, call
  `row.set_packages(updated_items);` immediately before/after `row.set_status_success(*updated_count)`.
  This requires no new captures or backend lookups — the data now travels with the event itself,
  which also makes it work unmodified for Site C (`spawn_cache_bypass`), which has no live
  `Arc<dyn Backend>` in scope to query.

## Implementation steps

1. `src/backends/mod.rs`: add `updated_items: Vec<String>` to `UpdateResult::Success` and
   `UpdateResult::SuccessWithSelfUpdate`; update the default `run_cleanup` impl (line 190).
2. `src/backends/nix.rs`: implement `parse_nix_build_items`, rewire `count_nix_store_operations`,
   update all `run_update()` branches, fix the VexOS `list_available()` placeholder (line 626).
3. `src/backends/flatpak.rs`: parse item IDs in `run_update()` and `run_selected_update()`,
   derive `updated_count` from the parsed list length.
4. `src/backends/os_package_manager.rs`, `homebrew.rs`, `fwupd.rs`, `src/plugins/backend.rs`: add
   `updated_items: Vec::new()` to existing construction sites (no behavior change).
5. `src/ui/window.rs`: add `row.set_packages(updated_items)` calls at the three
   `BackendFinished` match sites.
6. Update existing unit tests in `nix.rs`, `flatpak.rs`, `os_package_manager.rs`, `homebrew.rs`,
   `fwupd.rs` that pattern-match `UpdateResult::Success { updated_count: N }` to either destructure
   the new field (`{ updated_count: N, .. }`) or assert on it where the test's backend now
   produces real items (Nix, Flatpak).
7. Add new unit tests: `parse_nix_build_items` against sample `nix`/`nixos-rebuild` output
   (multi-derivation build, single fetch, empty/no-op); Flatpak item-ID parsing against a sample
   numbered update table.

## Dependencies

None — internal refactor only, no new crates. Context7 not required per policy (internal code
change, no new external dependency).

## Configuration changes

None.

## Risks and mitigations

- **Risk:** Nix store-path names parsed from build output may be internal/uninteresting
  derivations (e.g. `.drv` intermediate build artifacts) rather than user-facing packages,
  producing a long or noisy list for a large rebuild.
  **Mitigation:** cap display at the existing 50-item limit in `set_packages` (already handles
  this via the "… and N more" summary row, `update_row.rs:156-163`); no further change needed.
- **Risk:** Flatpak's numbered-table format could vary across Flatpak versions, breaking ID
  extraction.
  **Mitigation:** parsing reuses the same line-matching predicate already relied on for the
  existing (working) count, so failure modes are no worse than today's count logic; if ID
  extraction fails for a line, fall back to omitting that entry from `updated_items` rather than
  erroring the whole update.
- **Risk:** Touching `UpdateResult` construction in ~8 files is mechanical but broad; a missed
  call site fails to compile (Rust enforces all struct-variant fields), so there is no risk of a
  silent runtime gap — `cargo build` will catch any omission immediately.
