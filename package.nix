{ lib, stdenv, craneLib, pkg-config, cmake, ninja, glib, gtk3, babble, gcc, clang, boost
, tbb, iconv, darwin, openusd-minimal
}:
craneLib.buildPackage {
  src = craneLib.cleanCargoSource (craneLib.path ./.);
  strictDeps = true;

  LIBCLANG_PATH="${clang.cc.lib}/lib";

  nativeBuildInputs = [ pkg-config cmake ninja gcc clang ];

  buildInputs = [
    babble
    boost
    tbb
    openusd-minimal
  ] ++ lib.optionals stdenv.isDarwin
    ([ iconv ] ++ (with darwin.apple_sdk.frameworks; [ Carbon Cocoa Kernel ]))
    ++ lib.optionals stdenv.isLinux ([ gtk3 ]);
}
