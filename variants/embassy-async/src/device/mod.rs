//! Xtensa device personality: boot, executor start, task spawning.
//!
//! The concurrency model is the variant's point of difference: no superloop.
//! One async task per concern, cooperating over the statically bounded
//! channels in `clubins_firmware_v3::channels`; feature flags decide which
//! tasks exist, and the runtime config decides which transport is active.

use embassy_executor::Spawner;
use esp_backtrace as _; // panic handler + backtrace printing
use esp_println::println;

use clubins_firmware_v3::channels::Queues;
use clubins_firmware_v3::config;
use clubins_firmware_v3::features::{self, TransportSelection};

mod tasks;

#[esp_hal::main]
async fn main(spawner: Spawner) {
    println!("clubins firmware v3 (embassy-async): boot");

    let peripherals = esp_hal::init(esp_hal::Config::default());
    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    static QUEUES: Queues = Queues::new();
    spawn_enabled_tasks(&spawner, &QUEUES);

    // The spawned tasks own all real work; this main task parks forever.
    loop {
        embassy_time::Timer::after(embassy_time::Duration::from_secs(3600)).await;
    }
}

/// Spawn one task per compiled capability. With every flag off there is
/// nothing to spawn — the flag matrix builds that combination — hence the
/// `allow(unused_variables)`.
#[allow(unused_variables)]
fn spawn_enabled_tasks(spawner: &Spawner, queues: &'static Queues) {
    // Producers first: sensors and ops must be ready to publish before the
    // transport starts draining.
    #[cfg(feature = "sensor-decibel")]
    spawner.spawn(tasks::decibel_sensor_task(queues.measurements.sender()).unwrap());
    #[cfg(feature = "sensor-pax")]
    spawner.spawn(tasks::pax_sensor_task(queues.measurements.sender()).unwrap());
    #[cfg(feature = "sensor-bme280")]
    spawner.spawn(tasks::bme280_sensor_task(queues.measurements.sender()).unwrap());
    #[cfg(feature = "sensor-sht4x")]
    spawner.spawn(tasks::sht4x_sensor_task(queues.measurements.sender()).unwrap());
    #[cfg(feature = "sensor-scd4x")]
    spawner.spawn(tasks::scd4x_sensor_task(queues.measurements.sender()).unwrap());
    #[cfg(feature = "sensor-pms5003")]
    spawner.spawn(tasks::pms5003_sensor_task(queues.measurements.sender()).unwrap());
    #[cfg(feature = "sensor-bh1750")]
    spawner.spawn(tasks::bh1750_sensor_task(queues.measurements.sender()).unwrap());

    #[cfg(feature = "ops-status")]
    spawner.spawn(tasks::ops_status_task(queues.status.sender()).unwrap());

    // Transports last. Both compiled: the runtime config picked the active
    // one; the other task exits immediately. `features::resolve_transport`
    // owns that decision and its tests.
    let selection = features::resolve_transport(
        config::ACTIVE_TRANSPORT,
        &features::compiled_capabilities(),
    );
    println!("transport selection: {selection:?}");
    #[cfg(feature = "transport-http")]
    spawner.spawn(
        tasks::http_transport_task(
            queues.measurements.receiver(),
            queues.status.receiver(),
            selection == TransportSelection::Http,
        )
        .unwrap(),
    );
    #[cfg(feature = "transport-mqtt")]
    spawner.spawn(
        tasks::mqtt_transport_task(
            queues.measurements.receiver(),
            queues.status.receiver(),
            selection == TransportSelection::Mqtt,
        )
        .unwrap(),
    );
    if selection == TransportSelection::None {
        println!("no transport compiled: events will queue until full (backpressure)");
    }
}
