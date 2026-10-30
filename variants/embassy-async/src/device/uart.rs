//! The device UART: one controller, exclusive to its sensor.
//!
//! esp-hal's async `read_async` parks until at least one byte is available
//! or the driver reports a line error, so the read loop in
//! [`clubins_firmware_v3::sensors::pms5003`] gets backpressure for free.
//! The read timeout here is what stands between a healthy idle bus and a
//! task parked forever on a dead sensor: a silent port surfaces as
//! [`UartError::Timeout`], which the framework treats as a bus fault with
//! backoff, not a hang.

use embassy_time::{Duration, with_timeout};
use esp_hal::uart::{Config, RxError, Uart};

use clubins_firmware_v3::sensors::uart::{UartError, UartPort};

/// How long a single `read` may wait for the sensor to speak. A
/// passive-mode single-read reply arrives well inside this; the point is
/// to turn "sensor unplugged" into a fault the framework can back off
/// from.
const READ_TIMEOUT: Duration = Duration::from_secs(1);

/// Baud the PMS5003 datasheet fixes for its command/data protocol.
const PMS_BAUDRATE: u32 = 9_600;

/// The UART1 front end handed to the PMS5003 task.
///
/// esp-hal's async-mode driver is deliberately `!Send`, so like the I²C
/// bus server the task constructs the controller itself, from the `Send`
/// peripheral tokens, inside the spawned future.
pub(super) struct SensorUart<'a> {
    port: Uart<'a, esp_hal::Async>,
}

impl<'a> SensorUart<'a> {
    /// Own the controller and pins: the sensor's TX feeds `rx` (frames
    /// in), our TX feeds the sensor's RX (commands out).
    pub(super) fn new(
        uart: esp_hal::peripherals::UART1<'static>,
        rx: esp_hal::peripherals::GPIO18<'static>,
        tx: esp_hal::peripherals::GPIO17<'static>,
    ) -> Self {
        let port = Uart::new(uart, Config::default().with_baudrate(PMS_BAUDRATE))
            .expect("uart1 init at the PMS5003's fixed 9600 baud")
            .with_rx(rx)
            .with_tx(tx)
            .into_async();
        Self { port }
    }
}

impl UartPort for SensorUart<'_> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, UartError> {
        match with_timeout(READ_TIMEOUT, self.port.read_async(buf)).await {
            Ok(result) => result.map_err(line_error),
            Err(_elapsed) => Err(UartError::Timeout),
        }
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), UartError> {
        // esp-hal 1.2.2's `TxError` is uninhabited (`enum TxError {}`):
        // writes queue into the FIFO and cannot fail, so the result is
        // discarded outright.
        let _ = self.port.write_async(bytes).await;
        Ok(())
    }
}

/// Every reported RX error is a line-level corruption or overrun; the
/// frame layer re-syncs on the next start pair either way. The wildcard
/// keeps future esp-hal error variants line-level too.
fn line_error(error: RxError) -> UartError {
    match error {
        RxError::FifoOverflowed
        | RxError::GlitchOccurred
        | RxError::FrameFormatViolated
        | RxError::ParityMismatch
        | _ => UartError::LineNoise,
    }
}
