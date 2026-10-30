# Status reporting capability component — the C port of the ops-status crate's scaffold.
# REQUIRES mirrors the ops-status crate's dependencies: clubins_core.
idf_component_register(
    SRCS "src/ops_status.c"
    INCLUDE_DIRS "include"
    REQUIRES clubins_core
)
