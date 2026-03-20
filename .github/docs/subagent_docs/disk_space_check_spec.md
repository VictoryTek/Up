# Specification: Fix `check_disk_space()` False Failure on Parse Error

**Feature:** `disk_space_check_parse_error_fix`  
**File:** `.github/docs/subagent_docs/disk_space_check_spec.md`  
**Date:** 2026-03-19  
**Finding:** #9 — `check_disk_space()` false failure when `df` output cannot be parsed  

---

## 1. Current State Analysis

### 1.1 `CheckResult` Struct — Exact Definition

Located in `src/upgrade.rs`, lines 14–19:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    pub passed: bool,
    pub message: String,
}
```

`CheckResult` is a **2-state struct** (pass/fail only). There is no `warning`, `skip`, or
`unknown` variant. This rules out Option B (3-state model).

### 1.2 `check_disk_space()` — Exact Current Implementation

Located in `src/upgrade.rs`, approximately lines 216–253:

```rust
fn check_disk_space() -> CheckResult {
    // Check available space on /
    match Command::new("df")
        .args(["--output=avail", "-B1", "/"])
        .output()
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let avail_bytes: u64 = stdout
                .lines()
                .nth(1) // skip header
                .and_then(|l| l.trim().parse().ok())
                .unwrap_or(0);        // <-- SILENT FAILURE POINT

            let avail_gb = avail_bytes / (1024 * 1024 * 1024);
            let required_gb = 10;

            if avail_gb >= required_gb {
                CheckResult {
                    name: "Sufficient disk space".into(),
                    passed: true,
                    message: format!("{avail_gb} GB available"),
                }
            } else {
                CheckResult {
                    name: "Sufficient disk space".into(),
                    passed: false,
                    message: format!("Only {avail_gb} GB available, {required_gb} GB recommended"),
                }
            }
        }
        Err(e) => CheckResult {
            name: "Sufficient disk space".into(),
            passed: false,
            message: format!("Could not check: {e}"),
        },
    }
}
```

### 1.3 The Bug — Parse Chain Analysis

The parse chain is:
```
stdout.lines().nth(1)           → Option<&str>  (None if output has < 2 lines)
  .and_then(|l| l.trim().parse().ok())  → Option<u64>   (None if not valid integer)
  .unwrap_or(0)                 → u64           (0 on ANY parse failure)
```

When the result is `0` (either from genuine parse success or silent failure), the
subsequent calculation yields `avail_gb = 0`. Then `0 >= 10` is `false`, so the function
returns:

```
CheckResult { passed: false, message: "Only 0 GB available, 10 GB recommended" }
```

**This message is indistinguishable whether the disk is truly full or the parse silently failed.**

### 1.4 How the Message Is Displayed in the UI

In `src/ui/upgrade_page.rs`, the check results are rendered in the prerequisite checklist.
For each `CheckResult`, the `message` field is set as the subtitle of an `adw::ActionRow`:

```rust
if let Some(row) = rows.get(i) {
    row.set_subtitle(&result.message);
}
if let Some(icon) = icons.get(i) {
    if result.passed {
        icon.set_icon_name(Some("emblem-ok-symbolic"));
    } else {
        icon.set_icon_name(Some("dialog-error-symbolic"));
        all_passed = false;
    }
}
```

A failed check shows a red error icon and sets the subtitle to `result.message`. A false
failure from a parse error therefore shows the user `"Only 0 GB available, 10 GB
recommended"` — confusing and misleading when parsing is the actual problem.

---

## 2. Problem Definition

### 2.1 Bug Description

`check_disk_space()` uses `.unwrap_or(0)` to silently absorb parse failures on `df`
output. This produces an indistinguishable false negative with the same user-visible
message as a genuine low-disk failure.

### 2.2 Failure Triggers

Parse failure occurs in all of the following real-world scenarios:

| Scenario | Root Cause | Effect |
|---|---|---|
| `df --output=avail` not supported (older GNU coreutils < 8.21, BusyBox) | `--output` is a GNU coreutils 8.21+ extension; old/minimal systems still use `df` without it | `df` exits 1, stderr has error, stdout is empty → `.nth(1)` returns `None` → 0 |
| Flatpak sandbox / composefs overlay root | Root FS is a read-only composefs; df may report 0 bytes genuinely OR produce unexpected column widths | Genuine 0 is ambiguous with parse failure |
| Command spawned successfully but no data line | Output contains only the header line `Avail\n` | `.nth(1)` returns `None` → 0 |
| Non-numeric stdout content | If df writes an unexpected message to stdout (some busybox variants write errors to stdout) | `.trim().parse::<u64>()` fails → `None` → 0 |

### 2.3 Why `Err(e)` Branch Does Not Catch This

`Command::output()` returns `Err(e)` only when the OS cannot spawn the process at
all (e.g., `ENOENT` for missing `df` binary). If `df` is present but exits non-zero (not
supported flag, permission denied, unexpected format), `.output()` returns `Ok(output)`
with a non-empty stderr and empty or malformed stdout. The bug lives entirely within the
`Ok(output)` arm.

---

## 3. Research Sources

### Source 1 — Rust `Result`-based Error Propagation vs `unwrap_or`
**The Rust Programming Language Book, Chapter 9: Error Handling**  
https://doc.rust-lang.org/book/ch09-00-error-handling.html  
Pattern: use `match` or the `?` operator to propagate parse errors explicitly. Using
`.unwrap_or(default)` is appropriate only when a zero/default value is semantically
indistinguishable from an error state. Here, `0 bytes` is a valid value (completely full
disk), so absorbing parse errors into `0` hides the true failure reason from the user.

### Source 2 — `df --output` Column Selection (GNU Coreutils 8.21+)
**GNU Coreutils Manual — df invocation**  
https://www.gnu.org/software/coreutils/manual/html_node/df-invocation.html  
The `--output=avail` flag was introduced in GNU coreutils 8.21 (2013). It selects
individual output columns. With `-B1`, the `Avail` column contains a plain decimal
integer (bytes), no suffix. The output format is exactly:
```
     Avail
<whitespace><integer>
```
Two lines: one header, one data line. Any deviation from this triggers `.nth(1)` returning
`None`.

### Source 3 — `df` Behavior on BusyBox and Alpine Linux
**BusyBox source — coreutils/df.c**  
https://git.busybox.net/busybox/tree/coreutils/df.c  
BusyBox `df` does not support `--output`. Invocation with `--output=avail` causes
BusyBox to print an error message to stderr and exit with code 1. stdout is empty in that
case. This is a common pattern for Alpine Linux containers, NixOS containers, and
minimal base images.

### Source 4 — Flatpak Host Command Execution Context
**Flatpak Documentation — `flatpak-spawn --host`**  
https://docs.flatpak.org/en/latest/flatpak-command-reference.html#flatpak-spawn  
When the Up application runs inside a Flatpak sandbox, `df --output=avail -B1 /`
targets the Flatpak's root filesystem (a composefs overlay), not the host OS root. This
overlay typically reports 0 available bytes. This is a legitimate `Ok(0)` parse — but
indistinguishable from a parse failure with the current code. More importantly, the
check would need `flatpak-spawn --host df` to query the real host's disk space.
However, fixing the Flatpak context is out of scope for this finding. The present fix
focuses on parse failure only.

### Source 5 — Rust `str::parse::<u64>()` Error Handling
**Rust standard library documentation — `str::parse` and `ParseIntError`**  
https://doc.rust-lang.org/std/primitive.str.html#method.parse  
`str::parse::<u64>()` returns `Result<u64, ParseIntError>`. The `ParseIntError` type
implements `Display` and produces human-readable messages like `"cannot parse integer
from empty string"` or `"invalid digit found in string"`. By using `.parse::<u64>()` and
propagating the `Err` variant, the implementation can surface a meaningful diagnostic
message to the user.

### Source 6 — Rust Error Extraction Pattern with Helper Functions
**Rust API Guidelines — Error Messages**  
https://rust-lang.github.io/api-guidelines/interoperability.html#error-types  
Best practice: extract parsing logic into a small private helper function returning
`Result<T, String>`. This allows the calling function to handle success/failure
explicitly, eliminates silent `unwrap_or` defaults, and makes the parsing logic directly
unit-testable without requiring a mock system command.

---

## 4. Proposed Fix

### 4.1 Recommendation: Option A

**Option B is not applicable.** `CheckResult` has no third `warning` or `skip` state.

**Option A is the correct and only viable fix.** The parse chain must be changed to return
a distinct `passed: false` result with a clear diagnostic message when output cannot be
parsed, rather than silently treating the failure as `0 bytes`.

### 4.2 Solution Architecture

Extract a private helper function `parse_df_avail_bytes` that accepts the raw stdout
string and returns `Result<u64, String>`. This achieves two goals:

1. **Distinct error path** — parse failures return a meaningful `CheckResult` message,
   not `"Only 0 GB available, 10 GB recommended"`.
2. **Unit testability** — the helper can be tested in isolation without spawning a `df`
   process.

The `check_disk_space()` function then dispatches on the `Result` from the helper.

### 4.3 New Helper Function — Specification

```rust
/// Parses the available-bytes value from `df --output=avail -B1` stdout.
///
/// Expected format:
/// ```text
///      Avail
/// <decimal_integer>
/// ```
/// Returns the available bytes as `u64`, or an `Err` describing the parse failure.
fn parse_df_avail_bytes(stdout: &str) -> Result<u64, String> {
    let data_line = stdout
        .lines()
        .nth(1)
        .ok_or_else(|| "df output contained no data line (only header or empty)".to_string())?;

    let trimmed = data_line.trim();
    trimmed
        .parse::<u64>()
        .map_err(|e| format!("could not parse {:?} as available bytes: {e}", trimmed))
}
```

**Key behaviours of the helper:**

| Input | Output |
|---|---|
| `"     Avail\n 1073741824\n"` | `Ok(1073741824)` |
| `"     Avail\n    0\n"` | `Ok(0)` — genuine full disk, not an error |
| `"     Avail\n"` (no data line) | `Err("df output contained no data line ...")` |
| `""` (empty stdout) | `Err("df output contained no data line ...")` |
| `"     Avail\nnot_a_number\n"` | `Err("could not parse \"not_a_number\" as available bytes: ...")` |
| `"     Avail\n1,234,567\n"` (locale comma) | `Err("could not parse \"1,234,567\" as available bytes: ...")` |

### 4.4 Updated `check_disk_space()` — Specification

Replace the existing `check_disk_space()` function body with the following logic:

```rust
fn check_disk_space() -> CheckResult {
    match Command::new("df")
        .args(["--output=avail", "-B1", "/"])
        .output()
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            match parse_df_avail_bytes(&stdout) {
                Err(reason) => CheckResult {
                    name: "Sufficient disk space".into(),
                    passed: false,
                    message: format!("Could not parse disk space output: {reason}"),
                },
                Ok(avail_bytes) => {
                    let avail_gb = avail_bytes / (1024 * 1024 * 1024);
                    let required_gb: u64 = 10;
                    if avail_gb >= required_gb {
                        CheckResult {
                            name: "Sufficient disk space".into(),
                            passed: true,
                            message: format!("{avail_gb} GB available"),
                        }
                    } else {
                        CheckResult {
                            name: "Sufficient disk space".into(),
                            passed: false,
                            message: format!(
                                "Only {avail_gb} GB available, {required_gb} GB recommended"
                            ),
                        }
                    }
                }
            }
        }
        Err(e) => CheckResult {
            name: "Sufficient disk space".into(),
            passed: false,
            message: format!("Could not check: {e}"),
        },
    }
}
```

**Result message mapping after the fix:**

| Situation | `passed` | `message` |
|---|---|---|
| `df` binary not found | `false` | `"Could not check: No such file or directory (os error 2)"` |
| `df` runs but output is empty or format unexpected | `false` | `"Could not parse disk space output: df output contained no data line ..."` |
| `df` returns non-numeric value | `false` | `"Could not parse disk space output: could not parse \"X\" as available bytes: ..."` |
| Disk is genuinely full (0 bytes) | `false` | `"Only 0 GB available, 10 GB recommended"` |
| Disk has 5 GB available | `false` | `"Only 5 GB available, 10 GB recommended"` |
| Disk has ≥ 10 GB available | `true` | `"15 GB available"` *(example)* |

---

## 5. Implementation Steps

### Step 1 — Add the helper function

Insert `parse_df_avail_bytes` immediately **before** `check_disk_space()` in
`src/upgrade.rs`.

**Exact insertion point:** After the closing `}` of `check_packages_up_to_date()` and
before `fn check_disk_space() -> CheckResult {`.

No new imports required — the helper uses only `str::parse`, `Option::ok_or_else`, and
`Result::map_err`, all from the standard library.

### Step 2 — Replace `check_disk_space()` body

Replace the existing `check_disk_space()` implementation with the specification in
§4.4 above.

**Exact change:** The `.unwrap_or(0)` parse chain (approximately 4 lines) is removed and
replaced with a `match parse_df_avail_bytes(&stdout) { ... }` block.

No changes to function signature, visibility, return type, or call sites.
No changes to `CheckResult` struct.
No changes to `run_prerequisite_checks()`.
No changes to `src/ui/upgrade_page.rs`.

### Step 3 — Add unit tests in the existing `#[cfg(test)]` module

The existing test module in `src/upgrade.rs` (lines 527–570) tests `validate_hostname`.
Append tests for `parse_df_avail_bytes` to the same module.

**Tests to add:**

```rust
#[test]
fn parse_df_avail_bytes_normal_output() {
    let output = "     Avail\n 1073741824\n";
    assert_eq!(parse_df_avail_bytes(output), Ok(1_073_741_824));
}

#[test]
fn parse_df_avail_bytes_zero_is_ok() {
    // A completely full disk reports 0 — this should succeed, not error
    let output = "     Avail\n    0\n";
    assert_eq!(parse_df_avail_bytes(output), Ok(0));
}

#[test]
fn parse_df_avail_bytes_empty_output_is_error() {
    assert!(parse_df_avail_bytes("").is_err());
    let err = parse_df_avail_bytes("").unwrap_err();
    assert!(
        err.contains("no data line"),
        "expected 'no data line' in: {err}"
    );
}

#[test]
fn parse_df_avail_bytes_header_only_is_error() {
    let output = "     Avail\n";
    // nth(1) returns None — only one line (the newline creates no second line)
    // because "Avail\n".lines() yields exactly ["     Avail"]
    assert!(parse_df_avail_bytes(output).is_err());
}

#[test]
fn parse_df_avail_bytes_non_numeric_is_error() {
    let output = "     Avail\nnot_a_number\n";
    assert!(parse_df_avail_bytes(output).is_err());
    let err = parse_df_avail_bytes(output).unwrap_err();
    assert!(
        err.contains("not_a_number"),
        "error message should include the bad value: {err}"
    );
}

#[test]
fn parse_df_avail_bytes_locale_commas_is_error() {
    // Some locales may produce "1,234,567" — this must not silently produce 1
    let output = "     Avail\n1,234,567\n";
    assert!(parse_df_avail_bytes(output).is_err());
}
```

> **Note on `header_only_is_error` behaviour:**
> `"     Avail\n".lines()` uses Rust's `str::lines()` which yields `["     Avail"]` (one
> element, trailing newline is not a line separator that creates an empty line — it is
> consumed). Therefore `.nth(1)` returns `None` and the test correctly expects `Err`.
> This matches `df` behaviour when it outputs a header line but no data row.

---

## 6. Risks and Mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| Genuine zero-byte disk (100% full) now produces a different message from parse failure | Low | Already handled — `Ok(0)` maps to `"Only 0 GB available..."`, while `Err(...)` maps to `"Could not parse disk space output: ..."`. They are now distinct. |
| `parse_df_avail_bytes` visibility | None | Declared as private (`fn`, not `pub fn`). Only tested internally via `#[cfg(test)]`. |
| Breaking change to `CheckResult` struct or callers | None | `CheckResult` struct unchanged; `run_prerequisite_checks()` unchanged; `check_disk_space()` signature unchanged. |
| Test for `header_only` case may be fragile re: `str::lines()` semantics | Low | Verified: Rust's `str::lines()` strips a trailing `\n` and does NOT produce a trailing empty line. The test is correct. |
| Change introduces a non-`u64` type or overflow | None | The helper still returns `u64`; `avail_bytes / (1024 * 1024 * 1024)` truncates to integer GB, same as before. |

---

## 7. Files to Modify

| File | Change |
|---|---|
| `src/upgrade.rs` | Add `parse_df_avail_bytes` helper; replace `check_disk_space()` body; add 6 unit tests to `mod tests` |

No other files require modification.

---

## 8. Dependencies

No new crate dependencies. All changes use only the Rust standard library and the
existing `std::process::Command` invocation already present.

---

## 9. Summary

- **Root cause:** `.unwrap_or(0)` in the `df` output parse chain silently treats parse
  failure identically to a genuine zero-bytes reading, producing the misleading message
  `"Only 0 GB available, 10 GB recommended"` when `df` output is absent or malformed.

- **Recommended fix (Option A):** Extract `parse_df_avail_bytes(stdout: &str) ->
  Result<u64, String>` helper and replace `unwrap_or(0)` with a `match` on that `Result`.
  Parse failures produce `"Could not parse disk space output: <reason>"`. Genuine low-disk
  failures retain the existing `"Only X GB available, Y GB recommended"` message.

- **Unit tests:** 6 new tests added to the existing `mod tests` block, covering normal
  output, genuine zero, empty output, header-only, non-numeric data, and locale-formatted
  comma numbers.

- **No struct or API changes** — `CheckResult` remains a 2-state struct; all call sites
  are unaffected.
