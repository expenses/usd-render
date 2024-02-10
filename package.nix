{ lib, stdenv, craneLib, pkg-config, cmake, ninja, glib, gtk3, babble, gcc
, python311Packages, tbb, iconv, darwin, openusd-minimal
, useMinimalUsd ? true }:
craneLib.buildPackage {
  src = craneLib.cleanCargoSource (craneLib.path ./.);
  strictDeps = true;

  nativeBuildInputs = [ pkg-config cmake ninja gcc ];

  buildInputs = [
    babble
    (if useMinimalUsd then openusd-minimal else python311Packages.openusd)
  ] ++ lib.optionals stdenv.isDarwin
    ([ iconv ] ++ (with darwin.apple_sdk.frameworks; [ Carbon Cocoa Kernel ]))
    ++ lib.optionals stdenv.isLinux ([ gtk3 ]);
}
