# Decibel microphone front-end + DSP pipeline component — the C port of the driver-decibel crate's scaffold.
# REQUIRES the shared core: the sensor contract and the measurement model
# live there (V2 parity — every driver crate depends on firmware-core).
idf_component_register(
    SRCS "src/decibel.c"
    INCLUDE_DIRS "include"
    REQUIRES clubins_core
)
