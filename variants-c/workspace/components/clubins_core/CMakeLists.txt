# The shared core component (the port of V2's firmware-core crate): the
# typed measurement model, the sensor contract, the registry, and the HTTP
# JSON codec — pure platform-agnostic C with no IDF headers. Every
# component and the composition root consume this API.
#
# The exact same sources compile on the host under host/CMakeLists.txt for
# the CTest + Unity harness — the "one source, two personalities" rule the
# host-testing discipline rests on.
idf_component_register(
    SRCS "src/measurement.c" "src/registry.c" "src/http_json.c"
    INCLUDE_DIRS "include"
)
