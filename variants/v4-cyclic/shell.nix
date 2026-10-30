# Pinned development/CI shell for the V4 cyclic executive crate.
#
# Standalone by design: this variant is self-contained and must not reference
# repo-root files (see docs/research/firmware-internals-build-and-ci.md for the
# recipe this adapts). The pin matches the monorepo's shared nixpkgs source.
#
# All recipes assume this shell:
#   nix-shell --pure --run 'just check'
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
    jq
  ];
}
