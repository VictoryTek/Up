{
  description = "Up — a modern Linux system update & upgrade app";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        # crates.io now returns HTTP 403 for the legacy
        # `api/v1/crates/<name>/<version>/download` endpoint that this nixpkgs
        # pin hard-codes (User-Agent gating), which breaks every cold build.
        # Redirect the crate fetcher at the CDN, which serves the same path
        # shape and is not gated. `extraRegistries` overrides the built-in
        # crates.io-index download URL via the `//` merge in
        # import-cargo-lock.nix; the trailing sed then removes the spurious
        # extra `[source."…crates.io-index"]` block it also emits (cargo rejects
        # redefining its built-in `crates-io` source). Fixed-output derivations
        # are keyed on the sha256, not the URL, so crate store paths are
        # unchanged. Remove this once nixpkgs is bumped past the static.crates.io
        # fetcher fix.
        cargoDeps = (pkgs.rustPlatform.importCargoLock {
          lockFile = ./Cargo.lock;
          extraRegistries = {
            "https://github.com/rust-lang/crates.io-index" =
              "https://static.crates.io/crates";
          };
        }).overrideAttrs (old: {
          buildCommand = old.buildCommand + ''
            sed -i '\#^\[source\."https://github\.com/rust-lang/crates\.io-index"\]$#,+2 d' \
              $out/.cargo/config.toml
          '';
        });
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "up";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
          src = ./.;

          inherit cargoDeps;

          cargoBuildFlags = [ "--workspace" ];

          nativeBuildInputs = with pkgs; [
            pkg-config
            wrapGAppsHook4
            # glib provides glib-compile-resources (used by build.rs via glib-build-tools)
            glib
            # gtk4 provides gtk4-update-icon-cache used in postInstall
            gtk4
          ];

          buildInputs = with pkgs; [
            gtk4
            libadwaita
            dbus
            hicolor-icon-theme
          ];

          # wrapGAppsHook4 bakes XDG_DATA_DIRS from buildInputs into the wrapper
          # script, but does NOT add $out/share automatically. Without this, GTK
          # cannot find the icon installed to $out/share/icons/hicolor/ at runtime.
          preFixup = ''
            gappsWrapperArgs+=(--prefix XDG_DATA_DIRS : "$out/share")
          '';

          postInstall = ''
            install -Dm644 data/io.github.up.desktop \
              $out/share/applications/io.github.up.desktop
            install -Dm644 data/io.github.up.metainfo.xml \
              $out/share/metainfo/io.github.up.metainfo.xml
            install -Dm644 data/io.github.up.policy \
              $out/share/polkit-1/actions/io.github.up.policy
            install -Dm644 data/icons/hicolor/256x256/apps/io.github.up.png \
              $out/share/icons/hicolor/256x256/apps/io.github.up.png
            gtk4-update-icon-cache -qtf $out/share/icons/hicolor

            # Plugin backend descriptors
            install -Dm644 data/backends.d/apk.yaml \
              $out/share/up/backends.d/apk.yaml
            install -Dm644 data/backends.d/xbps.yaml \
              $out/share/up/backends.d/xbps.yaml
          '';

          meta = with pkgs.lib; {
            description = "A modern Linux system update & upgrade app";
            homepage = "https://github.com/user/up";
            license = licenses.gpl3Plus;
            platforms = platforms.linux;
            mainProgram = "up";
          };
        };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            cargo
            rustc
            rust-analyzer
            clippy
            rustfmt
            pkg-config
            meson
            ninja
          ];

          buildInputs = with pkgs; [
            gtk4
            libadwaita
            glib
            dbus
          ];
        };
      }) // {
        # Expose an overlay so NixOS configs can do:
        #   nixpkgs.overlays = [ inputs.up.overlays.default ];
        #   environment.systemPackages = [ pkgs.up ];
        overlays.default = final: prev: {
          up = self.packages.${final.stdenv.system}.default;
        };
      };
}
