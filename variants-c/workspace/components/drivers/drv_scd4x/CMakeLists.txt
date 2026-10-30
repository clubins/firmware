# SCD4x CO2 sensor component — the C port of the driver-scd4x crate's scaffold.
# REQUIRES the shared core: the sensor contract and the measurement model
# live there (V2 parity — every driver crate depends on firmware-core).
idf_component_register(
    SRCS "src/scd4x_driver.c"
    INCLUDE_DIRS "include"
    REQUIRES clubins_core
)
