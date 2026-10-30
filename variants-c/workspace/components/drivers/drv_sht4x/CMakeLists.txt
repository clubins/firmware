# SHT4x temperature/humidity sensor component — the C port of the driver-sht4x crate's scaffold.
# REQUIRES the shared core: the sensor contract and the measurement model
# live there (V2 parity — every driver crate depends on firmware-core).
idf_component_register(
    SRCS "src/sht4x_driver.c"
    INCLUDE_DIRS "include"
    REQUIRES clubins_core
)
