# BH1750 ambient-light sensor component — the C port of the driver-bh1750 crate's scaffold.
# REQUIRES the shared core: the sensor contract and the measurement model
# live there (V2 parity — every driver crate depends on firmware-core).
idf_component_register(
    SRCS "src/bh1750_driver.c"
    INCLUDE_DIRS "include"
    REQUIRES clubins_core
)
