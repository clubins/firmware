# Pinned development/CI shell for the V4 cyclic executive crate.
#
# This shell IS the toolchain pin: esp-rs Rust fork 1.97.0.0 + Xtensa GCC
# 15.2.0, same nixpkgs source as the monorepo. There is deliberately no
# rust-toolchain.toml in this crate — cargo reads toolchain override files
# natively, and an override naming the espup channel (`channel = "esp"`)
# breaks even nix-provided cargo when no rustup-linked channel exists
# (hosted-CI failure mode; see README "Why there is no rust-toolchain.toml").
#
# Standalone by design: this variant is self-contained and must not reference
# repo-root files (see docs/research/firmware-internals-build-and-ci.md for the
# recipe this adapts).
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
