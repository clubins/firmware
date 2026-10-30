//! GPIO edge seam — the interrupt front end the PIR counts on
//! (`sensor-pax`).
//!
//! The driver-side contract is one question: *what was the next level
//! transition?* On the device that question is answered by the pin's own
//! edge interrupts — the executor parks the task until the pad actually
//! moves, so the front end is event-driven, not polled. On the host,
//! [`fakes::ScriptedEdges`] replays a scripted edge sequence, which is
//! what makes the debounce flow in [`crate::sensors::pir`] testable as a
//! deterministic fixture.
//!
//! The seam returns edges, not levels: which edge arrived is the fact the
//! driver counts; the current level is the adapter's bookkeeping.

/// A level transition on the PIR's output line.
///
/// The HC-SR501 idles low and drives high while it holds a detection: a
/// rising edge opens a motion episode, a falling edge closes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// The line went from low to high.
    Rising,
    /// The line went from high to low.
    Falling,
}

/// The PIR's interrupt front end, inverted for testability.
///
/// Implementations await the next transition and report which one it
/// was. `next_edge` is cancel-safe by construction: a driver that stops
/// awaiting mid-wait has observed nothing, and the next call re-arms the
/// wait (the device adapter additionally re-syncs its tracked level
/// against the pad, so a transition missed while the task was busy
/// surfaces as the next edge instead of being lost).
// See the note on `Sleep`: task-local futures, never `Send`.
#[allow(async_fn_in_trait)]
pub trait EdgeSource {
    /// Wait for the next level transition and report it.
    async fn next_edge(&mut self) -> Edge;
}

/// Host fakes for the edge seam.
pub mod fakes {
    use super::Edge;
    use crate::sensors::gpio::EdgeSource;

    /// A scripted edge source: replays a fixed edge sequence, then parks
    /// forever.
    ///
    /// The final park models the real front end honestly — a PIR with no
    /// further motion never reports an edge — and keeps a fixture that
    /// runs past its script from spinning.
    #[derive(Debug, Clone)]
    pub struct ScriptedEdges {
        script: heapless::Vec<Edge, 64>,
        played: usize,
    }

    impl ScriptedEdges {
        /// Script the edge sequence the fake replays.
        #[must_use]
        pub fn new(script: &[Edge]) -> Self {
            let mut edges = heapless::Vec::<Edge, 64>::new();
            for &edge in script {
                edges.push(edge).expect("script fits the fixture buffer");
            }
            Self {
                script: edges,
                played: 0,
            }
        }
    }

    impl EdgeSource for ScriptedEdges {
        async fn next_edge(&mut self) -> Edge {
            if self.played < self.script.len() {
                let edge = self.script[self.played];
                self.played += 1;
                edge
            } else {
                // Park forever: no future completes, mirroring a silent
                // front end. The executor-free host test simply never
                // resumes past this point.
                core::future::pending::<()>().await;
                unreachable!("pending() never resolves")
            }
        }
    }
}
