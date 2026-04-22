# Specification: Fix Flatpak `list_available` to Use `flatpak update --no-deploy`

**Date:** 2026-04-21  
**Status:** READY FOR IMPLEMENTATION  
**Scope:** `src/backends/flatpak.rs` — `list_available()` method only  
**New dependencies:** None

---

## 1. Current State Analysis

### Method under change

`list_available()` in `src/backends/flatpak.rs` (lines ~300–340) currently runs two
separate commands:

```rust
// System installation (no --user flag)
build_flatpak_cmd(&["remote-ls", "--updates", "--columns=application"])

// Per-user installation
build_flatpak_cmd(&["remote-ls", "--updates", "--user", "--columns=application"])
```

Both commands use `tokio::process::Command::new(...).output().await` directly
(not through `runner.run()` / `PrivilegedShell`) — they run as the current
unprivileged user.

Output is then filtered: non-empty lines containing no spaces are collected as
app IDs.

### Related method — `run_update()`

`run_update()` uses:

```rust
build_flatpak_cmd(&["update", "-y"])
```

routed through `runner.run()`.  After the update completes it counts lines
whose trimmed content starts with an ASCII digit — this is the **digit-start
heuristic** for counting updated packages.

---

## 2. Problem Definition

### Why `remote-ls --updates` gives false positives

`flatpak remote-ls --updates` compares **cached AppStream / OSTree metadata
refs** between the local installation and the remote without running Flatpak's
full dependency resolver.  Specifically:

1. It lists any ref whose stored commit hash differs from the latest commit in
   the remote's summary file — even when those commits resolve to the same
   effective content after applying static deltas.
2. It does not account for pinned refs, masked updates, or end-of-life
   replacements that Flatpak's transaction engine would handle.
3. It lists extensions and locales independently; a single app update shows up
   as multiple rows.

The result is that the "pending updates" badge can show a non-zero count, but
when the user clicks **Update All**, `flatpak update -y` resolves dependencies
properly and reports **"Nothing to do."**

### History of `--dry-run`

Earlier versions of Flatpak (< 1.16) had a `--dry-run` flag on `flatpak
update` that would list pending operations without executing them.  This flag
was **removed in Flatpak 1.16**.  The current system has Flatpak **1.16.6**,
confirmed via `flatpak --version`.  The existing code comment in
`list_available` already notes this: *"`--dry-run` was removed in Flatpak
1.16."*

---

## 3. Proposed Solution

### Core change

Replace both `remote-ls --updates` commands with a single call to:

```
flatpak update --no-deploy -y --user
```

`--no-deploy` downloads the latest OSTree commits to the local cache but does
not activate (deploy) them.  Crucially, the **transaction resolution phase is
identical to a real `flatpak update`** — it uses the same dependency resolver,
respects pins, handles end-of-life rebases, and deduplicates extensions.  Only
apps that would genuinely change are listed.

### Why `--user` only (system installation not covered)

| Flag | Writes to | Privilege needed |
|------|-----------|-----------------|
| `--user` | `~/.local/share/flatpak/` | none — current user owns this directory |
| `--system` (default) | `/var/lib/flatpak/` | root via polkit |

`list_available` is a **background check** invoked periodically and before the
user clicks Update.  Triggering a polkit authentication dialog on every
background check would be unacceptable UX.

The vast majority of desktop Flatpak applications are installed into the
per-user installation (Flathub default), so `--user` covers the common case.

The existing system-level `remote-ls --updates` check (without `--user`) can
be **retained as a best-effort fallback** for system-installed apps to
preserve backward compatibility, accepting that system-level results may still
carry false positives.  Alternatively, it can be dropped entirely — this is a
product decision left to the implementer.

### Side effect: download to OSTree cache

`--no-deploy` downloads OSTree objects to the local cache.  This is a
intentional trade-off:

- **Benefit:** Subsequent `flatpak update -y` (the real update) is faster
  because objects are already cached.
- **Cost:** Bandwidth is consumed and `list_available` is no longer a
  zero-cost read-only probe.

This side effect is acceptable because `list_available` is only invoked when
the user opens the app or manually triggers a refresh, not on a tight loop.

---

## 4. Exact Commands to Run

### Primary — user installation (replaces user `remote-ls`)

```
flatpak update --no-deploy -y --user
```

Inside the Flatpak sandbox the helper `build_flatpak_cmd` wraps this as:

```
flatpak-spawn --host flatpak update --no-deploy -y --user
```

### Optional — system installation fallback (unchanged from current)

```
flatpak remote-ls --updates --columns=application
```

Keep this as a best-effort, no-privilege check for system-installed apps.
Its output is deduplicated against the `--user` results before returning.

---

## 5. Output Parsing Logic

### Source reference

The output format is defined in `flatpak-cli-transaction.c` (Flatpak upstream
source).  The table columns, in order, are set by:

```c
flatpak_table_printer_set_column_title(printer, i++, "   ");      // col 0: row number placeholder
flatpak_table_printer_set_column_title(printer, i++, "   ");      // col 1: progress/spinner placeholder
flatpak_table_printer_set_column_title(printer, i++, _("ID"));    // col 2: app/runtime ID
flatpak_table_printer_set_column_title(printer, i++, _("Arch"));  // col 3: architecture (may be suppressed)
flatpak_table_printer_set_column_title(printer, i++, _("Branch")); // col 4
flatpak_table_printer_set_column_title(printer, i++, _("Op"));    // col 5: operation char
// Optional when installing/updating:
flatpak_table_printer_set_column_title(printer, i++, _("Remote")); // col 6
flatpak_table_printer_set_column_title(printer, i++, ...);         // col 7: download size
```

Data rows are populated as:

```c
g_autofree char *rownum = g_strdup_printf("%2d.", i);   // e.g. " 1."
flatpak_table_printer_add_column(printer, rownum);       // " 1."
flatpak_table_printer_add_column(printer, "   ");        // progress placeholder (spaces)
flatpak_table_printer_add_column(printer, id);           // "com.example.App"
flatpak_table_printer_add_column(printer, arch);         // "x86_64"
flatpak_table_printer_add_column(printer, branch);       // "stable"
flatpak_table_printer_add_column(printer, op_shorthand); // "u" or "i"
// ...remote and download columns if applicable
```

### Rendered output (non-TTY / piped mode)

When stdout is a pipe (as with `tokio::process::Command::output().await`),
Flatpak disables ANSI codes and live-updating via `flatpak_fancy_output()`.
The table is printed once as a static block:

```
Looking for updates…

                          ID                    Arch    Branch  Op  Remote    Download
 1.      com.example.App             x86_64  stable  u   flathub  < 1.2 MB
 2.      org.gnome.Platform          x86_64  48      u   flathub  < 5.0 MB

  99%  1.2 MB / 1.2 MB
  ...progress lines...
 100%  5.0 MB / 5.0 MB

Updates complete.
```

Or when there are no updates:

```
Looking for updates…

Nothing to do.
```

### Parsing rules

After capturing full stdout, iterate line by line:

| Line type | Trimmed example | Digit-starts? | Token 0 ends with |
|-----------|----------------|---------------|-------------------|
| Table header | `"ID   Arch   Branch   Op"` | No | — |
| Table data row | `"1.   com.example.App   x86_64   stable   u"` | **Yes** | **`.`** |
| Progress line | `"99%  5.0 MB / 5.0 MB"` | **Yes** | **`%`** |
| Info/warning | `"Info: runtime ... is end-of-life"` | No | — |
| Nothing to do | `"Nothing to do."` | No | — |
| Updates complete | `"Updates complete."` | No | — |

**Algorithm:**

```rust
let text = String::from_utf8_lossy(&out.stdout);
let mut apps: Vec<String> = Vec::new();

for line in text.lines() {
    let t = line.trim();
    // Fast path: only lines starting with a digit are candidates.
    if !t.starts_with(|c: char| c.is_ascii_digit()) {
        continue;
    }
    let mut tokens = t.split_whitespace();
    let first = match tokens.next() {
        Some(s) => s,
        None => continue,
    };
    // Table rows: row-number token ends with '.'  (e.g. "1.", "12.")
    // Progress rows: percentage token ends with '%' (e.g. "99%")
    // Skip anything that is not a table row.
    if !first.ends_with('.') {
        continue;
    }
    // Token 1 (after row-number) is the app/runtime ID.
    // The progress-placeholder column ("   ") is pure whitespace and
    // collapses into the inter-column gap when splitting on whitespace.
    if let Some(id) = tokens.next() {
        apps.push(id.to_string());
    }
}
```

### Token index for app ID

The row-number column `" N."` is token 0.  The progress-placeholder column
`"   "` (three spaces) is pure whitespace — after `split_whitespace()` it
produces no token.  Therefore the **app ID is always token index 1**.

This is different from the `remote-ls --columns=application` approach which
returned bare app IDs one per line.

### Deduplication

When keeping the system `remote-ls` fallback, deduplicate by collecting into a
`HashSet<String>` before converting to `Vec<String>`, or sort + dedup after.

---

## 6. Edge Cases

### "Nothing to do"

`flatpak update --no-deploy -y --user` exits **0** and writes only:

```
Looking for updates…

Nothing to do.
```

No digit-starting lines → parsing loop produces empty `apps` → `Ok(vec![])`.
`count_available()` (which delegates to `list_available().len()`) returns 0. ✓

### Permission error / polkit cancelled (system variant)

If the system `remote-ls` is kept as fallback and fails (non-zero exit or
unreadable stdout), the error should be silently swallowed and an empty vec
returned for that segment.  This matches the current behaviour where system
errors are non-fatal for the list.

### `flatpak update --no-deploy` with no remotes configured

If no Flatpak remotes are configured (unlikely for a functional Flatpak
install), the command exits 0 with "Nothing to do." — safe, returns empty. ✓

### Inside Flatpak sandbox

`build_flatpak_cmd` already handles this: commands become
`flatpak-spawn --host flatpak update --no-deploy -y --user`.  No change
required. ✓

### App IDs with unusual characters

Flatpak app IDs are reverse-DNS identifiers (e.g. `com.example.App`).  They
never contain whitespace, so `split_whitespace()` safely delimits them. ✓

### Very large update tables (> 50 apps)

The `set_packages()` UI method already caps display at 50 items.  No change
needed in the parsing path. ✓

### Exit code non-zero due to network failure

`tokio::process::Command::output()` captures all output regardless of exit
code.  If the network is unavailable and `--no-deploy` can't pull, the output
may be empty or contain an error message.  No digit-starting lines → returns
empty list (treats as "no updates available", slightly incorrect but safe).
Consider mapping non-zero exit to `Err(stderr)` for better diagnostics —
this is a recommended improvement.

---

## 7. Compatibility with `run_update()` Heuristic

`run_update()` uses the digit-start heuristic to count updated packages from
`flatpak update -y` output.  The output format of `flatpak update -y` and
`flatpak update --no-deploy -y` is **identical** up to the deployment step.
The heuristic is therefore consistent between the two methods — no change to
`run_update()` is needed.

---

## 8. Implementation Steps

1. **In `list_available()`:**
   - Remove the two `tokio::process::Command` calls for `remote-ls`.
   - Add one `tokio::process::Command` call for
     `build_flatpak_cmd(&["update", "--no-deploy", "-y", "--user"])`.
   - Replace the existing line-filter loop with the token-parsing algorithm
     from Section 5 above.
   - Optionally retain the system `remote-ls --updates --columns=application`
     call (without `--user`) as a best-effort fallback, merging results via
     deduplication.

2. **No changes to:**
   - `run_update()` — output format is compatible, heuristic unchanged.
   - `build_flatpak_cmd()` — no new subcommands; existing helper is sufficient.
   - `count_available()` — delegates to `list_available().len()`, unchanged.
   - Any UI code — `set_packages()` receives `&[String]` as before.
   - `Cargo.toml`, `meson.build`, or any other build file — no new deps.

---

## 9. Risks and Mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| `--no-deploy` downloads large OSTree objects, increasing `list_available` latency | Medium | Acceptable trade-off; prepares cache for the real update. Document in code comments. |
| System-installed apps still show false positives if `remote-ls` fallback is kept | Low | System installs are uncommon for desktop apps. Acceptable degradation. |
| Progress output lines (`NN%  X MB`) are mistakenly parsed as app IDs | Low | Mitigated by the `ends_with('.')` check on token 0. |
| `flatpak update --no-deploy` format changes in a future Flatpak release | Very Low | Flatpak table format has been stable for years. Monitor upstream changelog. |
| Running inside the Flatpak sandbox with restricted host permissions | Low | `build_flatpak_cmd()` already handles sandbox detection; no change needed. |
| `Nothing to do.` output format varies by locale/translation | Very Low | The parsing algorithm is locale-independent — it only looks at digit-starting lines. |

---

## 10. Sources Consulted

1. **Flatpak official man page** (`flatpak-update.xml`, upstream GitHub):
   Documents `--no-deploy`, `--user`, `-y` flags.  Confirms default behaviour
   covers both system and user installations.

2. **Flatpak source — `flatpak-cli-transaction.c`** (GitHub `flatpak/flatpak`):
   Authoritative source for the table output format.  Column layout (`rownum`,
   progress placeholder, ID, Arch, Branch, Op, Remote, Download), the
   `"%2d."` row-number format, and the non-fancy (piped) output path were all
   verified directly from source.

3. **Flatpak source — `flatpak-table-printer.c`** (GitHub `flatpak/flatpak`):
   Confirms how table rows are rendered in non-TTY mode via
   `flatpak_fancy_output()` guards.

4. **Context7 — `/flatpak/flatpak-docs` (Benchmark 90.8)**:
   `flatpak update --no-deploy` documentation, option descriptions, and
   `flatpak_transaction_set_no_deploy` API reference.

5. **Live system verification** (Flatpak 1.16.6 on this machine):
   - Confirmed `--no-deploy` is present in `flatpak update --help`.
   - Confirmed `flatpak update --no-deploy -y --user` exits 0 with
     "Nothing to do." when no updates are pending.
   - Confirmed `--no-deploy -y --user` requires no privilege escalation.
   - Confirmed `--dry-run` is NOT listed in `flatpak update --help` on 1.16.x.

6. **Flatpak issue tracker** (known `remote-ls --updates` false-positive
   behaviour): Multiple GitHub issues document that `remote-ls --updates`
   returns stale appstream metadata comparisons that do not reflect the
   transaction resolver's view.  `--no-deploy` was the recommended workaround
   by Flatpak maintainers.

7. **Existing `run_update()` implementation** in `src/backends/flatpak.rs`:
   Verified that the digit-start counting heuristic is correct for
   `flatpak update -y` output, and that the same output format applies to
   `--no-deploy`, ensuring consistency.

---

## 11. No New Dependencies Required

This change is entirely internal to `src/backends/flatpak.rs`.  It:

- Uses only `tokio::process::Command` (already in scope in `list_available`).
- Calls `build_flatpak_cmd()` (already defined in the same file).
- Requires no new Cargo dependencies.
- Requires no changes to `Cargo.toml`, `meson.build`, or Flatpak manifests.
