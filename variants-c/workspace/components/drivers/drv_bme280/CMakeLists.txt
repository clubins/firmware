# BME280 environmental sensor component — the C port of the driver-bme280 crate's scaffold.
# REQUIRES the shared core: the sensor contract and the measurement model
# live there (V2 parity — every driver crate depends on firmware-core).
idf_component_register(
    SRCS "src/bme280_driver.c"
    INCLUDE_DIRS "include"
    REQUIRES clubins_core
)
