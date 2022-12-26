{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };
  outputs = { self, nixpkgs, flake-utils }:
    let
      out = system:
        let pkgs = import nixpkgs {
          inherit system;
          overlays = [ self.overlays.default ];
        };
        in
        {
          devShells.default = pkgs.mkShell {
            buildInputs = [
              pkgs.stdenv
              pkgs.openssl
              pkgs.pkg-config
            ];
          };
        };
    in
    flake-utils.lib.eachDefaultSystem out // {
      overlays = {
        default = final: prev: { };
      };
    };
}
