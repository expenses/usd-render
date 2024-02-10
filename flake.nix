{
  description = "Build a cargo project without extra checks";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    babble = {
      url = "github:expenses/babble-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    openusd-minimal = {
      url = "github:expenses/openusd-minimal-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, crane, flake-utils, babble, openusd-minimal, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        args = {
          babble = babble.packages.${system}.default;
          openusd-minimal = openusd-minimal.packages.${system}.default;
          craneLib = crane.lib.${system};
        };
      in { packages.default = pkgs.callPackage ./package.nix args; });
}
