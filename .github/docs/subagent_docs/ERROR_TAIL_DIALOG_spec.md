# ERROR_TAIL_DIALOG — Specification

MASTER_PLAN item 24 — show error tail on click instead of a truncated one-line
label. Source: FEATURES 11.

## Problem

On a failed backend update the UI shows only `"Error: <program> exited with
code N"`. The 100-line output tail that `CommandRunner` / `PrivilegedShell`
already retain (`tail_str`) is discarded — the user has no way to see *why* it
failed without leaving the app / re-running with a terminal.

## Design

### Carry the tail into the error

- `PrivilegedShell::run_command` (non-zero exit): `Err("Command exited with
  code N\n<output>")` when `output` is non-empty.
- `CommandRunner::run` direct-spawn path (non-zero exit):
  `BackendError::Exit { message: "<program> exited with code N\n<full_output>" }`.

### Protect the classifier

`BackendError::from_string` now runs its heuristics against **only the first
line** of the input (the generated error prefix); the appended output tail
(after the first `\n`) can no longer flip an `Exit` into a `Spawn`/`AuthCancelled`
misclassification (e.g. a tail containing "No such file or directory"). The
full text is still preserved in `message`.

### UI

`UpdateRow`:
- new `error_button` (icon `dialog-information-symbolic`, tooltip "Show error
  details", flat) + `error_details: Rc<RefCell<String>>`.
- `set_status_error(msg)`: label shows only `msg.lines().next()`; stores the
  full `msg`; shows `error_button` when `msg` has more than one line.
- every other `set_status_*` hides the button (`hide_error_button`).
- new free fn `show_error_details_dialog(parent, message)` —
  `adw::AlertDialog` ("Update Failed") with a non-editable monospace
  `TextView` in a 480×320 `ScrolledWindow`.

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| `from_string` behaviour change | Head-only check is strictly narrower; the two special exit-code special-cases (`fwupd` code 2, `nix` code 2 → CacheMiss) still work — those errors have no output tail. New tests cover the misclassification case and the still-working auth/spawn cases. |
| Very long tail in the dialog | Already capped at 100 lines by the runner; TextView scrolls. |
| Label wrapping on multi-line message | `set_status_error` now takes only the first line for the label. |

## Success criteria

- Failed update → short label + a details button opening a dialog with the
  full retained output.
- `BackendError::from_string` tests: tail with "No such file or directory"
  still → `Exit`; auth-cancel / genuine spawn-failure still classified.
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo build`,
  `cargo test` clean; `scripts/preflight.sh` exits 0.
