# Implementation Prompt: Nix Backend Migration to /etc/nixos/vexos-variant

## Context
You are implementing a change to the **Up** project's Nix backend to migrate from `/etc/nixos/up-flake-attr` to `/etc/nixos/vexos-variant`. This simplifies the flake attribute resolution logic significantly.

## Specification Reference
- Spec file: `.github/docs/subagent_docs/nixos_vexos_variant_spec.md`
- Affected code: `src/backends/nix.rs` → function `resolve_nixos_flake_attr()`

## Implementation Requirements

### 1. Change the Configuration File Constant
**Line ~72** in `src/backends/nix.rs`:

Change:
```rust
const CONFIG_FILE: &str = "/etc/nixos/up-flake-attr";
```

To:
```rust
const VARIANT_FILE: &str = "/etc/nixos/vexos-variant";
```

### 2. Simplify resolve_nixos_flake_attr() Function
**Lines ~100–146** in `src/backends/nix.rs`:

Remove:
- The `nix eval` command to list available configurations (current Step 2)
- The `/run/current-system` symlink parsing logic (current Step 3)
- The cross-reference validation logic (current Step 4)
- The complex error message that lists all available configurations

Keep:
- The file existence check for `/etc/nixos/vexos-variant` (line ~102–110)
- The `validate_flake_attr()` call for safety (line ~74–86)
- The hostname fallback (line ~145–146)

### 3. Update Function Documentation
**Lines ~54–70** in `src/backends/nix.rs`:

Update the doc comment to reflect the new simplified approach:

```rust
/// Determine the NixOS configuration attribute name to use for flake rebuilds.
///
/// The VexOS variant file `/etc/nixos/vexos-variant` provides the exact
/// flake attribute name. This file is created and maintained by the user's
/// NixOS configuration to explicitly track which variant is installed.
///
/// Resolution order:
///
/// 1. `/etc/nixos/vexos-variant` — user-maintained file containing the exact
///    flake attribute name (e.g. "vexos-nvidia"). Read once, used directly.
///    Create it with: `sudo sh -c 'echo vexos-nvidia > /etc/nixos/vexos-variant'`
///
/// 2. Fallback: raw hostname (`/proc/sys/kernel/hostname`) when the variant
///    file does not exist. This is a best-effort fallback for systems without
///    the explicit variant file.
///
/// 3. Validation: all candidate names pass `validate_flake_attr()` checks
///    before being returned to ensure safety.
fn resolve_nixos_flake_attr() -> Result<String, String> {
    // ...
}
```

### 4. Update Error Messages (if applicable)
If the hostname fallback is used (Step 2 above), the function should simply return:
```rust
validate_flake_attr(&nixos_hostname())
```

Remove the complex error message about cross-referencing.

### 5. Add Migration Note Comment
Add a comment near the constant definition to help existing users:

```rust
// Migration note: Previous versions used `/etc/nixos/up-flake-attr`.
//    Users should migrate to `/etc/nixos/vexos-variant` by running:
//    sudo sh -c 'echo vexos-nvidia > /etc/nixos/vexos-variant'
```

## Code Quality Requirements

1. **Follow Rust best practices**: No `unwrap()` (use `?` or `if let`)
2. **Maintain consistency**: Keep the same code style as the surrounding code
3. **Remove unused imports**: If any imports become unused after simplification
4. **Preserve safety**: Keep `validate_flake_attr()` calls unchanged
5. **Ensure build success**: Code must compile without warnings

## Testing Checklist

Before marking implementation as complete:
- [ ] Code compiles with `cargo build`
- [ ] No clippy warnings: `cargo clippy -- -D warnings`
- [ ] Formatting passes: `cargo fmt --check`
- [ ] Simplified function is clearly documented
- [ ] Removed approximately 50-60 lines of complex logic
- [ ] New logic is simple: check file → fallback to hostname

## Expected Outcome

After implementation:
- `resolve_nixos_flake_attr()` should be ~25-30 lines instead of ~70 lines
- No external process execution (`nix eval` removed)
- No JSON parsing or complex string manipulation
- Only two code paths: file check → hostname fallback
- Clear, unambiguous behavior

## Files to Modify

- `src/backends/nix.rs` — the only file that needs changes

## Return Format

After implementation is complete, return:
1. Summary of changes made
2. List of all modified file paths
3. Confirmation of successful build
