//! The async I2C bus seam and the host transaction fixtures.
//!
//! [`I2cBus`] is the three-operation contract every I2C driver in this
//! firmware codes against — the shape of `embedded-hal-async`'s `I2c`, kept
//! local so the `no_std` library never depends on the hal stack. On the
//! device, `device/i2c.rs` bridges the shared esp-hal controller to this
//! trait; on the host, [`fakes::ScriptedI2c`] replays scripted transactions,
//! so a driver's register flow is verified byte-for-byte without hardware.
//!
//! [`BusError`] is the closed vocabulary of bus failure the drivers map onto
//! [`SensorFault::Bus`](crate::sensor::SensorFault) (see each driver's error
//! table): nothing on the wire can fail any other way.

/// Why a bus transaction failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusError {
    /// No device acknowledged its address, or a data byte was NACKed: the
    /// sensor is absent, still booting, or mid-measurement.
    Nack,
    /// The transaction outlived the controller's bus timeout.
    Timeout,
    /// A controller-specific error the adapter could not classify (bus
    /// arbitration, controller fault). The payload is the controller's own
    /// cause number, for diagnostics only.
    Controller(u8),
}

impl core::fmt::Display for BusError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Nack => f.write_str("i2c nack"),
            Self::Timeout => f.write_str("i2c timeout"),
            Self::Controller(code) => write!(f, "i2c controller fault {code}"),
        }
    }
}

/// The async I2C contract: a write, a read, and the combined write-read
/// register transaction the I2C sensor world is built on (write the register
/// address, keep the bus, read the bytes). Drivers use [`I2cBus::write_read`]
/// for every register access; bare [`I2cBus::write`] carries commands with
/// no response (Sensirion command words), bare [`I2cBus::read`] pulls a
/// measurement already triggered.
// Task-local futures again — see the note on `SensorDriver`.
#[allow(async_fn_in_trait)]
pub trait I2cBus {
    async fn write(&mut self, addr: u8, write: &[u8]) -> Result<(), BusError>;
    async fn read(&mut self, addr: u8, read: &mut [u8]) -> Result<(), BusError>;
    async fn write_read(
        &mut self,
        addr: u8,
        write: &[u8],
        read: &mut [u8],
    ) -> Result<(), BusError>;
}

/// Host fixtures for the bus seam — the fake I2C that drivers' register
/// flows run against. Compiled unconditionally (it is tiny and `no_std`
/// safe) for the same reason `sensor::fakes` is: downstream integration
/// tests script the sensor tasks end to end.
pub mod fakes {
    use heapless::Vec as HeapVec;

    use super::{BusError, I2cBus};

    /// Maximum scripted transactions per fixture.
    const SCRIPT_DEPTH: usize = 64;

    /// One scripted transaction. The fixture matches each incoming bus
    /// transaction against the next step in order: the transaction kind and
    /// the written bytes must be exactly what the step declares, or the
    /// fixture panics — a wrong register flow is a test failure, not a
    /// wrong reading to paper over.
    #[derive(Debug, Clone)]
    pub enum Step {
        /// A write-only transaction carrying exactly these bytes (a
        /// command with no response phase).
        Write(&'static [u8]),
        /// A write-read: write exactly these bytes, then fill the read
        /// buffer with exactly this response.
        WriteRead(&'static [u8], &'static [u8]),
        /// A pure read filling the buffer with exactly these bytes.
        Read(&'static [u8]),
        /// Whatever transaction comes next fails with this error.
        Fail(BusError),
    }

    /// One bus transaction the driver actually issued, for assertions about
    /// the shape (count, direction, sizes) of the flow. The log keeps the
    /// first [`SCRIPT_DEPTH`] entries — a flow long enough to outgrow it is
    /// asserting on the step scripts, not the log.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Recorded {
        pub addr: u8,
        /// Bytes written.
        pub written: usize,
        /// Bytes requested for the read phase; `0` for write-only.
        pub read_len: usize,
    }

    /// A scripted I2C bus: replays [`Step`]s front-to-back, repeats its
    /// final step once the script runs out (an unbounded task loop fed a
    /// finite script settles instead of panicking — same contract as
    /// `sensor::fakes::ScriptedDriver`), and records every transaction.
    #[derive(Debug)]
    pub struct ScriptedI2c {
        addr: u8,
        steps: HeapVec<Step, SCRIPT_DEPTH>,
        cursor: usize,
        log: HeapVec<Recorded, SCRIPT_DEPTH>,
    }

    impl ScriptedI2c {
        /// A fixture for the sensor at `addr`, scripted with `steps`.
        #[must_use]
        pub fn new(addr: u8, steps: &[Step]) -> Self {
            let mut scripted = Self {
                addr,
                steps: HeapVec::new(),
                cursor: 0,
                log: HeapVec::new(),
            };
            for step in steps {
                scripted
                    .steps
                    .push(step.clone())
                    .expect("script depth is a test size");
            }
            scripted
        }

        /// The final step, repeated forever once the script runs out.
        /// An empty script is a test misconfiguration.
        fn next(&mut self) -> &Step {
            assert!(
                !self.steps.is_empty(),
                "scripted i2c used with an empty script — test misconfiguration"
            );
            let last = self.steps.len() - 1;
            let cursor = self.cursor.min(last);
            self.cursor = self.cursor.saturating_add(1);
            &self.steps[cursor]
        }

        /// Every transaction issued so far, in order.
        #[must_use]
        pub fn recorded(&self) -> &[Recorded] {
            &self.log
        }

        /// How many transactions the driver has issued.
        #[must_use]
        pub fn transactions(&self) -> usize {
            self.log.len()
        }

        fn record(&mut self, written: usize, read_len: usize) {
            if !self.log.is_full() {
                let _ = self.log.push(Recorded {
                    addr: self.addr,
                    written,
                    read_len,
                });
            }
        }

        fn check_write(&self, expected: &[u8], actual: &[u8]) {
            assert_eq!(
                actual, expected,
                "fixture at {:#04x} expected write {:?}, driver wrote {:?}",
                self.addr, expected, actual
            );
        }

        fn fill_read(&self, expected: &[u8], read: &mut [u8]) {
            assert_eq!(
                read.len(),
                expected.len(),
                "fixture at {:#04x} scripted a {}-byte response, driver asked for {}",
                self.addr,
                expected.len(),
                read.len()
            );
            read.copy_from_slice(expected);
        }
    }

    impl I2cBus for ScriptedI2c {
        async fn write(&mut self, addr: u8, write: &[u8]) -> Result<(), BusError> {
            assert_eq!(addr, self.addr, "transaction to the wrong address");
            match self.next().clone() {
                Step::Write(expected) => {
                    self.check_write(expected, write);
                    self.record(write.len(), 0);
                    Ok(())
                }
                Step::Fail(error) => Err(error),
                step => panic!(
                    "fixture at {:#04x} expected {step:?}, driver sent a write",
                    self.addr
                ),
            }
        }

        async fn read(&mut self, addr: u8, read: &mut [u8]) -> Result<(), BusError> {
            assert_eq!(addr, self.addr, "transaction to the wrong address");
            match self.next().clone() {
                Step::Read(expected) => {
                    self.fill_read(expected, read);
                    self.record(0, read.len());
                    Ok(())
                }
                Step::Fail(error) => Err(error),
                step => panic!(
                    "fixture at {:#04x} expected {step:?}, driver sent a read",
                    self.addr
                ),
            }
        }

        async fn write_read(
            &mut self,
            addr: u8,
            write: &[u8],
            read: &mut [u8],
        ) -> Result<(), BusError> {
            assert_eq!(addr, self.addr, "transaction to the wrong address");
            match self.next().clone() {
                Step::WriteRead(expected_write, expected_read) => {
                    self.check_write(expected_write, write);
                    self.fill_read(expected_read, read);
                    self.record(write.len(), read.len());
                    Ok(())
                }
                Step::Fail(error) => Err(error),
                step => panic!(
                    "fixture at {:#04x} expected {step:?}, driver sent a write-read",
                    self.addr
                ),
            }
        }
    }

    /// A wait that records every requested duration and resolves
    /// immediately — the driver-side twin of `sensor::fakes::MockClock`.
    #[derive(Debug, Default)]
    pub struct MockWait {
        pub waits: HeapVec<u64, 128>,
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
