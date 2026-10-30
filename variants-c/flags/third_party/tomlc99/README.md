# tomlc99 — vendored

Parses the committed `config/default.toml` (spec §The config file) into the
config pool in `main/config.c`. Vendored rather than fetched so neither
device builds nor the host matrix job touches the network.

- Upstream: https://github.com/cktan/tomlc99
- Pinned: master @ `29076dfd095bbbbd50a3c1b2760d29f4b83e74ac` (fetched 2026-10-30)
- License: MIT, intact in `LICENSE`
- `toml.c` / `toml.h` are unmodified upstream sources; this directory sits
  outside the variant's clang-format scope (which formats `main/`, `host/`,
  and `tools/` only), and on the host it builds as its own library
  target so upstream warnings never trip the core's `-Werror`.
