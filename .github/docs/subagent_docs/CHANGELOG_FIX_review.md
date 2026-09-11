# CHANGELOG_FIX — Review & Quality Assurance

Reviews the implementation against `CHANGELOG_FIX_spec.md`.

**Result: PASS**

---

## 1. Build & Test Validation

### Execution environment

The primary dev host is Windows, where the project cannot compile (GTK4/libadwaita
are Linux-only — a documented Resource Constraint). Validation was therefore run in
a throwaway Docker container built to match `.gitlab-ci.yml` exactly
(`ubuntu:24.04` + `libgtk-4-dev`, `libadwaita-1-dev`, `libglib2.0-dev`,
`libcairo2-dev`, `libpango1.0-dev`, `pkg-config`, `cmake`, `ninja-build`,
`desktop-file-utils`, `appstream`, plus rustup stable with clippy and rustfmt).
Nothing was installed on the user's machine.

One wrinkle worth recording: the Windows working copy has CRLF line endings
(`core.autocrlf=true`), so mounting it directly into Linux made `preflight.sh`
fail at `set -euo pipefail` and `desktop-file-validate` reject the desktop entry.
`git ls-files --eol` confirms the **index is LF for every tracked file**, so this
is purely a Windows-checkout artifact — the repository is correct and CI is
unaffected. Validation was re-run against an LF-normalized copy of the tree,
which is what a native Linux checkout produces.

### Command output

| Step | Command | Result |
|---|---|---|
| 1 | `cargo fmt --check` | **pass** (no output) |
| 2 | `cargo clippy -- -D warnings` | **pass**, exit 0 |
| 3 | `cargo build` | **pass**, `Finished dev profile in 3m 14s` |
| 4 | `cargo test` | **pass** — `167 passed; 0 failed; 0 ignored` |
| 5 | `desktop-file-validate data/io.github.up.desktop` | **pass** |
| 6 | `appstreamcli validate --no-net data/io.github.up.metainfo.xml` | **pass** — `Validation was successful` |
| 7 | `cargo audit` | skipped — `cargo-audit` not installed in the container |
| 8 | `nix flake check` | skipped — Nix not available in the container |

`scripts/preflight.sh` end-to-end: **`All preflight checks passed.` — exit 0.**

Steps 7 and 8 were skipped by the script's own tooling-detection branches, not by
failures. Both are unverified here and will run on the user's NixOS machine.

### New tests (11, all passing)

```
changelog::tests::parse_ref_origin_skips_header_and_keeps_apps_and_runtimes ... ok
changelog::tests::parse_ref_origin_handles_multiple_remotes ................. ok
changelog::tests::parse_ref_origin_empty_and_header_only .................... ok
changelog::tests::short_rev_abbreviates_and_tolerates_short_input ........... ok
changelog::tests::age_suffix_renders_relative_days .......................... ok
backends::nix::tests::diff_lock_nodes_captures_revs_dates_and_github_slug ... ok
backends::nix::tests::diff_lock_nodes_added_input_has_no_old_rev ............ ok
backends::nix::tests::diff_lock_nodes_tolerates_malformed_locks ............. ok
backends::nix::tests::test_compare_lock_nodes_no_change ..................... ok
backends::nix::tests::test_compare_lock_nodes_changed_rev ................... ok
backends::nix::tests::test_compare_lock_nodes_new_input_added ............... ok
```

The three `test_compare_lock_nodes_*` tests are the pre-existing update-detection
regression guard, kept with their original assertions intact.

### Note on `--all-targets`

`cargo clippy --all-targets -- -D warnings` reports **four pre-existing errors**,
none in code touched by this change:

- `src/backends/nix.rs:1307`, `:1341` — `await_holding_lock` in existing
  `#[cfg(test)]` helpers using `HOME_ENV_LOCK`
- `src/executor.rs:131` — `type_complexity` on a test-utility field
- `src/upgrade/detect.rs:108` — `items_after_test_module`

CI and `preflight.sh` both run clippy **without** `--all-targets`, so these are
invisible to the current gates. Reported for the user; not addressed, as they are
unrelated to this task.

---

## 2. Specification Compliance

Implemented as specified, with two deliberate deviations, both noted below as
improvements on the spec rather than omissions.

| Spec section | Status |
|---|---|
| §3.1 Flatpak ref resolution | Implemented |
| §3.2 Nix flake input deltas | Implemented |
| §3.3 Button beside backend name | Implemented |
| §5 No configuration changes | Honoured |
| §6 Localisation unchanged | Honoured |

### Deviation 1 — `compare_lock_nodes` removed rather than retained

The spec proposed keeping `compare_lock_nodes` as a one-line projection over
`diff_lock_nodes`. Once `nixos_flake_tempdir_check` was rewritten to project the
deltas itself, `compare_lock_nodes` had no non-test caller and became dead code —
which `cargo clippy -- -D warnings` rejects.

It was therefore deleted, and its three tests re-pointed at a local
`changed_names()` test helper performing the identical projection. The assertions
are unchanged, so the regression guard the spec asked for is fully preserved.

### Deviation 2 — project CSS classes instead of libadwaita's internal ones

The spec proposed reusing libadwaita's internal `.title` / `.subtitle` row
classes. That couples the row's typography to libadwaita's private stylesheet
structure, which can shift between versions. The implementation instead uses
project-owned `.row-title` / `.row-subtitle` classes defined in `data/style.css`,
combined with the standard `dim-label` class already used elsewhere in this file
for the status label. This removes the "typography drifts" risk from §7 entirely.

---

## 3. Code Review Findings

### Correctness

- The Flatpak fetcher no longer consumes the caller's `packages` list for
  identification, removing the display-name/app-ID mismatch that was the root
  cause. It resolves `--columns=ref,origin` itself, which is well-defined for
  both apps and runtimes.
- Per-ref `remote-info` failures now render inline rather than being swallowed by
  `if let Ok(...)`, so a total failure is distinguishable from genuinely empty
  changelog data.
- Enumeration failure across both scopes returns `ChangelogError::Exit` instead
  of the misleading "No changelog information available."
- `short_rev` slices by byte but git revisions are ASCII hex, so no UTF-8
  boundary panic is reachable; the `.min(8)` guard covers short input.
- `age_suffix` returns empty rather than a negative day count when a timestamp is
  in the future (clock skew), and is covered by a test for that case.
- `diff_lock_nodes` handles malformed locks without panicking — missing `nodes`,
  a non-object `nodes`, and nodes lacking a `locked` section are all tested.

### Performance

- `supports_lock_changelog()` is called from the GTK main thread on every status
  update, and under a Flatpak sandbox its underlying probes spawn a
  `flatpak-spawn` subprocess. It is wrapped in a `OnceLock<bool>`, so the probe
  runs at most once per session. This is a small improvement over the previous
  code path, which could not reach it at all.
- The Nix fetch runs a real `nix flake update` against the network on each button
  press and is not cached. This is accepted and documented: it is user-initiated,
  runs on a background thread via `spawn_background_async`, and is bounded by a
  180-second timeout with a visible progress placeholder.

### Security

- No new privileged operations. All added commands are unprivileged, read-only
  queries of a kind already used elsewhere in the codebase.
- Arguments are passed as separate `argv` elements through
  `tokio::process::Command`, never interpolated into a shell string, so
  flatpak-supplied ref and remote names cannot inject commands.
- No change to `data/io.github.up.Daemon.conf`, `data/io.github.up.policy`, or
  any systemd unit.

### Consistency & maintainability

- Matches the module's existing conventions: `run_cmd` helper, `truncate` cap,
  `build_flatpak_cmd` sandbox handling, per-backend `MAX_*` constants.
- Pure parsing/formatting logic is split into free functions with unit tests,
  matching how `src/backends/flatpak.rs` and `src/backends/nix.rs` already
  separate their parsers.
- Doc comments explain *why* the non-obvious choices were made (why the caller's
  package list is bypassed; why the title is hand-built; why the timeout differs).

### Accessibility

Emptying the `AdwActionRow` title removes its accessible name, so the row is
given an explicit `accessible::Property::Label` with the backend name, following
the pattern already used for `skip_checkbox` in the same constructor.

---

## 4. Outstanding / Unverified

1. **Runtime behaviour is unverified.** Flatpak and Nix are not present in the
   build container, and GTK cannot render there. The parsing and formatting logic
   is unit-tested, but the actual `flatpak remote-info --log` and
   `nix flake update` round-trips, and the new row layout, need one run on the
   user's NixOS machine (`scripts/run-dev.sh`).
2. **`--columns=name` comment in `src/backends/flatpak.rs:289-291`** asserts the
   column yields IDs including runtimes. That is inaccurate for appstream-cached
   remotes. The file was deliberately left untouched (it drives the package
   popover, where display names are reasonable), but the comment is misleading.
3. **`src/changelog.rs` is absent from `po/POTFILES.in`** and emits untranslated
   English throughout. Pre-existing and module-wide; new strings follow the same
   convention rather than widening this task.
4. **Four pre-existing `--all-targets` clippy errors** listed in §1.

---

## 5. Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 98% | A |
| Best Practices | 96% | A |
| Functionality | 90% | A- |
| Code Quality | 96% | A |
| Security | 100% | A+ |
| Performance | 92% | A- |
| Consistency | 96% | A |
| Build Success | 100% | A+ |

**Overall Grade: A (96%)**

Functionality is held at 90% solely because end-to-end behaviour against real
`flatpak` and `nix` binaries could not be exercised in this environment.

---

## 6. Verdict

**PASS** — proceed to Phase 6.

No CRITICAL issues. Both spec deviations are improvements and are documented
above. Build, lint, format, tests, and the full preflight script all exit 0.
