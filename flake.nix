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
      url = "github:expenses/openusd-minimal-nix/vulkan-support";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, crane, flake-utils, babble, openusd-minimal, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        args = {
          babble = babble.packages.${system}.default;
          vulkan-sdk = openusd-minimal.packages.${system}.vulkan-sdk;
          openusd-minimal = openusd-minimal.packages.${system}.default.override { vulkanSupport = true;};
          craneLib = crane.lib.${system};
        };
        nix-bundle-exe = pkgs.fetchgit {
          url = "https://github.com/3noch/nix-bundle-exe";
          rev = "3522ae68aa4188f4366ed96b41a5881d6a88af97";
          hash = "sha256-K9PT8LVvTLOm3gX9ZFxag0X85DFgB2vvJB+S12disWw=";
        };
      in {
        packages = rec {
        default = pkgs.callPackage ./package.nix args;
        bundle = pkgs.callPackage "${nix-bundle-exe}/default.nix" {} default;
        };
        
        devShells.default = with pkgs; mkShell {
          LIBCLANG_PATH = "${clang.cc.lib}/lib";
          VULKAN_SDK = "${args.vulkan-sdk}";

          packages = [
            args.babble args.openusd-minimal rustup pkg-config cmake ninja gcc clang
            vulkan-loader
            xorg.libXcursor
            xorg.libXrandr
            xorg.libXi
            tbb boost
          ];
        };
        });
}
