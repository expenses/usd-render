{ lib, stdenv, craneLib, pkg-config, cmake, ninja, babble, gcc, clang, iconv
, darwin, openusd-minimal, vulkan-sdk, xorg, pkgsCross, opensubdiv, shaderc
, libGL, bbl-usd-win, bbl-usd-win-headers }:
with pkgsCross;
craneLib.buildPackage {
  src = craneLib.cleanCargoSource (craneLib.path ./.);
  strictDeps = true;

  CARGO_BUILD_TARGET = "x86_64-pc-windows-gnu";

  LIBCLANG_PATH = "${clang.cc.lib}/lib";
  VULKAN_SDK = "${vulkan-sdk}";
  OPENUSD_LIB_DIR = "${openusd-minimal}/lib";
  TBB_LIB_DIR = "${mingwW64.tbb_2021_8}/lib";
  OPENSUBDIR_LIB_DIR = "${mingwW64.opensubdiv.static}/lib";
  SHADERC_LIB_DIR = "${mingwW64.shaderc.static}/lib";
  BBL_USD_LIB_DIR = "${bbl-usd-win}/lib";
  BBL_USD_HEADER = "${bbl-usd-win-headers}/src/openusd-c.h";
  MCFGTHREAD_LIB_DIR = "${mingwW64.windows.mcfgthreads}/bin";
  PTHREAD_LIB_DIR = "${mingwW64.windows.pthreads}/lib";

  nativeBuildInputs = [ babble clang ];
  buildInputs = [ openusd-minimal ];

  depsBuildBuild = [
    mingwW64.stdenv.cc
  ];
}
