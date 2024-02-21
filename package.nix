{ lib, stdenv, craneLib, pkg-config, cmake, ninja, babble, gcc, clang, iconv
, darwin, openusd-minimal, vulkan-sdk, xorg, tbb_2021_8, opensubdiv, shaderc
, libGL }:
craneLib.buildPackage {
  src = craneLib.cleanCargoSource (craneLib.path ./.);
  strictDeps = true;

  LIBCLANG_PATH = "${clang.cc.lib}/lib";
  VULKAN_SDK = "${vulkan-sdk}";
  OPENUSD_LIB_DIR = "${openusd-minimal}/lib";
  TBB_LIB_DIR = "${tbb_2021_8.override { static = true; }}/lib";
  OPENSUBDIR_LIB_DIR = "${opensubdiv.static}/lib";
  SHADERC_LIB_DIR = "${shaderc.static}/lib";
  OPENGL_LIB_DIR = "${libGL}/lib";

  nativeBuildInputs = [ pkg-config cmake ninja gcc clang ];

  buildInputs = [ babble openusd-minimal ] ++ lib.optionals stdenv.isDarwin
    ([ iconv ] ++ (with darwin.apple_sdk.frameworks; [ Carbon Cocoa Kernel ]))
    ++ lib.optionals stdenv.isLinux (with xorg; [ libXcursor libXrandr libXi ]);
}
