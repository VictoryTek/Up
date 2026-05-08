# Spec: Fix Race Condition in Nix Backend Tests

**Feature name:** `nix_test_race_fix`  
**Spec file:** `.github/docs/subagent_docs/nix_test_race_fix_spec.md`  
**Date:** 2026-05-07  

---

## 1. Current State Analysis

### 1.1 Affected Test

- **Test:** `backends::nix::tests::run_update_legacy_nix_env_success`
- **File:** `src/backends/nix.rs`, lines ~720–810

### 1.2 The Two Concurrent Tests

Both `#[tokio::test]` async tests manipulate the global `HOME` environment variable:

```
run_update_legacy_nix_env_success  (lines ~762–798)
run_update_legacy_nix_env_error    (lines ~800–830)
```

Each test follows this identical pattern:

```rust
// 1. Create a temporary directory with a v1 manifest
let tmp_home = std::env::temp_dir().join("up-test-nix-{pid}-{suffix}");
fs::create_dir_all(tmp_home.join(".nix-profile")).unwrap();
fs::write(tmp_home.join(".nix-profile/manifest.json"), r#"{"version": 1, ...}"#).unwrap();

// 2. Save and override HOME
let prev_home = std::env::var("HOME").unwrap_or_default();
std::env::set_var("HOME", &tmp_home);              // ← mutates global state

// 3. Run the system under test
let result = NixBackend.run_update(&executor).await;

// 4. Restore HOME and clean up
std::env::set_var("HOME", prev_home);              // ← mutates global state
let _ = std::fs::remove_dir_all(&tmp_home);
```

### 1.3 The Production Code Path That Reads HOME

`run_update` in `src/backends/nix.rs` (lines ~510–545) reads `HOME` inside the `else` branch (non-NixOS, non-Determinate-Nix):

```rust
let use_legacy_nix_env = {
    let manifest_path =
        std::path::Path::new(&std::env::var("HOME").unwrap_or_default())
            .join(".nix-profile/manifest.json");
    if let Ok(content) = std::fs::read_to_string(&manifest_path) {
        !content.contains("\"version\": 2")
    } else {
        false           // ← cannot read manifest → default to modern nix profile
    }
};
if use_legacy_nix_env {
    // dispatches to runner.run("nix-env", &["-u"])  ← MockExecutor handles this
} else {
    // calls nix_profile_upgrade_all() directly      ← spawns real "nix" binary
    match nix_profile_upgrade_all().await { ... }
}
```

### 1.4 Exact Race Condition

Rust's `cargo test` runner executes tests concurrently in OS-level threads by default. The `#[tokio::test]` macro creates a new single-threaded Tokio runtime per test, but all runtimes execute on OS threads run in parallel by the test harness.

The interleaving that causes failure:

```
Thread A (success test)              Thread B (error test)
────────────────────────────         ──────────────────────────────
set_var("HOME", tmp_home_A)
                                     set_var("HOME", tmp_home_B)
                                     NixBackend.run_update(&executor_B).await
                                       reads HOME → tmp_home_B/.nix-profile/manifest.json
                                       (exists, v1 → legacy path → MockExecutor handles it)
NixBackend.run_update(&executor_A).await
  reads HOME → ??? (could be tmp_home_B or even prev_home)
```

A worse interleaving:

```
Thread A                             Thread B
────────────────────                 ─────────────────────────────
set_var("HOME", tmp_home_A)
NixBackend.run_update(&executor_A).await
  [not yet read HOME]
                                     set_var("HOME", tmp_home_B)
  reads HOME → tmp_home_B/.nix-profile/manifest.json
  (tmp_home_B has a valid manifest → actually works accidentally)
```

And the failure interleaving:

```
Thread A                             Thread B
────────────────────                 ─────────────────────────────
set_var("HOME", tmp_home_A)
NixBackend.run_update(&executor_A).await
  [not yet read HOME]
                                     set_var("HOME", prev_home)  ← restore step
  reads HOME → prev_home (user's real $HOME, or "")
  manifest_path does NOT exist OR is v2
  use_legacy_nix_env = false
  → calls nix_profile_upgrade_all()
    → tokio::process::Command::new("nix")  ← real nix binary
    → FAILS: "No such file or directory (os error 2)" on CI
```

This is the confirmed failure path.

---

## 2. Problem Definition

### 2.1 Root Cause
`std::env::set_var` / `std::env::var` operate on the process-global environment. Two parallel tests each mutate `HOME` without coordination, creating a time-of-check/time-of-use (TOCTOU) race. The race causes `run_update` to read a `HOME` value belonging to a different test (or restored to the real HOME), making `use_legacy_nix_env` evaluate to `false`, bypassing MockExecutor, and spawning the real `nix` binary — which is absent on CI.

### 2.2 Scope
- **Failure environment:** GitHub Actions Ubuntu runner (no `nix` binary installed)
- **Passing environment:** Developer machines where `nix` happens to be installed
- **Affected tests:** Both `run_update_legacy_nix_env_success` and `run_update_legacy_nix_env_error`

### 2.3 Constraints
- Must not add new crate dependencies (`serial_test`, `temp_env`, `once_cell` are all absent from `Cargo.toml`)
- Must not modify production code (`src/backends/nix.rs` lines 1–619)
- Must be idiomatic Rust

---

## 3. Solution Research

### 3.1 Considered Approaches

| Approach | Requires new crate | Modifies prod code | Idiomatic | Chosen |
|---|---|---|---|---|
| `static Mutex<()>` (std only) | No | No | Yes | **Yes** |
| `serial_test` crate (`#[serial]`) | Yes (not in Cargo.toml) | No | Yes | No |
| `temp_env` crate RAII wrapper | Yes (not in Cargo.toml) | No | Yes | No |
| `std::sync::LazyLock<Mutex<()>>` | No | No | Yes | Alternative form |
| `#[tokio::test(flavor="current_thread")]` | No | No | No (doesn't help) | No |
| Dependency injection for HOME | No | **Yes** | Yes | No (scope) |
| `-- --test-threads=1` in CI | No | No | No (fragile) | No |

### 3.2 Chosen Solution: `static LazyLock<Mutex<()>>` Guard

Using `std::sync::LazyLock<std::sync::Mutex<()>>` (stabilised in Rust 1.80; current toolchain is **1.94.1**) provides a per-module mutex that serialises all tests that acquire it. This is a zero-dependency, test-only, idiomatic Rust pattern.

**Why `LazyLock` over `OnceLock`?**  
`LazyLock` does not require a helper function to initialise the value; it initialises on first dereference via the closure provided at declaration. This is cleaner and requires fewer lines.

---

## 4. Proposed Solution

### 4.1 Changes Required

**File:** `src/backends/nix.rs`  
**Section:** `#[cfg(test)] mod tests { ... }`  
**Lines affected:** Test module only (~720–830)

#### 4.1.1 Add the mutex declaration at the top of the `tests` module

Insert immediately after `mod tests {` and before the `use` statements (or after them — placement within the module does not matter to the compiler, but after imports is conventional):

```rust
/// Serialises all tests that mutate the HOME environment variable.
/// Without this guard, parallel test threads race on the process-global
/// `HOME` env var, causing `run_update` to read the wrong HOME and
/// fall through to the real `nix` binary (absent on CI).
static HOME_ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));
```

#### 4.1.2 Acquire the lock in `run_update_legacy_nix_env_success`

Replace the section between the `tmp_home` setup and the `set_var` call:

**Before:**
```rust
let prev_home = std::env::var("HOME").unwrap_or_default();
std::env::set_var("HOME", &tmp_home);

let executor = MockExecutor::with_output("upgrading 'hello-2.10' to 'hello-2.12'\n");
let result = NixBackend.run_update(&executor).await;

std::env::set_var("HOME", prev_home);
let _ = std::fs::remove_dir_all(&tmp_home);
```

**After:**
```rust
let prev_home = std::env::var("HOME").unwrap_or_default();
let _home_guard = HOME_ENV_LOCK.lock().unwrap();
std::env::set_var("HOME", &tmp_home);

let executor = MockExecutor::with_output("upgrading 'hello-2.10' to 'hello-2.12'\n");
let result = NixBackend.run_update(&executor).await;

std::env::set_var("HOME", prev_home);
drop(_home_guard);
let _ = std::fs::remove_dir_all(&tmp_home);
```

#### 4.1.3 Acquire the lock in `run_update_legacy_nix_env_error`

**Before:**
```rust
let prev_home = std::env::var("HOME").unwrap_or_default();
std::env::set_var("HOME", &tmp_home);

let executor = MockExecutor::with_error(1, "nix-env: error upgrading packages");
let result = NixBackend.run_update(&executor).await;

std::env::set_var("HOME", prev_home);
let _ = std::fs::remove_dir_all(&tmp_home);
```

**After:**
```rust
let prev_home = std::env::var("HOME").unwrap_or_default();
let _home_guard = HOME_ENV_LOCK.lock().unwrap();
std::env::set_var("HOME", &tmp_home);

let executor = MockExecutor::with_error(1, "nix-env: error upgrading packages");
let result = NixBackend.run_update(&executor).await;

std::env::set_var("HOME", prev_home);
drop(_home_guard);
let _ = std::fs::remove_dir_all(&tmp_home);
```

### 4.2 Why `drop(_home_guard)` is explicit

The lock must be released **after** `set_var("HOME", prev_home)` but **before** `remove_dir_all`. An explicit `drop` makes this ordering clear and prevents the guard from accidentally keeping the lock across the filesystem cleanup, which could delay other tests unnecessarily. Alternatively, a block `{ let _g = ...; ... }` may be used, but explicit `drop` is equally readable.

### 4.3 Complete Diff (minimal)

```diff
 #[cfg(test)]
 mod tests {
     use super::{
         compare_lock_nodes, count_determinate_upgraded, count_nix_store_operations,
         is_determinate_nix, is_nixos, upgrade_available_in_output, validate_flake_attr, NixBackend,
     };
     use crate::backends::{Backend, UpdateResult};
     use crate::executor::test_utils::MockExecutor;
+
+    /// Serialises all tests that mutate the HOME environment variable to prevent
+    /// a race condition where parallel threads read the wrong HOME value.
+    static HOME_ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
+        std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

     ...

     #[tokio::test]
     async fn run_update_legacy_nix_env_success() {
         ...

         let prev_home = std::env::var("HOME").unwrap_or_default();
+        let _home_guard = HOME_ENV_LOCK.lock().unwrap();
         std::env::set_var("HOME", &tmp_home);

         let executor = MockExecutor::with_output("upgrading 'hello-2.10' to 'hello-2.12'\n");
         let result = NixBackend.run_update(&executor).await;

         std::env::set_var("HOME", prev_home);
+        drop(_home_guard);
         let _ = std::fs::remove_dir_all(&tmp_home);

         ...
     }

     #[tokio::test]
     async fn run_update_legacy_nix_env_error() {
         ...

         let prev_home = std::env::var("HOME").unwrap_or_default();
+        let _home_guard = HOME_ENV_LOCK.lock().unwrap();
         std::env::set_var("HOME", &tmp_home);

         let executor = MockExecutor::with_error(1, "nix-env: error upgrading packages");
         let result = NixBackend.run_update(&executor).await;

         std::env::set_var("HOME", prev_home);
+        drop(_home_guard);
         let _ = std::fs::remove_dir_all(&tmp_home);

         ...
     }
 }
```

---

## 5. Files to Be Modified

| File | Section | Nature of Change |
|---|---|---|
| `src/backends/nix.rs` | `#[cfg(test)] mod tests` | Add `HOME_ENV_LOCK` static; add `lock()`/`drop()` in two tests |

**Production code:** No changes.  
**Cargo.toml:** No changes.  
**meson.build / flake.nix:** No changes.

---

## 6. Risks and Mitigations

### 6.1 Mutex Poison on Test Panic

If a test panics while holding the guard, `Mutex::lock()` will return `Err(PoisonError)` in subsequent tests. The current code uses `.unwrap()` which will propagate the panic to all subsequent tests holding the lock.

**Mitigation:** Both tests already `unwrap()` on other fallible operations (filesystem setup), so this is consistent behaviour. An alternative is `.unwrap_or_else(|e| e.into_inner())` to recover from poisoning, but this adds noise for a low-probability scenario and is not idiomatic in test code.

### 6.2 Lock Granularity

The lock serialises only the two `run_update_legacy_nix_env_*` tests, not all tests in the module. The other tests (`count_nix_store_ops_*`, `validate_flake_attr_*`, etc.) do not touch `HOME` and can still run concurrently.

**Risk:** None. The mutex is acquired only in the two affected tests.

### 6.3 Interaction with `list_available` Tests (Future)

The `list_available` implementation in `NixBackend` (lines ~561–613) also reads `HOME` for the same manifest path. Any future tests for `list_available` that set `HOME` must also acquire `HOME_ENV_LOCK`.

**Mitigation:** Document this requirement in the code comment on `HOME_ENV_LOCK`.

### 6.4 `std::env::set_var` is `unsafe` in Rust ≥ 2024 Edition

`std::env::set_var` was marked `unsafe` in the Rust 2024 edition (tracking issue #27970; stabilised as `unsafe` in ~1.80). The current edition is **2021** (`edition = "2021"` in `Cargo.toml`), so this change does not apply. No `unsafe` block is required.

**Mitigation:** No action needed for the current edition. If the project migrates to edition 2024, the two `set_var` calls in the test module will require `unsafe` blocks at that time (see Rust RFC 3375).

### 6.5 Test Runtime Performance

The serialised tests will run sequentially. This is acceptable because the tests are fast (filesystem I/O on tmpfs + MockExecutor async resolution — no real network or process spawning).

---

## 7. Verification Steps

After applying the fix, the implementor must verify:

1. `cargo test backends::nix -- --nocapture` passes all nix backend tests
2. `cargo test` passes without any failures
3. `cargo clippy -- -D warnings` produces no new warnings
4. `cargo fmt --check` passes
5. Run with `RUST_TEST_THREADS=1` and `RUST_TEST_THREADS=8` to confirm both serialised and parallel modes pass

---

## 8. References

- [Rust Reference: `std::sync::LazyLock`](https://doc.rust-lang.org/std/sync/struct.LazyLock.html) (stabilised Rust 1.80)
- [Rust Reference: `std::sync::Mutex`](https://doc.rust-lang.org/std/sync/struct.Mutex.html)
- [Rust issue #27970: make `set_var` unsafe](https://github.com/rust-lang/rust/issues/27970)
- [Rust RFC 3375: `env::set_var` in edition 2024](https://github.com/rust-lang/rfcs/pull/3375)
- [Rust Nomicon: Atomics and global state in tests](https://doc.rust-lang.org/nomicon/atomics.html)
- [`serial_test` crate](https://crates.io/crates/serial_test) (not used — would require new dev-dependency)
