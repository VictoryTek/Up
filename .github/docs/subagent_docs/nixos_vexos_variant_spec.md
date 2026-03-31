# Nix Backend: Migrate to `/etc/nixos/vexos-variant` — Specification

**Feature:** Replace `/etc/nixos/up-flake-attr` with `/etc/nixos/vexos-variant` and simplify resolution logic  
**Spec Author:** Research & Specification Agent  
**Date:** 2026-03-31  
**Status:** Ready for Implementation  
**Affected Files:** `src/backends/nix.rs`

---

## 1. Current State Analysis

### 1.1 Relevant File: `src/backends/nix.rs`

The `resolve_nixos_flake_attr()` function implements automatic detection of the NixOS flake attribute name. The implementation uses a 4-step resolution order:

**Current Resolution Logic (lines ~100–140):**

```rust
fn resolve_nixos_flake_attr() -> Result<String, String> {
    const CONFIG_FILE: &str = "/etc/nixos/up-flake-attr";

    // Step 1: User-maintained explicit override file.
    if let Ok(content) = std::fs::read_to_string(CONFIG_FILE) {
        let name = content.trim().to_string();
        if !name.is_empty() {
            return validate_flake_attr(&name);
        }
    }

    // Step 2: List available nixosConfigurations from the flake.
    let available_names: Option<Vec<String>> = (|| {
        // ... runs nix eval to get attribute names ...
    })();

    // Step 3: Parse the running system name from /run/current-system symlink.
    let system_name: Option<String> = (|| {
        // ... parses nixos-system-<hostname>-<version> ...
    })();

    // Step 4: Cross-reference system_name with available_names.
    if let (Some(names), Some(sys_name)) = (&available_names, &system_name) {
        if names.contains(sys_name) {
            return validate_flake_attr(sys_name);
        }
        // If no match, return descriptive error
        return Err(format!(
            "Cannot determine the active NixOS configuration automatically..."
        ));
    }

    // Step 5: Fallback to raw hostname
    validate_flake_attr(&nixos_hostname())
}
```

### 1.2 Key Characteristics

1. **Primary source of truth**: `/etc/nixos/up-flake-attr` (user-maintained file)
2. **Secondary fallback**: Cross-reference of `nixosConfigurations` with `/run/current-system`
3. **Tertiary fallback**: Raw hostname
4. **Complexity**: Multiple steps involving process execution (`nix eval`), filesystem reading, and string parsing

### 1.3 Current Usage

The `resolve_nixos_flake_attr()` function is called in `NixBackend::run_update()` (lines ~190-220):

```rust
let config_name = match resolve_nixos_flake_attr() {
    Ok(n) => n,
    Err(e) => return UpdateResult::Error(e),
};
```

This config name is then used to construct the flake argument:
```rust
let flake_arg = format!("/etc/nixos#{}", config_name);
```

---

## 2. Problem Definition

### 2.1 Motivation for Change

The current implementation, while functional, has several issues:

1. **Filename ambiguity**: `up-flake-attr` is generic and doesn't clearly indicate the variant
2. **Unnecessary complexity**: The cross-referencing logic (steps 2–4) is convoluted and requires:
   - Running `nix eval` as an unprivileged user
   - Parsing JSON output
   - Reading symlinks and store paths
   - Complex string manipulation
3. **Multiple points of failure**: Each step can fail silently, leading to unexpected fallbacks
4. **Unclear intent**: It's not immediately obvious that this is specifically for VexOS variants

### 2.2 Proposed Solution

Migrate to a dedicated `/etc/nixos/vexos-variant` file that:
- Is created and maintained by the user's NixOS configuration
- Contains the exact flake attribute name (e.g., `vexos-nvidia`)
- Provides a single source of truth with no ambiguity

**Key simplification**: If `/etc/nixos/vexos-variant` exists, use it directly. Remove the complex cross-referencing logic since the variant file provides the exact flake attribute.

---

## 3. Proposed Implementation

### 3.1 Architecture Changes

**New Resolution Logic (3-step instead of 5-step):**

1. **Primary**: Check `/etc/nixos/vexos-variant` — if present, use it directly
2. **Secondary**: Fallback to hostname (remove nix eval, JSON parsing, cross-referencing)
3. **Validation**: Keep `validate_flake_attr()` for safety

### 3.2 Configuration File Format

**Path**: `/etc/nixos/vexos-variant`  
**Content**: Single line containing the exact flake attribute name  
**Example**:
```
vexos-nvidia
```

**Creation command** (user runs once):
```bash
sudo sh -c 'echo vexos-nvidia > /etc/nixos/vexos-variant'
```

**Validation rules** (same as current `validate_flake_attr()`):
- Non-empty string
- Max 2 length: 253 characters
- Only ASCII alphanumeric, hyphen, underscore, and dot allowed

### 3.3 Simplified Resolution Algorithm

```rust
fn resolve_nixos_flake_attr() -> Result<String, String> {
    const VARIANT_FILE: &str = "/etc/nixos/vexos-variant";

    // Step 1: Check for explicit variant file
    if let Ok(content) = std::fs::read_to_string(VARIANT_FILE) {
        let name = content.trim().to_string();
        if !name.is_empty() {
            return validate_flake_attr(&name);
        }
    }

    // Step 2: Fallback to hostname (no complex nix eval logic)
    validate_flake_attr(&nixos_hostname())
}
```

### 3.4 Key Differences from Current Implementation

| Aspect | Current | Proposed |
|--------|---------|----------|
| Config filename | `up-flake-attr` | `vexos-variant` |
| Primary resolution | File check | File check |
| Secondary resolution | `nix eval` + JSON parsing | Removed |
| Tertiary resolution | Cross-reference with `/run/current-system` | Removed |
| Final fallback | Hostname | Hostname |
| Lines of code | ~70 | ~25 |
| External commands | `nix eval` | None |
| Error messages | Complex, lists all configs | Simple |

---

## 4. Implementation Steps

### 4.1 Code Changes

**File**: `src/backends/nix.rs`

1. **Change constant** (line ~72):
   - Old: `const CONFIG_FILE: &str = "/etc/nixos/up-flake-attr";`
   - New: `const VARIANT_FILE: &str = "/etc/nixos/vexos-variant";`

2. **Simplify `resolve_nixos_flake_attr()` function** (lines ~74–146):
   - Remove Step 2: `nix eval` command execution
   - Remove Step 3: Parsing `/run/current-system` symlink
   - Remove Step 4: Cross-reference logic
   - Keep Step 1: File reading and validation
   - Simplify Step 5: Direct hostname fallback

3. **Update comments** (lines ~54–70):
   - Change documentation to reflect new approach
   - Remove references to complex resolution logic
   - Add note about `/etc/nixos/vexos-variant` file format

4. **Update error messages** (lines ~138–144):
   - Remove complex error about listing configurations
   - Simplify to: "Running system name does not match any flake configuration"
   - Suggest creating `/etc/nixos/vexos-variant` file

5. **Update migration comment**:
   - Add note for existing users with `up-flake-attr`
   - Suggest migrating to `vexos-variant` file

### 4.2 Migration Path

**For existing users with `/etc/nixos/up-flake-attr`:**

1. Check if `/etc/nixos/vexos-variant` exists
2. If not, fall back to hostname (as new behavior)
3. **Optional**: Keep backward compatibility by checking both files
   - Priority: `vexos-variant` first
   - Fallback: `up-flake-attr` for existing installations

**Recommended approach**: Do NOT maintain backward compatibility automatically. Let the explicit check fail, and the user will be prompted during the next upgrade to create the new file. This ensures all users are on the new, simpler configuration.

---

## 5. Testing Considerations

### 5.1 Unit Tests (if test infrastructure is added)

Test cases to cover:

| Test Name | Input | Expected Output |
|-----------|-------|-----------------|
| `test_variant_file_exists` | File exists with `vexos-nvidia` | `Ok("vexos-nvidia")` |
| `test_variant_file_empty` | File exists but is empty | Falls back to hostname |
| `test_variant_file_invalid` | File exists with `invalid@name!` | `Err("Invalid flake attribute name...")` |
| `test_variant_file_missing` | File does not exist | Falls back to hostname |
| `test_hostname_validation` | Hostname `test-host` | `Ok("test-host")` |

### 5.2 Manual Testing

1. **Fresh installation**:
   - No `/etc/nixos/vexos-variant` file
   - Run update → should fallback to hostname
   
2. **New configuration**:
   - Create `/etc/nixos/vexos-variant` with valid content
   - Run update → should use variant file content

3. **Migration scenario**:
   - Existing `/etc/nixos/up-flake-attr` file
   - Run update → should fallback to hostname (not use old file)
   - Create `/etc/nixos/vexos-variant`
   - Run update → should use new variant file

### 5.3 Build Validation

Ensure the changes compile:
```bash
cargo build
cargo clippy -- -D warnings
cargo fmt --check
```

---

## 6. Risk Assessment

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| Existing users lose config | Medium | Medium | Provide clear error message |
| Missing variant file causes unexpected fallback | Low | Low | Document in changelog |
| Validation logic changes | Low | Low | Keep existing `validate_flake_attr()` |
| Loss of cross-reference logic | Low | Low | Intentional simplification |

---

## 7. Documentation Updates

### 7.1 Code Comments

Update function documentation to reflect simplified approach:

```rust
/// Determine the NixOS configuration attribute name to use for flake rebuilds.
///
/// The NixOS variant file `/etc/nixos/vexos-variant` provides the exact
/// flake attribute name. This file is created by the user's NixOS configuration
/// to explicitly track which variant is installed.
///
/// Resolution order:
///
/// 1. `/etc/nixos/vexos-variant` — a user-maintained file containing exactly
///    the flake attribute name (e.g. "vexos-nvidia"). Recommended approach.
///    Create it with: `sudo sh -c 'echo vexos-nvidia > /etc/nixos/vexos-variant'`
///
/// 2. Fallback: raw hostname when the variant file is absent.
///
/// 3. Final validation: all candidate names pass `validate_flake_attr()` checks.
fn resolve_nixos_flake_attr() -> Result<String, String> {
    // ...
}
```

### 7.2 Migration Notes

Add a comment in the code:

```rust
// Migration note: Previous versions used `/etc/nixos/up-flake-attr`.
// Existing users should migrate to `/etc/nixos/vexos-variant` to ensure
// proper flake attribute resolution.
```

---

## 8. Acceptance Criteria

- [ ] `resolve_nixos_flake_attr()` checks only `/etc/nixos/vexos-variant`
- [ ] Cross-referencing logic completely removed
- [ ] No calls to `nix eval` or JSON parsing in resolution logic
- [ ] `validate_flake_attr()` remains unchanged for safety
- [ ] Hostname fallback preserved
- [ ] Code is simpler and easier to understand (~25 lines instead of ~70)
- [ ] Error messages are clear and actionable
- [ ] `cargo build` succeeds
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo fmt --check` passes

---

## 9. Spec Completeness

This specification includes:
- Current state analysis with code excerpts
- Problem definition and motivation
- Proposed implementation details
- Code-level changes
- Testing strategy
- Risk assessment
- Documentation requirements
- Acceptance criteria

**Implementation Note**: This spec intentionally does NOT include automatic backward compatibility for `up-flake-attr`. The change is a clean migration to a more explicit and simpler approach. Users with the old file will be prompted to create the new file on their next upgrade.
