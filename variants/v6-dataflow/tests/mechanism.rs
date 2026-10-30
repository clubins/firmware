//! Mechanism-level host tests for the V6 static lock-free dataflow.
//!
//! These exercise the brief's named verifications against the public crate
//! API: simulated DMA throughput handoff at the real domain rates (UART
//! 115 200 Bd, I2S/PDM 48 kHz windows), the ring overflow/lag policy under
//! sustained overload, sink-drain burst absorption, and link-time budget
//! accounting. Zero-heap verification lives in `zero_heap.rs`.

use firmware_v6_dataflow::dma::{self, SimulatedDma};
#[cfg(not(any(
    feature = "sensor-decibel",
    feature = "sensor-pax",
    feature = "sensor-bme280",
    feature = "sensor-sht4x",
    feature = "sensor-scd4x",
    feature = "sensor-pms5003",
    feature = "sensor-bh1750",
    feature = "transport-http",
    feature = "transport-mqtt",
    feature = "transport-mtls",
    feature = "ops-status",
    feature = "ota",
    feature = "provisioning",
    feature = "deep-sleep",
    feature = "logging-defmt"
)))]
use firmware_v6_dataflow::graph::Graph;
use firmware_v6_dataflow::measurement::Measurement;
use firmware_v6_dataflow::ring::Spsc;

/// The DMA payload pattern (documented in `dma.rs`): byte 0 carries the
/// frame marker, the rest a sawtooth. Reimplemented here on purpose — the
/// tests verify the pattern independently of the engine's own encoder.
fn expect_byte(marker: u8, j: usize) -> u8 {
    if j == 0 { marker } else { j as u8 }
}

/// Drain `bytes` bytes from `ring`, asserting the exact pattern of the
/// frames they belong to (frames start at `first_marker`).
fn drain_checked<const CAP: usize>(ring: &Spsc<u8, CAP>, bytes: usize, first_marker: u8) {
    let mut marker = first_marker;
    for j in 0..bytes {
        let expected = expect_byte(marker, j % dma::uart::FRAME_BYTES);
        assert_eq!(ring.pop(), Some(expected), "byte {j} of frame {marker}");
        if j % dma::uart::FRAME_BYTES == dma::uart::FRAME_BYTES - 1 {
            marker = marker.wrapping_add(1);
        }
    }
}

/// Paced consumer: the UART engine hands 36 frames per 100 ms tick at its
/// sustained 11 520 B/s, the drain keeps pace — a full second passes with
/// zero loss, zero overruns, and byte-exact FIFO order.
#[test]
fn uart_throughput_handoff_is_lossless_when_paced() {
    // 64 frames of room: the paced consumer always drains the tick's
    // production (36 frames) before the next handoff, so nothing overruns.
    let ring: Spsc<u8, 2048> = Spsc::new();
    let mut engine = SimulatedDma::uart();
    let mut marker = 0u8;
    for _ in 0..10 {
        assert_eq!(engine.handoff(&ring, 100), 36); // 36 frames per 100 ms tick
        drain_checked(&ring, 36 * dma::uart::FRAME_BYTES, marker);
        marker = marker.wrapping_add(36);
    }
    assert_eq!(engine.frames(), 360);
    assert_eq!(engine.bytes(), u64::from(dma::uart::BYTES_PER_S)); // 11 520
    assert_eq!(engine.overruns(), 0);
    assert!(ring.is_empty());
    assert_eq!(ring.rejected(), 0);
    assert_eq!(ring.skipped(), 0);
}

/// Overloaded consumer: the drain keeps only half the rate. Every produced
/// frame is still exactly accounted — accepted or overrun, never torn —
/// the ring stays frame-aligned, and the ring's own `rejected` counter
/// stays zero because the engine checks space before writing.
#[test]
fn uart_overload_counts_overruns_and_stays_frame_aligned() {
    let ring: Spsc<u8, 1024> = Spsc::new(); // 32 frames
    let mut engine = SimulatedDma::uart();
    let mut drained_bytes = 0usize;
    for _ in 0..10 {
        engine.handoff(&ring, 100);
        // All-or-nothing: at every instant the ring holds whole frames.
        assert_eq!(ring.len() % dma::uart::FRAME_BYTES, 0);
        for _ in 0..(16 * dma::uart::FRAME_BYTES) {
            assert!(ring.pop().is_some());
        }
        drained_bytes += 16 * dma::uart::FRAME_BYTES;
    }
    // Exact conservation at the engine boundary: of 360 produced frames,
    // 176 landed (the first tick had a fully free ring) and 184 overran.
    assert_eq!(engine.frames(), 176);
    assert_eq!(engine.bytes(), 176 * dma::uart::FRAME_BYTES as u64);
    assert_eq!(engine.overruns(), 184);
    assert_eq!(engine.frames() + engine.overruns(), 360);
    // The ring's counter never fired: the engine pre-checks, it never
    // pushes into a full ring.
    assert_eq!(ring.rejected(), 0);
    // 160 drained frames left exactly 16 in the ring, aligned, in order.
    assert_eq!(drained_bytes, 160 * dma::uart::FRAME_BYTES);
    assert_eq!(ring.len(), 16 * dma::uart::FRAME_BYTES);
    drain_checked(&ring, ring.len(), 160);
}

/// I2S/PDM paced handoff: 8 windows/s into a one-window ring, drained
/// every 125 ms tick — one second, zero loss, byte-exact windows.
#[test]
fn i2s_window_handoff_is_lossless_when_paced() {
    let ring: Spsc<u8, 16_384> = Spsc::new(); // exactly one 12 000-B window
    let mut engine = SimulatedDma::i2s_pdm();
    for window in 0..8u8 {
        assert_eq!(engine.handoff(&ring, 125), 1);
        assert_eq!(ring.len(), dma::i2s_pdm::WINDOW_BYTES);
        for j in 0..dma::i2s_pdm::WINDOW_BYTES {
            assert_eq!(ring.pop(), Some(expect_byte(window, j)));
        }
    }
    assert_eq!(engine.frames(), 8);
    assert_eq!(engine.bytes(), 8 * dma::i2s_pdm::WINDOW_BYTES as u64);
    assert_eq!(engine.overruns(), 0);
}

/// I2S/PDM overrun: two windows become due in one 250 ms handoff while the
/// one-window ring still holds the first — the newest window is rejected
/// whole and counted; the resident window is never corrupted.
#[test]
fn i2s_overrun_rejects_the_newest_window_whole() {
    let ring: Spsc<u8, 16_384> = Spsc::new();
    let mut engine = SimulatedDma::i2s_pdm();
    assert_eq!(engine.handoff(&ring, 250), 1);
    assert_eq!(engine.overruns(), 1);
    assert_eq!(ring.len(), dma::i2s_pdm::WINDOW_BYTES);
    // The resident window is intact after the overrun.
    for j in 0..dma::i2s_pdm::WINDOW_BYTES {
        assert_eq!(ring.pop(), Some(expect_byte(0, j)));
    }
    // The overrun window never entered: the ring never held more than one.
    assert_eq!(engine.frames() + engine.overruns(), 2);
}

/// Ring overflow/lag policy edges: `skip(0)` is a no-op, skipping the
/// exact backlog drains it, and push-after-skip preserves FIFO order with
/// counters accumulating across rounds.
#[test]
fn ring_policy_edges_and_fifo_after_skip() {
    let ring: Spsc<u8, 8> = Spsc::new();
    assert_eq!(ring.skip(0), 0); // no-op, not counted
    assert_eq!(ring.skipped(), 0);
    for i in 0..8u8 {
        ring.push(i).unwrap();
    }
    assert_eq!(ring.push(8), Err(8)); // full: newest rejected, counted
    assert_eq!(ring.rejected(), 1);
    assert_eq!(ring.skip(8), 8); // skip the exact backlog
    assert!(ring.is_empty());
    assert_eq!(ring.skipped(), 8);
    // FIFO continues cleanly after a skip round.
    for i in 100..108u8 {
        assert_eq!(ring.push(i), Ok(()));
    }
    for i in 100..108u8 {
        assert_eq!(ring.pop(), Some(i));
    }
    assert_eq!(ring.rejected(), 1);
    assert_eq!(ring.skipped(), 8);
}

/// Sink-drain burst absorption (compiled only when a transport drain is):
/// a burst past the ring capacity is counted as rejected at the boundary;
/// one poll drains the whole backlog to completion and never blocks.
#[cfg(feature = "transport-http")]
#[test]
fn http_drain_absorbs_a_burst_and_catches_up() {
    use firmware_v6_dataflow::nodekit::{self, MeasRing};
    use firmware_v6_dataflow::transports::http::Node;

    let mut node = Node::new();
    let ring: &MeasRing = &node.in_ring;
    for i in 0..nodekit::MEAS_RING_CAP as u16 {
        assert!(ring.push(Measurement::Light { lux: i }).is_ok());
    }
    // Burst past capacity: the newest measurements are rejected and counted.
    assert!(!ring.push(Measurement::Light { lux: 9999 }).is_ok());
    assert_eq!(ring.rejected(), 1);
    // One drain step empties the backlog completely.
    assert_eq!(node.poll(), nodekit::MEAS_RING_CAP);
    assert!(node.in_ring.is_empty());
    assert_eq!(node.sent(), nodekit::MEAS_RING_CAP as u64);
    // Catch-up: the next burst drains in the following step.
    for i in 0..3u16 {
        assert!(node.in_ring.push(Measurement::Light { lux: i }).is_ok());
    }
    assert_eq!(node.poll(), 3);
    assert_eq!(node.sent(), nodekit::MEAS_RING_CAP as u64 + 3);
}

/// Budget floor: with every capability flag off, the statically-allocated
/// graph is essentially just its counters — the accounting must not charge
/// the system for capabilities it did not compile.
#[cfg(not(any(
    feature = "sensor-decibel",
    feature = "sensor-pax",
    feature = "sensor-bme280",
    feature = "sensor-sht4x",
    feature = "sensor-scd4x",
    feature = "sensor-pms5003",
    feature = "sensor-bh1750",
    feature = "transport-http",
    feature = "transport-mqtt",
    feature = "transport-mtls",
    feature = "ops-status",
    feature = "ota",
    feature = "provisioning",
    feature = "deep-sleep",
    feature = "logging-defmt"
)))]
#[test]
fn budget_all_off_graph_is_tiny() {
    assert!(Graph::BYTES <= 1024);
    let _ = Graph::build();
}
