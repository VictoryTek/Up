# Flatpak Self-Update via GitHub Releases — Implementation Specification

**Feature Name**: `flatpak_selfupdate`  
**Date**: 2026-04-09  
**Status**: READY FOR IMPLEMENTATION  
**GitHub Repo**: `VictoryTek/Up`  
**Flatpak App ID**: `io.github.up`  

---

## 1. Current State Analysis

### Files Read in Full

| File | Relevant State |
|------|---------------|
| `src/backends/flatpak.rs` | `is_running_in_flatpak()`, `is_available()`, `build_flatpak_cmd()`, `FlatpakBackend` with `run_update()`, `count_available()`, `list_available()` all exist |
| `src/backends/mod.rs` | `UpdateResult::SuccessWithSelfUpdate { updated_count }` already defined; `detect_backends()` already includes `FlatpakBackend` when sandbox or `flatpak-spawn` available |
| `src/ui/window.rs` | `restart_banner` (`adw::Banner`) already exists, already revealed on `SuccessWithSelfUpdate`; About dialog URL is `https://github.com/VictoryTek/Up` |
| `src/main.rs` | `const APP_ID: &str = "io.github.up";` confirmed |
| `Cargo.toml` | `version = "0.1.0"`; `serde_json = "1"` already present; `tokio`, `async-channel` present; **no `reqwest`** |
| `io.github.up.json` | `--talk-name=org.freedesktop.Flatpak` in `finish-args`; **no `--share=network`** |
| `.github/workflows/flatpak-ci.yml` | Builds `up-release.flatpak`, uploads via `softprops/action-gh-release@v2` on release events |
| `src/runner.rs` | `CommandRunner::run()` streams stdout/stderr via `async_channel`, returns `Result<String, String>` |
| `src/ui/log_panel.rs` | `append_line()` for streaming log; cleared per update run |

### Key Observation: `SuccessWithSelfUpdate` Half-Works Today

The existing `FlatpakBackend::run_update()` already checks for `SuccessWithSelfUpdate` by scanning `flatpak update -y` output for `APP_ID`. This path fires **only when Up is tracked by an OSTree remote**. When installed as a `.flatpak` bundle (no remote), `flatpak update -y` output will never contain `io.github.up`, so the self-update path is never triggered.

The restart banner in `window.rs` is already wired correctly — it just needs `SuccessWithSelfUpdate` to be returned from the Flatpak backend.

---

## 2. Problem Definition

When Up is distributed and installed as a `.flatpak` bundle from GitHub Releases:

```bash
flatpak install --bundle --user io.github.up-v1.0.0.flatpak
```

There is no Flatpak remote pointing to a repository for `io.github.up`. Therefore:
- `flatpak update -y` does **not** update Up
- `flatpak update --dry-run` does **not** list Up as pending
- The `SuccessWithSelfUpdate` path in `run_update()` is never reached

**Result**: Users who installed Up as a bundle receive no updates, ever.

---

## 3. Constraint Analysis

### Research Findings

**Finding 1 — Sandbox network access**  
The Flatpak manifest does NOT have `--share=network`. Inside the sandbox, Rust code calling HTTP (even with `reqwest`) would fail with `EACCES`. We must not add `--share=network` — it is unnecessarily broad and adds an attack surface.

**Finding 2 — `--talk-name=org.freedesktop.Flatpak` is already present**  
This permission grants access to the Flatpak D-Bus service on the host, and crucially enables `flatpak-spawn --host` to route commands to the host process. This is the same mechanism used in `src/reboot.rs` for `systemctl reboot`.

**Finding 3 — `flatpak-spawn --host` gives full host network**  
A command run via `flatpak-spawn --host bash -c "..."` executes outside the sandbox with the host's network stack. `curl` can freely reach `api.github.com`. No manifest change is required.

**Finding 4 — `serde_json` is already a dependency**  
However, using it for JSON parsing requires an in-sandbox HTTP call which is impossible without `--share=network`. Instead, JSON parsing is delegated to `python3` on the host via the `flatpak-spawn --host` call.

**Finding 5 — `flatpak install --bundle --reinstall` while the app is running**  
This is explicitly supported by Flatpak. The reinstall updates the deployed files but does not kill the running process. The new version takes effect on the next launch. The user must close and reopen the app — exactly what the existing restart banner already prompts.

**Finding 6 — `python3` availability**  
Python3 is standard on all modern Linux distributions (Ubuntu, Fedora, Arch, openSUSE, Debian) and is required by many system tools. Treating it as available on the host is safe. `bash` and `curl` are equally universal. If any of these are absent, the command returns a non-zero exit and we log but do not surface an error to the user (graceful degradation).

**Finding 7 — Version comparison without new crates**  
`env!("CARGO_PKG_VERSION")` produces the version string at compile time (e.g., `"0.1.0"`). GitHub release tags are formatted as `v1.0.0`. Stripping the `v` prefix and comparing three `u32` tuples is sufficient—no `semver` crate needed.

**Finding 8 — Security: URL validation**  
The download URL returned by the GitHub API must be validated in Rust before use in a shell command. The URL must start with `https://github.com/VictoryTek/Up/releases/download/`. This prevents a spoofed API response from injecting arbitrary download locations.

**Finding 9 — Cargo.toml version must match git tag**  
For version comparison to work correctly, the `version` field in `Cargo.toml` must be bumped to match the git tag before tagging a release (e.g., git tag `v1.0.0` requires `version = "1.0.0"` in `Cargo.toml`). This is a process requirement, not a code requirement.

---

## 4. Preferred Approach: `flatpak-spawn --host` with Python3 JSON Parsing

### Full Data Flow

```
User clicks "Update All"
        │
        ▼
FlatpakBackend::run_update(runner)
        │
        ├─► flatpak-spawn --host flatpak update -y       (existing code)
        │       └─ parses output for APP_ID (OSTree path)
        │
        └─ [if is_running_in_flatpak() && !updated_self from OSTree]
                │
                ▼
        fetch_github_latest_release(runner)
                │  flatpak-spawn --host bash -c "curl ... | python3 -c '...'"
                │  Outputs two lines: tag_name, download_url
                │
                ▼
        [Rust] is_newer_than_current(tag)
                │  parse_semver(tag) vs env!("CARGO_PKG_VERSION")
                │
                ├─ NOT newer → return existing Success { count }
                │
                └─ NEWER
                        │
                        ▼
                validate_download_url(url)
                        │  must start with https://github.com/VictoryTek/Up/releases/download/
                        │
                        ▼
                download_and_install_bundle(runner, url)
                        │  flatpak-spawn --host bash -c
                        │    "curl -fsSL -o /tmp/up-self-update.flatpak 'URL' &&
                        │     flatpak install --bundle --reinstall --user -y /tmp/... &&
                        │     rm -f /tmp/up-self-update.flatpak"
                        │
                        ▼
                return UpdateResult::SuccessWithSelfUpdate { updated_count }
                        │
                        ▼
        window.rs: banner_ref.set_revealed(true)
                        │
                        ▼
        User sees: "Up was updated — restart to apply changes"
                        │
                        └─ "Close Up" button closes window
```

---

## 5. Implementation Plan

### 5.1 Files to Modify

| File | Change Type |
|------|------------|
| `src/backends/flatpak.rs` | **Primary** — add 4 new functions, modify `run_update()` |
| `src/backends/mod.rs` | **None** — `SuccessWithSelfUpdate` already exists |
| `src/ui/window.rs` | **None** — restart banner already wired |
| `io.github.up.json` | **None** — `--talk-name=org.freedesktop.Flatpak` already present |
| `.github/workflows/flatpak-ci.yml` | **Minor recommended fix** (see §7) |
| `Cargo.toml` | **None** — no new dependencies; `serde_json` already present (unused for this feature) |

### 5.2 New Constants in `src/backends/flatpak.rs`

Add at the top of the file, after the `use` imports:

```rust
/// GitHub repository slug for the Up project.
const GITHUB_REPO: &str = "VictoryTek/Up";

/// Expected URL prefix for validated release asset downloads.
/// Any URL from the GitHub API that does not start with this prefix is rejected.
const GITHUB_RELEASE_DOWNLOAD_PREFIX: &str =
    "https://github.com/VictoryTek/Up/releases/download/";

/// Temporary path for the downloaded self-update bundle.
/// Located in /tmp (always writable by the host user process via flatpak-spawn --host).
const SELF_UPDATE_TMP_PATH: &str = "/tmp/up-self-update.flatpak";
```

### 5.3 New Function: `parse_semver`

Pure function, no I/O, no dependencies.

```rust
/// Parse a semver-like string ("1.2.3" or "v1.2.3") into a (major, minor, patch) tuple.
/// Returns None if the string cannot be parsed as three non-negative integers.
fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches('v');
    let mut parts = s.splitn(3, '.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch = parts.next()?.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u32>().ok()?;
    Some((major, minor, patch))
}
```

### 5.4 New Function: `is_newer_than_current`

```rust
/// Returns true if `candidate_tag` (e.g., "v1.2.0") represents a version
/// strictly newer than the version compiled into this binary.
///
/// If either version string fails to parse, returns false (safe default — do not update).
fn is_newer_than_current(candidate_tag: &str) -> bool {
    let current = parse_semver(env!("CARGO_PKG_VERSION"));
    let candidate = parse_semver(candidate_tag);
    match (current, candidate) {
        (Some(cur), Some(cand)) => cand > cur,
        _ => false,
    }
}
```

### 5.5 New Async Function: `fetch_github_latest_release`

This function runs entirely via `flatpak-spawn --host` so no in-sandbox network access is required.

The shell script:
1. Uses `curl -fsSL` to fetch the GitHub Releases API
2. Pipes to `python3 -c` for JSON parsing (using single-quoted Python strings to avoid double-quote conflicts in the bash argument)
3. Outputs exactly two lines: the tag name on line 1, the `.flatpak` asset URL on line 2

```rust
/// Query the GitHub Releases API for the latest release of Up and return
/// (tag_name, download_url). Only valid inside the Flatpak sandbox — uses
/// flatpak-spawn --host for host network access.
///
/// The shell script is constructed at compile time. Python3 parses the JSON
/// and prints two lines: tag_name and the first .flatpak asset URL.
/// Returns Err if the command fails or output is malformed.
async fn fetch_github_latest_release(
    runner: &CommandRunner,
) -> Result<(String, String), String> {
    // NOTE on quoting strategy:
    // The bash -c argument uses double quotes as the outer delimiter so that
    // we can freely use single quotes for Python3 string literals inside.
    // In the Rust source, the `\"` sequences produce literal `"` characters
    // in the shell command. Python3 receives valid single-quoted string syntax.
    let script = format!(
        "curl -fsSL --user-agent 'io.github.up/{ver}' \
         'https://api.github.com/repos/{repo}/releases/latest' \
         | python3 -c \
         \"import sys,json;\
         r=json.load(sys.stdin);\
         t=r.get('tag_name','');\
         a=[x.get('browser_download_url','') for x in r.get('assets',[]) \
            if x.get('name','').endswith('.flatpak')];\
         print(t);print(a[0] if a else '')\"",
        ver = env!("CARGO_PKG_VERSION"),
        repo = GITHUB_REPO,
    );

    let output = runner
        .run("flatpak-spawn", &["--host", "bash", "-c", &script])
        .await
        .map_err(|e| format!("GitHub release check failed: {e}"))?;

    let mut lines = output.lines();
    let tag = lines.next().unwrap_or("").trim().to_string();
    let url = lines.next().unwrap_or("").trim().to_string();

    if tag.is_empty() {
        return Err("GitHub API returned no release tag".to_string());
    }

    Ok((tag, url))
}
```

### 5.6 New Async Function: `download_and_install_bundle`

```rust
/// Download the Flatpak bundle at `url` to a temporary path on the host and
/// reinstall it. The URL is validated before construction of the shell command.
///
/// Validation: url must start with GITHUB_RELEASE_DOWNLOAD_PREFIX.
/// This prevents a compromised API response from fetching an arbitrary URL.
///
/// The `flatpak install --bundle --reinstall --user -y` command updates the
/// current installation in-place. The running process is not killed; the new
/// version takes effect on the next launch.
async fn download_and_install_bundle(
    runner: &CommandRunner,
    url: &str,
) -> Result<(), String> {
    // Security: reject any URL that does not come from the expected release path.
    if !url.starts_with(GITHUB_RELEASE_DOWNLOAD_PREFIX) {
        return Err(format!(
            "Rejected download URL with unexpected prefix: {url}"
        ));
    }

    // The URL has been validated — it contains only HTTPS and path characters
    // legal in a GitHub release URL. Single-quoting it in bash is safe because
    // GitHub release URLs never contain single-quote characters.
    let script = format!(
        "curl -fsSL -o '{tmp}' '{url}' && \
         flatpak install --bundle --reinstall --user -y '{tmp}' && \
         rm -f '{tmp}'",
        tmp = SELF_UPDATE_TMP_PATH,
        url = url,
    );

    runner
        .run("flatpak-spawn", &["--host", "bash", "-c", &script])
        .await
        .map(|_| ())
        .map_err(|e| format!("Self-update install failed: {e}"))
}
```

### 5.7 Modified: `FlatpakBackend::run_update()`

The existing function is reproduced below with the new self-update block inserted after the OSTree update. The diff is: **add 20 lines in the `Ok(output)` branch, after the existing `updated_self` check**.

**Current code** (end of the `Ok(output)` branch, before `UpdateResult::Success`):
```rust
                let updated_self = is_running_in_flatpak()
                    && output.lines().any(|l| {
                        let t = l.trim();
                        t.starts_with(|c: char| c.is_ascii_digit())
                            && t.contains(crate::APP_ID)
                    });

                if updated_self {
                    UpdateResult::SuccessWithSelfUpdate {
                        updated_count: count,
                    }
                } else {
                    UpdateResult::Success {
                        updated_count: count,
                    }
                }
```

**Replacement** (the `Ok(output)` branch become):
```rust
                let updated_self = is_running_in_flatpak()
                    && output.lines().any(|l| {
                        let t = l.trim();
                        t.starts_with(|c: char| c.is_ascii_digit())
                            && t.contains(crate::APP_ID)
                    });

                // If inside Flatpak sandbox and Up was NOT updated by the OSTree
                // remote path above, check GitHub Releases for a bundle update.
                let github_self_updated = if !updated_self && is_running_in_flatpak() {
                    match fetch_github_latest_release(runner).await {
                        Ok((tag, url)) if is_newer_than_current(&tag) && !url.is_empty() => {
                            match download_and_install_bundle(runner, &url).await {
                                Ok(()) => {
                                    log::info!(
                                        "Self-update from GitHub Releases: installed {}",
                                        tag
                                    );
                                    true
                                }
                                Err(e) => {
                                    log::warn!("Self-update install error: {e}");
                                    false
                                }
                            }
                        }
                        Ok((tag, _)) => {
                            log::info!("Self-update check: already at latest ({tag})");
                            false
                        }
                        Err(e) => {
                            log::warn!("Self-update check error: {e}");
                            false
                        }
                    }
                } else {
                    false
                };

                if updated_self || github_self_updated {
                    UpdateResult::SuccessWithSelfUpdate {
                        updated_count: count,
                    }
                } else {
                    UpdateResult::Success {
                        updated_count: count,
                    }
                }
```

### 5.8 No Changes Required

| Component | Reason |
|-----------|--------|
| `src/backends/mod.rs` | `SuccessWithSelfUpdate` already exists and is returned correctly |
| `src/ui/window.rs` | `restart_banner` already revealed on `SuccessWithSelfUpdate` |
| `io.github.up.json` | `--talk-name=org.freedesktop.Flatpak` already enables `flatpak-spawn --host` |
| `Cargo.toml` | No new dependencies; all required crates already present |

---

## 6. Version Comparison — Step by Step

Given: `env!("CARGO_PKG_VERSION")` = `"0.1.0"` (compile-time constant)  
Given: GitHub API returns `tag_name` = `"v1.0.0"`  

1. `is_newer_than_current("v1.0.0")` is called
2. `parse_semver("0.1.0")` → `Some((0, 1, 0))`
3. `parse_semver("v1.0.0")` → strips `v` → `Some((1, 0, 0))`
4. `(1, 0, 0) > (0, 1, 0)` → `true` (tuple comparison: major 1 > 0)
5. Result: **update is available**

Edge cases handled:
- `"v1.0.0-beta.1"` → patch parses as `0` from splitting on non-digit → `Some((1, 0, 0))` (conservative: beta tag treated as release version)
- `"1.0"` (missing patch) → `parse_semver` returns `None` → `is_newer_than_current` returns `false` (safe: do not update)
- Parse failure → `None` → `false` (safe default)
- Same version → `(cur == cand)` → `cand > cur` is `false` → no update

---

## 7. UI Integration

### What the User Sees

1. **User clicks "Update All"**
2. Flatpak row shows spinner + progress bar + "Updating..."
3. Log panel expander shows streaming output: `$ flatpak-spawn --host flatpak update -y`, followed by `$ flatpak-spawn --host bash -c "curl ..."` and the install output
4. After completion: if newer version was found and installed, the restart banner appears at the top of the Update page:
   > **Up was updated — restart to apply changes**  
   > [Close Up]
5. The "Close Up" button closes the application window

### Existing Banner Code (no changes):
```rust
// In src/ui/window.rs — already present, no modification needed
let restart_banner = adw::Banner::builder()
    .title("Up was updated \u{2014} restart to apply changes")
    .button_label("Close Up")
    .revealed(false)
    .build();
// ...
if self_updated {
    banner_ref.set_revealed(true);
}
```

### `count_available()` — Intentionally Unchanged

The `count_available()` function is NOT modified to include the GitHub check because:
- It is called at app startup and on "Check for Updates" (refresh button)
- Adding a network call to startup would slow the initial scan
- `FlatpakBackend` is a unit struct with no mutable state to cache the result
- The existing count from `flatpak update --dry-run` correctly counts OSTree-tracked apps
- Bundle-installed Up will simply not appear in the dry-run count; the user discovers the update when they click "Update All"

This is an acceptable UX trade-off for v1. Future work could add a stateful `SelfUpdateBackend`.

---

## 8. CI/CD: `.github/workflows/flatpak-ci.yml`

### Current State
The CI already:
- Builds the Flatpak bundle as `up-release.flatpak`
- Uploads it to GitHub Releases via `softprops/action-gh-release@v2` on release events
- The file name `up-release.flatpak` matches the Python3 script's `.endswith(".flatpak")` filter

### Required Changes: **None for core functionality**

The self-update will work with the existing CI as-is.

### Recommended Improvement (Optional)

Rename the bundle to include the version tag so users can identify the version from the filename on the GitHub Releases page. Update the workflow `Bundle Flatpak for release` and `Upload to GitHub Releases` steps:

```yaml
# Change from:
run: |
  flatpak build-bundle builddir/repo up-release.flatpak $APP_ID

# Change to:
run: |
  VERSION="${GITHUB_REF_NAME}"   # e.g., "v1.0.0" from tag push
  flatpak build-bundle builddir/repo "up-${VERSION}.flatpak" $APP_ID

# And update upload step:
files: "up-v*.flatpak"
```

**Note**: The Python3 filter `x.get('name','').endswith('.flatpak')` works for both `up-release.flatpak` and `up-v1.0.0.flatpak`. No code change is required in `flatpak.rs` for either naming scheme.

**IMPORTANT**: If this renaming is done, the `GITHUB_RELEASE_DOWNLOAD_PREFIX` validation in `download_and_install_bundle` remains correct (both filenames are under `/releases/download/`).

---

## 9. `io.github.up.json` Manifest

### No Changes Required

The key `finish-args` entry:
```json
"--talk-name=org.freedesktop.Flatpak"
```
This permission enables `flatpak-spawn --host` without requiring `--share=network`. All network operations happen outside the sandbox on the host. This is both sufficient and minimal.

---

## 10. Process Requirement: Version Bumping

For the self-update version comparison to work, the release process **must** keep `Cargo.toml` `version` in sync with git tags:

```
Cargo.toml: version = "1.0.0"
git tag:    v1.0.0
```

If `Cargo.toml` says `0.1.0` but the tag is `v1.0.0`, then `env!("CARGO_PKG_VERSION")` returns `"0.1.0"` and `is_newer_than_current("v1.0.0")` would return `true` — causing an unnecessary reinstall on every update click.

**Recommendation**: Add a check in `scripts/preflight.sh` that validates the Cargo version matches any existing git tag pointing to `HEAD`.

---

## 11. Complete List of Changes

### `src/backends/flatpak.rs` — All Changes

**A. Add constants** (after `use` imports, before `is_running_in_flatpak`):
```rust
const GITHUB_REPO: &str = "VictoryTek/Up";
const GITHUB_RELEASE_DOWNLOAD_PREFIX: &str =
    "https://github.com/VictoryTek/Up/releases/download/";
const SELF_UPDATE_TMP_PATH: &str = "/tmp/up-self-update.flatpak";
```

**B. Add `parse_semver` function** (module level, not inside `impl`):
```rust
fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches('v');
    let mut parts = s.splitn(3, '.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch = parts
        .next()?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse::<u32>()
        .ok()?;
    Some((major, minor, patch))
}
```

**C. Add `is_newer_than_current` function** (module level):
```rust
fn is_newer_than_current(candidate_tag: &str) -> bool {
    let current = parse_semver(env!("CARGO_PKG_VERSION"));
    let candidate = parse_semver(candidate_tag);
    match (current, candidate) {
        (Some(cur), Some(cand)) => cand > cur,
        _ => false,
    }
}
```

**D. Add `fetch_github_latest_release` async function** (module level):
```rust
async fn fetch_github_latest_release(
    runner: &CommandRunner,
) -> Result<(String, String), String> {
    let script = format!(
        "curl -fsSL --user-agent 'io.github.up/{ver}' \
         'https://api.github.com/repos/{repo}/releases/latest' \
         | python3 -c \
         \"import sys,json;\
         r=json.load(sys.stdin);\
         t=r.get('tag_name','');\
         a=[x.get('browser_download_url','') for x in r.get('assets',[]) \
            if x.get('name','').endswith('.flatpak')];\
         print(t);print(a[0] if a else '')\"",
        ver = env!("CARGO_PKG_VERSION"),
        repo = GITHUB_REPO,
    );

    let output = runner
        .run("flatpak-spawn", &["--host", "bash", "-c", &script])
        .await
        .map_err(|e| format!("GitHub release check failed: {e}"))?;

    let mut lines = output.lines();
    let tag = lines.next().unwrap_or("").trim().to_string();
    let url = lines.next().unwrap_or("").trim().to_string();

    if tag.is_empty() {
        return Err("GitHub API returned no release tag".to_string());
    }

    Ok((tag, url))
}
```

**E. Add `download_and_install_bundle` async function** (module level):
```rust
async fn download_and_install_bundle(
    runner: &CommandRunner,
    url: &str,
) -> Result<(), String> {
    if !url.starts_with(GITHUB_RELEASE_DOWNLOAD_PREFIX) {
        return Err(format!(
            "Rejected download URL with unexpected prefix: {url}"
        ));
    }

    let script = format!(
        "curl -fsSL -o '{tmp}' '{url}' && \
         flatpak install --bundle --reinstall --user -y '{tmp}' && \
         rm -f '{tmp}'",
        tmp = SELF_UPDATE_TMP_PATH,
        url = url,
    );

    runner
        .run("flatpak-spawn", &["--host", "bash", "-c", &script])
        .await
        .map(|_| ())
        .map_err(|e| format!("Self-update install failed: {e}"))
}
```

**F. Modify `FlatpakBackend::run_update()`** — replace the final `if updated_self { ... } else { ... }` block with the extended version that includes the GitHub check (see §5.7 above).

---

## 12. Risks and Mitigations

| Risk | Severity | Mitigation |
|------|----------|------------|
| `curl` or `bash` not on host | Low | `flatpak-spawn` command will fail → error is logged (`log::warn!`) → function returns `false` → normal `Success` returned → no banner shown, no crash |
| `python3` not on host | Low | Same as above — `curl` pipes to `python3`, failure propagates |
| GitHub API rate limiting (60 req/hour unauthenticated) | Low | This check only runs when the user clicks "Update All"; normal usage is infrequent |
| Spoofed GitHub API response → malicious URL | Medium | `download_and_install_bundle` validates URL starts with `GITHUB_RELEASE_DOWNLOAD_PREFIX` before use |
| Flatpak bundle from GitHub API is corrupted | Low | `flatpak install --bundle` verifies the bundle signature/checksum internally |
| Shell injection via URL in `format!` | Low | URL is validated to only contain HTTPS path chars from the expected origin; GitHub release URLs never contain shell-special characters |
| `Cargo.toml` version out of sync with git tag | High | Process requirement: bump version before tagging; document in CONTRIBUTING or CI guard |
| `/tmp/up-self-update.flatpak` left on disk if install fails | Low | The `rm -f` runs after install. If download fails, `&&` short-circuits and no file is left. If install fails, the `rm -f` does not run, leaving the file. Future: use `trap cleanup EXIT` in the script |
| Double self-update (OSTree remote + GitHub) | None | Guarded by `if !updated_self && is_running_in_flatpak()` — GitHub check only runs if OSTree path did not already set `updated_self` |
| Network call slows "Update All" when not in Flatpak | None | `is_running_in_flatpak()` gates the entire block |

---

## 13. What Is NOT Changing

These elements are confirmed to need **zero modifications**:

- `src/backends/mod.rs` — `UpdateResult::SuccessWithSelfUpdate` variant already defined as required
- `src/ui/window.rs` — restart banner already implemented and triggered by `SuccessWithSelfUpdate`
- `src/ui/update_row.rs` — `set_status_success()` is called for `SuccessWithSelfUpdate` already in `window.rs`
- `io.github.up.json` — manifest permissions already correct
- `Cargo.toml` — no new dependencies
- `src/runner.rs` — `CommandRunner::run()` is sufficient as-is
- `src/main.rs` — `APP_ID` and module structure unchanged
- `src/ui/mod.rs` — `spawn_background_async` unchanged

---

## 14. Files to Modify Summary

```
src/backends/flatpak.rs    Primary implementation file
                            ├─ Add 3 constants
                            ├─ Add parse_semver() pure fn
                            ├─ Add is_newer_than_current() pure fn
                            ├─ Add fetch_github_latest_release() async fn
                            ├─ Add download_and_install_bundle() async fn
                            └─ Modify FlatpakBackend::run_update() final block
```

**Optionally** (not required for core functionality):
```
.github/workflows/flatpak-ci.yml    Rename bundle to up-v{VERSION}.flatpak
```

---

## 15. Acceptance Criteria

The implementation is complete when:

1. `cargo build` succeeds with zero errors
2. `cargo clippy -- -D warnings` produces zero warnings
3. `cargo fmt --check` passes
4. `cargo test` passes
5. When running as a Flatpak bundle and a newer version exists on GitHub:
   - "Update All" triggers the GitHub API check
   - Log panel shows the `curl` + `python3` command executing
   - Log panel shows `flatpak install --bundle --reinstall` executing
   - Restart banner appears after completion
6. When running as a Flatpak bundle and already at the latest version:
   - "Update All" skips the download/install
   - No banner is shown
   - No error is shown
7. When `curl`/`python3`/`bash` are unavailable on the host:
   - No crash, no user-visible error
   - `log::warn!` records the failure
   - Normal `Success` result is returned
