{
  description = "Minimalistic coding agent written in Rust, optimized for memory footprint and performance";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
      in
      {
        packages = {
          zerostack = pkgs.callPackage ./nix/package/zerostack.nix { };

          default = self.packages.${system}.zerostack;
        };

        apps = {
          zerostack = {
            type = "app";
            program = pkgs.lib.getExe self.packages.${system}.zerostack;
          };

          default = self.apps.${system}.zerostack;
        };

        devShells.default = pkgs.callPackage ./nix/package/dev-shell.nix {
          inherit (self.packages.${system}) zerostack;
        };
      }
    );
}
