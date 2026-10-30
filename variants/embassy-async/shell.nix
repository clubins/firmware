# Development shell for the embassy-async firmware variant.
#
# Self-contained: the nixpkgs pin is inlined below (the source monorepo shares
# its pin via a repo-root system.nix, which this variant tree deliberately does
# not reference — see the isolation rules in the README).
#
# Entry point for every check, build and test:
#   nix-shell --pure --run 'just check'
#
# The pinned fork toolchain provides rustc, cargo, clippy and rustfmt;
# espflash is intentionally not included until a task actually flashes
# hardware (compile-level verification only so far).
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
    just
  ];
}
