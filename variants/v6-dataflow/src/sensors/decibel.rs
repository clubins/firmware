//! Decibel source node — gated by the `sensor-decibel` flag.
//!
//! Driver contract (spec §Sensor Expansion + the source monorepo's
//! implementation_notes/decibel.md): SPK0641HT4H-1 PDM mic on I2S0 at
//! 48 kHz, A/C weighting, LAeq/LCeq with Fast extrema, overload detection,
//! per-unit calibration. In V6 the I2S/PDM DMA engine pushes raw bytes
//! straight into [`Node`]'s ring; [`Node::poll`] folds every complete
//! 125 ms Fast window into one `decibel` measurement (`dba`/`dbc`, 0.1 dB
//! fixed point). The DSP chain itself lands with the driver PR — the
//! scaffold folds deterministically and labels the level a proxy.

use crate::dma::i2s_pdm;
use crate::measurement::Measurement;
use crate::nodekit::{self, MeasRing};
use crate::ring::Spsc;

/// I2S0 PDM sample rate (Hz) per the design note.
pub const PDM_SAMPLE_RATE_HZ: u32 = i2s_pdm::SAMPLE_RATE_HZ;

/// Bytes per sample: mono, 16-bit PCM after decimation.
pub const BYTES_PER_SAMPLE: usize = i2s_pdm::BYTES_PER_SAMPLE;

/// Fast-window length per the design note (ms).
pub const FAST_WINDOW_MS: u32 = i2s_pdm::FAST_WINDOW_MS;

/// Bytes per Fast window: 48 000 Hz × 0.125 s × 2 B = 12 000.
pub const WINDOW_BYTES: usize = i2s_pdm::WINDOW_BYTES;

/// Ingest type name on the wire (HTTP path `/v1/sensors/decibel`, MQTT
/// topic suffix `…/data/decibel`).
pub const INGEST_TYPE: &str = "decibel";

/// Input byte-ring capacity (power of two): one full window plus slack.
pub const IN_RING_CAP: usize = 16_384;

/// The decibel node: ring in (DMA-fed PCM), measurements out.
pub struct Node {
    /// PCM byte ring — written by the I2S/PDM DMA engine, drained by
    /// [`Node::poll`] (the only consumer).
    pub pcm: Spsc<u8, IN_RING_CAP>,
    /// Completed measurements waiting for a transport drain.
    pub out: MeasRing,
    windows: u64,
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
            pcm: Spsc::new(),
            out: MeasRing::new(),
            windows: 0,
            emitted: 0,
        }
    }

    /// Fast windows folded since boot.
    pub fn windows(&self) -> u64 {
        self.windows
    }

    /// Measurements accepted into the output ring since boot.
    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    /// Static footprint of this node (rings + state), for budget accounting.
    pub const BYTES: usize = core::mem::size_of::<Node>();

    /// One stage step: fold every complete Fast window in the ring into a
    /// `decibel` measurement. Returns windows completed this step.
    pub fn poll(&mut self) -> usize {
        let mut done = 0;
        while self.pcm.len() >= WINDOW_BYTES {
            let mut acc = 0u32;
            for _ in 0..WINDOW_BYTES {
                match self.pcm.pop() {
                    Some(b) => acc += b as u32,
                    // Unreachable: the ring only grows between the length
                    // check and the pops (this poll is the sole consumer).
                    None => break,
                }
            }
            // Scaffold fold: a deterministic proxy level from the window's
            // bytes. The driver PR replaces this with the decibel.md chain
            // (A/C weighting → LAeq/LCeq with Fast extrema → overload
            // detection → per-unit calibration from NVS).
            let dba = 300 + (acc % 500) as u16; // 30.0–79.9 dB fixed point
            let dbc = dba + 50; // C weighting sees more low end
            self.windows += 1;
            if nodekit::offer(&self.out, Measurement::Decibel { dba, dbc }) {
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
    fn dma_windows_fold_into_decibel_measurements() {
        let mut node = Node::new();
        let mut engine = SimulatedDma::new(WINDOW_BYTES);
        assert_eq!(engine.transfer(&node.pcm, 1), 1);
        assert_eq!(node.poll(), 1);
        assert_eq!(node.windows(), 1);
        assert_eq!(node.emitted(), 1);
        match node.out.pop() {
            Some(Measurement::Decibel { dba, dbc }) => {
                assert!((300..800).contains(&dba), "proxy level in range: {dba}");
                assert_eq!(dbc, dba + 50);
            }
            other => panic!("expected a decibel measurement, got {other:?}"),
        }
        assert!(node.pcm.is_empty());
    }

    #[test]
    fn partial_window_is_not_emitted() {
        let mut node = Node::new();
        let mut engine = SimulatedDma::new(WINDOW_BYTES / 2);
        assert_eq!(engine.transfer(&node.pcm, 1), 1);
        assert_eq!(node.poll(), 0);
        assert_eq!(node.windows(), 0);
        assert!(node.out.is_empty());
        // The half window is still buffered — it completes later.
        assert_eq!(node.pcm.len(), WINDOW_BYTES / 2);
        assert_eq!(engine.transfer(&node.pcm, 1), 1);
        assert_eq!(node.poll(), 1);
        assert_eq!(node.windows(), 1);
    }

    #[test]
    fn identical_windows_give_identical_levels() {
        // Determinism of the scaffold fold: two identical byte streams must
        // produce identical levels (the DSP PR will keep this property).
        let mut a = Node::new();
        let mut b = Node::new();
        for node in [&a, &b] {
            let mut engine = SimulatedDma::new(WINDOW_BYTES);
            assert_eq!(engine.transfer(&node.pcm, 1), 1);
        }
        a.poll();
        b.poll();
        assert_eq!(a.out.pop(), b.out.pop());
    }

    #[test]
    fn node_footprint_accounts_for_its_rings() {
        // The budget story is honest: the node's declared footprint covers
        // its rings' payload space.
        assert!(
            Node::BYTES
                >= IN_RING_CAP + nodekit::MEAS_RING_CAP * size_of::<Measurement>()
        );
    }
}
