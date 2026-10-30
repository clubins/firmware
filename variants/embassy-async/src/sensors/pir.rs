//! PIR occupancy sensing — HC-SR501-class motion detection over the GPIO
//! edge seam (`sensor-pax` → `pax`).
//!
//! The front end is a passive-infrared module that idles its output low
//! and drives it high while it holds a detection (retriggerable mode):
//! a rising edge opens a motion episode, a falling edge closes it. The
//! module's own hold-time logic already debounces the *sensor*; what
//! remains to debounce is the *episode*: motion bursts separated by less
//! than [`crate::config::PIR_DEBOUNCE_MS`] of observed idle time merge
//! into one detection instead of counting as separate people walking
//! through.
//!
//! The counting contract, in the order the loop enforces it:
//!
//! 1. **Settle** — a PIR reports garbage for the first tens of seconds
//!    after power-on while its lens and amplifier stabilise. The task
//!    waits [`crate::config::PIR_SETTLE_MS`] of silent time before the
//!    first edge wait; no edges are counted during warm-up.
//! 2. **Count** — every observed rising edge is one detection, followed
//!    by a blind window of `debounce_ms`: transitions inside the window
//!    are not awaited (the front end is already holding the line high,
//!    so in retriggerable mode there is no new edge to miss).
//! 3. **Publish** — the first observed falling edge after any detection
//!    closes the episode and publishes its count as one `pax` event.
//!    A falling edge before any detection is an idle glitch and keeps
//!    waiting.
//!
//! A front end stuck high therefore never publishes — the honest reading
//! is "one ongoing episode", and the framework-level supervision sees a
//! quiet task, not a fabricated one.
//!
//! This is a dedicated task body, not the `SensorDriver` sampling
//! framework: the sensor reports events, not periodic samples, so the
//! loop parks on the edge seam between episodes instead of pacing a
//! poll. Publishing still goes through the measurement channel with a
//! per-boot [`Sequencer`](crate::session::Sequencer), exactly as the
//! framework's tasks do.

use crate::events::{MeasurementEvent, MeasurementPayload, PaxSample};
use crate::sensor::Sleep;
use crate::sensors::gpio::{Edge, EdgeSource};
use crate::session::Sequencer;

/// The PIR occupancy driver: edge source + time seam, one episode
/// counter.
pub struct Pir<E, S> {
    edges: E,
    sleeper: S,
    settle_ms: u64,
    debounce_ms: u64,
}

impl<E: EdgeSource, S: Sleep> Pir<E, S> {
    /// Build a driver over `edges`, timing warm-up and the debounce
    /// window with `sleeper`.
    #[must_use]
    pub const fn new(edges: E, sleeper: S, settle_ms: u64, debounce_ms: u64) -> Self {
        Self {
            edges,
            sleeper,
            settle_ms,
            debounce_ms,
        }
    }

    /// Wait out the front end's power-on stabilisation. Silent time, not
    /// a poll: the executor runs every other task meanwhile.
    async fn settle(&mut self) {
        self.sleeper.sleep(self.settle_ms).await;
    }

    /// Count one motion episode: park on edges until a detection has
    /// been seen and the line has returned to idle, then report the
    /// debounced detection count.
    async fn sample(&mut self) -> PaxSample {
        let mut count: u32 = 0;
        loop {
            match self.edges.next_edge().await {
                Edge::Rising => {
                    count += 1;
                    // The debounce window: a motion burst landing inside
                    // it is the same detection. The window is blind —
                    // transitions during it are not awaited — so a line
                    // that fell inside it resurfaces as the next edge
                    // (the device adapter re-syncs against the pad).
                    self.sleeper.sleep(self.debounce_ms).await;
                }
                // A fall after detections closes the episode.
                Edge::Falling if count > 0 => {
                    return PaxSample {
                        occupancy_count: count,
                    };
                }
                // A fall before any detection is an idle glitch.
                Edge::Falling => {}
            }
        }
    }
}

/// The pax task body: settle, then publish one `pax` event per motion
/// episode, counted through the debounce window.
pub async fn run<E: EdgeSource, S: Sleep>(
    driver: &mut Pir<E, S>,
    publisher: &mut crate::channels::MeasurementSender<'_>,
) {
    driver.settle().await;
    let mut seq = Sequencer::new();
    loop {
        let payload = MeasurementPayload::Pax(driver.sample().await);
        let event = MeasurementEvent {
            payload,
            time_ms: 0,
            seq: seq.next_seq(),
        };
        publisher.send(event).await;
    }
}

#[cfg(test)]
mod tests {
    use embassy_futures::block_on;
    use embassy_futures::select::{Either, select};

    use super::{Pir, run};
    use crate::channels::Queues;
    use crate::events::MeasurementPayload;
    use crate::sensor::fakes::MockClock;
    use crate::sensors::gpio::{Edge, fakes::ScriptedEdges};

    /// One episode — a rise then the line returning to idle — publishes
    /// a single detection, and the waits are settle-then-window.
    #[test]
    fn one_episode_counts_one_detection() {
        block_on(async {
            let mut clock = MockClock::new();
            let mut driver = Pir::new(
                ScriptedEdges::new(&[Edge::Rising, Edge::Falling]),
                &mut clock,
                100,
                50,
            );
            driver.settle().await;
            let sample = driver.sample().await;
            assert_eq!(sample.occupancy_count, 1);
            assert_eq!(clock.sleeps.as_slice(), &[100, 50]);
        });
    }

    /// Two motion bursts — separate rise/fall pairs — are separate
    /// episodes: one detection each, one debounce window each. (A fall
    /// inside a blind window is the device adapter's resync case; the
    /// fixture only advances edges on demand, so it models two clean
    /// hold periods.)
    #[test]
    fn separate_bursts_count_separately() {
        block_on(async {
            let mut driver = Pir::new(
                ScriptedEdges::new(&[
                    Edge::Rising,
                    Edge::Falling,
                    Edge::Rising,
                    Edge::Falling,
                ]),
                MockClock::new(),
                0,
                50,
            );
            let first = driver.sample().await;
            let second = driver.sample().await;
            assert_eq!(first.occupancy_count, 1);
            assert_eq!(second.occupancy_count, 1);
            let Pir { sleeper, .. } = driver;
            assert_eq!(sleeper.sleeps.as_slice(), &[50, 50]);
        });
    }

    /// A falling edge before any detection is an idle glitch: it counts
    /// nothing and opens no window.
    #[test]
    fn idle_glitch_before_motion_counts_nothing() {
        block_on(async {
            let mut clock = MockClock::new();
            let mut driver = Pir::new(
                ScriptedEdges::new(&[Edge::Falling, Edge::Rising, Edge::Falling]),
                &mut clock,
                0,
                50,
            );
            let sample = driver.sample().await;
            assert_eq!(sample.occupancy_count, 1);
            assert_eq!(clock.sleeps.as_slice(), &[50]);
        });
    }

    /// The task body publishes one pax event per episode, sequenced from
    /// 0 with the unsynchronised timestamp, and parks on the edge seam
    /// between episodes (the drain future completing drops the loop
    /// mid-wait, which the edge seam survives by construction).
    #[test]
    fn task_publishes_one_event_per_episode() {
        block_on(async {
            let queue = Queues::new();
            let mut sender = queue.measurements.sender();
            let mut clock = MockClock::new();
            let mut driver = Pir::new(
                ScriptedEdges::new(&[Edge::Rising, Edge::Falling]),
                &mut clock,
                100,
                50,
            );

            let drain = async { queue.measurements.receive().await };
            // The run loop parks on the edge seam after the first
            // episode (the script is exhausted), so the drain future is
            // what completes: select drops the loop mid-wait, which the
            // edge seam survives by construction.
            let Either::Second(event) =
                select(run(&mut driver, &mut sender), drain).await
            else {
                unreachable!("the drain future completes while the run loop parks")
            };

            assert_eq!(event.seq, 0);
            assert_eq!(event.time_ms, 0);
            assert_eq!(
                event.payload,
                MeasurementPayload::Pax(crate::events::PaxSample { occupancy_count: 1 })
            );
            // Settle, then one debounce window.
            assert_eq!(clock.sleeps.as_slice(), &[100, 50]);
        });
    }
}
