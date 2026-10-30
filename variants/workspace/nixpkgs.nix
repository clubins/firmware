# Shared nixpkgs pin for the workspace shell. Mirrors the source monorepo's
# system.nix pin (nixos-26.05.12400.da6dc0e7b260) so local shells and CI build
# the same packages as the proven recipe in
# docs/research/firmware-internals-build-and-ci.md.
#
# To upgrade, follow the monorepo's system.nix comments (list the channel
# release, prefetch, convert the hash) and update url + sha256 together.
let
  nixpkgsSource = builtins.fetchTarball {
    url = "https://releases.nixos.org/nixos/26.05/nixos-26.05.12400.da6dc0e7b260/nixexprs.tar.xz";
    sha256 = "sha256-q1ku4ygV7tdmv5Q9QYwhVDsNMZRJcRYjVczoX5+XWBo=";
  };
in
# The import *function*, not the evaluated set — callers apply their
# config/overlays: `import ./nixpkgs.nix { }`.
import nixpkgsSource
