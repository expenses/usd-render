{
  description = "Build a cargo project without extra checks";

  inputs = {
    nixpkgs.url = "github:expenses/nixpkgs/my-patches-for-openusd";
    flake-utils.url = "github:numtide/flake-utils";
    openusd-minimal.url = "github:expenses/openusd-minimal-nix/windows-tbb";

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    babble = {
      url = "github:expenses/babble-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    bbl-usd = {
      url = "github:expenses/bbl-usd-nix";
      inputs = {
        nixpkgs.follows = "nixpkgs";
        openusd-minimal.follows = "openusd-minimal";
        babble.follows = "babble";
      };
    };
  };

  outputs = { nixpkgs, crane, flake-utils, babble, openusd-minimal, bbl-usd
    , fenix, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        openusdPkgs = openusd-minimal.packages.${system};
        bbl-usd-win-headers = bbl-usd.packages.${system}.default.override {
          openusd-minimal = openusdPkgs.windows;
        };
        bbl-usd-win = bbl-usd.packages.${system}.windows.override {
          headers = bbl-usd-win-headers;
          openusd-minimal = openusdPkgs.windows;
        };

        args = {
          babble = babble.packages.${system}.default;
          vulkan-sdk = openusdPkgs.vulkan-sdk;
          openusd-minimal = openusdPkgs.default.override {
            vulkanSupport = true;
            static = true;
          };
          craneLib = crane.lib.${system};
        };
        windowsArgs = args // {
          vulkan-sdk = openusdPkgs.vulkan-sdk-win;
          openusd-minimal = openusdPkgs.windows.override {
            vulkanSupport = true;
            static = true;
          };
          inherit bbl-usd-win-headers bbl-usd-win;
          craneLib = (crane.mkLib pkgs).overrideToolchain (
            with fenix.packages.${system};
          combine [
            stable.rustc
            stable.cargo
            targets.x86_64-pc-windows-gnu.stable.rust-std
          ]
          );

        };
        nix-bundle-exe = pkgs.fetchgit {
          url = "https://github.com/3noch/nix-bundle-exe";
          rev = "3522ae68aa4188f4366ed96b41a5881d6a88af97";
          hash = "sha256-K9PT8LVvTLOm3gX9ZFxag0X85DFgB2vvJB+S12disWw=";
        };
      in {
        packages = rec {
          default = pkgs.callPackage ./package.nix args;
          windows = pkgs.callPackage ./windows.nix windowsArgs;
          bundle = pkgs.callPackage "${nix-bundle-exe}/default.nix" { } default;
        };

        devShells = {
          default = with pkgs;
            mkShell {
              LIBCLANG_PATH = "${clang.cc.lib}/lib";
              VULKAN_SDK = "${args.vulkan-sdk}";
              OPENUSD_LIB_DIR = "${args.openusd-minimal}/lib";
              TBB_LIB_DIR = "${tbb_2021_8.override { static = true; }}/lib";
              OPENSUBDIR_LIB_DIR = "${opensubdiv.static}/lib";
              SHADERC_LIB_DIR = "${shaderc.static}/lib";
              OPENGL_LIB_DIR = "${libGL}/lib";

              packages = [ args.babble args.openusd-minimal xorg.libX11 ];
            };
          windows = with pkgs.pkgsCross.mingwW64;
            mkShell {
              LIBCLANG_PATH = "${pkgs.clang.cc.lib}/lib";
              VULKAN_SDK = "${windowsArgs.vulkan-sdk}";
              OPENUSD_LIB_DIR = "${windowsArgs.openusd-minimal}/lib";
              TBB_LIB_DIR = "${tbb_2021_8}/lib";
              OPENSUBDIR_LIB_DIR = "${opensubdiv.static}/lib";
              SHADERC_LIB_DIR = "${shaderc.static}/lib";
              BBL_USD_LIB_DIR = "${bbl-usd-win}/lib";
              BBL_USD_HEADER = "${bbl-usd-win-headers}/src/openusd-c.h";
              MCFGTHREAD_LIB_DIR = "${windows.mcfgthreads}/bin";
              PTHREAD_LIB_DIR = "${windows.pthreads}/lib";

              packages = [ windowsArgs.babble windowsArgs.openusd-minimal ];
            };
        };
      });
}
