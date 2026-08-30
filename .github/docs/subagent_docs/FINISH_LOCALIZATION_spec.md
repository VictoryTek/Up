# FINISH_LOCALIZATION — Specification

MASTER_PLAN item 25 — initialize gettext and wrap remaining UI strings.
Source: ARCH L4, FEATURES 14.

## Problem

The translation infrastructure (`po/`, `i18n` meson merge, `gettext-rs`) is
fully present but non-functional: `main.rs` never calls
`bindtextdomain`/`textdomain`, and `window.rs` / `update_row.rs` /
`log_panel.rs` contain raw string literals despite being listed in
`po/POTFILES.in`.

## Changes

### `src/main.rs` — bind the text domain

New `init_gettext()` called from `main()`:

```rust
setlocale(LcAll, "");
let localedir = option_env!("LOCALEDIR").unwrap_or("/usr/share/locale");
bindtextdomain(APP_ID, localedir);          // APP_ID == "io.github.up" == the po package
bind_textdomain_codeset(APP_ID, "UTF-8");
textdomain(APP_ID);
```

`LOCALEDIR` is set by the meson `cargo build` custom target and read at
compile time via `option_env!`; a plain `cargo` build falls back to the FHS
default.

### `src/ui/log_panel.rs`, `src/ui/update_row.rs`, `src/ui/window.rs`

Wrap every user-visible literal in `gettext(...)` (or `ngettext(...)` for the
"N update(s) available" count). Interpolations use the codebase's existing
`gettext("… {} …").replace("{}", &v)` idiom. Left untranslated:
- action / response ids (`"win.cleanup"`, `"update"`, `"cancel"`)
- CSS classes, icon names
- the app name `"Up"` and the `[{kind}]` log-line prefix (mechanical)
- pre-existing accessible `Property::Label` literals that were already raw
  (out of scope; a separate a11y pass)

`ngettext` keyword is already configured in `po/meson.build`
(`--keyword=ngettext:1,2`).

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| Wrapping a string used in logic | Audited: the only `==` / `starts_with` comparisons in these files are on distro ids and dialog response ids, never display text. |
| `record_history_entry` result strings | In `window.rs` but NOT display text (persisted schema) — left untouched. |
| Builder `.tooltip_text` needless-borrow | `gettext()` returns `String`; pass by value to builders, `Some(&…)` to `set_tooltip_text`. |
| Nix build has no `LOCALEDIR` | Falls back to FHS default; the flake ships no catalogs anyway. Meson (the documented install path) sets it correctly. |

## Success criteria

- `main()` binds the `io.github.up` text domain.
- No raw user-visible literal remains in the three files (ids / CSS / icons /
  app-name / log-prefix excepted).
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo build`,
  `cargo test` clean; `scripts/preflight.sh` exits 0.
