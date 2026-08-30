# SHARED_BACKEND_FINISHED — Specification

MASTER_PLAN item 16 — orchestrator event loop duplicated in the UI with
behavioural drift. Source: ARCH M7.

## Problem

`src/ui/window.rs` had four `while let Ok(event) = event_rx.recv().await` loops
over `OrchestratorEvent`:

1. **Update All** (main) — rows + VexOS cache dialog + history + progress bar +
   restart banner + cancel handle + `has_error`.
2. **Retry** (per-row) — rows + cache dialog + history; **no** progress bar,
   **no** restart banner, **no** cancel handle. `BackendFinished` handling was
   an ~85-line near-verbatim copy of loop 1's, including the entire cache-block
   dialog spawn.
3. **Cleanup** — log-panel only, no rows / dialog / history.
4. **Cache-bypass** — rows + status label, no dialog / history.

Loops 3 and 4 are genuinely different flows. Loops 1 and 2 share the
`BackendFinished` → row + cache-dialog + history logic verbatim, and every new
`UpdateResult` variant has to be handled in both. The retry copy had already
drifted: it ignored `SuccessWithSelfUpdate` (no restart banner).

## Design

Extract one helper:

```rust
#[derive(Default)]
struct BackendFinishedOutcome { is_error: bool, is_self_update: bool }

fn apply_backend_finished(
    kind: &BackendKind,
    result: &UpdateResult,
    rows: &Rc<RefCell<Vec<(BackendKind, UpdateRow)>>>,
    log_panel: &LogPanel,
    status_label: &gtk::Label,
    dialog_anchor: &gtk::Button,
    nix_log_lines: &[String],
) -> BackendFinishedOutcome
```

It performs the row status update, the VexOS cache-block dialog (with its two
`spawn_cache_bypass` closures), and `record_history_entry`, returning the two
flags the caller folds into its own loop state.

- **Loop 1**: `let o = apply_backend_finished(…); has_error |= o.is_error;
  self_updated |= o.is_self_update;` then its unchanged progress-bar math.
- **Loop 2 (retry)**: `let o = apply_backend_finished(…); if o.is_self_update
  { restart_banner_spawn.set_revealed(true); }` — this also **fixes the
  drift**: a self-update during a retry now reveals the restart banner.
  Requires threading `restart_banner` into the detect-completion closure
  (`#[weak] restart_banner`) and a per-row `restart_banner_retry` clone.

Loops 3 and 4 are left as-is (different, legitimately). The retry loop's
missing progress bar / cancel handle are a UX design choice for a
single-backend quick action and are out of scope.

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| Behaviour change in the main loop | Helper body is the main loop's exact former code; only `has_error`/`self_updated` become return flags instead of closure-captured `bool`s. |
| `glib::clone!` over `&`-typed params | `#[strong]` clones through the reference (`Rc`/`gtk` objects are `Clone`); build-verified. |
| Retry banner reveal is new behaviour | Intended — it fixes the documented drift; the banner is the same widget the main loop reveals. |

## Success criteria

- One definition of `BackendFinished` row/dialog/history handling.
- Retry path reveals the restart banner on `SuccessWithSelfUpdate`.
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo build`,
  `cargo test` clean; `scripts/preflight.sh` exits 0.
