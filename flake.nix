{
  description = "Up — a modern Linux system update & upgrade app";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
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
          version = "0.1.0";
          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          nativeBuildInputs = with pkgs; [
            pkg-config
            meson
            ninja
            wrapGAppsHook4
          ];

          buildInputs = with pkgs; [
            gtk4
            libadwaita
            glib
            dbus
          ];

          meta = with pkgs.lib; {
            description = "A modern Linux system update & upgrade app";
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
      });
}
