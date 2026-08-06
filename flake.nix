{
  description = "Capability-scoped WebAssembly component plugin demo";

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
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        rust = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "clippy" "rust-src" "rustfmt" ];
          targets = [ "wasm32-wasip2" ];
        };
      in {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            rust
            cargo-component
            wasm-tools
          ];

          RUST_BACKTRACE = "1";
        };
      });
}
