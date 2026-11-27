{
  description = "Antre - Personal AI gateway inspired by OpenClaw";

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
        toolchainConfig = builtins.fromTOML (builtins.readFile ./rust-toolchain.toml);
        nightly = pkgs.lib.removePrefix "nightly-" toolchainConfig.toolchain.channel;
        wasmCraneLib = (crane.mkLib pkgs).overrideToolchain (
          p:
          p.rust-bin.nightly.${nightly}.default.override {
            targets = [ "wasm32-wasip2" ];
          }
        );

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
              nativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.autoPatchelfHook ];
              buildInputs = [ pkgs.zlib ];
              installPhase = ''
                runHook preInstall
                install -Dm755 libzvec_c_api${pkgs.stdenv.hostPlatform.extensions.library} -t $out/lib
                runHook postInstall
              '';
            };

        antre-web-assets = pkgs.buildNpmPackage {
          inherit src;
          pname = "antre-web-assets";
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

        antre-wasm-tools = wasmCraneLib.buildPackage {
          inherit src;
          pname = "antre-wasm-tools";
          doCheck = false;
          cargoExtraArgs = "--target wasm32-wasip2 -p antre-wasm-calc -p antre-wasm-web-fetch -p antre-wasm-web-search ";
          nativeBuildInputs =
            with pkgs;
            [
              rustPlatform.bindgenHook
              cmake
              perl
              pkg-config
            ]
            ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
              pkgs.libiconv
            ];
        };
      in
      {
        packages.default = rustPlatform.buildRustPackage {
          pname = "antre";
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
            cp -r ${antre-web-assets}/. crates/web/src/assets/
            mkdir -p target/wasm32-wasip2/release/
            ln -s ${antre-wasm-tools}/lib/* target/wasm32-wasip2/release/
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
            "antre"
          ];

          # rebrand:keep-start
          # Services and wrappers written for Moltis still run `moltis`.
          postInstall = ''
            cat > $out/bin/moltis <<EOF
            #!${pkgs.runtimeShell}
            echo "warning: the moltis command is deprecated, run antre instead" >&2
            exec "$out/bin/antre" "\$@"
            EOF
            chmod +x $out/bin/moltis
          '';
          # rebrand:keep-end

          meta = with pkgs.lib; {
            description = "Personal AI gateway inspired by OpenClaw";
            homepage = "https://www.moltis.org/";
            license = licenses.mit;
            mainProgram = "antre";
          };
        };

        packages.antre = self.packages.${system}.default;
        # rebrand:keep-start
        packages.moltis =
          pkgs.lib.warn "the moltis package is deprecated, use antre (packages.default)"
            self.packages.${system}.default;
        # rebrand:keep-end

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
