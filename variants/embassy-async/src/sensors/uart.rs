//! The async UART port seam and the host byte-stream fixtures.
//!
//! [`UartPort`] is the two-operation contract the UART sensor driver codes
//! against — the shape of `embedded-hal-async`'s serial traits, kept local
//! so the `no_std` library never depends on the hal stack (the same choice
//! as [`super::i2c::I2cBus`]). On the device, `device/uart.rs` bridges the
//! esp-hal UART controller to this trait, with a per-read timeout so a
//! silent sensor surfaces as an error instead of a parked future; on the
//! host, [`fakes::ScriptedUart`] replays scripted byte chunks, so frame
//! reassembly is verified byte-for-byte without hardware.
//!
//! The seam is chunk-oriented on purpose: a real UART delivers bytes as
//! they arrive, and a frame's bytes split across reads in every possible
//! way. The driver must be indifferent to chunk boundaries — the fixtures
//! pin that.

/// Why a UART transaction failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UartError {
    /// No byte arrived within the adapter's read window: the sensor is
    /// silent (unpowered, unplugged, or mid-recovery). The adapter owns
    /// the timeout policy; the driver sees the outcome.
    Timeout,
    /// The line itself lied — framing, parity, break, or a receiver
    /// overrun. Bytes read around such a fault are untrustworthy.
    LineNoise,
    /// A controller-specific error the adapter could not classify. The
    /// payload is the controller's own cause number, for diagnostics only.
    Controller(u8),
}

impl core::fmt::Display for UartError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Timeout => f.write_str("uart read timeout"),
            Self::LineNoise => f.write_str("uart line error"),
            Self::Controller(code) => write!(f, "uart controller fault {code}"),
        }
    }
}

/// The async UART contract: drain whatever the port holds, and push a
/// command out. The sensor world needs no more — a frame-oriented sensor
/// speaks in bursts, not in single-byte handshakes.
// Task-local futures again — see the note on `SensorDriver`.
#[allow(async_fn_in_trait)]
pub trait UartPort {
    /// Read the bytes currently available into `buf`, returning how many
    /// were read. `Ok(0)` is not a success shape: an adapter with nothing
    /// to deliver reports [`UartError::Timeout`] instead, so the driver
    /// can never spin on an idle port.
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, UartError>;

    /// Write a whole command frame, byte for byte.
    async fn write(&mut self, bytes: &[u8]) -> Result<(), UartError>;
}

/// Host fixtures for the port seam — the fake UART that frame reassembly
/// runs against. Compiled unconditionally (it is tiny and `no_std` safe)
/// for the same reason `sensors::i2c::fakes` is: downstream integration
/// tests script the sensor tasks end to end.
pub mod fakes {
    use heapless::Vec as HeapVec;

    use super::{UartError, UartPort};

    /// Maximum scripted reads and recorded writes per fixture.
    const SCRIPT_DEPTH: usize = 64;

    /// One scripted read outcome: either a chunk of bytes (the whole
    /// chunk, in one read — chunk boundaries are the fixture's tool for
    /// exercising frame reassembly) or an error.
    #[derive(Debug, Clone)]
    pub enum ReadStep {
        /// The next read returns exactly these bytes.
        Bytes(&'static [u8]),
        /// The next read fails with this error.
        Fail(UartError),
    }

    /// A scripted UART: replays [`ReadStep`]s front-to-back, repeats its
    /// final step once the script runs out (an unbounded task loop fed a
    /// finite script settles instead of panicking — same contract as
    /// `sensors::i2c::fakes::ScriptedI2c`), and records every write.
    #[derive(Debug)]
    pub struct ScriptedUart {
        reads: HeapVec<ReadStep, SCRIPT_DEPTH>,
        cursor: usize,
        writes: HeapVec<u8, SCRIPT_DEPTH>,
    }

    impl ScriptedUart {
        /// A fixture scripted with `reads`, in read order.
        #[must_use]
        pub fn new(reads: &[ReadStep]) -> Self {
            let mut scripted = Self {
                reads: HeapVec::new(),
                cursor: 0,
                writes: HeapVec::new(),
            };
            for step in reads {
                scripted
                    .reads
                    .push(step.clone())
                    .expect("script depth is a test size");
            }
            scripted
        }

        /// The final step, repeated forever once the script runs out.
        /// An empty script is a test misconfiguration.
        fn next(&mut self) -> &ReadStep {
            assert!(
                !self.reads.is_empty(),
                "scripted uart used with an empty script — test misconfiguration"
            );
            let last = self.reads.len() - 1;
            let cursor = self.cursor.min(last);
            self.cursor = self.cursor.saturating_add(1);
            &self.reads[cursor]
        }

        /// Every byte written so far, in order — command-frame assertions
        /// read this.
        #[must_use]
        pub fn written(&self) -> &[u8] {
            &self.writes
        }

        /// How many reads the driver has issued.
        #[must_use]
        pub fn reads(&self) -> usize {
            self.cursor.min(SCRIPT_DEPTH)
        }
    }

    impl UartPort for ScriptedUart {
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, UartError> {
            match self.next() {
                ReadStep::Bytes(bytes) => {
                    let len = bytes.len().min(buf.len());
                    buf[..len].copy_from_slice(&bytes[..len]);
                    Ok(len)
                }
                ReadStep::Fail(error) => Err(*error),
            }
        }

        async fn write(&mut self, bytes: &[u8]) -> Result<(), UartError> {
            for byte in bytes {
                self.writes
                    .push(*byte)
                    .expect("script depth is a test size");
            }
            Ok(())
        }
    }

    /// The driver-level wait fixture, matching `sensors::i2c::fakes`:
    /// records every requested wait and resolves immediately.
    #[derive(Debug, Default)]
    pub struct MockWait {
        pub waits: HeapVec<u64, SCRIPT_DEPTH>,
    }

    impl MockWait {
        #[must_use]
        pub const fn new() -> Self {
            Self {
                waits: HeapVec::new(),
            }
        }
    }

    impl super::super::Wait for MockWait {
        async fn wait(&mut self, ms: u64) {
            let _ = self.waits.push(ms);
        }
    }
}
