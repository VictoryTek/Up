# ADAPTIVE_LAYOUT — Responsive window sizing & short-screen layout

**Phase 1 — Research & Specification**
**Feature branch target:** `main`
**Author:** Orchestrating Agent (Up)
**Date:** 2026-09-03

---

## 1. Current State Analysis

### 1.1 Window construction

`src/ui/window.rs` builds the top-level window with a **fixed** default size:

```rust
let window = adw::ApplicationWindow::builder()
    .application(app)
    .title("Up")
    .default_width(760)
    .default_height(730)
    .build();
```

There is no query of the monitor geometry, and no `AdwBreakpoint` anywhere in the
codebase (`grep -rn "breakpoint" src/` → no matches). The layout is therefore
identical at every window size until the `ScrolledWindow` inside the Update page
starts clipping.

### 1.2 Update-page vertical layout (`build_update_page`)

```
page_box (Vertical)
├── restart_banner        (hidden unless self-update)
├── done_banner           (hidden unless run finished)
├── metered_banner        (hidden unless metered)
├── low_space_banner      (hidden unless low space)
└── scrolled (vexpand)          ← ScrollbarPolicy default = Automatic (vertical)
    └── adw::Clamp  (max 800, margin_top 24, margin_bottom 24, start/end 12)
        └── content_box (Vertical, spacing 18)
            ├── hero_box              .up-hero  → CSS padding: 24px 12px 8px
            │     Image pixel_size(52) + title(22px) + subtitle(13px)
            ├── progress_bar          margin_top 4 + margin_bottom 4, opacity 0
            │                         (space permanently reserved — 2.3.3 fix, keep)
            ├── sys_info_group        PreferencesGroup "System Information" + 2 rows
            ├── backends_group        PreferencesGroup "Sources" + description + N rows
            └── log_panel.expander    margin_start/end 12, margin_bottom 12
```

### 1.3 Measured overflow

On the reporter's VM (1280×800, GNOME Shell):

- Usable height ≈ 800 − 32 (top bar) ≈ 768 px.
- Window default height 730 → CSD header bar ≈ 46 px → scrolled viewport ≈ 684 px.
- `content_box` natural height at that width exceeds ~684 px (hero ≈ 52 + hero
  padding 32 + spacing 18 + progress strip ≈ 14 + spacing 18 + two preferences
  groups + log expander + clamp margins 48), so the vertical scrollbar appears
  **on launch, before any interaction**. Screenshot confirms the "Terminal
  Output" expander is partly below the fold with the scrollbar engaged.

### 1.4 Relevant CSS (`data/style.css`)

| Selector | Current | Note |
|----------|---------|------|
| `.up-hero` | `padding: 24px 12px 8px;` | Largest fixed vertical block |
| `.up-hero-title` | `font-size: 22px;` | Only oversized custom font on the page |
| `.up-hero-subtitle` | `font-size: 13px; margin-top: 2px;` | |
| `preferencesgroup > box` | `border-radius: 14px;` | |
| `.log-expander > box > box > label` | `font-size: 12px;` | monospace, fine |

`@define-color` brand tokens live at the top of the file; libadwaita's
`window_bg_color` / `window_fg_color` are used throughout.

### 1.5 Toolkit versions (from `Cargo.lock`)

- `gtk4` 0.9.7 (feature `v4_12`)
- `libadwaita` 0.7.2 → wraps libadwaita **1.6** (feature `v1_5`)

`AdwBreakpoint` / `adw::ApplicationWindow::add_breakpoint()` have existed since
libadwaita **1.4**, so they are available. `AdwBreakpointCondition::parse()` and
`Breakpoint::connect_apply` / `connect_unapply` are in `libadwaita` 0.7.

---

## 2. Problem Definition

1. **Primary:** the window opens with a vertical scrollbar on a 1280×800 display
   because the fixed 730 px default height + fixed margins/paddings/spacing make
   `content_box` taller than the scrolled viewport.
2. **Secondary:** nothing in the UI adapts to the available space — margins,
   section spacing, hero padding and the 22 px hero title stay the same whether
   the window is 700 px or 1100 px tall.
3. **Constraint from the user:** do **not** invent a resolution-based font
   scaler — the app must keep honouring GNOME's configured text scaling
   (standard GTK behaviour). Only the app's own oversized custom sizes may be
   reduced, and relative units are preferred.

### Non-goals

- No mobile/phone form factor, no `AdwNavigationSplitView` restructuring.
- No change to the page's widget hierarchy or to the 2.3.3 progress-bar
  space-reservation behaviour.
- No global font-scale computation.

---

## 3. Proposed Solution

Three independent, additive changes.

### 3.1 Monitor-aware default window size — `src/ui/window.rs`

Add a private helper and use it in the builder:

```rust
/// Default window size, clamped so it never exceeds the monitor it will most
/// likely open on. The fixed 760×730 default overflowed a 1280×800 screen
/// once the top panel and CSD header bar were subtracted, forcing a scroll
/// bar on launch.
fn default_window_size() -> (i32, i32) {
    // Comfortable target on a roomy display.
    const IDEAL_W: i32 = 760;
    const IDEAL_H: i32 = 720;
    // Never shrink below this — the page is unusable smaller.
    const MIN_W: i32 = 600;
    const MIN_H: i32 = 500;

    let Some(geo) = gtk::gdk::Display::default()
        .and_then(|d| d.monitors().item(0))
        .and_then(|obj| obj.downcast::<gtk::gdk::Monitor>().ok())
        .map(|m| m.geometry())
    else {
        return (IDEAL_W, IDEAL_H);
    };

    // Leave headroom for the shell panel and window decorations.
    let max_w = (geo.width() * 95) / 100;
    let max_h = (geo.height() * 90) / 100;

    (
        IDEAL_W.min(max_w).max(MIN_W),
        IDEAL_H.min(max_h).max(MIN_H),
    )
}
```

Builder change:

```rust
let (win_w, win_h) = Self::default_window_size();
let window = adw::ApplicationWindow::builder()
    .application(app)
    .title("Up")
    .default_width(win_w)
    .default_height(win_h)
    .build();
```

Notes / rationale:
- `monitors().item(0)` is a heuristic (the window is not mapped yet, so the real
  target monitor is unknown). On the overwhelmingly common single-monitor case
  it is exact; on multi-monitor it picks the first monitor, which is an
  acceptable default and is further corrected by the breakpoint in §3.2 once the
  window is actually mapped.
- Integer math only — no `f64` cast, no new imports beyond `gtk::gdk::Monitor`
  and the `Cast` trait already in scope via `adw::prelude::*`.
- `1280×800` → `max_h = 720`, so height resolves to `720`. `max_w = 1216`, width
  stays `760`. On a 1080p+ display the ideal values are used unchanged.

### 3.2 Short-screen adaptive breakpoint — `src/ui/window.rs`

After `window.set_content(...)`, register one breakpoint that toggles a
`.compact` style class on the window whenever the window is short:

```rust
// Collapse to a tighter layout when the window is too short to show the
// whole Update page at its normal spacing (e.g. a 1280×800 display).
if let Ok(condition) = adw::BreakpointCondition::parse("max-height: 740px") {
    let breakpoint = adw::Breakpoint::new(condition);
    breakpoint.connect_apply(glib::clone!(
        #[weak]
        window,
        move |_| window.add_css_class("compact")
    ));
    breakpoint.connect_unapply(glib::clone!(
        #[weak]
        window,
        move |_| window.remove_css_class("compact")
    ));
    window.add_breakpoint(breakpoint);
}
```

- `BreakpointCondition::parse` returns `Option` in `libadwaita` 0.7 — the `if let`
  keeps a parse failure non-fatal (falls back to the normal layout).
- The class is toggled on the **window**, so a single CSS block (`§3.3`) styles
  every descendant. No widget needs to be threaded out of `build_update_page`;
  the existing return tuple is untouched.
- `connect_apply`/`connect_unapply` fire on the GTK main thread — safe to touch
  widgets directly.
- Threshold `740px`: above the ~730 px natural content height plus header, so a
  window that *can* show everything normally does; the reporter's 720 px window
  is below it and gets the compact treatment.

### 3.3 Reduce baseline footprint + compact overrides — `data/style.css` + `src/ui/window.rs`

**Static reductions (apply at every size — modest, help all displays):**

`data/style.css`:

```css
.up-hero {
  padding: 18px 12px 6px;   /* was 24px 12px 8px */
}

.up-hero-title {
  font-size: 1.4em;         /* was 22px — relative, tracks system text scale */
}
```

`src/ui/window.rs` — `build_update_page`:

```rust
let content_box = gtk::Box::new(gtk::Orientation::Vertical, 14);   // was 18
```

```rust
let clamp = adw::Clamp::builder()
    .maximum_size(800)
    .tightening_threshold(600)
    .margin_top(18)      // was 24
    .margin_bottom(18)   // was 24
    .margin_start(12)
    .margin_end(12)
    .build();
```

**Compact overrides (`data/style.css`, appended after the hero block):**

```css
/* ── Compact layout (short windows — see Breakpoint in window.rs) ── */
window.up-window.compact .up-hero {
  padding: 8px 12px 4px;
}

window.up-window.compact .up-hero-title {
  font-size: 1.15em;
}

window.up-window.compact .up-hero-subtitle {
  font-size: 0.9em;
}

/* Tighten the gap the toolkit puts above every PreferencesGroup title. */
window.up-window.compact preferencesgroup > box {
  margin-top: 0;
}
```

Budget check for the reporter's window (720 px tall, ~674 px scrolled viewport):

| Block | Normal | After §3.3 |
|-------|-------:|----------:|
| clamp margin top+bottom | 48 | 36 |
| hero padding top+bottom | 32 | 12 (compact) |
| hero title line | ~30 | ~22 (compact 1.15em) |
| 4× content_box spacing | 72 | 56 (spacing 14) |
| Net saving | — | **≈ 68 px** |

≈ 68 px removed comfortably covers the observed overflow (Terminal Output
expander was only a little below the fold). If Phase 3 measurement still shows
clipping, Phase 4 adds `Breakpoint::add_setter` calls for `content_box`
`spacing` and the `clamp` margins (requires returning those two widgets from
`build_update_page`) — held back now to keep the change minimal.

---

## 4. Implementation Steps

1. **`src/ui/window.rs`**
   1. Add `fn default_window_size() -> (i32, i32)` (associated fn on `UpWindow`).
   2. Replace the `.default_width(760).default_height(730)` builder calls with
      the computed pair.
   3. After `window.set_content(Some(&main_box));`, add the `max-height: 740px`
      breakpoint that toggles the `.compact` class.
   4. In `build_update_page`: `content_box` spacing `18 → 14`; `clamp`
      `margin_top`/`margin_bottom` `24 → 18`.
   5. Verify imports: add `use gtk::gdk::Monitor;` only if the turbofish
      `downcast::<gtk::gdk::Monitor>()` needs it (fully-qualified path avoids a
      new `use`). No other new imports.
2. **`data/style.css`**
   1. `.up-hero` padding `24px 12px 8px → 18px 12px 6px`.
   2. `.up-hero-title` `font-size: 22px → 1.4em`.
   3. Append the `window.up-window.compact …` block after the hero section.
3. No changes to `data/io.github.up.gresource.xml` (style.css already bundled),
   `po/POTFILES.in` or `po/meson.build` — **no new user-visible strings**.
4. No daemon (`daemon/`) changes.

---

## 5. Dependencies

**None added.** All APIs are in the already-locked `gtk4` 0.9.7 / `libadwaita`
0.7.2:

| API | Since | Context7-verified |
|-----|-------|-------------------|
| `gdk::Display::monitors()` → `gio::ListModel` | GTK 4.0 | yes |
| `gdk::Monitor::geometry()` → `gdk::Rectangle` | GTK 4.0 | yes |
| `adw::BreakpointCondition::parse()` | libadwaita 1.4 | yes (gtk4-rs book `todo_4`, `AdwBreakpoint` `<condition>max-width: 500sp</condition>`) |
| `adw::Breakpoint::new(condition)` | libadwaita 1.4 | yes |
| `adw::Breakpoint::connect_apply` / `connect_unapply` | libadwaita 1.4 | yes |
| `adw::ApplicationWindow::add_breakpoint()` | libadwaita 1.4 | yes |

GTK CSS supports `em` / `rem` relative font units (resolved against the
inherited font size, which tracks GNOME's text-scaling factor) — this is why
`1.4em` satisfies the "respect system text scaling" constraint where `22px`
did not.

---

## 6. Configuration Changes

None. No new config keys, no persisted window geometry (out of scope — the user
asked for fit-to-screen, not session restore).

---

## 7. Risks & Mitigations

| # | Risk | Likelihood | Mitigation |
|---|------|-----------|------------|
| R1 | `monitors().item(0)` is not the monitor the window opens on (multi-head) | Low | Clamp only ever *shrinks* from the ideal; §3.2 breakpoint re-adapts once mapped on the real monitor. |
| R2 | `BreakpointCondition::parse` signature differs (`Result` vs `Option`) between 0.7.x point releases | Low | Implementation handles whichever with `if let Ok(..)` / `if let Some(..)`; Phase 3 build catches it. |
| R3 | `1.4em` renders differently than expected against libadwaita's base font | Low | Visual check in Phase 6 on the VM; `em` is well-supported and only the hero title changes. |
| R4 | `.compact` breakpoint fl*icker* when the user resizes across 740 px | Very low | Single class toggle, no layout thrash; matches how libadwaita's own adaptive widgets behave. |
| R5 | 68 px saving still insufficient on an even smaller display | Low | Documented Phase 4 fallback: `add_setter` for spacing + clamp margins. |
| R6 | Cannot build/preflight on the orchestrator host (Windows) | Certain | Phase 3 build + Phase 6 preflight (`scripts/preflight.sh`) MUST be run on the Linux VM / CI; results pasted back before Phase 7. |

---

## 8. Verification Criteria (for Phase 3 / Phase 6)

1. `cargo build` and `cargo build -p up-daemon` succeed with no new warnings.
2. `cargo fmt --check` clean; `cargo clippy -- -D warnings` clean.
3. `cargo test` passes (no behavioural tests touch layout; regression only).
4. `desktop-file-validate` / `appstreamcli` unaffected (no data-file schema
   changes) — preflight still green.
5. Manual, on the 1280×800 VM: launch Up → **no vertical scrollbar** on the
   Update page at rest; all sections (hero, System Information, Sources, Terminal
   Output) visible without scrolling.
6. Manual, on a ≥1080p display: window opens at 760×720, `.compact` **not**
   applied, hero title visually unchanged size (~22 px at 1.0 text scale).
7. Resizing the window below ~740 px height toggles the tighter layout live.

---

## 9. Summary of Findings

- The launch-time scrollbar is a fixed-size-layout problem, not a bug in the
  2.3.3 progress-bar work: 730 px default height + 48 px clamp margins + 32 px
  hero padding + 72 px of section spacing don't fit a 1280×800 screen's ~684 px
  scrolled viewport.
- libadwaita 1.6 (already vendored) provides `AdwBreakpoint`; no new dependency
  is needed for an adaptive layout.
- Fix is three additive changes: (a) clamp the default window size to 90 % of
  monitor height, (b) a `max-height: 740px` breakpoint toggling a `.compact`
  class, (c) trim baseline hero padding / section spacing / clamp margins and
  move the hero title to a relative `em` unit so it still honours system text
  scaling.
- Estimated ≈ 68 px reclaimed on the reporter's display — enough for the
  observed overflow, with a documented Phase 4 fallback (`add_setter`) if
  measurement disagrees.
- **Build + preflight cannot run on this host** and must be completed on the
  Linux VM/CI.

**Spec file:** `.github/docs/subagent_docs/ADAPTIVE_LAYOUT_spec.md`
