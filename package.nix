{ lib, stdenv, craneLib, pkg-config, cmake, ninja, gtk3, babble, gcc, clang
, iconv, darwin, openusd-minimal, vulkan-sdk, xorg }:
craneLib.buildPackage rec {
  src = craneLib.cleanCargoSource (craneLib.path ./.);
  strictDeps = true;

  LIBCLANG_PATH = "${clang.cc.lib}/lib";
  VULKAN_SDK = "${vulkan-sdk}/x86_64";
  LD_LIBRARY_PATH = "${lib.makeLibraryPath buildInputs}";

  nativeBuildInputs = [ pkg-config cmake ninja gcc clang ];

  buildInputs = [ babble openusd-minimal ] ++ lib.optionals stdenv.isDarwin
    ([ iconv ] ++ (with darwin.apple_sdk.frameworks; [ Carbon Cocoa Kernel ]))
    ++ lib.optionals stdenv.isLinux ([ gtk3 ]);
}
