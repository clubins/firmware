//! DMA-backed source abstractions, simulated on the host.
//!
//! The V6 dataflow claim under simulation: a DMA engine hands frames of
//! bytes into a ring **non-blockingly** and **all-or-nothing** — a frame
//! either lands complete in the ring or is counted as an overrun, and the
//! engine's private buffer is reused for its next transfer (hardware
//! semantics). Rejection happens *before* any byte is written, so no torn
//! frame ever enters the ring and the stream stays frame-aligned.
//!
//! The simulated engines use the real domain rates: I2S/PDM at 48 kHz
//! (one 125 ms Fast window per frame) and UART at 115_200 Bd (32-byte
//! PMS5003 frames). The payload is a deterministic pattern (frame marker
//! byte + sawtooth) so consumers can verify integrity; a real engine's
//! payload is its peripheral's data — the pattern is the host-test
//! stand-in.

use crate::ring::Spsc;

/// Domain constants for the I2S/PDM engine (per implementation_notes/
/// decibel.md: SPK0641HT4H-1, I2S0 PDM, 48 kHz).
pub mod i2s_pdm {
    /// PDM sample rate (Hz).
    pub const SAMPLE_RATE_HZ: u32 = 48_000;
    /// Bytes per sample: mono, 16-bit PCM after decimation.
    pub const BYTES_PER_SAMPLE: usize = 2;
    /// Fast-window length (ms) — the engine's natural frame.
    pub const FAST_WINDOW_MS: u32 = 125;
    /// Bytes per Fast window: 48 000 Hz × 0.125 s × 2 B.
    pub const WINDOW_BYTES: usize =
        SAMPLE_RATE_HZ as usize * BYTES_PER_SAMPLE * FAST_WINDOW_MS as usize / 1000;
    /// Fast windows handed per second at the sustained rate (one per
    /// 125 ms window — exact, no drift).
    pub const WINDOWS_PER_S: u64 = 1000 / FAST_WINDOW_MS as u64;
}

/// Domain constants for the UART engine (PMS5003 front-end).
pub mod uart {
    /// Fixed by the PMS5003 datasheet.
    pub const BAUD: u32 = 115_200;
    /// 8N1: ten bits on the wire per byte.
    pub const BITS_PER_BYTE: u32 = 10;
    /// Sustained wire rate in bytes/second.
    pub const BYTES_PER_S: u32 = BAUD / BITS_PER_BYTE;
    /// PMS5003 frame length (start ×2, length, data ×28, checksum).
    pub const FRAME_BYTES: usize = 32;
    /// Whole frames handed per second at the sustained wire rate
    /// (11 520 B/s ÷ 32 B per frame — exact, no drift).
    pub const FRAMES_PER_S: u64 = BYTES_PER_S as u64 / FRAME_BYTES as u64;
}

/// A simulated DMA engine producing a deterministic byte stream into a
/// ring. Never blocks; losses are counted, never torn.
pub struct SimulatedDma {
    frame_len: usize,
    /// Sustained frame rate (frames per second) of the clocked
    /// [`SimulatedDma::handoff`] model; 0 for hand-stepped engines.
    rate_frames_per_s: u64,
    /// Unhanded milli-frames carried between clocked handoffs, so odd
    /// rates × short intervals sum to the exact sustained throughput.
    rate_milliframes: u64,
    next_frame: u64,
    frames: u64,
    overruns: u64,
    bytes: u64,
}

impl SimulatedDma {
    /// An engine handing `frame_len`-byte frames. No sustained rate: it
    /// is stepped by hand with [`SimulatedDma::transfer`] (the graph pump
    /// does this one frame per step).
    pub const fn new(frame_len: usize) -> Self {
        Self {
            frame_len,
            rate_frames_per_s: 0,
            rate_milliframes: 0,
            next_frame: 0,
            frames: 0,
            overruns: 0,
            bytes: 0,
        }
    }

    /// The I2S/PDM engine: one 12 000-byte Fast window per handoff, 8
    /// windows/s sustained (48 kHz mono 16-bit, 125 ms windows).
    pub const fn i2s_pdm() -> Self {
        Self::clocked(i2s_pdm::WINDOW_BYTES, i2s_pdm::WINDOWS_PER_S)
    }

    /// The UART engine: one 32-byte PMS5003 frame per handoff unit, 360
    /// frames/s sustained (115 200 Bd 8N1 ÷ 10 bits/byte ÷ 32 B).
    pub const fn uart() -> Self {
        Self::clocked(uart::FRAME_BYTES, uart::FRAMES_PER_S)
    }

    /// An engine with an explicit sustained rate (frames per second).
    const fn clocked(frame_len: usize, frames_per_s: u64) -> Self {
        Self {
            frame_len,
            rate_frames_per_s: frames_per_s,
            rate_milliframes: 0,
            next_frame: 0,
            frames: 0,
            overruns: 0,
            bytes: 0,
        }
    }

    /// Frame size the engine transfers (bytes).
    pub fn frame_len(&self) -> usize {
        self.frame_len
    }

    /// Frames accepted into a ring so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Frames lost to a full ring so far (never entered the ring).
    pub fn overruns(&self) -> u64 {
        self.overruns
    }

    /// Bytes accepted into a ring so far.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Hand `frames` frames into `ring`. Returns how many landed. Never
    /// blocks; a frame that does not fit whole is skipped and counted.
    pub fn transfer<const CAP: usize>(
        &mut self,
        ring: &Spsc<u8, CAP>,
        frames: usize,
    ) -> usize {
        let mut handed = 0;
        for _ in 0..frames {
            if CAP - ring.len() < self.frame_len {
                // All-or-nothing: the whole frame is lost, counted as an
                // overrun, and the engine's private buffer moves on.
                self.overruns += 1;
                continue;
            }
            let marker = self.next_frame as u8;
            let mut written = 0;
            while written < self.frame_len {
                // The space check above plus the SPSC invariant (only this
                // producer fills, the consumer only drains) make these
                // pushes succeed; a rejection is still counted honestly.
                if ring.push(frame_byte(marker, written)).is_err() {
                    break;
                }
                written += 1;
            }
            if written == self.frame_len {
                self.frames += 1;
                self.bytes += self.frame_len as u64;
                self.next_frame = self.next_frame.wrapping_add(1);
                handed += 1;
            } else {
                // Unreachable for a sole producer (free space only grows
                // while we push); count the frame as lost, never as sent.
                self.overruns += 1;
            }
        }
        handed
    }

    /// Clocked handoff: credit `elapsed_ms` of sustained-rate production
    /// and hand every frame that became due into `ring`. The milli-frame
    /// accumulator carries sub-frame remainders, so summing many short
    /// handoffs reproduces the exact sustained rate (the UART engine's
    /// 360 frames/s is 36 frames per 100 ms tick, 3.6 per 10 ms tick).
    /// Returns how many frames landed this call. Never blocks; overruns
    /// are counted by [`SimulatedDma::transfer`] exactly as for
    /// hand-stepped engines. Rate-0 engines hand nothing.
    pub fn handoff<const CAP: usize>(
        &mut self,
        ring: &Spsc<u8, CAP>,
        elapsed_ms: u32,
    ) -> usize {
        self.rate_milliframes = self
            .rate_milliframes
            .saturating_add(self.rate_frames_per_s.saturating_mul(u64::from(elapsed_ms)));
        let due = (self.rate_milliframes / 1000) as usize;
        self.rate_milliframes %= 1000;
        self.transfer(ring, due)
    }
}

/// The payload pattern: byte 0 carries the frame marker, the rest sawtooth.
fn frame_byte(marker: u8, j: usize) -> u8 {
    if j == 0 { marker } else { j as u8 }
}

/// Reassemble the pattern a consumer expects for frame `marker`.
#[cfg(test)]
pub fn expect_frame_byte(marker: u8, j: usize) -> u8 {
    frame_byte(marker, j)
}

/// A simulated GPIO ISR: pushes event stamps into a ring exactly like a DMA
/// engine hands frames — non-blockingly, losses visible as the ring's
/// `rejected` counter. The driver PR replaces this with the real GPIO
/// interrupt handler.
pub struct SimulatedIsr {
    next_stamp: u64,
}

impl Default for SimulatedIsr {
    fn default() -> Self {
        Self::new()
    }
}

impl SimulatedIsr {
    pub const fn new() -> Self {
        Self { next_stamp: 0 }
    }

    /// Fire `events` detections into `ring`; returns how many landed.
    pub fn fire<const CAP: usize>(
        &mut self,
        ring: &Spsc<u64, CAP>,
        events: usize,
    ) -> usize {
        let mut landed = 0;
        for _ in 0..events {
            if ring.push(self.next_stamp).is_ok() {
                self.next_stamp = self.next_stamp.wrapping_add(1);
                landed += 1;
            }
        }
        landed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hands_off_complete_frames_with_integrity() {
        let ring: Spsc<u8, 16> = Spsc::new();
        let mut engine = SimulatedDma::new(4);
        assert_eq!(engine.transfer(&ring, 3), 3);
        assert_eq!(engine.frames(), 3);
        assert_eq!(engine.bytes(), 12);
        assert_eq!(engine.overruns(), 0);
        assert_eq!(ring.len(), 12);
        // Frames are contiguous and patterned: marker then sawtooth.
        for frame in 0..3u8 {
            for j in 0..4 {
                assert_eq!(ring.pop(), Some(expect_frame_byte(frame, j)));
            }
        }
        assert!(ring.is_empty());
    }

    #[test]
    fn overruns_when_the_ring_is_full() {
        let ring: Spsc<u8, 16> = Spsc::new();
        let mut engine = SimulatedDma::new(8);
        // Two frames fill the ring exactly; the third overruns.
        assert_eq!(engine.transfer(&ring, 3), 2);
        assert_eq!(engine.frames(), 2);
        assert_eq!(engine.overruns(), 1);
        assert_eq!(ring.len(), 16);
        // The two landed frames are intact.
        for frame in 0..2u8 {
            for j in 0..8 {
                assert_eq!(ring.pop(), Some(expect_frame_byte(frame, j)));
            }
        }
    }

    #[test]
    fn frame_atomicity_no_partial_frame_enters() {
        let ring: Spsc<u8, 16> = Spsc::new();
        // Pre-fill 12 bytes from the "previous" transfers; only 4 free.
        let mut small = SimulatedDma::new(4);
        assert_eq!(small.transfer(&ring, 3), 3);
        // An 8-byte frame does not fit: it must be skipped whole.
        let mut engine = SimulatedDma::new(8);
        assert_eq!(engine.transfer(&ring, 1), 0);
        assert_eq!(engine.overruns(), 1);
        // The ring holds exactly the earlier bytes, uncorrupted.
        assert_eq!(ring.len(), 12);
        for frame in 0..3u8 {
            for j in 0..4 {
                assert_eq!(ring.pop(), Some(expect_frame_byte(frame, j)));
            }
        }
    }

    #[test]
    fn domain_constants_match_the_design_notes() {
        // decibel.md: 48 kHz PDM, 125 ms Fast windows, 16-bit PCM.
        assert_eq!(i2s_pdm::SAMPLE_RATE_HZ, 48_000);
        assert_eq!(i2s_pdm::WINDOW_BYTES, 12_000);
        // PMS5003: 115_200 8N1 → 11_520 B/s, 32-byte frames.
        assert_eq!(uart::BYTES_PER_S, 11_520);
        assert_eq!(uart::FRAME_BYTES, 32);
    }

    #[test]
    fn isr_stamps_land_and_overflow_is_visible() {
        let ring: Spsc<u64, 4> = Spsc::new();
        let mut isr = SimulatedIsr::new();
        assert_eq!(isr.fire(&ring, 3), 3);
        assert_eq!(isr.fire(&ring, 3), 1); // one slot left
        assert_eq!(ring.len(), 4);
        assert_eq!(ring.rejected(), 2); // the lost detections are counted
        // Stamps are monotonic.
        assert_eq!(ring.pop(), Some(0));
        assert_eq!(ring.pop(), Some(1));
        assert_eq!(ring.pop(), Some(2));
        assert_eq!(ring.pop(), Some(3));
    }

    #[test]
    fn clocked_domain_rates_are_exact() {
        // The rate model only stays honest if the divides were exact.
        assert_eq!(i2s_pdm::WINDOWS_PER_S, 8);
        assert_eq!(uart::FRAMES_PER_S, 360);
        assert_eq!(SimulatedDma::i2s_pdm().frame_len(), i2s_pdm::WINDOW_BYTES);
        assert_eq!(SimulatedDma::uart().frame_len(), uart::FRAME_BYTES);
    }

    #[test]
    fn handoff_reproduces_the_sustained_rate() {
        // One second of UART in a single handoff: exactly 360 frames,
        // 11 520 bytes — the sustained wire rate, no drift.
        let ring: Spsc<u8, 16_384> = Spsc::new();
        let mut engine = SimulatedDma::uart();
        assert_eq!(engine.handoff(&ring, 1000), 360);
        assert_eq!(engine.frames(), 360);
        assert_eq!(engine.overruns(), 0);
        assert_eq!(engine.bytes(), u64::from(uart::BYTES_PER_S));
        // One second of I2S/PDM in a single handoff: 8 windows due, but
        // the ring holds one 12 000-byte window — 1 lands, 7 overrun.
        let pcm: Spsc<u8, 16_384> = Spsc::new();
        let mut pdm = SimulatedDma::i2s_pdm();
        assert_eq!(pdm.handoff(&pcm, 1000), 1);
        assert_eq!(pdm.overruns(), 7);
        assert_eq!(pcm.len(), i2s_pdm::WINDOW_BYTES);
    }

    #[test]
    fn handoff_carries_milli_frame_remainders() {
        // 10 ms of UART = 3.6 frames: three whole frames, the 0.6 carried.
        let ring: Spsc<u8, 16_384> = Spsc::new();
        let mut engine = SimulatedDma::uart();
        assert_eq!(engine.handoff(&ring, 10), 3);
        // Next 10 ms: the carry sums to 4.2 due — four whole frames.
        assert_eq!(engine.handoff(&ring, 10), 4);
        // 20 ms at 360 fps is 7.2 frames — 3 + 4, exact in total.
        assert_eq!(engine.frames(), 7);
        // Sub-frame ticks accumulate too: three 1 ms ticks (3.6 frames
        // total) hand the first whole frame on the third tick.
        let mut drip = SimulatedDma::uart();
        let drip_ring: Spsc<u8, 16_384> = Spsc::new();
        assert_eq!(drip.handoff(&drip_ring, 1), 0);
        assert_eq!(drip.handoff(&drip_ring, 1), 0);
        assert_eq!(drip.handoff(&drip_ring, 1), 1);
    }

    #[test]
    fn rate_zero_engine_hands_nothing() {
        let ring: Spsc<u8, 16_384> = Spsc::new();
        let mut engine = SimulatedDma::new(uart::FRAME_BYTES);
        assert_eq!(engine.handoff(&ring, 1000), 0);
        assert!(ring.is_empty());
    }
}
