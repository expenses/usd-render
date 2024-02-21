(import <nixpkgs> {}).pkgsCross.mingwW64.stdenv.mkDerivation {
name = "windows";
src = ./target/x86_64-pc-windows-gnu/release/usd-render.exe;

dontUnpack = true;

installPhase = ''
    mkdir -p $out/bin
    cp $src $out/bin/usd-render.exe
'';
}