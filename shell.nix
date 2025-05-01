let
  pkgs = import <nixpkgs> { };
in
pkgs.mkShell {
  buildInputs = with pkgs; [
    pkg-config
    cargo
    rustup
    cmake
  ];
}