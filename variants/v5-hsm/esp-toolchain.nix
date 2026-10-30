# Xtensa Rust toolchain for the ESP32-S3, matching what `espup install` sets up
# as the rustup `esp` channel. Upstream rustc has no Xtensa backend and nixpkgs
# does not package the esp-rs fork, so this repackages the prebuilt releases.
#
# Adapted from the source monorepo's hardware-firmware/esp-toolchain.nix
# (proven recipe: docs/research/firmware-internals-build-and-ci.md). Made
# self-contained so this variant builds from its own directory.
#
# To upgrade: pick a release from https://github.com/esp-rs/rust-build/releases
# and the GCC version espup pairs with it (`espup install` prints it, or see
# https://github.com/espressif/crosstool-NG/releases), then update the versions
# and hashes below. Get each hash with:
#   nix-prefetch-url <url>
#   nix --extra-experimental-features nix-command hash convert --hash-algo sha256 --to sri <hash>
{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
  symlinkJoin,
  zlib,
  zstd,
}:
let
  rustVersion = "1.97.0.0";
  gccVersion = "15.2.0_20250920";
  platform =
    {
      x86_64-linux = {
        rustTarget = "x86_64-unknown-linux-gnu";
        gccTarget = "x86_64-linux-gnu";
        rustHash = "sha256-qZv+5pIh6f9thjiPaBHuaIzUBeagQAo80XhOjUY+nZk=";
        gccHash = "sha256-49d60UVEgUUnu+ei0PeexFkqTiM5LFHHOIwOaGtqaXc=";
      };
      aarch64-darwin = {
        rustTarget = "aarch64-apple-darwin";
        gccTarget = "aarch64-apple-darwin";
        rustHash = "430fcbf54967e99d16debe48926a1df558ab8b67af3c060a658d25ce752bd790";
        gccHash = "6f48b791c5b849339a521a000ccbc6cd83e702ad36c7dd6983d057d0c868be33";
      };
    }
    .${stdenv.hostPlatform.system}
      or (throw "The ESP toolchain supports x86_64-linux and aarch64-darwin.");

  rust = stdenv.mkDerivation {
    pname = "rust-esp";
    version = rustVersion;

    srcs = [
      (fetchurl {
        url = "https://github.com/esp-rs/rust-build/releases/download/v${rustVersion}/rust-${rustVersion}-${platform.rustTarget}.tar.xz";
        sha256 = platform.rustHash;
      })
      (fetchurl {
        url = "https://github.com/esp-rs/rust-build/releases/download/v${rustVersion}/rust-src-${rustVersion}.tar.xz";
        hash = "sha256-Vo1oi5+PMy7E0EZXVE+tI+mc4R6ebPWDWXnmiijGi3M=";
      })
    ];
    sourceRoot = ".";

    nativeBuildInputs = lib.optionals stdenv.isLinux [ autoPatchelfHook ];
    buildInputs = lib.optionals stdenv.isLinux [
      stdenv.cc.cc.lib
      zlib
    ];

    dontConfigure = true;
    dontBuild = true;
    dontStrip = true;

    installPhase = ''
      runHook preInstall
      patchShebangs rust-nightly-${platform.rustTarget}/install.sh rust-src-nightly/install.sh
      rust-nightly-${platform.rustTarget}/install.sh --prefix=$out --disable-ldconfig \
        --components=rustc,cargo,rust-std-${platform.rustTarget},clippy-preview,rustfmt-preview
      rust-src-nightly/install.sh --prefix=$out --disable-ldconfig
      runHook postInstall
    '';
  };

  gcc = stdenv.mkDerivation {
    pname = "xtensa-esp-elf-gcc";
    version = gccVersion;

    src = fetchurl {
      url = "https://github.com/espressif/crosstool-NG/releases/download/esp-${gccVersion}/xtensa-esp-elf-${gccVersion}-${platform.gccTarget}.tar.xz";
      sha256 = platform.gccHash;
    };

    nativeBuildInputs = lib.optionals stdenv.isLinux [ autoPatchelfHook ];
    buildInputs = lib.optionals stdenv.isLinux [
      stdenv.cc.cc.lib
      zlib
      zstd
    ];

    dontConfigure = true;
    dontBuild = true;
    dontStrip = true;
    # On macOS, skip Nix's automatic binary adjustments to preserve the supplied
    # compiler's code signatures and avoid modifying binaries built for the ESP32.
    dontFixup = stdenv.isDarwin;

    installPhase = ''
      runHook preInstall
      cp -r . $out
      runHook postInstall
    '';
  };
in
symlinkJoin {
  name = "esp-toolchain-${rustVersion}";
  paths = [
    rust
    gcc
  ];
  meta.platforms = [
    "x86_64-linux"
    "aarch64-darwin"
  ];
}
