{
  description = "Legacy Node packaging for swarmling's pre-Rust tree. A Rust flake building the `swarmling` binary lands with the packaging milestone.";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:

    let
      systems = [
        "x86_64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;

    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.callPackage ./nix/package.nix { };
        }
      );
      overlays.default = final: prev: {
        swarmling-legacy = final.callPackage ./nix/package.nix { };
      };

    };

}
