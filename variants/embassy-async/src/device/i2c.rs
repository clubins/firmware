//! The device I²C bus: one server task, per-sensor clients.
//!
//! BME280, SHT4x and SCD4x share one I2C0 controller, and esp-hal's
//! async-mode drivers are deliberately `!Send` (the `Async` marker holds a
//! `*const ()`), so the controller can neither live in a static nor cross
//! a spawn boundary. The one task that needs it — the bus server —
//! therefore constructs it from the `Send` peripheral tokens and serves
//! typed requests: every sensor task holds a zero-sized [`SharedI2c`]
//! client whose transactions queue on one bounded request channel and
//! come back on the client's own one-deep reply slot.
//!
//! That is the task-architecture version of a bus mutex, at the
//! granularity the drivers want anyway: a transaction is atomic (the
//! server serves one request at a time) while everything between
//! transactions — a forced-mode conversion, a Sensirion measurement
//! window — belongs to the sensor task, so one sensor's conversion never
//! keeps another off the bus.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use esp_hal::i2c::master::{Config, Error as HalError, I2c};

use clubins_firmware_v3::sensors::i2c::{BusError, I2cBus};

/// The longest read any compiled driver issues: the BME280's 26-byte
/// temperature/pressure calibration block.
const BUS_MAX_READ: usize = 32;

/// The longest write: two bytes (register + value, or a 16-bit command).
const BUS_MAX_WRITE: usize = 2;

// Client ids — the index into [`REPLIES`]. Slots are independent, so any
// combination of sensor features compiles; an unused slot is never
// addressed.
pub(super) const BME280_BUS: usize = 0;
pub(super) const SHT4X_BUS: usize = 1;
pub(super) const SCD4X_BUS: usize = 2;

/// One queued bus transaction.
struct BusRequest {
    client: usize,
    addr: u8,
    write: [u8; BUS_MAX_WRITE],
    write_len: usize,
    read_len: usize,
}

/// A completed transaction: the bus outcome and the bytes read.
struct BusReply {
    // Ok(n) carries how many bytes were read (0 for a plain write).
    result: Result<usize, BusError>,
    data: [u8; BUS_MAX_READ],
}

/// Transactions queue here; the server drains one at a time. Depth 8 ≫
/// the three clients, so a sensor task never backs up behind a full queue
/// unless the server is wedged — which is the fault the queue exposes.
static REQUESTS: Channel<CriticalSectionRawMutex, BusRequest, 8> = Channel::new();

/// One reply slot per client. A request is in flight exactly while the
/// server holds it, so a depth-1 reply channel cannot overflow.
static REPLIES: [Channel<CriticalSectionRawMutex, BusReply, 1>; 3] =
    [Channel::new(), Channel::new(), Channel::new()];

/// The bus server: constructs the controller (here — the async driver is
/// `!Send`, so it must never cross a spawn boundary) and serves queued
/// transactions for as long as the device runs.
#[embassy_executor::task]
pub(super) async fn bus_server(
    i2c0: esp_hal::peripherals::I2C0<'static>,
    sda: esp_hal::peripherals::GPIO8<'static>,
    scl: esp_hal::peripherals::GPIO9<'static>,
) {
    let mut controller = I2c::new(i2c0, Config::default())
        .expect("i2c controller init")
        .with_sda(sda)
        .with_scl(scl)
        .into_async();

    loop {
        let request = REQUESTS.receive().await;
        let mut data = [0u8; BUS_MAX_READ];
        let result = match (request.write_len, request.read_len) {
            // Read-only and write-only are distinct transactions; a request
            // with neither is a protocol bug on the client side.
            (0, 0) => Ok(()),
            (0, _) => {
                controller
                    .read_async(request.addr, &mut data[..request.read_len])
                    .await
            }
            (_, 0) => {
                controller
                    .write_async(request.addr, &request.write[..request.write_len])
                    .await
            }
            (_, _) => {
                controller
                    .write_read_async(
                        request.addr,
                        &request.write[..request.write_len],
                        &mut data[..request.read_len],
                    )
                    .await
            }
        };
        let reply = BusReply {
            result: result.map(|_| request.read_len).map_err(map_hal_error),
            data,
        };
        REPLIES[request.client].send(reply).await;
    }
}

/// A per-sensor bus handle. Zero-sized — all bus state lives in the
/// server task; `ID` picks the client's reply slot (0 = BME280, 1 =
/// SHT4x, 2 = SCD4x).
#[derive(Debug)]
pub(super) struct SharedI2c<const ID: usize>;

impl<const ID: usize> SharedI2c<ID> {
    /// Queue one transaction and wait for the server's reply. The bus
    /// outcome rides inside the reply; callers unpack it where they can
    /// still act on it (copy the bytes, map the error).
    async fn transact(
        &mut self,
        addr: u8,
        write: Option<&[u8]>,
        read_len: usize,
    ) -> BusReply {
        let mut payload = [0u8; BUS_MAX_WRITE];
        let write_len = match write {
            Some(bytes) => {
                assert!(
                    bytes.len() <= BUS_MAX_WRITE,
                    "i2c write longer than the bus protocol's fixed slot"
                );
                payload[..bytes.len()].copy_from_slice(bytes);
                bytes.len()
            }
            None => 0,
        };
        REQUESTS
            .send(BusRequest {
                client: ID,
                addr,
                write: payload,
                write_len,
                read_len,
            })
            .await;
        REPLIES[ID].receive().await
    }
}

// Task-local futures again — see the note on `I2cBus`.
impl<const ID: usize> I2cBus for SharedI2c<ID> {
    async fn write(&mut self, addr: u8, write: &[u8]) -> Result<(), BusError> {
        self.transact(addr, Some(write), 0).await.result.map(|_| ())
    }

    async fn read(&mut self, addr: u8, read: &mut [u8]) -> Result<(), BusError> {
        let reply = self.transact(addr, None, read.len()).await;
        let len = reply.result?;
        read[..len].copy_from_slice(&reply.data[..len]);
        Ok(())
    }

    async fn write_read(
        &mut self,
        addr: u8,
        write: &[u8],
        read: &mut [u8],
    ) -> Result<(), BusError> {
        let reply = self.transact(addr, Some(write), read.len()).await;
        let len = reply.result?;
        read[..len].copy_from_slice(&reply.data[..len]);
        Ok(())
    }
}

fn map_hal_error(error: HalError) -> BusError {
    match error {
        HalError::Timeout => BusError::Timeout,
        HalError::AcknowledgeCheckFailed(_) => BusError::Nack,
        HalError::ArbitrationLost => BusError::Controller(1),
        HalError::FifoExceeded => BusError::Controller(2),
        HalError::ExecutionIncomplete => BusError::Controller(3),
        _ => BusError::Controller(4),
    }
}
