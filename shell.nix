# Dev/CI shell for the Clubins ESP32-S3 firmware.
#
# Everything runs through `nix-shell --pure` so local builds and CI see the
# exact pinned toolchain (esp-rs Rust fork 1.97.0.0 + Xtensa GCC
# 15.2.0_20250920). Adapted from the monorepo recipe recorded in
# docs/research/firmware-internals-build-and-ci.md: pinned nixpkgs fetched by
# hash, no NixOS or self-hosted-runner assumptions (GitHub-hosted runners use
# this file directly).
let
  # Stable nixos-26.05 pin — the same channel the source monorepo pinned at
  # research time. Upgrade with:
  #   nix-prefetch-url --unpack <url>
  #   nix --extra-experimental-features nix-command hash to-sri --type sha256 <base32 hash>
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
