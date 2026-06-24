{
  system ? builtins.currentSystem,
  nixpkgs ? <nixpkgs>,
}:

let
  # TODO: use an ‘input pinner’ to lock in a stable Nixpkgs version
  # <nixpkgs> uses the system’s Nixpkgs version which is impure.
  pkgs = import nixpkgs { 
    inherit system; 
    overlays = [
      (import ./nix/overlay)
      (import ./nix/overlay/development.nix)
    ];
  };
in
{
  inherit (pkgs) zerostack;
  default = pkgs.zerostack;
  shell = pkgs.zerostack-dev-shell;
}
