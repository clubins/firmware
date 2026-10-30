//! Register-flow tests across the graph: both I²C sensor drivers walk
//! their datasheet transaction sequences against the scripted bus while
//! the pump paces them, with the recorded transaction log asserted
//! between steps.
//!
//! Only compiled when both I²C sensor flags are on — the flag matrix's
//! trio and all-on anchors run these; other anchors skip the file.
//!
//! Cadence recap (one graph step = one poll per node, BME280 first):
//! BME280 bring-up takes one poll of 7 transactions, then emits one
//! reading per poll (forced mode + a scripted always-clear status). The
//! SHT4x takes one poll per flow leg — reset, serial probe, then
//! command/frame pairs — so it emits every other step.
//!
//! Assertions use the per-node emitted/fault counters and the shared
//! bus log — never `Tick::measurements`: under the all-on anchor every
//! compiled scaffold source (decibel, pax, scd4x, pms5003, bh1750)
//! emits on its own cadence and the aggregate counts them too.

#![cfg(all(feature = "sensor-bme280", feature = "sensor-sht4x"))]

use firmware_v6_dataflow::bus::{OpKind, SimulatedI2c};
use firmware_v6_dataflow::graph::Graph;

const BME280_ADDR: u8 = 0x76;
const SHT4X_ADDR: u8 = 0x44;

/// The BME280's datasheet canonical calibration + conversion result, as
/// scripted bytes (see `sensors::bme280` for the derivation; the worked
/// example reads 25.08 °C / 100 653.3 Pa).
const CAL26: [u8; 26] = [
    0x70, 0x6B, 0x43, 0x67, 0x18, 0xFC, 0x7D, 0x8E, 0x43, 0xD6, 0xD0, 0x0B, 0x27, 0x0B,
    0x8C, 0x00, 0xF9, 0xFF, 0x8C, 0x3C, 0xF8, 0xC6, 0x70, 0x17, 0x00, 0x4B,
];
const CAL7: [u8; 7] = [0x6B, 0x01, 0x00, 0x13, 0x05, 0x1E, 0x1E];
const FRAME8: [u8; 8] = [0x65, 0x5A, 0xC0, 0x7E, 0xED, 0x00, 0x6F, 0xF0];

/// Script the BME280's complete bring-up poll (7 transactions).
fn script_bme280_bringup(bus: &mut SimulatedI2c) {
    bus.expect_write_read(BME280_ADDR, &[0xD0], &[0x60]); // chip id
    bus.expect_write(BME280_ADDR, &[0xE0, 0xB6]); // soft reset
    bus.expect_write_read(BME280_ADDR, &[0xF3], &[0x00]); // NVM copy done
    bus.expect_write_read(BME280_ADDR, &[0x88], &CAL26); // calib T/P
    bus.expect_write_read(BME280_ADDR, &[0xE1], &CAL7); // calib H
    bus.expect_write(BME280_ADDR, &[0xF2, 0x01]); // ctrl_hum
    bus.expect_write(BME280_ADDR, &[0xF5, 0x00]); // config (sleep)
}

/// Script one complete BME280 conversion (trigger, status clear, burst).
fn script_bme280_conversion(bus: &mut SimulatedI2c) {
    bus.expect_write(BME280_ADDR, &[0xF4, 0x25]); // forced x1
    bus.expect_write_read(BME280_ADDR, &[0xF3], &[0x00]); // measuring clear
    bus.expect_write_read(BME280_ADDR, &[0xF7], &FRAME8); // data burst
}

#[test]
fn the_pump_walks_both_register_flows_to_their_readings() {
    let mut g = Graph::build();

    // Step 1: the BME280 identifies, resets, reads calibration and
    // configures (7 transactions); the SHT4x soft-resets (1).
    script_bme280_bringup(&mut g.i2c);
    g.i2c.expect_write(SHT4X_ADDR, &[0x94]);
    g.step();
    assert_eq!(g.bme280.faults(), 0);
    assert_eq!(g.sht4x.faults(), 0);
    assert_eq!(g.i2c.logged(), 8);

    // Step 2: the BME280's first conversion emits; the SHT4x probes its
    // serial number (CRC-checked, 0x12345678).
    script_bme280_conversion(&mut g.i2c);
    g.i2c.expect_write(SHT4X_ADDR, &[0x89]);
    g.i2c
        .expect_read(SHT4X_ADDR, &[0x12, 0x34, 0x37, 0x56, 0x78, 0x7D]);
    g.step();
    assert_eq!(g.bme280.emitted(), 1);
    assert_eq!(g.sht4x.emitted(), 0);

    // Step 3: the BME280 emits again; the SHT4x issues its measurement
    // command (collection lands on the next poll).
    script_bme280_conversion(&mut g.i2c);
    g.i2c.expect_write(SHT4X_ADDR, &[0xFD]);
    g.step();
    assert_eq!(g.bme280.emitted(), 2);

    // Step 4: the BME280 emits a third time; the SHT4x collects and
    // emits — both parts delivering through the same pump.
    script_bme280_conversion(&mut g.i2c);
    g.i2c
        .expect_read(SHT4X_ADDR, &[0x66, 0x66, 0x93, 0x80, 0x00, 0xA2]);
    g.step();
    assert_eq!(g.bme280.emitted(), 3);
    assert_eq!(g.sht4x.emitted(), 1);
    assert_eq!(g.sht4x.faults(), 0);
    assert_eq!(g.bme280.faults(), 0);

    // Every scripted transaction was served: the pump walked both flows
    // exactly as the datasheets sequence them — 21 transactions logged.
    assert_eq!(g.i2c.logged(), 21);
    assert!(g.i2c.script_served());
    // The shared bus served both devices, distinguished by address.
    let bme_txns = (0..g.i2c.logged())
        .filter(|&i| g.i2c.op(i).unwrap().addr == BME280_ADDR)
        .count();
    let sht_txns = (0..g.i2c.logged())
        .filter(|&i| g.i2c.op(i).unwrap().addr == SHT4X_ADDR)
        .count();
    assert_eq!(bme_txns, 16); // 7 bring-up + 3 × 3 conversions
    assert_eq!(sht_txns, 5); // reset + probe(2) + command + frame
    // The node rings drained during routing (no compiled transport, so
    // the copies drop there — by design, never queued behind).
    assert!(g.bme280.out.is_empty());
    assert!(g.sht4x.out.is_empty());
}

#[test]
fn a_faulted_sensor_is_isolated_from_its_neighbour() {
    let mut g = Graph::build();

    // Both bring-ups scripted, but the BME280 answers with a foreign id.
    g.i2c.expect_write_read(BME280_ADDR, &[0xD0], &[0x00]); // not a BME280
    g.i2c.expect_write(SHT4X_ADDR, &[0x94]);
    g.step();
    assert_eq!(g.bme280.faults(), 1);
    assert_eq!(g.sht4x.faults(), 0);

    // The SHT4x sails on (serial probe) while the BME280 re-runs the
    // full bring-up — now scripted correctly.
    script_bme280_bringup(&mut g.i2c);
    g.i2c.expect_write(SHT4X_ADDR, &[0x89]);
    g.i2c
        .expect_read(SHT4X_ADDR, &[0x12, 0x34, 0x37, 0x56, 0x78, 0x7D]);
    g.step();
    assert_eq!(g.bme280.faults(), 1); // no new fault: recovery succeeded

    // Steady state after recovery: both parts measure, one fault
    // forgotten but counted, nothing propagated between neighbours.
    script_bme280_conversion(&mut g.i2c);
    g.i2c.expect_write(SHT4X_ADDR, &[0xFD]);
    g.step();
    assert_eq!(g.bme280.emitted(), 1);
    assert_eq!(g.sht4x.emitted(), 0);

    script_bme280_conversion(&mut g.i2c);
    g.i2c
        .expect_read(SHT4X_ADDR, &[0x66, 0x66, 0x93, 0x80, 0x00, 0xA2]);
    g.step();

    assert_eq!(g.bme280.emitted(), 2);
    assert_eq!(g.sht4x.emitted(), 1);
    assert_eq!(g.bme280.faults(), 1);
    assert_eq!(g.sht4x.faults(), 0);
    assert!(g.i2c.script_served());
}

#[test]
fn repeated_conversions_keep_pacing_through_the_pump() {
    let mut g = Graph::build();

    // Bring-up: two steps cover both parts' initialization. Entries are
    // queued per step in poll order (BME280 first), so the shared FIFO
    // serves each poll the transaction the datasheet sequences next.
    script_bme280_bringup(&mut g.i2c);
    g.i2c.expect_write(SHT4X_ADDR, &[0x94]); // sht reset leg
    g.step();
    script_bme280_conversion(&mut g.i2c);
    g.i2c.expect_write(SHT4X_ADDR, &[0x89]); // sht serial probe
    g.i2c
        .expect_read(SHT4X_ADDR, &[0x12, 0x34, 0x37, 0x56, 0x78, 0x7D]);
    g.step();
    assert_eq!(g.bme280.emitted(), 1);
    assert_eq!(g.sht4x.emitted(), 0);

    // Two steady-state rounds: each SHT4x measurement spans two steps
    // (command leg, then frame leg), while the BME280 — one conversion
    // per poll — emits a reading in both. Two rounds fit the bus's
    // 32-entry script cap and still prove the pacing repeats without
    // drift: a second round re-serves the same datasheet sequence.
    for _ in 0..2 {
        script_bme280_conversion(&mut g.i2c);
        g.i2c.expect_write(SHT4X_ADDR, &[0xFD]); // command leg
        g.step();
        script_bme280_conversion(&mut g.i2c);
        g.i2c
            .expect_read(SHT4X_ADDR, &[0x66, 0x66, 0x93, 0x80, 0x00, 0xA2]);
        g.step(); // frame leg + emit
    }

    assert_eq!(g.bme280.emitted(), 5); // 1 + 2 per round
    assert_eq!(g.sht4x.emitted(), 2); // one per round, on the frame leg
    assert_eq!(g.bme280.faults(), 0);
    assert_eq!(g.sht4x.faults(), 0);
    assert!(g.i2c.script_served());
    // Every transaction used one of the three I²C shapes; the SHT4x —
    // registerless — never got a write_read.
    for i in 0..g.i2c.logged() {
        let op = g.i2c.op(i).unwrap();
        assert!(matches!(
            op.kind,
            OpKind::Write | OpKind::Read | OpKind::WriteRead
        ));
        if op.addr == SHT4X_ADDR {
            assert_ne!(op.kind, OpKind::WriteRead, "op {i}");
        }
    }
}
