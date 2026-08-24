# Specification: True Package-Level Progress Bar

**Feature:** Make the update progress bar advance in step with real package/derivation
progress instead of stepping once per backend
**Project:** Up (GTK4/libadwaita system updater, Rust)
**Spec Path:** `.github/docs/subagent_docs/true_progress_bar_spec.md`

---

## 1. Current State Analysis

The progress bar is created in `src/ui/window.rs:232` and driven from exactly three places
inside the orchestrator event loop:

| Location | Event | Fraction set |
|---|---|---|
| `src/ui/window.rs:533` | `BackendStarted` | `(finished + 0.5) / total_backends` |
| `src/ui/window.rs:641` | `BackendFinished` | `finished / total_backends` |
| `src/ui/window.rs:644` | `AllFinished` | `1.0` |

`total_backends` / `finished_backends` are `Cell<usize>` counters holding the number of
non-skipped detected backends.

`OrchestratorEvent` (`src/orchestrator.rs:45-61`) has no progress variant. Backends are run
strictly sequentially in `run_all()`; every line of stdout+stderr of every backend command is
already streamed to the UI as `OrchestratorEvent::BackendLog(kind, line)` — stderr is merged
into stdout by the privileged shell (`src/runner.rs:147`, `... 2>&1`), and `stdbuf -oL -eL`
is already applied to the Nix commands so lines arrive live rather than in one block.

### Observed symptom

With 3 active backends the bar jumps to `0.5/3 ~= 0.17` when the first backend starts and then
does not move at all for the whole duration of that backend (which for a NixOS rebuild is
essentially the entire run). It then steps to 0.33, 0.67, and `AllFinished` slams it to 1.0.
This is exactly the "gets a third of the way, then shoots across at the end" behaviour reported.

## 2. Problem Definition

The bar's resolution is *one backend*, but all of the elapsed time happens *inside* a backend.
No intra-backend signal is currently produced, even though the information needed to derive one
is already flowing through `BackendLog` (transaction counters, derivation counts, `n/m` progress
lines emitted by every supported package manager).

**Goal:** the bar must move continuously and monotonically while a backend is working, driven by
real events from that backend's own output — no timers, no synthetic animation.

## 3. Proposed Solution Architecture

### 3.1 New module: `src/progress.rs`

A pure-Rust, GTK-free, per-backend log parser:

```rust
pub struct ProgressTracker { /* kind, total, done, seen set, last_emitted */ }

impl ProgressTracker {
    pub fn new(kind: &BackendKind) -> Self;
    /// Feed one line of backend output. Returns Some(fraction) in 0.0..=1.0
    /// only when the fraction has advanced meaningfully; None otherwise.
    pub fn observe(&mut self, line: &str) -> Option<f64>;
}
```

Two kinds of signal are recognised per backend:

1. **Totals** — a plan/preamble line that announces how much work there is.
2. **Ticks** — a line indicating one unit of that work completed, or an explicit `n/m` counter.

Explicit `n/m` counters take precedence over tick counting when both are present.

Per-backend rules:

| Backend | Total source | Tick / counter source |
|---|---|---|
| **Nix** | `these N derivations will be built:` / `these N paths will be fetched`, plus the singular `this derivation will be built:` / `this path will be fetched` forms (N=1). Totals from both lines are summed. | `building '/nix/store/...drv'...`, `copying path '/nix/store/...'...`, `downloading '...'`. Store paths are de-duplicated via a `HashSet` so a path announced and later confirmed only counts once. Build-log lines prefixed `name> ` (from `--print-build-logs`) are ignored. |
| **Nix (activation)** | — | `activating the configuration`, `setting up /etc`, `restarting`/`reloading` unit lines pin the fraction to 0.95 — the build is done, activation remains. |
| **Flatpak** | numbered transaction-table rows (`^\s*\d+\.\s`) counted before execution begins | `(\d+)/(\d+)` on lines containing `Updating`/`Installing`/`Downloading` |
| **DNF / Zypper / Pacman** | implicit from the counter | trailing `n/m` (DNF: `... 12/345` at end of line) and leading `( n/m)` (Pacman/Zypper) |
| **APT** | implicit — percentages are absolute | `APT::Status-Fd` machine protocol: `dlstatus:<pkg>:<pct>:<msg>` mapped to 0–40 % and `pmstatus:<pkg>:<pct>:<msg>` mapped to 40–100 % |
| **Homebrew** | `==> Upgrading N outdated packages` | `==> Upgrading <formula>` lines |
| **Fwupd** | device count | `(\d+)/(\d+)` counters; `Successfully installed firmware` ticks |
| **Plugin(id)** | — | opt-in protocol line `up:progress:<0-100>` emitted by the plugin |

Unrecognised output yields `None` — such a backend simply keeps today's coarse behaviour rather
than showing a wrong bar.

**Monotonicity & throttling** are enforced inside the tracker: the returned fraction never
decreases, is clamped to `[0.0, 1.0]`, and is only returned when it has grown by at least
`0.005` since the last emission (prevents thousands of redundant GTK redraws during nix build
log floods).

### 3.2 APT command change

`AptBackend::run_update` / `run_selected_update` gain `-o APT::Status-Fd=1` so apt emits its
machine-readable progress protocol on stdout, which the runner already streams. The generated
`dlstatus:`/`pmstatus:` lines are filtered out of the visible log panel (they are protocol
noise, not user-facing output) but are still fed to the tracker.

### 3.3 New orchestrator event

```rust
OrchestratorEvent::BackendProgress(BackendKind, f64)   // fraction within this backend
```

Emitted from the existing log-forwarding task in `run_all()` (`src/orchestrator.rs:120-129`),
which already receives every `BackendEvent::LogLine`. The task keeps one `ProgressTracker` and
recreates it whenever the incoming `BackendKind` differs from the previous line's kind (valid
because backends run strictly sequentially and each kind appears once in the list). Ordering is
preserved because progress events are sent on the same channel, from the same task, as the log
lines they were derived from.

### 3.4 UI change

In `src/ui/window.rs`, handle the new event:

```rust
OrchestratorEvent::BackendProgress(_, f) => {
    let total = total_backends.get();
    if total > 0 {
        let finished = finished_backends.get() as f64;
        let target = (finished + f.clamp(0.0, 1.0)) / total as f64;
        if target > progress_bar.fraction() { progress_bar.set_fraction(target); }
    }
}
```

`BackendStarted` stops setting `(finished + 0.5) / total` — that half-step is what makes the bar
lie about position — and instead sets `finished / total`, the true segment floor. `AllFinished`
keeps setting 1.0. The `> fraction()` guard makes the bar globally monotonic even if a backend
emits a lower fraction after a segment boundary.

### 3.5 Rejected alternative: weighting backends by package count

Weighting each backend's segment by `row.last_available_count()` was considered and rejected:
`NixBackend::list_available()` returns *changed flake inputs* (typically 3–6), which has no
relationship to rebuild duration, so weighting would systematically under-weight the longest
backend and make the bar worse on exactly the system reporting the bug. Equal per-backend
segments are kept; the fix is intra-segment resolution, not inter-segment weighting.

## 4. Implementation Steps

1. Create `src/progress.rs` with `ProgressTracker` + per-backend parsers + unit tests → verify: `cargo test progress::`
2. Register `mod progress;` in `src/main.rs` → verify: compiles
3. Add `OrchestratorEvent::BackendProgress` and drive it from the forwarding task in `src/orchestrator.rs` → verify: compiles, existing orchestrator tests pass
4. Add `-o APT::Status-Fd=1` to the two APT update commands → verify: existing apt tests pass
5. Handle `BackendProgress` in `src/ui/window.rs`, change `BackendStarted` to set the segment floor, filter `pmstatus:`/`dlstatus:` from the log panel → verify: compiles
6. Run `scripts/preflight.sh` on Linux → verify: exit code 0

## 5. Dependencies

No new crates. `regex` is avoided deliberately — all parsing is done with `str` methods and
manual digit scanning, matching the existing hand-rolled parser style in
`src/backends/os_package_manager.rs` and `src/backends/nix.rs`. Context7 lookup is therefore not
required (internal change, no new external dependency).

## 6. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| A parser mis-reads output and the bar jumps around | Monotonic clamp: the bar can never move backwards, and an unrecognised line yields `None` (status quo behaviour) |
| Nix totals are announced only for the build phase, so downloads/activation exceed the total | `done` is clamped to `total`; activation markers pin at 0.95 until `BackendFinished` |
| `--print-build-logs` output contains lines resembling tick markers | Lines matching the `name> ` build-log prefix are skipped before parsing |
| High-frequency redraws during nix build log floods | 0.005 minimum delta before an event is emitted |
| Cannot build/test on the developer's Windows host (no GTK4) | Parser logic is validated standalone in a GTK-free scratchpad crate; `scripts/preflight.sh` must be run by the user on Linux before commit |
