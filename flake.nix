{
  description = "Moltis - Personal AI gateway inspired by OpenClaw";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
      crane,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        nightly = "2025-11-30";
        wasmCraneLib = (crane.mkLib pkgs).overrideToolchain (
          p:
          p.rust-bin.nightly.${nightly}.default.override {
            targets = [ "wasm32-wasip2" ];
          }
        );

        # Pinned nightly to avoid recursion limit overflow in matrix-sdk
        # Latest nightly (2026-04) has query depth changes that break matrix-sdk 0.16
        rustToolchain = pkgs.rust-bin.nightly.${nightly}.default;

        rustPlatform = pkgs.makeRustPlatform {
          cargo = rustToolchain;
          rustc = rustToolchain;
        };

        # Create a clean source that includes necessary files and the wit directory
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter =
            path: type: (pkgs.lib.cleanSourceFilter path type) || (builtins.match ".*/wit.*" path != null);
        };

        zvecPrebuilt =
          let
            prebuilts = {
              x86_64-linux = {
                target = "x86_64-unknown-linux-gnu";
                hash = "sha256-4BLQIbBR/3w0oMmZfoAUpF2I6o5Ijzmy8+yTS5lDiSU=";
              };
              aarch64-linux = {
                target = "aarch64-unknown-linux-gnu";
                hash = "sha256-xzLD0rc7kWz3jNizT6MmjCjOiqXRVjNaZKDEKLNeBEI=";
              };
              aarch64-darwin = {
                target = "aarch64-apple-darwin";
                hash = "sha256-xihSumf3Mm+SzD0UWlWRs+H7pAkP+cehFD2VacFr73I=";
              };
            };
            prebuilt = prebuilts.${system} or null;
          in
          if prebuilt == null then
            null
          else
            pkgs.stdenvNoCC.mkDerivation {
              pname = "zvec-prebuilt";
              version = "0.6.0";
              src = pkgs.fetchurl {
                inherit (prebuilt) hash;
                url = "https://github.com/zvec-ai/zvec-rust/releases/download/v0.6.0/zvec-prebuilt-${prebuilt.target}.tar.gz";
              };
              sourceRoot = ".";
              dontConfigure = true;
              dontBuild = true;
              nativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [ pkgs.autoPatchelfHook ];
              buildInputs = [ pkgs.zlib ];
              installPhase = ''
                runHook preInstall
                install -Dm755 libzvec_c_api${pkgs.stdenv.hostPlatform.extensions.library} -t $out/lib
                runHook postInstall
              '';
            };

        moltis-web-assets = pkgs.buildNpmPackage {
          inherit src;
          pname = "moltis-web-assets";
          version = "0.1.0";
          npmRoot = "crates/web/ui";
          npmDeps = pkgs.fetchNpmDeps {
            src = ./crates/web/ui;
            hash = "sha256-h5taxiKvdgRj/1HO/sEMt53NM9jVdKcPn03PCADCYGM=";
          };
          buildPhase = ''
            runHook preBuild
            npm run build:all --prefix crates/web/ui
            runHook postBuild
          '';
          installPhase = ''
            runHook preInstall
            mkdir -p $out
            cp -r crates/web/src/assets/. $out/
            runHook postInstall
          '';
        };

        moltis-wasm-tools = wasmCraneLib.buildPackage {
          inherit src;
          pname = "moltis-wasm-tools";
          doCheck = false;
          cargoExtraArgs = "--target wasm32-wasip2 -p moltis-wasm-calc -p moltis-wasm-web-fetch -p moltis-wasm-web-search ";
          nativeBuildInputs =
            with pkgs;
            [
              rustPlatform.bindgenHook
              cmake
              perl
              pkg-config
            ]
            ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
              pkgs.libiconv
            ];
        };
      in
      {
        packages.default = rustPlatform.buildRustPackage {
          pname = "moltis";
          version = "0.1.0";
          inherit src;
          doCheck = false;

          buildNoDefaultFeatures = true;
          # zvec-rust publishes no x86_64-darwin prebuilt; use portable there.
          buildFeatures = [
            (if zvecPrebuilt == null then "portable" else "full")
            "embedded-assets"
            "embedded-wasm"
          ];
          preBuild = ''
            # The crate expects prefixed symbols; nixpkgs' jemalloc is unprefixed.
            unset JEMALLOC_OVERRIDE
            cp -r ${moltis-web-assets}/. crates/web/src/assets/
            mkdir -p target/wasm32-wasip2/release/
            ln -s ${moltis-wasm-tools}/lib/* target/wasm32-wasip2/release/
          '';
          cargoDeps = rustPlatform.importCargoLock {
            lockFile = ./Cargo.lock;
            allowBuiltinFetchGit = true;
          };
          nativeBuildInputs = with pkgs; [
            rustPlatform.bindgenHook
            cmake
            perl
            pkg-config
          ];
          buildInputs =
            with pkgs;
            [
              aws-lc
              openssl
              sqlite
              zlib
              zstd
            ]
            ++ pkgs.lib.optional (zvecPrebuilt != null) zvecPrebuilt;
          AWS_LC_SYS_USE_SYSTEM = "1";
          ZVEC_LIB_DIR = pkgs.lib.optionalString (zvecPrebuilt != null) "${zvecPrebuilt}/lib";
          cargoBuildFlags = [
            "--bin"
            "moltis"
          ];
          MOLTIS_VERSION = toString (self.shortRev or self.dirtyShortRev or self.lastModified or "nix");

          meta = with pkgs.lib; {
            description = "Personal AI gateway inspired by OpenClaw";
            homepage = "https://www.moltis.org/";
            license = licenses.mit;
            mainProgram = "moltis";
          };
        };

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustPlatform.bindgenHook
            pkgs.rust-bin.nightly.${nightly}.default
            rust-analyzer
            cmake
            perl
            pkg-config
          ];
        };
      }
    );
}
