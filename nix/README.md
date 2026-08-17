# Flake install (legacy Node build)

> **Note:** this packages the pre-Rust TypeScript tree, which still produces a binary named `torlnk`.
> swarmling is mid-rewrite; a Rust flake producing the `swarmling` binary arrives with the packaging
> milestone, and this directory is deleted when the TypeScript tree is.

Add this repo to your ```flake.nix```. The package is built using the unstable channel. You can overwrite this by setting ```inputs.nixpkgs.follows = "nixpkgs"``` (if your default is 26.05).

**The binary is executed as ```torlnk```.**

```nix
inputs = {
  ...
  swarmling.url = "github:biokraft/swarmling";
  ...
}
```

You can install the package in either home.nix or your configuration.nix depending on your preference.

**User**
```nix
# home.nix
{ pkgs, inputs, ... }: 

{
  home.packages = with pkgs; [
    ...
    inputs.swarmling.packages.${pkgs.system}.default
    ...
  ];
}
```

**System**
```nix
# configuration.nix
{ pkgs, inputs, ... }: 

{
  environment.systemPackages = with pkgs; [
    ...
    inputs.swarmling.packages.${pkgs.system}.default
    ...
  ];
}
```
