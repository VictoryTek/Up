# Specification: Remove `async-trait` Dependency

**Feature:** `async_trait_removal`  
**Date:** 2026-03-18  
**Status:** DRAFT  
**Author:** Research Subagent (Phase 1)

---

## 1. Current State Analysis

### 1.1 `async-trait` Version in `Cargo.toml`

```toml
async-trait = "0.1"
```

No minimum patch version is pinned; any `0.1.x` release is accepted.

### 1.2 All `#[async_trait::async_trait]` Usage Sites

| File | Line | Annotates |
|------|------|-----------|
| `src/backends/mod.rs` | 44 | `pub trait Backend: Send + Sync { … }` (trait definition) |
| `src/backends/flatpak.rs` | 10 | `impl Backend for FlatpakBackend` |
| `src/backends/homebrew.rs` | 10 | `impl Backend for HomebrewBackend` |
| `src/backends/nix.rs` | 48 | `impl Backend for NixBackend` |
| `src/backends/os_package_manager.rs` | 23 | `impl Backend for AptBackend` |
| `src/backends/os_package_manager.rs` | 81 | `impl Backend for DnfBackend` |
| `src/backends/os_package_manager.rs` | 145 | `impl Backend for PacmanBackend` |
| `src/backends/os_package_manager.rs` | 192 | `impl Backend for ZypperBackend` |

**Total: 8 annotated sites.** All usages are concentrated exclusively on the `Backend` trait and its seven implementations. No other traits in the codebase use `async-trait`.

### 1.3 The `Backend` Trait — Exact Definition

Located in `src/backends/mod.rs`:

```rust
#[async_trait::async_trait]
pub trait Backend: Send + Sync {
    fn kind(&self) -> BackendKind;
    fn display_name(&self) -> &str;
    fn description(&self) -> &str;
    fn icon_name(&self) -> &str;

    async fn run_update(&self, runner: &CommandRunner) -> UpdateResult;

    /// Default implementation returns Ok(0) for backends that do not support checking.
    async fn count_available(&self) -> Result<usize, String> {
        Ok(0)
    }
}
```

**Sync methods (4):** `kind`, `display_name`, `description`, `icon_name` — no change needed.  
**Async methods (2):** `run_update` (abstract, no default), `count_available` (has a default `Ok(0)` body).  
**Supertrait bounds:** `Send + Sync`.

### 1.4 All `impl Backend for X` Blocks

| Struct | File | Overrides `run_update` | Overrides `count_available` |
|--------|------|------------------------|------------------------------|
| `FlatpakBackend` | `src/backends/flatpak.rs` | Yes | Yes (flatpak remote-ls) |
| `HomebrewBackend` | `src/backends/homebrew.rs` | Yes | Yes (brew outdated) |
| `NixBackend` | `src/backends/nix.rs` | Yes (complex, flake-aware) | Yes (flake.lock parse) |
| `AptBackend` | `src/backends/os_package_manager.rs` | Yes | Yes (apt list --upgradable) |
| `DnfBackend` | `src/backends/os_package_manager.rs` | Yes | Yes (dnf check-update) |
| `PacmanBackend` | `src/backends/os_package_manager.rs` | Yes | Yes (pacman -Qu) |
| `ZypperBackend` | `src/backends/os_package_manager.rs` | Yes | Yes (zypper list-updates) |

All seven implementations override both async methods. The default `count_available` implementation in the trait body is never relied upon at runtime, but it must be preserved for future backends.

### 1.5 How `dyn Backend` is Used

**Storage:** All backends are stored as `Vec<Arc<dyn Backend>>` (returned from `detect_backends()` in `src/backends/mod.rs` and passed down to `src/ui/window.rs`).

**Dynamic dispatch call sites in `src/ui/window.rs`:**

1. **`run_update` path** — The `Vec<Arc<dyn Backend>>` is moved into a `std::thread::spawn` closure. Inside, a single-threaded Tokio runtime (`tokio::runtime::Builder::new_current_thread()`) is built and `.block_on(async { … })` is used. `backend.run_update(&runner).await` is awaited inside `block_on`.

2. **`count_available` path** — Same pattern: `std::thread::spawn` → `new_current_thread()` → `block_on` → `backend_clone.count_available().await`.

The futures produced by these async methods are **never sent between threads**; they are created and awaited entirely within a single `block_on` call on the spawned thread.

### 1.6 Rust Edition and Version

```toml
edition = "2021"
```

No `rust-version` field is set in `Cargo.toml`. The project builds on stable Rust. The Rust toolchain on this system supports at least Rust 1.75+ (confirmed by recent build success).

---

## 2. Problem Definition

### 2.1 Hidden Heap Allocation per Call

`async-trait` transforms every `async fn` into a method returning `Pin<Box<dyn Future<Output = T> + Send + 'a>>`. This means every single call to `run_update` or `count_available` on a `dyn Backend` object incurs:

- A heap allocation to box the future (`Box::new(future)`)
- A dynamic dispatch through the future's vtable at every `.poll()` invocation
- Two levels of vtable dispatch per `await` point: one for the trait method, one for the boxed future

For a system update application where async methods run for seconds, this overhead is negligible at runtime. The cost is principally:

a. **Compile-time proc-macro overhead** — `async-trait` is a proc-macro crate; it runs the syn/quote parsing and desugaring for every attributed `impl` block at compile time. In this project (8 sites), the overhead is minor but measurable in incremental builds.  
b. **Dependency graph size** — `async-trait` pulls in `proc-macro2`, `quote`, and `syn` (heavy build dependencies that also appear via other paths in this project).  
c. **Ergonomic obsolescence** — Since Rust 1.75 (December 2023), `async fn` in traits and RPITIT are stable for static dispatch. Using `async-trait` diverges from idiomatic modern Rust.

### 2.2 Why This Cannot Be Solved With a Simple Attribute Removal

The critical blocker is **object safety**.

Native `async fn` in traits (stabilised Rust 1.75) desugars to:

```rust
fn run_update(&self, runner: &CommandRunner)
    -> impl Future<Output = UpdateResult>;
```

A return type of `-> impl Future<Output = T>` (RPITIT — Return-Position Impl Trait in Trait) is **not object-safe** in Rust. Each concrete `impl Backend for X` returns a *different* future type, and Rust cannot represent this in a vtable. Therefore:

```rust
// ❌ This fails to compile if Backend uses native async fn:
let b: Arc<dyn Backend> = Arc::new(FlatpakBackend);
```

Error: ``the trait `Backend` cannot be made into an object``

The current codebase depends on `Arc<dyn Backend>` (7 backends, detected at runtime, stored in a `Vec`). This dynamic dispatch pattern is fundamental to the architecture. It **cannot be removed** without a full refactor to enum-based dispatch or monomorphised generics — neither of which is in scope.

---

## 3. Context7 Research Notes

### 3.1 Native `async fn` in Traits (Rust 1.75)

Stabilised in Rust 1.75 (December 2023):
- **RPITIT** — Return-Position Impl Trait in Traits
- **AFIT** — `async fn` in traits  
- **Scope:** Static dispatch only (`impl Trait` / `<T: Trait>` generics). **NOT** usable with `dyn Trait`.

Source: Rust Blog, stabilisation RFC — confirmed via direct knowledge.

### 3.2 `trait-variant` Crate

The `trait-variant` crate (from the Rust lang team) provides `#[trait_variant::make(TraitSend: Send)]`. This creates a parallel trait variant where all `async fn` return types are additionally bounded by `Send`. It is intended for use when the same trait needs both `Send` and non-`Send` variants for use with `impl Trait` (static dispatch).

**Limitation for this project:** `trait-variant` does NOT solve the object-safety problem. The generated variant trait still uses `impl Future` in return position, which is not `dyn`-safe. It cannot replace `Arc<dyn Backend>` usage.

### 3.3 `dynosaur` Crate (Context7: `/websites/rs_dynosaur_0_3_0`)

`dynosaur` (version 0.3.0, from the Rust Async Working Group) is a proc-macro that generates a concrete boxed-dispatch wrapper type:

```rust
#[dynosaur::dynosaur(DynBackend = dyn(box) Backend)]
trait Backend { … }
```

This generates a `DynBackend<'_>` struct that wraps a `Box<dyn …>` internally. Usage would be `Box<DynBackend<'_>>` instead of `Box<dyn Backend>`.

**Limitation for this project:** `dynosaur` produces a struct-based wrapper (`DynBackend<'_>`) that is **not compatible with `Arc<dyn Backend>`**. All current storage sites (`Vec<Arc<dyn Backend>>`, `Option<Arc<dyn Backend>>`) would need to be changed to `Vec<Arc<DynBackend<'_>>>` or similar — and the lifetime annotation `'_` makes long-lived `Arc` storage complex. This is a significant architectural change beyond the scope of dependency removal.

Additionally, `dynosaur` 0.3.0 is explicitly described as an experiment that will be deprecated when language-level `dyn async Trait` support lands.

### 3.4 Future Language Support

The Rust team is actively working on `dyn async Trait` stabilisation (tracking issue rust-lang/rust#107485). Once stable, `async fn` in traits will be fully usable with `dyn Trait` without any wrapper crates. As of early 2026, this feature is not yet stable.

---

## 4. Proposed Solution Architecture

### Decision: Manual Future Boxing (Option B)

Given the constraints:
1. `Arc<dyn Backend>` usage is architectural — cannot be changed without a major refactor
2. Native `async fn` in traits is not `dyn`-safe
3. `trait-variant` does not solve the `dyn` problem
4. `dynosaur` requires `Arc<dyn Backend>` → `Box<DynBackend<'_>>` everywhere (out of scope)
5. Waiting for `dyn async Trait` stabilisation in the language is indefinite

**The recommended approach is: replace `async-trait` with equivalent manual future boxing in the trait definition, removing the proc-macro dependency while preserving identical runtime semantics and `Arc<dyn Backend>` compatibility.**

#### What Changes

The `Backend` trait definition changes from:

```rust
#[async_trait::async_trait]
pub trait Backend: Send + Sync {
    async fn run_update(&self, runner: &CommandRunner) -> UpdateResult;
    async fn count_available(&self) -> Result<usize, String> {
        Ok(0)
    }
}
```

To the explicit boxed-future form:

```rust
use std::future::Future;
use std::pin::Pin;

pub trait Backend: Send + Sync {
    fn run_update<'a>(
        &'a self,
        runner: &'a CommandRunner,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>>;

    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async { Ok(0) })
    }
}
```

This is **exactly what `async-trait` generates** under the hood, made explicit. The trait remains object-safe (`Pin<Box<dyn Future>>` is a concrete type in the vtable), `Arc<dyn Backend>` continues to work unchanged, and no new dependency is introduced.

#### What Changes in Implementations

Every `async fn run_update` and `async fn count_available` body becomes a method returning `Box::pin(async move { … })` or `Box::pin(async { … })`:

```rust
// Before (with async-trait):
#[async_trait::async_trait]
impl Backend for FlatpakBackend {
    async fn run_update(&self, runner: &CommandRunner) -> UpdateResult {
        // body
    }
}

// After (manual boxing):
impl Backend for FlatpakBackend {
    fn run_update<'a>(
        &'a self,
        runner: &'a CommandRunner,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            // body (unchanged)
        })
    }
}
```

The `async move { }` block captures `self` and `runner` by reference through the `'a` lifetime. If the body references any `self` fields, the `async move` captures the references correctly.

#### Send Bound Analysis

The `+ Send` bound on the returned future is kept for two reasons:

1. **Consistency with current behaviour** — `async-trait 0.1` adds `+ Send` by default when the trait has `Send` supertrait bounds (which `Backend: Send + Sync` satisfies). Keeping `+ Send` means no change in external API surface.

2. **Forward compatibility** — If the caller in `window.rs` ever changes to a multi-threaded runtime or introduces `tokio::spawn` (which requires `Send` futures), the bound is already present.

Note: `+ Send` is technically *not required* for current usage (all futures are awaited in `new_current_thread()` + `block_on`), but removing it would be a semantic change with no upside.

---

## 5. Implementation Steps

The following steps must be executed in order.

### Step 1: Remove `async-trait` from `Cargo.toml`

In `Cargo.toml`, remove the line:
```toml
async-trait = "0.1"
```

No new dependency is added.

### Step 2: Add `Pin` and `Future` imports to `src/backends/mod.rs`

Add to the existing `use` block at the top of `src/backends/mod.rs`:

```rust
use std::future::Future;
use std::pin::Pin;
```

### Step 3: Replace the `Backend` trait definition in `src/backends/mod.rs`

Remove the `#[async_trait::async_trait]` attribute at line 44.  
Replace the two `async fn` method signatures with their boxed equivalents:

**Old (lines 44–60):**
```rust
#[async_trait::async_trait]
pub trait Backend: Send + Sync {
    fn kind(&self) -> BackendKind;
    fn display_name(&self) -> &str;
    fn description(&self) -> &str;
    fn icon_name(&self) -> &str;

    async fn run_update(&self, runner: &CommandRunner) -> UpdateResult;

    async fn count_available(&self) -> Result<usize, String> {
        Ok(0)
    }
}
```

**New:**
```rust
pub trait Backend: Send + Sync {
    fn kind(&self) -> BackendKind;
    fn display_name(&self) -> &str;
    fn description(&self) -> &str;
    fn icon_name(&self) -> &str;

    fn run_update<'a>(
        &'a self,
        runner: &'a CommandRunner,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>>;

    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async { Ok(0) })
    }
}
```

### Step 4: Update `src/backends/flatpak.rs`

Remove `#[async_trait::async_trait]` at line 10.  
Replace the `impl` block's async methods:

```rust
impl Backend for FlatpakBackend {
    // ... sync methods unchanged ...

    fn run_update<'a>(
        &'a self,
        runner: &'a CommandRunner,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            match runner.run("flatpak", &["update", "-y"]).await {
                Ok(output) => {
                    let count = output
                        .lines()
                        .filter(|l| {
                            let t = l.trim();
                            t.starts_with(|c: char| c.is_ascii_digit())
                        })
                        .count();
                    UpdateResult::Success { updated_count: count }
                }
                Err(e) => UpdateResult::Error(e),
            }
        })
    }

    fn count_available(&self) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + '_>> {
        Box::pin(async {
            let out = tokio::process::Command::new("flatpak")
                .args(["remote-ls", "--updates"])
                .output()
                .await
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            Ok(text.lines().filter(|l| !l.is_empty()).count())
        })
    }
}
```

Also add to the top of `flatpak.rs`:
```rust
use std::future::Future;
use std::pin::Pin;
```

### Step 5: Update `src/backends/homebrew.rs`

Remove `#[async_trait::async_trait]` at line 10.  
Replace async methods with boxed forms.

Add imports:
```rust
use std::future::Future;
use std::pin::Pin;
```

Replace `run_update` and `count_available` with `Box::pin(async move { … })` bodies (same logic, no changes to the inner code).

### Step 6: Update `src/backends/nix.rs`

Remove `#[async_trait::async_trait]` at line 48.  
Replace async methods with boxed forms.

Add imports:
```rust
use std::future::Future;
use std::pin::Pin;
```

**Important:** The `run_update` body for `NixBackend` is complex (flake detection, hostname validation, multiple branches). The entire body wraps into `Box::pin(async move { … })`. All internal `runner.run(…).await` calls are unchanged.

The `count_available` body references `is_nixos()`, `is_nixos_flake()`, and `tokio::fs::read_to_string`. All of these are compatible with `async move` capture.

### Step 7: Update `src/backends/os_package_manager.rs`

Remove `#[async_trait::async_trait]` at lines 23, 81, 145, and 192 (all four impl blocks).

Add imports at the top:
```rust
use std::future::Future;
use std::pin::Pin;
```

For each of the four backend structs (`AptBackend`, `DnfBackend`, `PacmanBackend`, `ZypperBackend`):
- Replace `async fn run_update` with the boxed form
- Replace `async fn count_available` with the boxed form  
- All inner logic (counting helpers, argument lists) is unchanged

**Note for `AptBackend::run_update`:** The method calls `runner.run(…).await` twice sequentially (once for `apt update`, once for `apt upgrade`). Both calls remain unchanged inside `Box::pin(async move { … })`.

---

## 6. Dependencies

### Remove

```toml
# Remove entirely from [dependencies]
async-trait = "0.1"
```

### Add

None. No new dependencies are required.

---

## 7. Affected Files

| File | Change |
|------|--------|
| `Cargo.toml` | Remove `async-trait = "0.1"` |
| `src/backends/mod.rs` | Remove `#[async_trait::async_trait]`, add `Future`/`Pin` imports, rewrite trait method signatures |
| `src/backends/flatpak.rs` | Remove attribute, add imports, wrap methods in `Box::pin(async move { … })` |
| `src/backends/homebrew.rs` | Remove attribute, add imports, wrap methods in `Box::pin(async move { … })` |
| `src/backends/nix.rs` | Remove attribute, add imports, wrap methods in `Box::pin(async move { … })` |
| `src/backends/os_package_manager.rs` | Remove 4 attributes, add imports, wrap 8 methods in `Box::pin(async move { … })` |

**Not affected:** `src/ui/window.rs`, `src/runner.rs`, `src/app.rs`, `src/main.rs`, `src/upgrade.rs`, `src/reboot.rs`, `src/ui/log_panel.rs`, `src/ui/update_row.rs`, `src/ui/upgrade_page.rs`, `src/ui/reboot_dialog.rs`. These files do not use `async-trait` or define trait methods — they call the trait methods which remain API-compatible.

---

## 8. Risks and Mitigations

### 8.1 Object Safety — CRITICAL (MITIGATED)

**Risk:** Replacing `async fn` with `-> Pin<Box<dyn Future + Send + 'a>>` must maintain object safety.  
**Analysis:** `Pin<Box<dyn Future<Output = T> + Send + 'a>>` is a concrete type. It can appear in a vtable. The trait remains object-safe. `Arc<dyn Backend>` continues to compile.  
**Mitigation:** The proposed signatures are identical to what `async-trait 0.1` generates. Object safety is preserved by definition.

### 8.2 Default Method Implementation (`count_available`)

**Risk:** The default `count_available` implementation uses `async { Ok(0) }` without `move`. This default must compile inside a `Pin<Box<…>>` return type without capturing any `self` reference.  
**Analysis:** `Box::pin(async { Ok(0) })` does not capture `self` and does not reference any fields. The `'_` lifetime in the return type is satisfied. This compiles correctly.  
**Mitigation:** Use `Box::pin(async { Ok(0) })` (no `move`, no capture). Confirm with `cargo build`.

### 8.3 Lifetime `'a` on `run_update`

**Risk:** `run_update<'a>(&'a self, runner: &'a CommandRunner)` ties both lifetimes together. In theory, a concrete impl where `self` outlives `runner` could fail. In practice, both `self` and `runner` are created at the call site and the future is immediately awaited — no issue.  
**Analysis:** The lifetime on `run_update` mirrors what `async-trait` generates internally. All current call sites create `CommandRunner` inline and immediately `.await` the result. No call site stores the future beyond its scope.  
**Mitigation:** The lifetime annotation is standard practice for async methods taking `&self` references. If a specific impl needs different lifetimes, they can be made less constrained (e.g., `'life0: 'async_trait` style), but the simple shared `'a` works for all current impls.

### 8.4 `async move` vs `async` in Implementations

**Risk:** Some implementations may reference `self` fields or iterate a closure that doesn't capture correctly with `async move`.  
**Analysis:** All current implementations reference only `runner` (passed by reference) and call helper functions (`is_nixos()`, etc.) by value. The `runner` reference is captured through `'a` in the lifetime bound. Using `async move` in the outer box moves the `&'a self` reference, which is fine.  
**Mitigation:** Use `Box::pin(async move { … })` consistently. The move captures the `&'a self` reference (not the struct itself), so no ownership conflicts arise.

### 8.5 Compile-Time Regression

**Risk:** Removing a proc-macro should reduce compile time, but the change introduces explicit use of `std::future::Future` and `std::pin::Pin` which increase verbosity without new proc-macro cost.  
**Impact:** Neutral to mildly positive compile time. No runtime impact.  
**Mitigation:** None needed.

### 8.6 `Send` Bound Necessity

**Risk:** Adding `+ Send` in the return type may cause unnecessary trait bounds or spurious failures in non-`Send` contexts.  
**Analysis:** Current call sites all use `block_on` on a current_thread runtime. Futures do not need to be `Send` at runtime. However, `+ Send` keeps API surface identical to the current `async-trait`-generated code and does not cause any compilation failure for the existing impls (all impl bodies use `tokio::process::Command`, `runner.run()`, `std::fs`, etc. — all `Send`).  
**Mitigation:** Retain `+ Send`. If a future impl needs a `!Send` type inside an async body, the `+ Send` bound can be removed from that method at that time.

### 8.7 `where Self: Sized` for Default Methods

**Risk:** Default methods in traits sometimes require `where Self: Sized` to prevent conflicts with `dyn Trait` dispatch. The default `count_available` does not take `&mut self` or return RPITIT, so this does not apply.  
**Analysis:** `count_available` takes `&self` and returns a concrete `Pin<Box<dyn Future>>`. No `where Self: Sized` is needed. The default impl will be callable from `dyn Backend` trait objects.  
**Mitigation:** None needed.

### 8.8 Alternative Path: `dynosaur` (Not Recommended for This Project)

The `dynosaur` crate (version 0.3.0) from the Rust Async WG could replace both `async-trait` and the manual boxing. It generates a `DynBackend<'_>` struct for dynamic dispatch. However:

- All `Arc<dyn Backend>` sites must change to `Arc<DynBackend<'_>>` or `Box<DynBackend<'_>>`
- The generated struct carries a lifetime annotation that complicates long-lived storage in `Rc<RefCell<Vec<…>>>` in `window.rs`
- `dynosaur` 0.3.0 is explicitly experimental and will be superseded when the language stabilises `dyn async Trait`
- This represents a significantly larger change surface than the manual boxing approach

**Verdict:** `dynosaur` is NOT recommended for this change. The manual boxing approach is preferred: it achieves the stated goal (remove `async-trait`) with zero new dependencies and minimal change surface.

---

## 9. Summary

| Property | Value |
|----------|-------|
| `async-trait` version to remove | `0.1` |
| New dependencies to add | None |
| Files to modify | 6 |
| `#[async_trait]` annotations to remove | 8 |
| `async fn` bodies to wrap in `Box::pin` | 14 (2 methods × 7 impl blocks) |
| `Arc<dyn Backend>` sites affected | 0 (unchanged) |
| Runtime behaviour change | None (identical boxing semantics) |
| Send bound preserved | Yes (`+ Send` retained) |
| Default impl preserved | Yes (`count_available` → `Box::pin(async { Ok(0) })`) |
| Rust edition required | 2021 (already set) |
| Minimum Rust version required | None (standard library features only) |

The proposed approach achieves complete removal of the `async-trait` proc-macro dependency by inlining the equivalent manual future boxing. The `Backend` trait remains object-safe, `Arc<dyn Backend>` usage is untouched, and no new dependencies are introduced.
