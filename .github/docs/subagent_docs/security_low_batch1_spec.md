# Security Low Batch 1 — Specification

**Findings:** #5 (tokio minimal features), #6 (APT DEBIAN_FRONTEND), #8 (URL consistency)  
**Severity:** LOW (hygiene fixes only)  
**New dependencies required:** None  

---

## Finding #5 — tokio `features = ["full"]` → Minimal Feature Set

### Current State

`Cargo.toml` line:

```toml
tokio = { version = "1", features = ["full"] }
```

`"full"` compiles every tokio subsystem: networking (`net`), DNS, filesystem watcher, signal handling, `io-std`, `time`, `sync`, `rt-multi-thread`, etc. The app uses none of these extra subsystems.

### Tokio Usage Audit

Every tokio symbol used across the entire codebase (all files read: `main.rs`, `app.rs`, `runner.rs`, `upgrade.rs`, `backends/mod.rs`, `backends/os_package_manager.rs`, `backends/flatpak.rs`, `backends/homebrew.rs`, `backends/nix.rs`, `ui/window.rs`, `ui/upgrade_page.rs`):

| File | Tokio symbol / usage | Feature required |
|------|----------------------|-----------------|
| `src/runner.rs` | `use tokio::io::{AsyncBufReadExt, BufReader}` | `io-util` |
| `src/runner.rs` | `use tokio::process::Command` | `process` |
| `src/runner.rs` | `tokio::join!(stdout_task, stderr_task)` | `macros` |
| `src/ui/window.rs` | `tokio::runtime::Builder::new_current_thread()` | `rt` |
| `src/ui/window.rs` | `.enable_all().build()` | `rt` (no extra feature; enables available drivers only) |
| `src/backends/os_package_manager.rs` | `tokio::process::Command::new(...)` (APT, DNF, Pacman, Zypper) | `process` |
| `src/backends/flatpak.rs` | `tokio::process::Command::new("flatpak")` | `process` |
| `src/backends/homebrew.rs` | `tokio::process::Command::new("brew")` | `process` |
| `src/backends/nix.rs` | (uses `CommandRunner` only, no direct tokio imports) | — |
| All others | No tokio usage | — |

**Absent usages confirmed:**
- `tokio::net` — not used anywhere
- `tokio::time` — not used anywhere
- `tokio::sync` — not used (project uses `async-channel` crate instead)
- `tokio::signal` — not used anywhere
- `tokio::fs` — not used (project uses `std::fs`)
- `tokio::io::stdin/stdout` — not used
- `#[tokio::main]` — not used (`main.rs` uses plain `fn main()` with GTK's event loop)
- `rt-multi-thread` — not used (runtime created as `new_current_thread()`)

### Runtime Construction Note

`window.rs` creates the tokio runtime with `Builder::new_current_thread().enable_all().build()`. The `enable_all()` call at runtime only enables reactors that were **compiled in**. Without `net` or `time` features, `enable_all()` simply enables the IO reactor (required by `process`) and nothing else. No compilation or runtime behavior changes.

### Minimal Feature Set

| Feature | Why needed |
|---------|-----------|
| `rt` | `tokio::runtime::Builder`, single-threaded runtime execution |
| `macros` | `tokio::join!` macro in `runner.rs` |
| `io-util` | `AsyncBufReadExt`, `BufReader` in `runner.rs` |
| `process` | `tokio::process::Command` in `runner.rs` and all backends |

### Exact Replacement for `Cargo.toml`

**Remove:**
```toml
tokio = { version = "1", features = ["full"] }
```

**Replace with:**
```toml
tokio = { version = "1", features = ["rt", "macros", "io-util", "process"] }
```

### Compilation Safety

- `cargo build` will succeed: all four features satisfy every tokio import in the codebase.
- No networking, time, sync, or signal subsystems are referenced — removing them produces no missing-symbol errors.
- `rt-multi-thread` omission is safe: the runtime is explicitly created with `new_current_thread()`.

---

## Finding #6 — APT `DEBIAN_FRONTEND=noninteractive` Missing

### Current State

In `src/backends/os_package_manager.rs`, `AptBackend::run_update()` (lines ~41–50):

```rust
async fn run_update(&self, runner: &CommandRunner) -> UpdateResult {
    if let Err(e) = runner.run("pkexec", &["apt", "update"]).await {
        return UpdateResult::Error(e);
    }
    match runner.run("pkexec", &["apt", "upgrade", "-y"]).await {
        Ok(output) => { ... }
        Err(e) => UpdateResult::Error(e),
    }
}
```

### Problem Analysis

`apt upgrade -y` can still block on interactive debconf prompts even with `-y`. The `-y` flag answers "yes" to apt's own "do you want to continue?" but does **not** suppress debconf configuration dialogs triggered by packages' `postinst` scripts (e.g. grub-pc asking which disk to install to, keyboard-configuration asking locale, etc.).

Setting `DEBIAN_FRONTEND=noninteractive` causes debconf to use default answers and skip all interactive prompts, preventing process hangs.

### Why Pattern B Is Not Applicable

The `CommandRunner::run()` signature is:
```rust
pub async fn run(&self, program: &str, args: &[&str]) -> Result<String, String>
```

It instantiates `tokio::process::Command` directly and does **not** expose `.env()`. Adding `.env()` support would be a separate architectural change. Pattern A (using the `env` utility as the command) is the correct fix that works with the existing abstraction.

### Corrected Commands

**Pattern A — use `env` as the intermediate command:**

```
pkexec env DEBIAN_FRONTEND=noninteractive apt update
pkexec env DEBIAN_FRONTEND=noninteractive apt upgrade -y
```

The POSIX `env` utility accepts `VAR=value` arguments before the command and executes it in the modified environment. Since `pkexec` runs `env` with root privileges, `env` then inherits that privilege and sets the variable before launching `apt`.

### Other Backends — Interactive Prompt Assessment

| Backend | Command | Interactive prompt risk | Action needed |
|---------|---------|------------------------|---------------|
| APT | `apt update` + `apt upgrade -y` | **HIGH** — debconf prompts possible | **Fix** |
| DNF | `dnf upgrade -y` | LOW — `-y` is sufficient for DNF; no debconf equivalent | None |
| Pacman | `pacman -Syu --noconfirm` | NONE — `--noconfirm` fully suppresses interaction | None |
| Zypper | `zypper refresh` + `zypper update -y` | LOW — zypper with `-y` does not have debconf | None |

Only **APT** requires this fix.

### Exact Lines to Change in `src/backends/os_package_manager.rs`

**Remove:**
```rust
    async fn run_update(&self, runner: &CommandRunner) -> UpdateResult {
        if let Err(e) = runner.run("pkexec", &["apt", "update"]).await {
            return UpdateResult::Error(e);
        }
        match runner.run("pkexec", &["apt", "upgrade", "-y"]).await {
```

**Replace with:**
```rust
    async fn run_update(&self, runner: &CommandRunner) -> UpdateResult {
        if let Err(e) = runner
            .run("pkexec", &["env", "DEBIAN_FRONTEND=noninteractive", "apt", "update"])
            .await
        {
            return UpdateResult::Error(e);
        }
        match runner
            .run(
                "pkexec",
                &["env", "DEBIAN_FRONTEND=noninteractive", "apt", "upgrade", "-y"],
            )
            .await
        {
```

No other changes to `os_package_manager.rs` are needed.

---

## Finding #8 — Inconsistent Placeholder Repository URLs

### Current State

| File | Field | Current value |
|------|-------|---------------|
| `Cargo.toml` | `repository` | `https://github.com/user/up` |
| `data/io.github.up.metainfo.xml` | `<url type="homepage">` | `https://github.com/up-project/up` |
| `data/io.github.up.metainfo.xml` | `<url type="bugtracker">` | `https://github.com/up-project/up/issues` |
| `README.md` | Nix flake examples and git clone | `https://github.com/user/up` |
| `meson.build` | — | No URL references |

### Canonical URL Assessment

The true repository URL cannot be determined from the source files — all values are placeholders. However:

- `Cargo.toml` and `README.md` both use `https://github.com/user/up` consistently.
- `data/io.github.up.metainfo.xml` uses a **different** placeholder (`up-project/up`), which is inconsistent.

The metainfo XML is the only inconsistent file. The fix is to align it to the existing `https://github.com/user/up` placeholder used in the two other files.

**Note:** The real canonical URL should be updated by the project owner when the repository is public. These fixes only ensure cross-file consistency at the placeholder level.

### Exact Changes

#### `Cargo.toml`

No change required. Current value `https://github.com/user/up` is already the consistent placeholder.

#### `data/io.github.up.metainfo.xml`

**Remove:**
```xml
  <url type="homepage">https://github.com/up-project/up</url>
  <url type="bugtracker">https://github.com/up-project/up/issues</url>
```

**Replace with:**
```xml
  <url type="homepage">https://github.com/user/up</url>
  <url type="bugtracker">https://github.com/user/up/issues</url>
```

---

## Summary of All Changes

| File | Change |
|------|--------|
| `Cargo.toml` | Replace `features = ["full"]` with `features = ["rt", "macros", "io-util", "process"]` |
| `src/backends/os_package_manager.rs` | Add `env DEBIAN_FRONTEND=noninteractive` to both APT runner calls |
| `data/io.github.up.metainfo.xml` | Align homepage/bugtracker URLs to `https://github.com/user/up[/issues]` |

## No New Dependencies

All three fixes are changes to existing code and metadata only. No new crates are added.

---

## Implementation Notes for Phase 2

1. Apply `Cargo.toml` tokio feature change first and verify with `cargo build`.
2. Apply the `os_package_manager.rs` APT fix — no logic change, only argument list expansion.
3. Apply the `metainfo.xml` URL fix — two-line substitution.
4. Run `cargo build && cargo clippy -- -D warnings && cargo fmt --check` to confirm clean build.
