//! The sensors' bus/clock abstraction — the mechanism the drivers run over.
//!
//! V5 is event-driven: machines never block, so the bus is synchronous
//! register transactions plus a millisecond clock, and *pacing* lives in
//! the driver's state machine (poll on `SIG_TICK`, never `delay`). The
//! target-side adapter onto `esp-hal` I2C arrives with the bus bring-up
//! wave (docs/plan/build-dag.md, `feat/driver-buses`); until then the
//! drivers are exercised on the host against [`ScriptedBus`], which
//! replays recorded transaction sequences, and [`SharedClock`] supplies
//! scripted time.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// A closed error set for bus transactions. Target adapters map their
/// driver-specific errors into this set; drivers match on nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusError {
    /// The device did not acknowledge (address or data NACK).
    Nack,
    /// Any other transport-level failure (bus contention, timeout, ...).
    Other,
}

/// Sensirion's CRC-8 (polynomial 0x31, init 0xFF, MSB first) — the
/// checksum protecting every transferred word on the Sensirion I2C
/// sensors (SHT4x, SCD4x; the app note's check value 0xBEEF → 0x92).
pub fn crc8(bytes: &[u8]) -> u8 {
    let mut crc: u8 = 0xFF;
    for &b in bytes {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x31
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Synchronous I2C transactions — the register-level voice of the drivers.
/// Mirrors the embedded-hal operation set (write / read / write-read) so
/// the future `esp-hal` adapter is a thin wrapper.
pub trait I2cBus {
    /// Writes `bytes` to the device at 7-bit `addr`.
    fn write(&mut self, addr: u8, bytes: &[u8]) -> Result<(), BusError>;
    /// Reads from the device into `buf` (a plain read; no register address).
    fn read(&mut self, addr: u8, buf: &mut [u8]) -> Result<(), BusError>;
    /// Writes `bytes`, then reads a reply into `buf` — the register-read
    /// pattern (send the register address, restart, read).
    fn write_read(
        &mut self,
        addr: u8,
        bytes: &[u8],
        buf: &mut [u8],
    ) -> Result<(), BusError>;
}

/// Milliseconds since boot — the pacing source for datasheet timings
/// (conversion durations, reset startup). The target adapter wraps
/// `esp-hal`'s monotonic time; host tests script it with [`FixedClock`].
pub trait Clock {
    fn now_ms(&self) -> u64;
}

/// A clock the test advances directly, so a flow can move time between
/// dispatches without sleeping. The driver owns a `Box<dyn Clock>` over a
/// clone of the same handle — `Rc` makes the clones share one clock.
#[derive(Clone, Debug, Default)]
pub struct SharedClock(alloc::rc::Rc<core::cell::Cell<u64>>);

impl SharedClock {
    pub fn new(ms: u64) -> Self {
        Self(alloc::rc::Rc::new(core::cell::Cell::new(ms)))
    }

    pub fn set(&self, ms: u64) {
        self.0.set(ms);
    }

    pub fn advance(&self, ms: u64) {
        self.0.set(self.0.get() + ms);
    }
}

impl Clock for SharedClock {
    fn now_ms(&self) -> u64 {
        self.0.get()
    }
}

/// One recorded bus transaction. The drivers replay these on the host to
/// pin the datasheet register flows byte-for-byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Expect `write(addr, bytes)` with a byte-exact payload.
    Write(u8, Vec<u8>),
    /// Expect `read(addr, buf)` with `buf.len() == reply.len()`; reply bytes.
    Read(u8, Vec<u8>),
    /// Expect `write_read(addr, bytes, buf)`; payload and reply bytes.
    WriteRead(u8, Vec<u8>, Vec<u8>),
    /// Fail the next transaction with `error`, whatever its shape.
    Fail(BusError),
}

/// What a [`ScriptedBus`] actually did — the performed-transaction log a
/// test asserts on (shape, not just outcome).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Performed {
    Write {
        addr: u8,
        bytes: Vec<u8>,
    },
    Read {
        addr: u8,
        len: usize,
    },
    WriteRead {
        addr: u8,
        bytes: Vec<u8>,
        len: usize,
    },
}

/// A scripted bus: every transaction must match the next recorded step,
/// byte-for-byte, or the test panics at the exact divergence point. A
/// recorded sequence the driver never finishes is a bug in the flow, not a
/// pass — [`ScriptedBus::expect_done`] pins that too.
#[derive(Clone, Debug)]
pub struct ScriptedBus {
    script: VecDeque<Step>,
    pub performed: Vec<Performed>,
}

impl ScriptedBus {
    pub fn new(script: impl IntoIterator<Item = Step>) -> Self {
        Self {
            script: VecDeque::from_iter(script),
            performed: Vec::new(),
        }
    }

    /// Panics unless every recorded step was consumed.
    pub fn expect_done(&self) {
        assert!(
            self.remaining() == 0,
            "scripted bus did not reach the end of the recorded sequence ({} step(s) left)",
            self.remaining()
        );
    }

    fn next(&mut self, op: &str) -> Step {
        self.script
            .pop_front()
            .unwrap_or_else(|| panic!("{op} past the end of the scripted sequence"))
    }

    /// Number of recorded steps not yet consumed.
    pub fn remaining(&self) -> usize {
        self.script.len()
    }
}

/// A shared handle to a [`ScriptedBus`]: the driver owns a
/// `Box<dyn I2cBus>` over a clone, while the test keeps typed access to
/// the script's consumed state and performed-transaction log. `Rc`
/// makes the clones share one scripted bus.
#[derive(Clone, Debug)]
pub struct SharedBus(alloc::rc::Rc<core::cell::RefCell<ScriptedBus>>);

impl SharedBus {
    pub fn new(script: impl IntoIterator<Item = Step>) -> Self {
        Self(alloc::rc::Rc::new(core::cell::RefCell::new(
            ScriptedBus::new(script),
        )))
    }

    /// Panics unless every recorded step was consumed.
    pub fn expect_done(&self) {
        self.0.borrow().expect_done();
    }

    /// The performed-transaction log, for shape assertions.
    pub fn performed(&self) -> Vec<Performed> {
        self.0.borrow().performed.clone()
    }
}

impl I2cBus for SharedBus {
    fn write(&mut self, addr: u8, bytes: &[u8]) -> Result<(), BusError> {
        self.0.borrow_mut().write(addr, bytes)
    }

    fn read(&mut self, addr: u8, buf: &mut [u8]) -> Result<(), BusError> {
        self.0.borrow_mut().read(addr, buf)
    }

    fn write_read(
        &mut self,
        addr: u8,
        bytes: &[u8],
        buf: &mut [u8],
    ) -> Result<(), BusError> {
        self.0.borrow_mut().write_read(addr, bytes, buf)
    }
}

impl I2cBus for ScriptedBus {
    fn write(&mut self, addr: u8, bytes: &[u8]) -> Result<(), BusError> {
        self.performed.push(Performed::Write {
            addr,
            bytes: Vec::from(bytes),
        });
        match self.next("write") {
            Step::Write(exp_addr, exp_bytes) => {
                assert!(
                    addr == exp_addr && bytes == &exp_bytes[..],
                    "write mismatch: {addr:#04x} {bytes:#04x?}, expected \
                     {exp_addr:#04x} {exp_bytes:#04x?}"
                );
                Ok(())
            }
            Step::Fail(e) => Err(e),
            other => panic!("write: next scripted step was {other:?}"),
        }
    }

    fn read(&mut self, addr: u8, buf: &mut [u8]) -> Result<(), BusError> {
        self.performed.push(Performed::Read {
            addr,
            len: buf.len(),
        });
        match self.next("read") {
            Step::Read(exp_addr, reply) => {
                assert!(
                    addr == exp_addr && buf.len() == reply.len(),
                    "read mismatch: {addr:#04x} len {}, expected \
                     {exp_addr:#04x} len {}",
                    buf.len(),
                    reply.len()
                );
                buf.copy_from_slice(&reply);
                Ok(())
            }
            Step::Fail(e) => Err(e),
            other => panic!("read: next scripted step was {other:?}"),
        }
    }

    fn write_read(
        &mut self,
        addr: u8,
        bytes: &[u8],
        buf: &mut [u8],
    ) -> Result<(), BusError> {
        self.performed.push(Performed::WriteRead {
            addr,
            bytes: Vec::from(bytes),
            len: buf.len(),
        });
        match self.next("write_read") {
            Step::WriteRead(exp_addr, exp_bytes, reply) => {
                assert!(
                    addr == exp_addr && bytes == &exp_bytes[..],
                    "write_read mismatch: {addr:#04x} {bytes:#04x?}, expected \
                     {exp_addr:#04x} {exp_bytes:#04x?}"
                );
                assert!(
                    buf.len() == reply.len(),
                    "write_read reply length mismatch: {}, expected {}",
                    buf.len(),
                    reply.len()
                );
                buf.copy_from_slice(&reply);
                Ok(())
            }
            Step::Fail(e) => Err(e),
            other => panic!("write_read: next scripted step was {other:?}"),
        }
    }
}
