# NixOS Update Implementation

## Current Implementation Summary

The `Up` application handles NixOS system updates differently based on whether the system uses **Flakes** or **Legacy Channels**.

### NixOS with Flakes (`is_nixos_flake() == true`)

On flake-based NixOS systems, the application executes **two sequential `pkexec` commands**:

#### Command 1: Update Flake Inputs
```bash
pkexec env PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin \
  nix --extra-experimental-features "nix-command flakes" \
  flake update \
  --flake /etc/nixos
```

#### Command 2: Rebuild System with Vexos Variant
```bash
pkexec env PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin \
  nixos-rebuild switch \
  --flake /etc/nixos#<variant_name>
```

Where `<variant_name>` is determined by reading `/etc/nixos/vexos-variant`:
1. Reads the file contents
2. Strips all whitespace (tabs, newlines, spaces)
3. Validates the variant name is a safe flake attribute (ASCII alphanumeric, hyphens, underscores, dots only)
4. Returns error if file is missing or empty

#### Example (VexOS with nvidia variant):
```bash
pkexec env PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin \
  nixos-rebuild switch \
  --flake /etc/nixos#vexos-nvidia
```

---

### NixOS without Flakes (`is_nixos_flake() == false`)

On legacy channel-based NixOS systems, the application executes **two sequential `pkexec` commands**:

#### Command 1: Update Channel Metadata
```bash
pkexec nix-channel --update
```

#### Command 2: Rebuild System
```bash
pkexec nixos-rebuild switch
```

These commands are simpler and do not require PATH manipulation or variant resolution.

---

## Key Implementation Details

1. **Privilege Escalation**: Both modes use `pkexec` for root access when executing system commands.

2. **PATH Configuration (Flake Mode Only)**: The flake mode explicitly sets:
   ```
   PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin
   ```
   Because `pkexec` resets to standard directories that may not include NixOS binaries.

3. **Two-Stage Execution**: Flake mode runs update and rebuild as **separate `runner.run()` calls** rather than a single shell command, avoiding shell injection vulnerabilities.

4. **Variant Detection**: The `resolve_nixos_flake_attr()` function:
   - Reads `/etc/nixos/vexos-variant` (primary source of truth)
   - Validates variant name using `validate_flake_attr()`
   - Returns error if file is missing, empty, or invalid

5. **No Fallback Logic**: Unlike the previous implementation, there is no automatic detection via hostname or `nix eval` cross-referencing. The variant file is required.

6. **Update Counting**: Success response counts non-empty output lines from nixos-rebuild.

---

## Example: VexOS Configuration

For a VexOS system with the nvidia variant, the user must ensure:

```bash
sudo sh -c 'echo vexos-nvidia > /etc/nixos/vexos-variant'
```

This file is created by the VexOS configuration during system installation or manually by the user.

## Files Involved

- Primary implementation: `src/backends/nix.rs`
  - `resolve_nixos_flake_attr()` function
  - `run_update()` method in `impl Backend for NixBackend`
  