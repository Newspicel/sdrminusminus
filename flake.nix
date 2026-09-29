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
          pnpm =
            (pkgs.pnpm_12.override {
              version = "12.8.0";
              srcHash = "sha256-zOCOCsuOHWeKmndr+Y9rooj9KxxnppAC7k6CYPEX+3s=";
              cargoHash = "sha256-rT3kFHLPVSwAqM2e0HXfeF2rGG/btFr3lsokVhs9OIk=";
            }).overrideAttrs
              (old: {
                postPatch = (old.postPatch or "") + "rm .cargo/config.toml\n";
              });
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
