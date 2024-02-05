{ craneLib, pkg-config, cmake, ninja, glib, gtk3, babble, python311Packages, tbb
}:
craneLib.buildPackage {
  src = craneLib.cleanCargoSource (craneLib.path ./.);
  strictDeps = true;

  nativeBuildInputs = [ pkg-config cmake ninja ];

  buildInputs = [ gtk3 babble python311Packages.openusd tbb ];
}
