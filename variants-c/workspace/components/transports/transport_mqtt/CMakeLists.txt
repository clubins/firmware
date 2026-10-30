# MQTT v2 transport component — the C port of the transport-mqtt crate's scaffold.
# REQUIRES mirrors the transport-mqtt crate's dependencies: clubins_core/proto_v2.
idf_component_register(
    SRCS "src/transport_mqtt.c"
    INCLUDE_DIRS "include"
    REQUIRES clubins_core proto_v2
)
