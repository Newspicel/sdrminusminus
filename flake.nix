{
  description = "SDR-- software-defined radio receiver";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      ...
    }:
    let
      systems = [
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          toolchainConfig = builtins.fromTOML (builtins.readFile ./rust-toolchain.toml);
          toolchain = pkgs.rust-bin.stable.${toolchainConfig.toolchain.channel}.minimal;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = toolchain;
            rustc = toolchain;
          };
          pnpmExe =
            {
              aarch64-linux = {
                arch = "arm64";
                hash = "sha256-VJHwSZxAB2dUM+RMfdhn0VJ1RjuLzKBTsSNbbj4gNyA=";
              };
              x86_64-linux = {
                arch = "x64";
                hash = "sha256-xwEBMvujPSv6ApKDVdGsgpGHOqsvTISaNPU4YsS3Qrs=";
              };
            }
            .${system};
          pnpm = pkgs.stdenv.mkDerivation rec {
            pname = "pnpm";
            version = "12.8.0";
            src = pkgs.fetchurl {
              url = "https://registry.npmjs.org/@pnpm/exe.linux-${pnpmExe.arch}/-/exe.linux-${pnpmExe.arch}-${version}.tgz";
              inherit (pnpmExe) hash;
            };
            nativeBuildInputs = [ pkgs.autoPatchelfHook ];
            buildInputs = [ pkgs.stdenv.cc.cc.lib ];
            installPhase = ''
              install -Dm755 pnpm $out/bin/pnpm
            '';
            passthru = {
              inherit (pkgs.pnpm_12) nodejs-slim;
              majorVersion = pkgs.lib.versions.major version;
            };
            meta.mainProgram = "pnpm";
          };
          sdrmmDesktop = pkgs.callPackage ./packaging/nix/package.nix {
            inherit pnpm rustPlatform;
          };
        in
        {
          default = sdrmmDesktop;
          sdrmm = sdrmmDesktop;
          sdrmm-desktop = sdrmmDesktop;
        }
      );
    };
}
