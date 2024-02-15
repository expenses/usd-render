{ lib, stdenv, craneLib, pkg-config, cmake, ninja, babble, gcc, clang, boost, tbb
, iconv, darwin, vulkan-loader, openusd-minimal, vulkan-sdk, xorg }:
craneLib.buildPackage {
  src = craneLib.cleanCargoSource (craneLib.path ./.);
  strictDeps = true;

  LIBCLANG_PATH = "${clang.cc.lib}/lib";
  VULKAN_SDK = "${vulkan-sdk}";

  nativeBuildInputs = [ pkg-config cmake ninja gcc clang ];

  buildInputs = [ babble openusd-minimal  vulkan-loader boost tbb ]
  ++ lib.optionals stdenv.isDarwin
    ([ iconv ] ++ (with darwin.apple_sdk.frameworks; [ Carbon Cocoa Kernel ]))
    ++ lib.optionals stdenv.isLinux (with xorg;[
      libXcursor
      libXrandr
      libXi
    ]);
}
