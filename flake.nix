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
    flake-utils.lib.eachSystem [
      "aarch64-darwin"
      "aarch64-linux"
      "x86_64-linux"
    ] (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        rust = pkgs.rust-bin.nightly.latest.default.override {
          extensions = [ "clippy" "rust-analyzer" "rust-src" "rustfmt" ];
        };
        wasiSdkAsset = {
          aarch64-darwin = {
            name = "arm64-macos";
            hash = "sha256-gdgfr7zTw7XRqwNVu1e5bv+yzggaJKJ2P3wVUBgJsm4=";
          };
          aarch64-linux = {
            name = "arm64-linux";
            hash = "sha256-hb3N+itPqOH98Z1eDT/tAe/W6BgdSibhat7z/6+KjeY=";
          };
          x86_64-linux = {
            name = "x86_64-linux";
            hash = "sha256-EU7l/mPLXypsD+vO5EYGAmcpgVikCcerOsDgbm1L2c4=";
          };
        }.${system};
        wasiSdkArchive = pkgs.fetchurl {
          url = "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-34-rc.2/wasi-sdk-34.0-rc.2-${wasiSdkAsset.name}.tar.gz";
          inherit (wasiSdkAsset) hash;
        };
        wasiSdk = pkgs.stdenvNoCC.mkDerivation {
          pname = "wasi-sdk";
          version = "34.0-rc.2";
          src = wasiSdkArchive;
          nativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
            pkgs.autoPatchelfHook
          ];
          buildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
            pkgs.libxml2
            pkgs.ncurses
            pkgs.stdenv.cc.cc.lib
            pkgs.zlib
            pkgs.zstd
          ];
          installPhase = ''
            runHook preInstall
            cp -R . "$out"
            runHook postInstall
          '';
        };
        wasiLibcSource = pkgs.fetchFromGitHub {
          owner = "WebAssembly";
          repo = "wasi-libc";
          rev = "f3e872871c6fb77db9727a18ab812a1a8f6e85ce";
          hash = "sha256-zume/L6ovStZnbfSRX/J0T1XofjyIQGsnp60RpEVabk=";
        };
        wasiSysroot = pkgs.stdenvNoCC.mkDerivation {
          pname = "wasi-sysroot-wasip3-coop";
          version = "34.0-rc.2";
          src = wasiLibcSource;
          nativeBuildInputs = with pkgs; [ cmake gnumake wasm-tools ];
          cmakeFlags = [
            "-DCMAKE_C_COMPILER=${wasiSdk}/bin/clang"
            "-DCMAKE_AR=${wasiSdk}/bin/llvm-ar"
            "-DCMAKE_RANLIB=${wasiSdk}/bin/llvm-ranlib"
            "-DCMAKE_NM=${wasiSdk}/bin/llvm-nm"
            "-DTARGET_TRIPLE=wasm32-wasip3"
            "-DENABLE_COOP_THREADS=ON"
            "-DBUILD_SHARED=OFF"
            "-DBINDINGS_TARGET=OFF"
            "-DUSE_WASM_COMPONENT_LD=OFF"
            "-DBUILTINS_LIB=${wasiSdk}/lib/clang/23/lib/wasm32-unknown-wasip3/libclang_rt.builtins.a"
          ];
          postInstall = ''
            wrapper_dir=$(mktemp -d)
            cd "$wrapper_dir"
            ${wasiSdk}/bin/llvm-ar x \
              "$out/lib/wasm32-wasip3/libc.a" \
              __cabi_realloc_wrapper.S.obj
            mv __cabi_realloc_wrapper.S.obj \
              "$out/lib/wasm32-wasip3/__cabi_realloc_wrapper.o"
          '';
        };
      in {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            rust
            jq
          ];

          RUST_BACKTRACE = "1";
          WASI_SYSROOT = wasiSysroot;
        };
      });
}
