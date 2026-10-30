# Pinned build environment for the v5-hsm variant. Self-contained: pins the
# same nixpkgs release as the source monorepo's system.nix instead of
# importing it — this variant builds and runs CI from its own directory.
#
# CI runs `nix-shell --pure --run 'just <recipe>'` from this directory, so
# everything a recipe needs is in this shell (toolchain, just, espflash).
let
  nixpkgsSource = builtins.fetchTarball {
    url = "https://releases.nixos.org/nixos/26.05/nixos-26.05.12400.da6dc0e7b260/nixexprs.tar.xz";
    sha256 = "sha256-q1ku4ygV7tdmv5Q9QYwhVDsNMZRJcRYjVczoX5+XWBo=";
  };
  pkgs = import nixpkgsSource { };
in
pkgs.mkShell {
  packages = with pkgs; [
    (callPackage ./esp-toolchain.nix { })
    espflash
    just
  ];
}
