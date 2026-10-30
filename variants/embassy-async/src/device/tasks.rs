//! Device task skeletons — one async task per concern.
//!
//! SCAFFOLD STATUS: each task owns its cadence and its channel endpoint; the
//! drivers, DSP and encoders behind them arrive in their own flag-scoped
//! tasks. Skeletons never fabricate measurements — the only real events so
//! far are the ops-status boot report and heartbeat, which are contract-real
//! from the first boot (`service/status.go`).
//!
//! The transport skeleton drains both queues with a select; when both
//! transports are compiled, exactly one runs (per
//! `features::resolve_transport`) and the other exits immediately, so each
//! event reaches exactly one transport.

use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer};
use esp_println::println;

use clubins_firmware_v3::channels::{
    MeasurementReceiver, MeasurementSender, StatusReceiver, StatusSender,
};
use clubins_firmware_v3::config;
use clubins_firmware_v3::events::{StatusEvent, StatusResult};
use clubins_firmware_v3::session::Sequencer;

// ---- sensors ---------------------------------------------------------------

#[cfg(feature = "sensor-decibel")]
#[embassy_executor::task]
pub(super) async fn decibel_sensor_task(_measurements: MeasurementSender<'static>) {
    println!("decibel-sensor: skeleton; PDM capture + DSP arrive with the decibel task");
    loop {
        Timer::after(Duration::from_millis(config::SENSOR_POLL_INTERVAL_MS)).await;
    }
}

#[cfg(feature = "sensor-pax")]
#[embassy_executor::task]
pub(super) async fn pax_sensor_task(_measurements: MeasurementSender<'static>) {
    println!("pax-sensor: skeleton; GPIO PIR front-end arrives with the pax task");
    loop {
        Timer::after(Duration::from_millis(config::SENSOR_POLL_INTERVAL_MS)).await;
    }
}

#[cfg(feature = "sensor-bme280")]
#[embassy_executor::task]
pub(super) async fn bme280_sensor_task(_measurements: MeasurementSender<'static>) {
    println!("bme280-sensor: skeleton; I2C driver arrives with the bme280 task");
    loop {
        Timer::after(Duration::from_millis(config::SENSOR_POLL_INTERVAL_MS)).await;
    }
}

#[cfg(feature = "sensor-sht4x")]
#[embassy_executor::task]
pub(super) async fn sht4x_sensor_task(_measurements: MeasurementSender<'static>) {
    println!("sht4x-sensor: skeleton; I2C driver arrives with the sht4x task");
    loop {
        Timer::after(Duration::from_millis(config::SENSOR_POLL_INTERVAL_MS)).await;
    }
}

#[cfg(feature = "sensor-scd4x")]
#[embassy_executor::task]
pub(super) async fn scd4x_sensor_task(_measurements: MeasurementSender<'static>) {
    println!("scd4x-sensor: skeleton; I2C driver arrives with the scd4x task");
    loop {
        Timer::after(Duration::from_millis(config::SENSOR_POLL_INTERVAL_MS)).await;
    }
}

#[cfg(feature = "sensor-pms5003")]
#[embassy_executor::task]
pub(super) async fn pms5003_sensor_task(_measurements: MeasurementSender<'static>) {
    println!("pms5003-sensor: skeleton; UART driver arrives with the pms5003 task");
    loop {
        Timer::after(Duration::from_millis(config::SENSOR_POLL_INTERVAL_MS)).await;
    }
}

#[cfg(feature = "sensor-bh1750")]
#[embassy_executor::task]
pub(super) async fn bh1750_sensor_task(_measurements: MeasurementSender<'static>) {
    println!("bh1750-sensor: skeleton; I2C driver arrives with the bh1750 task");
    loop {
        Timer::after(Duration::from_millis(config::SENSOR_POLL_INTERVAL_MS)).await;
    }
}

// ---- ops -------------------------------------------------------------------

#[cfg(feature = "ops-status")]
fn status_event(seq: u32, result: StatusResult, message: &str) -> StatusEvent {
    // time_ms stays 0 until a time-sync task lands (NTP discipline):
    // "clock not synchronised" is the honest value, not a guess.
    let mut event = StatusEvent {
        time_ms: 0,
        seq,
        subsystem: heapless::String::new(),
        result,
        message: heapless::String::new(),
    };
    // These literals are well inside the heapless bounds; a panic means an
    // edit outgrew the buffer, which must fail loudly, not truncate.
    event
        .subsystem
        .push_str("lifecycle")
        .expect("subsystem fits StatusEvent buffer");
    event
        .message
        .push_str(message)
        .expect("message fits StatusEvent buffer");
    event
}

#[cfg(feature = "ops-status")]
#[embassy_executor::task]
pub(super) async fn ops_status_task(status: StatusSender<'static>) {
    // topics.md session lifecycle: the status topic's seq starts at 0 on
    // every boot — the boot report is its first message.
    let mut seq = Sequencer::new();
    // Boot report + heartbeat: both contracts expect status to flow from the
    // first boot. Without a compiled transport these queue up and the
    // backpressure blocks this task — honest, and harmless on the bench.
    status
        .send(status_event(
            seq.next_seq(),
            StatusResult::Started,
            "boot: embassy-async scaffold",
        ))
        .await;
    loop {
        Timer::after(Duration::from_millis(config::REPORT_INTERVAL_MS)).await;
        status
            .send(status_event(
                seq.next_seq(),
                StatusResult::Success,
                "heartbeat",
            ))
            .await;
    }
}

#[cfg(feature = "ota")]
#[embassy_executor::task]
pub(super) async fn ota_task() {
    println!("ota: skeleton; esp-ota update handling arrives with the ota task");
    loop {
        Timer::after(Duration::from_secs(3600)).await;
    }
}

#[cfg(feature = "provisioning")]
#[embassy_executor::task]
pub(super) async fn provisioning_task() {
    println!(
        "provisioning: skeleton; BLE Wi-Fi provisioning arrives with the provisioning task"
    );
    loop {
        Timer::after(Duration::from_secs(3600)).await;
    }
}

#[cfg(feature = "deep-sleep")]
#[embassy_executor::task]
pub(super) async fn deep_sleep_task() {
    println!(
        "deep-sleep: skeleton; duty-cycled sampling arrives with the deep-sleep task"
    );
    loop {
        Timer::after(Duration::from_secs(3600)).await;
    }
}

// ---- transports --------------------------------------------------------------

#[cfg(feature = "transport-http")]
#[embassy_executor::task]
pub(super) async fn http_transport_task(
    measurements: MeasurementReceiver<'static>,
    status: StatusReceiver<'static>,
    is_active: bool,
) {
    if !is_active {
        println!("http-transport: not the active transport; exiting");
        return;
    }
    println!(
        "http-transport: skeleton; Wi-Fi + HTTPS ingest arrive with the transport task"
    );
    loop {
        match select(measurements.receive(), status.receive()).await {
            Either::First(event) => println!(
                "http-transport: would POST /v1/sensors/{} (encoder arrives with the transport task)",
                event.payload.ingest_type()
            ),
            Either::Second(event) => println!(
                "http-transport: would POST /v1/sensors/status result={}",
                event.result.as_str()
            ),
        }
    }
}

#[cfg(feature = "transport-mqtt")]
#[embassy_executor::task]
pub(super) async fn mqtt_transport_task(
    measurements: MeasurementReceiver<'static>,
    status: StatusReceiver<'static>,
    is_active: bool,
) {
    if !is_active {
        println!("mqtt-transport: not the active transport; exiting");
        return;
    }
    println!(
        "mqtt-transport: skeleton; v2 protobuf client arrives with the transport task"
    );
    loop {
        match select(measurements.receive(), status.receive()).await {
            Either::First(event) => println!(
                "mqtt-transport: would publish v2/sensors/<uuid>/data/{} (proto encoder arrives later)",
                event.payload.ingest_type()
            ),
            Either::Second(event) => println!(
                "mqtt-transport: would publish retained status result={}",
                event.result.as_str()
            ),
        }
    }
}
