# ADAPTIVE_LAYOUT — Review & QA

**Phase 3 — Review & Quality Assurance**
**Date:** 2026-09-03
**Reviewed against:** `.github/docs/subagent_docs/ADAPTIVE_LAYOUT_spec.md`

---

## 1. Files Modified (Phase 2)

| File | Change |
|------|--------|
| `src/ui/window.rs` | `default_window_size()` helper; builder uses computed size; `max-height: 740px` breakpoint toggling `.compact`; `content_box` spacing 18→14; `clamp` margins 24→18 |
| `data/style.css` | `.up-hero` padding 24/8→18/6; `.up-hero-title` `22px`→`1.4em`; new `window.up-window.compact …` block |
| `.github/docs/subagent_docs/ADAPTIVE_LAYOUT_spec.md` | Phase 1 spec (new) |
| `.github/docs/subagent_docs/ADAPTIVE_LAYOUT_review.md` | this file (new) |

## 2. Specification Compliance

| Spec item | Status |
|-----------|--------|
| §3.1 monitor-aware default size, integer math, fallback to ideal | ✅ implemented as specified |
| §3.1 `IDEAL_H` lowered to 720 | ✅ |
| §3.2 single `max-height: 740px` breakpoint, non-fatal parse, `.compact` toggle on window | ✅ (`parse` returns `Result` → `if let Ok`, spec updated) |
| §3.3 static: hero padding, `1.4em` title, spacing 14, clamp margins 18 | ✅ |
| §3.3 compact CSS block (hero padding/title/subtitle, preferencesgroup margin-top) | ✅ |
| No new deps / strings / gresource / daemon changes | ✅ confirmed — `po/`, `data/*.gresource.xml`, `daemon/` untouched |
| Respect system text scaling (no px on title) | ✅ `1.4em` / `1.15em` / `0.9em` are all relative |

## 3. Code Review

### 3.1 `default_window_size()`
- Pure function, no side effects, no `unwrap`/`expect`. Graceful `let-else`
  fallback to `(760, 720)` if no display / no monitor / wrong type.
- `downcast::<gtk::gdk::Monitor>().ok()` mirrors the existing pattern at
  `window.rs:388` (`downcast::<gtk::Window>().ok()`).
- `d.monitors().item(0)` uses `gio::ListModelExt`, in scope via
  `adw::prelude::*`.
- Integer percentage math (`* 95 / 100`) — no float cast, no precision lint.
- Clamp ordering `IDEAL.min(max).max(MIN)`: if `max < MIN` (absurdly tiny
  monitor) `MIN` wins — acceptable, window managers will constrain anyway.

### 3.2 Breakpoint
- `adw::BreakpointCondition::parse` → `Result`; `if let Ok(..)` keeps a bad
  string non-fatal (cannot happen with this literal, but defensive and cheap).
- `glib::clone!(#[weak] window, move |_| …)` with a `()`-returning closure and
  no `#[upgrade_or]` — identical shape to the existing
  `connect_network_metered_notify` call at `window.rs:~709`, so it compiles
  under glib 0.20.
- `connect_apply` / `connect_unapply` fire on the GTK main thread → direct
  `add_css_class` / `remove_css_class` is safe.
- `add_breakpoint` consumes `breakpoint` by value — no leak, no clone needed.

### 3.3 CSS
- `.compact` scoped as `window.up-window.compact …` so specificity beats the
  base rules; unapply cleanly reverts (class removed).
- `preferencesgroup > box` `margin-top: 0` targets the card wrapper libadwaita
  generates; consistent with the existing `preferencesgroup > box` rule at
  `style.css:89`.
- `em` units resolve against the inherited font size, which libadwaita derives
  from the GNOME font + text-scaling factor → the constraint is satisfied.

### 3.4 Surgical-change check
Every changed line traces to the request:
- window size / breakpoint / spacing / margins → "size itself according to the
  resolution", "scroll bar … too large to fit".
- hero title `em` + compact font sizes → "the font size … may need to be
  adjusted".
No adjacent refactoring, no reformatting, no dead-code removal. Return tuple of
`build_update_page` deliberately left unchanged (Phase 4 fallback only).

## 4. Build Validation

| Command | Result |
|---------|--------|
| `cargo build` | **NOT RUN — host is Windows (win32); GTK4/libadwaita system libs unavailable).** Must run on the Linux VM / GitLab CI. |
| `cargo build -p up-daemon` | NOT RUN (same reason); daemon crate untouched by this change. |
| `cargo fmt --check` | NOT RUN on host. New code follows rustfmt defaults (4-space, trailing commas, `let-else` formatting matches rustfmt 2024). |
| `cargo clippy -- -D warnings` | NOT RUN on host. Static review found no obvious lint (no needless clone, no `unwrap`, no shadowing, doc comment present). |
| `cargo test` | NOT RUN on host. No test exercises window geometry or CSS; regression-only risk. |
| `scripts/preflight.sh` | Deferred to Phase 6 on Linux. |

### Static-analysis confidence
- API surface (`Display::monitors`, `Monitor::geometry`, `Rectangle::width/height`,
  `BreakpointCondition::parse`, `Breakpoint::new/connect_apply/connect_unapply`,
  `ApplicationWindow::add_breakpoint`) verified against libadwaita-rs 0.7 /
  gtk4-rs 0.9 docs (Context7 + gtk-rs book + libadwaita-rs API docs).
- Every new call has a same-shape precedent already compiling in this file.

## 5. Score Table

| Category | Score | Grade |
|----------|-------|-------|
| Specification Compliance | 100% | A |
| Best Practices | 95% | A |
| Functionality | 90% | A- |
| Code Quality | 95% | A |
| Security | 100% | A |
| Performance | 100% | A |
| Consistency | 98% | A |
| Build Success | — | NOT VERIFIABLE ON HOST |

**Overall Grade: A- (pending Linux build)**

## 6. Findings

- **CRITICAL:** none.
- **BLOCKING (process):** `cargo build` + `scripts/preflight.sh` have not been
  executed because the orchestrator host cannot compile this project. Phase 6
  must be completed on the Linux VM / CI before Phase 7.
- **RECOMMENDED:** after building, visually confirm on the 1280×800 VM that the
  Update page shows no scrollbar at rest (spec §8 criteria 5–7). If it still
  clips, apply the documented Phase 4 fallback (`Breakpoint::add_setter` for
  `content_box` spacing + `clamp` margins).

## 7. Verdict

**PASS (static)** — implementation matches the spec, no correctness or safety
issues found. Build + preflight verification completed on Linux — see §8.

---

## 8. Phase 6 — Preflight Result (Linux / Nix dev shell via WSL)

`nix develop --command bash scripts/preflight.sh` (LF-normalized copy — the
Windows working tree is CRLF via `core.autocrlf`; the repo index is LF so CI is
unaffected).

| Step | Command | Result |
|------|---------|--------|
| 1 | `cargo fmt --check` | ✅ pass |
| 2 | `cargo clippy -- -D warnings` | ✅ pass — no warnings (7m03s) |
| 3 | `cargo build` | ✅ pass (17m29s) |
| 4 | `cargo test` | ✅ **159 passed; 0 failed; 0 ignored** (11m14s) |
| 5 | `desktop-file-validate` | ⏭ skipped — not in dev shell (CI covers it; `data/*.desktop` unchanged) |
| 6 | `appstreamcli validate --no-net` | ✅ "Validation was successful." |
| 7 | `cargo audit` | ⏭ skipped — not installed (CI covers it; no dependency change) |
| 8 | `nix flake check` | ✅ passed |

**`All preflight checks passed.` — exit code 0.**

Steps 5 and 7 are not installed in the local Nix shell but are exercised by
`.gitlab-ci.yml`; neither is affected by this change (no `data/*.desktop`
schema change, no dependency change).

**Final verdict: APPROVED — CI-ready.**
