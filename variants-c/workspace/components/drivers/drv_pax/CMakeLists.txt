# PIR occupancy front-end component — the C port of the driver-pax crate's scaffold.
# REQUIRES the shared core: the sensor contract and the measurement model
# live there (V2 parity — every driver crate depends on firmware-core).
idf_component_register(
    SRCS "src/pir_driver.c"
    INCLUDE_DIRS "include"
    REQUIRES clubins_core
)
