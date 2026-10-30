//! PMS5003 source node — gated by the `sensor-pms5003` flag (pulls
//! `driver-uart`).
//!
//! Driver contract (spec §Sensor Expansion): UART; emits PM2.5 in µg/m³ as
//! the new `pm25` measurement type (one-topic-per-type rule; no server
//! receiver yet). In V6 the UART DMA engine pushes raw bytes into
//! [`Node`]'s ring; [`Node::poll`] parses complete 32-byte frames and
//! emits `pm25` measurements. The real field parse + checksum lands with
//! the driver PR — the scaffold folds the frame's marker byte into a
//! deterministic proxy value.

use crate::dma::uart;
use crate::measurement::Measurement;
use crate::nodekit::{self, MeasRing};
use crate::ring::Spsc;

/// UART baud rate for the PMS5003 (fixed by the datasheet).
pub const UART_BAUD: u32 = uart::BAUD;

/// PMS5003 frame length (32 bytes: start ×2, length, data ×28, checksum).
pub const FRAME_BYTES: usize = uart::FRAME_BYTES;

/// Ingest type name on the wire (new type: `pm25`).
pub const INGEST_TYPE: &str = "pm25";

/// Input byte-ring capacity (power of two): ~32 frames of slack.
pub const IN_RING_CAP: usize = 1024;

/// The PMS5003 node: byte ring in (UART-DMA-fed), measurements out.
pub struct Node {
    /// UART byte ring — written by the DMA engine, drained by poll.
    pub bytes: Spsc<u8, IN_RING_CAP>,
    /// Completed measurements waiting for a transport drain.
    pub out: MeasRing,
    frames: u64,
    emitted: u64,
}

impl Default for Node {
    fn default() -> Self {
        Self::new()
    }
}

impl Node {
    pub const fn new() -> Self {
        Self {
            bytes: Spsc::new(),
            out: MeasRing::new(),
            frames: 0,
            emitted: 0,
        }
    }

    /// Frames parsed since boot.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Measurements accepted into the output ring since boot.
    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    /// Static footprint of this node (rings + state), for budget accounting.
    pub const BYTES: usize = core::mem::size_of::<Node>();

    /// One stage step: parse every complete frame in the ring into a `pm25`
    /// measurement. Returns frames parsed this step.
    pub fn poll(&mut self) -> usize {
        let mut done = 0;
        while self.bytes.len() >= FRAME_BYTES {
            let mut marker = 0u8;
            for j in 0..FRAME_BYTES {
                match self.bytes.pop() {
                    Some(b) => {
                        if j == 0 {
                            marker = b;
                        }
                    }
                    // Unreachable: the ring only grows between the length
                    // check and the pops (this poll is the sole consumer).
                    None => break,
                }
            }
            // Scaffold fold: the frame marker byte maps to a deterministic
            // proxy value; the driver PR parses the real µg/m³ field.
            let ug_m3 = marker as u16 * 2;
            self.frames += 1;
            if nodekit::offer(&self.out, Measurement::Pm25 { ug_m3 }) {
                self.emitted += 1;
                done += 1;
            }
        }
        done
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dma::SimulatedDma;

    #[test]
    fn uart_frames_parse_to_pm25_measurements() {
        let mut node = Node::new();
        let mut engine = SimulatedDma::new(FRAME_BYTES);
        assert_eq!(engine.transfer(&node.bytes, 3), 3);
        assert_eq!(node.poll(), 3);
        assert_eq!(node.frames(), 3);
        assert_eq!(node.emitted(), 3);
        // Frames carry increasing markers → increasing proxy values.
        let a = node.out.pop();
        let b = node.out.pop();
        match (a, b) {
            (
                Some(Measurement::Pm25 { ug_m3: a }),
                Some(Measurement::Pm25 { ug_m3: b }),
            ) => {
                assert!(b > a)
            }
            other => panic!("expected pm25 measurements, got {other:?}"),
        }
        assert!(node.bytes.is_empty());
    }

    #[test]
    fn partial_frame_is_held_back() {
        let mut node = Node::new();
        let mut engine = SimulatedDma::new(FRAME_BYTES / 2);
        assert_eq!(engine.transfer(&node.bytes, 1), 1);
        assert_eq!(node.poll(), 0);
        assert_eq!(node.frames(), 0);
        assert_eq!(node.bytes.len(), FRAME_BYTES / 2);
    }
}
