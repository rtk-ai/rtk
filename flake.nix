{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };
  outputs = {
    self,
    nixpkgs,
    flake-utils,
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = import nixpkgs {inherit system;};
    in {
      packages.default = pkgs.callPackage ./nix/package.nix {};
    })
    // {
      overlays.default = final: prev: {
        rtk = final.callPackage ./nix/package.nix {};
      };

      homeManagerModules.default = import ./nix/homeManagerModule.nix self.packages;
    };
}
