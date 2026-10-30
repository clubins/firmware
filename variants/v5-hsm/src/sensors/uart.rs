//! The sensors' UART transport abstraction — the byte-stream voice of the
//! Plantower-family sensors. The I2C counterpart lives in
//! [`crate::sensors::bus`]; the same closed-error and never-block rules
//! apply.

use alloc::vec::Vec;

/// A closed error set for UART transactions. Target adapters map their
/// driver-specific errors into this set; drivers match on nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UartError {
    /// The receive side dropped bytes before the driver drained them.
    Overrun,
    /// Any other transport-level failure (framing noise, parity, ...).
    Other,
}

/// Synchronous UART transactions — the byte-stream voice of the drivers.
/// Mirrors the embedded-hal serial operation set (write / read) so the
/// future `esp-hal` adapter is a thin wrapper. Reads never block: they
/// hand back whatever has arrived, up to `buf.len()`.
pub trait UartIo {
    /// Writes command bytes to the sensor.
    fn write(&mut self, bytes: &[u8]) -> Result<(), UartError>;
    /// Drains up to `buf.len()` received bytes; returns how many landed.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, UartError>;
}

// ---- host-side scripted UART (the [`ScriptedBus`] counterpart) ----

/// One recorded UART transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UartStep {
    /// Expect `write(bytes)` with a byte-exact payload.
    Write(Vec<u8>),
    /// Hand these bytes out on the next read — one chunk of the stream.
    Read(Vec<u8>),
    /// Fail the next transaction with `error`, whatever its shape.
    Fail(UartError),
}

/// What a [`ScriptedUart`] actually did — the performed-transaction log
/// a test asserts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UartPerformed {
    Write { bytes: Vec<u8> },
    Read { len: usize },
}

/// A scripted UART: every written command must match the next recorded
/// step byte-for-byte, and reads hand out the recorded chunks in order.
/// The stream asymmetry is deliberate — a write past the script is a bug
/// in the flow (it panics), while a read past the script simply reports
/// an empty wire (`Ok(0)`), because "nothing has arrived yet" is the
/// normal state of a receive side.
#[derive(Clone, Debug)]
pub struct ScriptedUart {
    script: alloc::collections::VecDeque<UartStep>,
    pub performed: alloc::vec::Vec<UartPerformed>,
}

impl ScriptedUart {
    pub fn new(script: impl IntoIterator<Item = UartStep>) -> Self {
        Self {
            script: alloc::collections::VecDeque::from_iter(script),
            performed: alloc::vec::Vec::new(),
        }
    }

    /// Panics unless every recorded step was consumed.
    pub fn expect_done(&self) {
        assert!(
            self.script.is_empty(),
            "scripted uart did not reach the end of the recorded sequence ({} step(s) left)",
            self.script.len()
        );
    }

    fn next(&mut self, op: &str) -> UartStep {
        self.script
            .pop_front()
            .unwrap_or_else(|| panic!("{op} past the end of the scripted sequence"))
    }
}

impl UartIo for ScriptedUart {
    fn write(&mut self, bytes: &[u8]) -> Result<(), UartError> {
        self.performed.push(UartPerformed::Write {
            bytes: alloc::vec::Vec::from(bytes),
        });
        match self.next("write") {
            UartStep::Write(exp) if bytes == &exp[..] => Ok(()),
            UartStep::Write(exp) => {
                panic!("write mismatch: {bytes:#04x?}, expected {exp:#04x?}")
            }
            UartStep::Fail(e) => Err(e),
            UartStep::Read(_) => panic!("write: next scripted step was a read"),
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, UartError> {
        // A drained script is an empty wire — the normal receive state —
        // not a flow bug, so reads report no data instead of panicking.
        let Some(step) = self.script.pop_front() else {
            return Ok(0);
        };
        let chunk = match step {
            UartStep::Read(chunk) => chunk,
            UartStep::Fail(e) => return Err(e),
            UartStep::Write(_) => panic!("read: next scripted step was a write"),
        };
        assert!(
            chunk.len() <= buf.len(),
            "read buffer smaller than the scripted chunk: {} < {}",
            buf.len(),
            chunk.len()
        );
        buf[..chunk.len()].copy_from_slice(&chunk);
        self.performed
            .push(UartPerformed::Read { len: chunk.len() });
        Ok(chunk.len())
    }
}

/// A shared handle to a [`ScriptedUart`]: the driver owns a
/// `Box<dyn UartIo>` over a clone, while the test keeps typed access to
/// the script's consumed state and performed-transaction log. `Rc`
/// makes the clones share one scripted uart.
#[derive(Clone, Debug)]
pub struct SharedUart(alloc::rc::Rc<core::cell::RefCell<ScriptedUart>>);

impl SharedUart {
    pub fn new(script: impl IntoIterator<Item = UartStep>) -> Self {
        Self(alloc::rc::Rc::new(core::cell::RefCell::new(
            ScriptedUart::new(script),
        )))
    }

    /// Panics unless every recorded step was consumed.
    pub fn expect_done(&self) {
        self.0.borrow().expect_done();
    }

    /// The written-command log, for shape assertions.
    pub fn written(&self) -> alloc::vec::Vec<alloc::vec::Vec<u8>> {
        self.0
            .borrow()
            .performed
            .iter()
            .filter_map(|p| match p {
                UartPerformed::Write { bytes } => Some(bytes.clone()),
                UartPerformed::Read { .. } => None,
            })
            .collect()
    }

    /// The performed-transaction log, for shape assertions.
    pub fn performed(&self) -> alloc::vec::Vec<UartPerformed> {
        self.0.borrow().performed.clone()
    }
}

impl UartIo for SharedUart {
    fn write(&mut self, bytes: &[u8]) -> Result<(), UartError> {
        self.0.borrow_mut().write(bytes)
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, UartError> {
        self.0.borrow_mut().read(buf)
    }
}
