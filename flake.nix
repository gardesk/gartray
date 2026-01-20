{
  description = "gartray - System tray and quick settings panel";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        };
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustToolchain
            pkg-config

            # X11 dependencies
            xorg.libX11
            xorg.libXcursor
            xorg.libXrandr
            xorg.libXi
            xorg.libxcb

            # GLib/GTK for gartk-render
            glib
            cairo
            pango

            # Audio (PulseAudio)
            libpulseaudio

            # D-Bus for SNI/zbus
            dbus
          ];

          shellHook = ''
            echo "gartray development shell"
            echo "Run 'cargo build' to build"
            echo "Run 'cargo run -- daemon' to start daemon"
          '';

          RUST_BACKTRACE = 1;
        };

        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "gartray";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = with pkgs; [ pkg-config ];
          buildInputs = with pkgs; [
            xorg.libX11
            xorg.libxcb
            glib
            cairo
            pango
            libpulseaudio
            dbus
          ];
        };
      }
    );
}
