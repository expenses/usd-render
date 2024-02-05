{ lib, stdenv, craneLib, pkg-config, cmake, ninja, glib, gtk3, babble, python311Packages, tbb
}:
craneLib.buildPackage {
  src = craneLib.cleanCargoSource (craneLib.path ./.);
  strictDeps = true;

  nativeBuildInputs = [ pkg-config cmake ninja ];

  buildInputs = [ babble ]
    ++ lib.optionals stdenv.isLinux ([ python311Packages.openusd gtk3 ]);
}
