# CHANGELOG_FIX — Specification

Repair the "What's new" changelog viewer so it produces real output for Flatpak,
add support for the Nix backend, and relocate the trigger button to sit directly
after the backend name in each row.

Supersedes the Nix decision recorded in `changelog_viewer_spec.md` (line 119).

---

## 1. Current State Analysis

### 1.1 Feature wiring

| Piece | Location |
|---|---|
| Backend eligibility predicate | `src/changelog.rs:7-9` (`supports_changelog`) |
| Per-backend fetchers | `src/changelog.rs:40-52` (`fetch_changelog`) |
| Flatpak fetcher | `src/changelog.rs:125-160` (`fetch_flatpak`) |
| Trigger button | `src/ui/update_row.rs:124-135`, added as a suffix at `:180` |
| Visibility rules | `src/ui/update_row.rs:197-201`, `:383`, `:408-409`, `:415`, `:477` |
| Dialog | `src/ui/update_row.rs:520-569` (`show_changelog_dialog`) |
| Package list source | `src/ui/window.rs:864-865` -> `UpdateRow::set_packages` |

### 1.2 Why the button only appears on Flatpak

Not a defect. `supports_changelog` excludes `BackendKind::Nix` and
`BackendKind::Plugin(_)`. On a NixOS host the enabled backends are Nix and
Flatpak, so Flatpak is the only row that qualifies. `changelog_viewer_spec.md`
made this call deliberately, reasoning that Nix has no per-package changelog.

That reasoning is correct for *packages* but overlooks the unit that actually
changes on a flake-based NixOS: the flake inputs. Section 3.2 addresses this.

### 1.3 Why the Flatpak changelog is always empty

A four-step identifier mismatch:

1. `src/backends/flatpak.rs:297` enumerates pending updates with
   `flatpak remote-ls --updates <scope> --columns=name`.
2. Flatpak's `name` column resolves to the **appstream display name**
   ("Firefox") whenever appstream data is cached for the remote, falling back to
   the ref ID only when it is not. The list handed to `set_packages` therefore
   contains display names on any normally-configured system.
3. `src/changelog.rs:130` builds its app-to-remote map from
   `flatpak list --app --columns=application,origin`, keyed by **application ID**
   (`org.mozilla.firefox`).
4. Every `remote_map.get(app_id)` lookup misses, the code falls back to guessing
   the remote `"flathub"`, and then runs
   `flatpak remote-info --log flathub Firefox` — not a resolvable ref.

`src/changelog.rs:152` discards the resulting error with `if let Ok(text)`, so
`results` stays empty and the dialog prints the generic
"No changelog information available."

Two aggravating defects in the same function:

- `flatpak list --app` omits runtimes, so `org.gnome.Platform` and friends could
  never be mapped to a remote even with correct IDs.
- Swallowing every per-app error makes a total failure indistinguishable from a
  genuine absence of changelog data.

### 1.4 Verification limitation (stated explicitly)

This is a Windows development host. `flatpak`, `nix`, and GTK cannot be executed
here, so the behaviour of `--columns=name` was established by reading flatpak's
column semantics and the code path, not by running it. The Flatpak fix in §3.1 is
therefore designed to be **correct under either reading** of that column: it stops
depending on the display-name list entirely and resolves refs itself. Visual
confirmation of the §3.3 layout change requires a Linux run (`scripts/run-dev.sh`).

### 1.5 Dependency status

No new dependencies. All work uses crates already in `Cargo.toml`
(`tokio`, `serde_json`, `gtk4`, `libadwaita`, `gettext-rs`).

Context7 was unavailable this session (`CONNECT_TIMEOUT` on the MCP server).
Per CLAUDE.md's Dependency & Documentation Policy, Context7 is not required for
"internal code changes with no new dependencies" or for styling/UI-only changes,
both of which describe this work. No dependency decision rests on unverified
external documentation.

---

## 2. Problem Definition

1. The Flatpak changelog never renders content, and fails silently when it fails.
2. The Nix backend — the primary backend on the target platform — has no
   changelog at all, despite having a well-defined notion of "what changed".
3. The trigger button is visually buried among five other right-aligned suffix
   widgets; it should read as an affordance attached to the backend name.

### Success criteria

- `parse_flatpak_ref_origin` unit tests pass over representative
  `remote-ls --columns=ref,origin` output, including the header line, apps, and
  runtimes.
- `diff_lock_nodes` unit tests pass, covering unchanged, revision-changed, and
  newly-added inputs, and the existing `compare_lock_nodes` tests still pass
  unmodified.
- A Flatpak fetch failure surfaces its command error in the dialog rather than
  resolving to "No changelog information available."
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and
  `scripts/preflight.sh` all exit 0.

---

## 3. Proposed Solution

### 3.1 Flatpak — resolve refs directly

Rewrite `fetch_flatpak` so it does not consume the display-name list at all.

For each scope in `["--user", "--system"]`:

```
flatpak remote-ls --updates <scope> --columns=ref,origin
```

`ref` is unambiguous and uniform for apps and runtimes alike
(`app/org.gnome.Calculator/x86_64/stable`, `runtime/org.gnome.Platform/x86_64/47`),
and `origin` gives the exact remote, removing the `"flathub"` guess. Pairing the
two columns in one call also removes the second `flatpak list` round-trip.

Then, for up to `MAX_FLATPAK_ENTRIES` (5) targets:

```
flatpak remote-info <scope> --log <origin> <ref>
```

Behavioural rules:

- Header lines are rejected by requiring the first field to contain `/`; a valid
  ref always does, a `Ref` header never does.
- Targets are de-duplicated on the ref, keeping the first scope that reports it.
- A per-target failure is **rendered inline** (`<ref>` followed by
  `Changelog unavailable: <error>`) rather than discarded.
- If both scopes fail to enumerate, return the first error as
  `ChangelogError::Exit` so the dialog shows it.
- The existing `packages.is_empty()` early return is retained, so the parameter
  stays meaningful as a "nothing pending" signal.

New pure helper in `src/changelog.rs`, unit-testable without flatpak installed:

```rust
/// Parse `flatpak remote-ls --updates <scope> --columns=ref,origin` output into
/// `(scope, remote, ref)` triples, skipping the column header.
fn parse_flatpak_ref_origin(output: &str, scope: &str) -> Vec<(String, String, String)>
```

`src/backends/flatpak.rs` is **not** modified. Its `--columns=name` choice drives
the package popover, where a display name is arguably the better label, and
changing it is outside this task's scope. The comment at `flatpak.rs:289-291`
asserting that `name` yields IDs is inaccurate for appstream-cached remotes; this
is recorded here as an observation, not acted upon.

### 3.2 Nix — flake input revision deltas

On a flake-based NixOS, the meaningful answer to "what's new" is which flake
inputs moved and how far. `src/backends/nix.rs` already computes exactly this in
`nixos_flake_tempdir_check` (`:345-408`) — it copies `/etc/nixos/flake.{nix,lock}`
to a temp directory, runs `nix flake update` there, and diffs the locks — but
`compare_lock_nodes` (`:247`) throws away everything except the input names.

Preserve that detail instead of discarding it.

**`src/backends/nix.rs` changes:**

```rust
/// A flake input whose locked revision differs between two `flake.lock` files.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InputDelta {
    pub name: String,
    pub old_rev: Option<String>,
    pub new_rev: Option<String>,
    /// Unix timestamps from `locked.lastModified`.
    pub old_modified: Option<i64>,
    pub new_modified: Option<i64>,
    /// `(owner, repo)` when `locked.type == "github"`, for a compare URL.
    pub github: Option<(String, String)>,
}

pub(crate) fn diff_lock_nodes(old: &Value, new: &Value) -> Vec<InputDelta>;

/// Copy the system flake to a temp dir, update it, and report the input deltas.
pub(crate) async fn nixos_flake_lock_deltas() -> Result<Vec<InputDelta>, String>;

/// True when this host has a flake-based NixOS config (cached for the session).
pub(crate) fn supports_lock_changelog() -> bool;
```

- `compare_lock_nodes` is reduced to
  `diff_lock_nodes(old, new).into_iter().map(|d| d.name).collect()`, so its three
  existing tests keep passing untouched and there is one comparison implementation.
- `nixos_flake_tempdir_check` becomes the same one-line projection over
  `nixos_flake_lock_deltas`, preserving `list_available`'s current contract.
- `supports_lock_changelog` wraps `is_nixos() && is_nixos_flake()` in a
  `OnceLock<bool>`. This matters: `supports_changelog` is called from the GTK main
  thread on every status update, and under a Flatpak sandbox `is_nixos()` spawns a
  `flatpak-spawn` subprocess. Caching a session-static answer keeps the UI thread
  free of repeated process spawns.

**`src/changelog.rs` changes:**

`fetch_nix` returns `NotSupported` unless `supports_lock_changelog()`, then
formats `nixos_flake_lock_deltas()` output, capped at 10 inputs:

```
nixpkgs
  Current: a1b2c3d4 (14 days ago)
  New:     e5f6a7b8 (today)
  Compare: https://github.com/NixOS/nixpkgs/compare/a1b2c3d4...e5f6a7b8

home-manager
  Current: 1122aabb (9 days ago)
  New:     ccdd3344 (2 days ago)
```

Age is rendered as a relative day count derived from the `lastModified` epoch —
plain integer arithmetic, deliberately avoiding a calendar dependency such as
`chrono`. The `Compare:` line is emitted only for `type == "github"` inputs with
both revisions present.

`supports_changelog` becomes:

```rust
pub fn supports_changelog(kind: &BackendKind) -> bool {
    match kind {
        BackendKind::Nix => crate::backends::nix::supports_lock_changelog(),
        BackendKind::Plugin(_) => false,
        _ => true,
    }
}
```

**Timeout.** `run_cmd`'s 30-second ceiling is too tight for `nix flake update`
over the network. `nixos_flake_lock_deltas` is given its own 180-second timeout.
This is acceptable because the fetch is explicitly user-initiated, runs on a
background thread via `spawn_background_async`, and the dialog already displays a
"Fetching changelog…" placeholder for the duration.

**Cost, stated plainly.** This performs a real `nix flake update` against the
network on each button press. It is not cached between presses. That is the price
of accurate old-to-new revisions without reimplementing input-URL resolution, and
it only runs when the user asks for it.

### 3.3 Move the button beside the backend name

`adw::ActionRow` places prefixes left of the title and suffixes at the trailing
edge, with no API to insert a widget between the title text and the suffix area.
Getting the button to sit immediately after "Nix" / "Flatpak" therefore requires
owning the title area.

Approach: clear the row's own `title`/`subtitle` and supply an equivalent prefix:

```
prefix box (horizontal, spacing 12)
├── icon                              (unchanged)
├── text box (vertical, valign Center)
│   ├── title label     css: "title"
│   └── subtitle label  css: "subtitle"   (hidden when description is empty)
└── changelog_button    valign Center
```

- `.title` and `.subtitle` are the same style classes libadwaita applies to
  `AdwActionRow`'s own labels, and the descendant selectors that style them match
  inside the row, so typography is preserved.
- `row.add_suffix(&changelog_button)` at `update_row.rs:180` is removed; all
  existing `set_visible` call sites (`:197-201`, `:383`, `:408-409`, `:415`,
  `:477`) are untouched, since only the parent changes.
- Accessibility: emptying the row title removes its accessible name, so the row
  is given an explicit `accessible::Property::Label` carrying the backend name.
- A `.changelog-button` rule is added to `data/style.css` under the existing
  "Backend rows" section to tighten its margins against the title. No new asset,
  so no `data/io.github.up.gresource.xml` change.

---

## 4. Implementation Steps

1. **`src/changelog.rs` — Flatpak.** Add `parse_flatpak_ref_origin` plus its unit
   tests; rewrite `fetch_flatpak` per §3.1; delete the now-unused
   `flatpak list --app` mapping step.
   *Verify:* `cargo test` — new parser tests pass, including a runtime ref.
2. **`src/backends/nix.rs` — deltas.** Add `InputDelta`, `diff_lock_nodes`,
   `nixos_flake_lock_deltas`, `supports_lock_changelog`; reduce
   `compare_lock_nodes` and `nixos_flake_tempdir_check` to projections over them.
   *Verify:* `cargo test` — the three pre-existing `compare_lock_nodes` tests pass
   without edits; new `diff_lock_nodes` tests cover unchanged / changed-rev /
   added-input / github-URL cases.
3. **`src/changelog.rs` — Nix.** Add `fetch_nix` and the relative-age helper; wire
   `BackendKind::Nix` into `fetch_changelog`; update `supports_changelog`.
   *Verify:* `cargo test`; age helper covered by unit tests at fixed epochs.
4. **`src/ui/update_row.rs` — layout.** Build the prefix title box per §3.3,
   remove the suffix registration, set the accessible label.
   *Verify:* `cargo build`; visual confirmation deferred to a Linux run.
5. **`data/style.css`.** Add the `.changelog-button` rule.
   *Verify:* `cargo build` (CSS is bundled via GResource).
6. **Phase 6 preflight.** `scripts/preflight.sh` must exit 0.

---

## 5. Configuration Changes

None. No new D-Bus policy, systemd unit, desktop-entry, or metainfo change.
Both new command families are unprivileged, read-only, and already permitted:
`flatpak remote-ls` / `remote-info` are used elsewhere in the codebase, and
`nix flake update` in a temp directory is the existing check path.

---

## 6. Localisation

`src/changelog.rs` is not listed in `po/POTFILES.in` and currently emits plain
untranslated English literals throughout. New strings in that module follow the
same convention, so no `po/POTFILES.in` or `po/meson.build` change is required.

This is a pre-existing localisation gap covering the whole module, not something
introduced here. It is flagged for the user as separate follow-up work rather
than widened into this task.

`src/ui/update_row.rs` is already in `POTFILES.in`, and §3.3 adds no new
user-visible string — the title and subtitle labels reuse the existing
`backend.display_name()` and `backend.description()` values.

---

## 7. Risks and Mitigations

| Risk | Severity | Mitigation |
|---|---|---|
| `--columns=name` may already yield IDs, making §1.3's root cause wrong | High | The fix does not depend on the answer. Resolving refs via `--columns=ref,origin` is correct either way, and additionally fixes the runtime-mapping and swallowed-error defects, which are independently real. |
| `nix flake update` exceeds the timeout on a large flake | Medium | 180-second budget; background thread; dialog shows a progress placeholder; timeout surfaces as a visible error, not silence. |
| Network fetch on every button press | Medium | Accepted and documented in §3.2. User-initiated only. |
| `.title`/`.subtitle` typography drifts from stock `AdwActionRow` | Medium | Same classes libadwaita uses internally, applied in the same row context; `data/style.css` is available to correct any residual difference once seen on Linux. |
| Emptying the row title degrades screen-reader output | Medium | Explicit `accessible::Property::Label` set on the row. |
| Refactoring `compare_lock_nodes` regresses update *detection* | High | It becomes a projection over the shared implementation, and its three existing tests are kept verbatim as the regression guard. |
| `remote-info --log` output is large | Low | Existing `MAX_CHARS` (10 000) truncation and the 5-entry cap both still apply. |
| No Linux/GTK verification available this session | Medium | Stated in §1.4; `scripts/preflight.sh` is the gate, and the layout change needs the user's eyes on a real run. |

---

## 8. Files to Modify

- `src/changelog.rs`
- `src/backends/nix.rs`
- `src/ui/update_row.rs`
- `data/style.css`
