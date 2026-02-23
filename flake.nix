{
  description = "Pizza Order Manager for Freenet - Development Environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
          targets = [ "wasm32-unknown-unknown" ];
        };
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            # Rust toolchain
            rustToolchain
            cargo-watch
            cargo-edit
            cargo-make

            # Dioxus CLI
            dioxus-cli

            # Build dependencies
            pkg-config
            openssl
            nodejs

            # For WASM
            wasm-pack
            wasm-bindgen-cli
            binaryen  # wasm-opt

            # Development tools
            just  # Command runner (optional)
            bacon  # Background rust code checker (optional)
          ];

          # Environment variables
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";

          shellHook = ''
            echo "Freetalk Environment"
            echo ""
            echo "Available commands:"
            echo "  cargo test          - Run all tests"
            echo "  cargo check         - Type check the project"
            echo "  dx serve            - Start UI dev server (from ui/)"
            echo "  dx build --release  - Build UI for production"
            echo ""
            echo "Rust version: $(rustc --version)"
            echo "Dioxus CLI version: $(dx --version)"
          '';
        };
      }
    );
}
