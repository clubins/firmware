//! The I²C bus mechanism — the variant's bus abstraction for the polled
//! sensor sources.
//!
//! Drivers (BME280, SHT4x, later SCD4x/BH1750) are written against the
//! [`I2c`] trait, which is embedded-hal's transaction shape: write, read,
//! and write-then-read (register pointer + burst). The engine owns one
//! [`SimulatedI2c`] bus the way the runtime owns its stream buffers: it is
//! the statically-allocated stand-in for the ESP32-S3's I²C peripheral
//! until the device glue PR lands, and the host-test instrument that lets
//! driver tests run against *recorded register transaction sequences* —
//! the test scripts the exact datasheet flow (each expected transaction
//! with its reply), the driver walks it, and the recorded log is asserted
//! back. A transaction that does not match the script fails with
//! [`BusError::Timeout`] (nothing answered) and is logged, so the first
//! divergent step shows up in the test's flow assertion.
//!
//! Everything here is fixed-capacity: no heap, no locks — the bus is
//! driven from the engine's executor, one pipeline beat at a time.
//!
//! Gated by the internal `driver-i2c` plumbing flag: every I²C sensor
//! leaf pulls it, so the bus (and the peripheral the device glue will
//! hang on it) exists exactly when an I²C capability is compiled in.

/// Why an I²C transaction failed. The closed vocabulary drivers (and the
/// source executors' fault counters) act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusError {
    /// The addressed device refused the transaction (address NACK), or a
    /// received frame failed its on-wire check.
    Nack,
    /// The transaction did not complete — the device stalled, or (on the
    /// simulated bus) no scripted response was pending.
    Timeout,
}

/// The I²C controller contract. One instance = one bus = one peripheral;
/// devices are distinguished by address. Transaction shapes follow
/// embedded-hal's `i2c` traits so the device-side adapter is a thin shim.
pub trait I2c {
    /// Send bytes to a device (e.g. a register write `0xF4, 0x25`, or an
    /// SHT4x command byte).
    fn write(&mut self, addr: u8, bytes: &[u8]) -> Result<(), BusError>;
    /// Receive bytes from a device (e.g. an SHT4x measurement frame after
    /// its conversion command).
    fn read(&mut self, addr: u8, buf: &mut [u8]) -> Result<(), BusError>;
    /// Write the register pointer, then read — the register-file idiom
    /// (BME280): `write_read(0x76, &[0xF7], &mut buf[..8])`.
    fn write_read(&mut self, addr: u8, tx: &[u8], rx: &mut [u8]) -> Result<(), BusError>;
}

/// Largest transaction payload the simulated bus records or scripts.
/// Roomy for the widest flow step (the BME280's 26-byte calibration burst).
const MAX_TX: usize = 8;
const MAX_RX: usize = 32;

/// Scripted-transaction capacity (expectations queued per test).
const SCRIPT_CAP: usize = 32;
/// Recorded-transaction capacity (the log tests assert against).
const LOG_CAP: usize = 32;

/// Kind of an I²C transaction on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Write,
    Read,
    WriteRead,
}

/// One recorded or scripted transaction. Buffers are inline (fixed cap);
/// unused tail bytes are zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Op {
    pub kind: OpKind,
    pub addr: u8,
    pub tx: [u8; MAX_TX],
    pub tx_len: usize,
    pub rx: [u8; MAX_RX],
    pub rx_len: usize,
}

impl Op {
    fn record(kind: OpKind, addr: u8, tx: &[u8], rx: &[u8]) -> Self {
        let mut op = Self {
            kind,
            addr,
            tx: [0; MAX_TX],
            tx_len: tx.len(),
            rx: [0; MAX_RX],
            rx_len: rx.len(),
        };
        op.tx[..tx.len()].copy_from_slice(tx);
        op.rx[..rx.len()].copy_from_slice(rx);
        op
    }

    /// The bytes written (register pointer or command).
    pub fn written(&self) -> &[u8] {
        &self.tx[..self.tx_len]
    }

    /// The bytes received (empty for a plain write).
    pub fn received(&self) -> &[u8] {
        &self.rx[..self.rx_len]
    }
}

/// The simulated I²C bus: a FIFO script of expected transactions (with
/// their replies) plus a recording of every transaction actually issued.
///
/// On the device it is the engine's inert bus — with no script, every
/// poll times out and the source executors count faults (exactly the
/// interim behaviour the runtime PR documented for `Pending` sources,
/// minus the idling). In tests it is the recorded register flow: program
/// the datasheet sequence, run the driver, assert the log.
#[derive(Debug, Clone)]
pub struct SimulatedI2c {
    script: [Option<Op>; SCRIPT_CAP],
    /// Index of the next script entry to serve.
    served: usize,
    /// Total entries queued (the script tail).
    queued: usize,
    log: [Option<Op>; LOG_CAP],
    logged: usize,
}

impl SimulatedI2c {
    /// An empty bus. Const, for `Engine::instantiate`.
    pub const fn new() -> Self {
        Self {
            script: [None; SCRIPT_CAP],
            served: 0,
            queued: 0,
            log: [None; LOG_CAP],
            logged: 0,
        }
    }

    // -- scripting (test side) ------------------------------------------

    /// Script a plain write (e.g. a BME280 control-register write).
    pub fn expect_write(&mut self, addr: u8, tx: &[u8]) {
        self.queue(Op::record(OpKind::Write, addr, tx, &[]));
    }

    /// Script a plain read (e.g. an SHT4x measurement frame).
    pub fn expect_read(&mut self, addr: u8, rx: &[u8]) {
        self.queue(Op::record(OpKind::Read, addr, &[], rx));
    }

    /// Script a register-pointer write followed by a burst read.
    pub fn expect_write_read(&mut self, addr: u8, tx: &[u8], rx: &[u8]) {
        self.queue(Op::record(OpKind::WriteRead, addr, tx, rx));
    }

    /// Queue one expectation. Panics on overflow — a test scripting more
    /// than `SCRIPT_CAP` steps is a test bug, not a runtime path.
    fn queue(&mut self, op: Op) {
        assert!(
            self.queued < SCRIPT_CAP,
            "simulated I2C script overflow (cap {SCRIPT_CAP})"
        );
        self.script[self.queued] = Some(op);
        self.queued += 1;
    }

    // -- recording (test assertions) -------------------------------------

    /// Transactions recorded so far (in issue order).
    pub fn logged(&self) -> usize {
        self.logged
    }

    /// The i-th recorded transaction.
    pub fn op(&self, index: usize) -> Option<&Op> {
        self.log.get(index).and_then(|s| s.as_ref())
    }

    /// Whether every scripted transaction was served exactly — the flow
    /// ran to the end without extra steps.
    pub fn script_served(&self) -> bool {
        self.served == self.queued
    }

    /// Serve one driver transaction against the script FIFO. The next
    /// pending entry is consumed and its reply returned only when the
    /// transaction matches it; anything else (empty script, wrong
    /// kind/address/pointer) times out — no device answers an
    /// unscripted transaction — and the script entry stays pending.
    fn serve(
        &mut self,
        kind: OpKind,
        addr: u8,
        tx: &[u8],
        rx: &mut [u8],
    ) -> Result<(), BusError> {
        let idx = self.served;
        let scripted = if idx < self.queued {
            Some(self.script[idx].expect("script entry below queued cursor"))
        } else {
            None
        };

        let matches = scripted
            .is_some_and(|s| s.kind == kind && s.addr == addr && s.tx[..s.tx_len] == *tx);
        if let Some(scripted) = scripted.filter(|_| matches) {
            self.served += 1;
            let n = scripted.rx_len.min(rx.len());
            rx[..n].copy_from_slice(&scripted.rx[..n]);
            self.record(Op::record(kind, addr, tx, &scripted.rx[..n]));
            return Ok(());
        }
        self.record(Op::record(kind, addr, tx, &[]));
        Err(BusError::Timeout)
    }

    fn record(&mut self, op: Op) {
        if let Some(slot) = self.log.iter_mut().find(|s| s.is_none()) {
            *slot = Some(op);
            self.logged += 1;
        }
        // A full log drops the record: tests never script beyond LOG_CAP.
    }
}

impl Default for SimulatedI2c {
    fn default() -> Self {
        Self::new()
    }
}

impl I2c for SimulatedI2c {
    fn write(&mut self, addr: u8, bytes: &[u8]) -> Result<(), BusError> {
        self.serve(OpKind::Write, addr, bytes, &mut [])
    }

    fn read(&mut self, addr: u8, buf: &mut [u8]) -> Result<(), BusError> {
        self.serve(OpKind::Read, addr, &[], buf)
    }

    fn write_read(&mut self, addr: u8, tx: &[u8], rx: &mut [u8]) -> Result<(), BusError> {
        self.serve(OpKind::WriteRead, addr, tx, rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_serves_the_scripted_reply_in_order() {
        let mut bus = SimulatedI2c::new();
        bus.expect_write(0x76, &[0xF4, 0x25]);
        bus.expect_write_read(0x76, &[0xF7], &[1, 2, 3, 4, 5, 6, 7, 8]);

        assert!(bus.write(0x76, &[0xF4, 0x25]).is_ok());
        let mut buf = [0u8; 8];
        assert!(bus.write_read(0x76, &[0xF7], &mut buf).is_ok());
        assert_eq!(buf, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(bus.logged(), 2);
        assert_eq!(bus.op(0).map(Op::written), Some(&[0xF4, 0x25][..]));
        assert_eq!(
            bus.op(1).map(Op::received),
            Some(&[1, 2, 3, 4, 5, 6, 7, 8][..])
        );
        assert!(bus.script_served());
    }

    #[test]
    fn a_mismatched_pointer_times_out_and_is_still_recorded() {
        let mut bus = SimulatedI2c::new();
        bus.expect_write_read(0x76, &[0xD0], &[0x60]);

        let mut buf = [0u8; 1];
        assert_eq!(
            bus.write_read(0x76, &[0xF3], &mut buf),
            Err(BusError::Timeout)
        );
        // The stray transaction is recorded, the script entry stays queued
        // (the flow assertion will show both facts).
        assert_eq!(bus.logged(), 1);
        assert_eq!(bus.op(0).map(Op::written), Some(&[0xF3][..]));
        assert!(!bus.script_served());
    }

    #[test]
    fn an_empty_script_times_out_every_transaction() {
        let mut bus = SimulatedI2c::new();
        let mut buf = [0u8; 6];
        assert_eq!(bus.read(0x44, &mut buf), Err(BusError::Timeout));
        assert_eq!(bus.write(0x44, &[0xFD]), Err(BusError::Timeout));
        assert_eq!(bus.logged(), 2);
    }

    #[test]
    fn entries_queued_after_partial_service_are_still_served() {
        // Regression: the script cursor must track queued entries, not the
        // first free slot — otherwise a mid-flow queue lands behind the
        // cursor and the driver times out on a scripted transaction.
        let mut bus = SimulatedI2c::new();
        bus.expect_write(0x76, &[0xF4]);
        bus.expect_read(0x76, &[1, 2]);

        let mut buf = [0u8; 2];
        assert!(bus.write(0x76, &[0xF4]).is_ok());
        assert!(bus.read(0x76, &mut buf).is_ok());
        bus.expect_write_read(0x76, &[0xD0], &[0x60, 0x00]);
        assert!(bus.write_read(0x76, &[0xD0], &mut buf).is_ok());
        assert_eq!(buf, [0x60, 0x00]);
        assert_eq!(bus.logged(), 3);
        assert!(bus.script_served());
    }
}
