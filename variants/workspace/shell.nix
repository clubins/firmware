# Dev + CI shell for the V2 workspace. Adapted from the monorepo recipe in
# docs/research/firmware-internals-build-and-ci.md. Every check runs inside
# this shell:
#
#   nix-shell --pure --run 'just check'
#
# --pure is load-bearing: it guarantees the build only sees the pinned
# toolchain, not whatever rustc the host happens to have (stable rustc breaks
# esp-hal <= 1.2.2).
{ pkgs ? import ./nixpkgs.nix { } }:
pkgs.mkShell {
  packages = with pkgs; [
    (callPackage ./esp-toolchain.nix { })
    just
  ];

  # nix-shell --pure leaves HOME unset (=/homeless-shelter); cargo needs a
  # writable home for the registry cache. /tmp is ephemeral by design.
  CARGO_HOME = "/tmp/firmware-v2-cargo-home";
}
