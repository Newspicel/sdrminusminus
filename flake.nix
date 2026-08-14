{
  description = "sdr-- software-defined radio receiver";

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
          toolchainDate = pkgs.lib.removePrefix "nightly-" toolchainConfig.toolchain.channel;
          toolchain = pkgs.rust-bin.nightly.${toolchainDate}.minimal;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = toolchain;
            rustc = toolchain;
          };
          pnpm = pkgs.pnpm_11.overrideAttrs {
            version = "11.15.1";
            src = pkgs.fetchurl {
              url = "https://registry.npmjs.org/pnpm/-/pnpm-11.15.1.tgz";
              hash = "sha256-J0YGKbEBEWBOf5iIJ1O1M5iYaCDCDgoGXzpKXp59tx8=";
            };
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
