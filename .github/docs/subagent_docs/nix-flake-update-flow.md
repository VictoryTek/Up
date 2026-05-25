# Nix Flake Update Flow (VexOS)

Complete sequential process for flake-based NixOS (VexOS) updates in Up.

---

## Update Check (counting pending updates)

**Step 1 — Backend detection**
`which nix` (via `which::which`) — if `nix` is on PATH, the Nix backend is registered.

**Step 2 — NixOS & flake detection**
- Checks `/run/current-system` exists → NixOS confirmed
- Checks `/etc/nixos/flake.nix` exists → flake config confirmed

**Step 3a — Check for changed inputs (Nix ≥ 2.19, preferred)**
```sh
nix --extra-experimental-features "nix-command flakes" \
    --option eval-cache false \
    --option tarball-ttl 0 \
    flake update --dry-run /etc/nixos
```
Parses output for lines like `• Updated input '…':` to count changed inputs.

**Step 3b — Check fallback (all Nix versions)**
If the dry-run flag is unrecognized:
1. Creates `/tmp/up-nix-check-<timestamp>/`
2. Copies `/etc/nixos/flake.nix` and `/etc/nixos/flake.lock` into the temp dir
3. Runs (inside temp dir):
```sh
nix --extra-experimental-features "nix-command flakes" \
    --option eval-cache false \
    --option tarball-ttl 0 \
    flake update
```
4. Compares old vs new `flake.lock` (checks `locked.rev` and `locked.lastModified` per input)
5. Returns list of changed input names → count shown in UI

---

## Update Execution (when you click Update)

**Step 4 — Authentication (single Polkit prompt)**
```sh
pkexec /bin/sh
```
A long-lived elevated shell is opened once with stdin/stdout piped. All subsequent privileged commands are written to this shell's stdin — no repeated auth prompts.

**Step 5 — Flake attribute resolution**
Reads `/etc/nixos/vexos-variant` (filesystem read, no subprocess) to get the `nixosConfigurations` attribute name (e.g. `vexos-nvidia`). Validates it is safe (ASCII alphanumeric/`-`/`_`/`.`, max 253 chars).

**Step 6 — Flake update + rebuild (sent to the elevated shell from Step 4)**
```sh
env PATH=/run/current-system/sw/bin:/run/wrappers/bin:/nix/var/nix/profiles/default/bin \
  sh -c "
    stdbuf -oL -eL \
      nix --extra-experimental-features 'nix-command flakes' \
      flake update --flake /etc/nixos \
    && \
    stdbuf -oL -eL \
      nixos-rebuild switch --flake /etc/nixos#<vexos-variant> \
      --refresh --print-build-logs
  "
```

`stdbuf -oL -eL` forces line-buffered output so each build log line streams to the UI immediately.

**Step 7 — Completion detection**
The runner reads stdout line-by-line, forwarding each to the UI. It watches for a unique sentinel string to detect the exit code. If `nixos-rebuild switch` restarts systemd and kills the shell before the sentinel is written, the runner detects NixOS activation markers in the buffered output (e.g. `activating the configuration`, `setting up /etc`) and treats the unexpected EOF as success.

---

## Partial Update (selected inputs only)

Same as Step 6 but with named inputs:
```sh
stdbuf -oL -eL \
  nix --extra-experimental-features 'nix-command flakes' \
  flake update <input1> <input2> ... --flake /etc/nixos \
&& \
stdbuf -oL -eL \
  nixos-rebuild switch --flake /etc/nixos#<vexos-variant> \
  --refresh --print-build-logs
```

---

## Key Design Notes

- The check **never modifies** `/etc/nixos` — it either uses `--dry-run` or a temp-dir copy.
- `/etc/nixos/vexos-variant` determines the flake output attribute — not the hostname.
- One `pkexec` prompt covers both `flake update` and `nixos-rebuild switch`.
