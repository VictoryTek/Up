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
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "up";
          version = "1.0.1";
          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

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
            install -Dm644 data/icons/hicolor/256x256/apps/io.github.up.png \
              $out/share/icons/hicolor/256x256/apps/io.github.up.png
            gtk4-update-icon-cache -qtf $out/share/icons/hicolor
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
