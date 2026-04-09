# Flatpak Self-Update — Final Re-Review

**Date**: 2026-04-09  
**Reviewer**: Re-Review Subagent  
**Prior Review**: `.github/docs/subagent_docs/flatpak_selfupdate_review.md`  
**Primary File**: `src/backends/flatpak.rs`

---

## Verdict: APPROVED

All three RECOMMENDED issues from the Phase 3 review have been correctly resolved.
No new issues were introduced by the fixes.

---

## Fix Verification

### R1 — Temp file cleaned up unconditionally on install failure ✓ RESOLVED

**Review finding**: `rm -f` was guarded by `&&`, so it was skipped if `flatpak install` exited non-zero.

**Current code** (`download_and_install_bundle`, bash script format string):
```rust
"curl -fsSL --connect-timeout 10 --max-time 300 -o '{tmp}' '{url}' && \
 flatpak install --bundle --reinstall --user -y '{tmp}'; \
 rm -f '{tmp}'"
```

The `rm -f` is now separated by `;` (not `&&`), so it executes unconditionally regardless of whether the `flatpak install` step succeeds or fails. Fix is correct. ✓

---

### R2 — curl timeout flags added ✓ RESOLVED

**Review finding**: No `--connect-timeout` or `--max-time` on either curl invocation.

**GitHub API call** (`fetch_github_latest_release`, flatpak-sandbox path):
```rust
"curl -fsSL --connect-timeout 10 --max-time 30 --user-agent 'io.github.up/{ver}' \
 'https://api.github.com/repos/{repo}/releases/latest' | python3 -c ..."
```
`--connect-timeout 10 --max-time 30` present. ✓

**Bundle download call** (`download_and_install_bundle`):
```rust
"curl -fsSL --connect-timeout 10 --max-time 300 -o '{tmp}' '{url}' && ..."
```
`--connect-timeout 10 --max-time 300` present. ✓

Both timeouts are correctly and independently set, with the larger `--max-time 300` appropriately applied to the potentially large bundle download. Fix is correct. ✓

---

### R3 — Explicit single-quote guard added ✓ RESOLVED

**Review finding**: No `url.contains('\'')` guard; injection safety was implicit.

**Current code** (`download_and_install_bundle`, validation block):
```rust
// Reject URLs that do not originate from the expected GitHub release path.
if !url.starts_with(GITHUB_RELEASE_DOWNLOAD_PREFIX) {
    return Err(format!(
        "Rejected download URL with unexpected prefix: {url}"
    ));
}

// Reject URLs that contain a single-quote, which would break bash quoting.
if url.contains('\'') {
    return Err(format!(
        "Rejected download URL containing invalid character: {url}"
    ));
}
```

The `GITHUB_RELEASE_DOWNLOAD_PREFIX` guard executes first, then the single-quote guard. The security ordering is correct — a spoofed URL is rejected at the prefix check before reaching the character check. Both checks are separate `return Err(...)` paths with distinct messages, maintaining clarity. Fix is correct. ✓

---

## Additional Verification

### Security prefix order preserved ✓

`GITHUB_RELEASE_DOWNLOAD_PREFIX` is checked on the first guard. The `url.contains('\'')` check is the second guard. The URL never reaches a shell command unless both guards pass.

### `UpdateResult::SuccessWithSelfUpdate` — all match arms covered ✓

In `src/ui/window.rs`, the single `match &result` block covers all four variants:

```rust
UpdateResult::Success { updated_count } => row.set_status_success(*updated_count),
UpdateResult::SuccessWithSelfUpdate { updated_count } => {
    row.set_status_success(*updated_count);
    self_updated = true;
}
UpdateResult::Error(msg) => { row.set_status_error(msg); has_error = true; }
UpdateResult::Skipped(msg) => row.set_status_skipped(msg),
```

No match arm is missing. `SuccessWithSelfUpdate` correctly sets `self_updated = true`, which triggers `banner_ref.set_revealed(true)`. ✓

### No new issues introduced ✓

- The three edits are syntactically valid Rust (string literals, char literal `'\''`).
- No new imports, dependencies, or behavioral changes outside the three targeted fixes.
- The `--max-time 300` value for bundle download vs `--max-time 30` for the API call is appropriate and intentional.
- All pre-existing MINOR issues from Phase 3 (`Cargo.toml` repo URL, outside-sandbox read timeout, raw API output in log) remain. None were introduced by these fixes.

---

## Updated Score Table

| Category | Score | Grade | Change |
|----------|-------|-------|--------|
| Specification Compliance | 98% | A+ | — |
| Best Practices | 95% | A | ↑ (+13%) |
| Functionality | 95% | A | — |
| Code Quality | 93% | A | ↑ (+3%) |
| Security | 95% | A | ↑ (+10%) |
| Performance | 90% | A- | ↑ (+5%) |
| Consistency | 96% | A | — |
| Build Success | 92% | A- | — |

**Overall Grade: A (94%)** *(up from A- 90%)*

---

## Summary

| Fix | Status |
|-----|--------|
| R1 — `rm -f` uses `;` for unconditional cleanup | ✓ RESOLVED |
| R2 — curl API timeout `--connect-timeout 10 --max-time 30` | ✓ RESOLVED |
| R2 — curl download timeout `--connect-timeout 10 --max-time 300` | ✓ RESOLVED |
| R3 — `url.contains('\'')` guard before shell construction | ✓ RESOLVED |
| Security ordering (prefix check before single-quote check) | ✓ VERIFIED |
| `SuccessWithSelfUpdate` match arms complete | ✓ VERIFIED |
| No new issues introduced | ✓ VERIFIED |

**Verdict: APPROVED**
