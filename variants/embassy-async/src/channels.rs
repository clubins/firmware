//! Typed bounded channels connecting device tasks.
//!
//! One channel per event kind, statically allocated, each depth a named
//! constant in [`config`](crate::config). Sensor and ops tasks are producers,
//! the active transport task is the single consumer of both queues.
//! `send().await` blocks the producer when a channel is full — that
//! backpressure is the isolation boundary: a stalled transport slows
//! sampling instead of losing data or hiding the slowdown in a growing heap.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};

use crate::config::{MEASUREMENT_QUEUE_DEPTH, STATUS_QUEUE_DEPTH};
use crate::events::{MeasurementEvent, StatusEvent};

/// Bounded queue of measurements, sensor tasks → transport task.
pub type MeasurementQueue =
    Channel<CriticalSectionRawMutex, MeasurementEvent, MEASUREMENT_QUEUE_DEPTH>;

/// Bounded queue of status events, ops tasks → transport task.
pub type StatusQueue = Channel<CriticalSectionRawMutex, StatusEvent, STATUS_QUEUE_DEPTH>;

/// Producer endpoint for measurement events (one per sensor task).
pub type MeasurementSender<'a> =
    Sender<'a, CriticalSectionRawMutex, MeasurementEvent, MEASUREMENT_QUEUE_DEPTH>;

/// Consumer endpoint for measurement events (the active transport task).
pub type MeasurementReceiver<'a> =
    Receiver<'a, CriticalSectionRawMutex, MeasurementEvent, MEASUREMENT_QUEUE_DEPTH>;

/// Producer endpoint for status events (one per ops task).
pub type StatusSender<'a> =
    Sender<'a, CriticalSectionRawMutex, StatusEvent, STATUS_QUEUE_DEPTH>;

/// Consumer endpoint for status events (the active transport task).
pub type StatusReceiver<'a> =
    Receiver<'a, CriticalSectionRawMutex, StatusEvent, STATUS_QUEUE_DEPTH>;

/// Both queues, constructed once at boot (a static on the device, a local in
/// tests). [`Channel::new`] is const, so this is a const expression too.
#[derive(Debug)]
pub struct Queues {
    pub measurements: MeasurementQueue,
    pub status: StatusQueue,
}

impl Queues {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            measurements: Channel::new(),
            status: Channel::new(),
        }
    }
}

impl Default for Queues {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use embassy_futures::block_on;
    use embassy_futures::join::join;
    use embassy_sync::channel::TrySendError;

    use super::*;
    use crate::events::{Co2Sample, DecibelSample, MeasurementPayload, StatusResult};
    use crate::session::Sequencer;

    fn decibel_event(seq: u32, laeq: f32) -> MeasurementEvent {
        MeasurementEvent {
            payload: MeasurementPayload::Decibel(DecibelSample {
                window_ms: 1000,
                overload: false,
                laeq,
                la_fmin: laeq - 10.0,
                la_fmax: laeq + 10.0,
                lceq: laeq + 5.0,
                lc_fmax: laeq + 12.0,
            }),
            time_ms: 0,
            seq,
        }
    }

    #[test]
    fn measurements_round_trip_in_send_order() {
        let queues = Queues::new();
        assert!(queues.measurements.try_send(decibel_event(0, 90.0)).is_ok());
        assert!(queues.measurements.try_send(decibel_event(1, 91.5)).is_ok());
        assert!(queues.measurements.try_send(decibel_event(2, 93.0)).is_ok());

        // FIFO: the first event sent is the first event received, seq
        // included.
        let first = block_on(queues.measurements.receive());
        let second = block_on(queues.measurements.receive());
        match (first.payload, second.payload) {
            (MeasurementPayload::Decibel(a), MeasurementPayload::Decibel(b)) => {
                assert_eq!(a.laeq, 90.0);
                assert_eq!(b.laeq, 91.5);
                assert_eq!(first.time_ms, 0);
                assert_eq!(first.seq, 0);
                assert_eq!(second.seq, 1);
            }
            _ => panic!("expected decibel payloads"),
        }
    }

    #[test]
    fn full_measurement_queue_applies_backpressure() {
        let queues = Queues::new();
        for i in 0..MEASUREMENT_QUEUE_DEPTH {
            let event = MeasurementEvent {
                payload: MeasurementPayload::Co2(Co2Sample { ppm: i as f32 }),
                time_ms: 0,
                seq: i as u32,
            };
            assert!(
                queues.measurements.try_send(event).is_ok(),
                "slot {i} should fit"
            );
        }

        // One event past capacity is refused; the value comes back so the
        // producer keeps it.
        let rejected = MeasurementEvent {
            payload: MeasurementPayload::Co2(Co2Sample { ppm: -1.0 }),
            time_ms: 0,
            seq: u32::MAX,
        };
        match queues.measurements.try_send(rejected) {
            Err(TrySendError::Full(returned)) => {
                assert_eq!(returned.payload.ingest_type(), "co2");

                // Draining one slot frees exactly one.
                block_on(queues.measurements.receive());
                assert!(queues.measurements.try_send(returned).is_ok());
            }
            _ => panic!("a full queue must refuse with Full carrying the value"),
        }
    }

    #[test]
    fn status_and_measurement_queues_are_independent() {
        let queues = Queues::new();
        let mut status = StatusEvent {
            time_ms: 0,
            seq: 0,
            subsystem: heapless::String::new(),
            result: StatusResult::Started,
            message: heapless::String::new(),
        };
        status.subsystem.push_str("lifecycle").unwrap();
        status.message.push_str("boot").unwrap();

        assert!(queues.status.try_send(status.clone()).is_ok());
        // The measurement queue is empty and independent of the status queue.
        assert!(queues.measurements.try_send(decibel_event(0, 88.0)).is_ok());
        assert_eq!(
            block_on(queues.status.receive()).subsystem.as_str(),
            "lifecycle"
        );
    }

    #[test]
    fn producer_and_consumer_coordinate_through_the_channel() {
        let queues = Queues::new();
        let mut seq = Sequencer::new();
        let report = block_on(join(
            async {
                for i in 0..3 {
                    let laeq = 90.0 + i as f32;
                    queues
                        .measurements
                        .send(decibel_event(seq.next_seq(), laeq))
                        .await;
                }
            },
            async {
                let mut seen = 0;
                while seen < 3 {
                    let event = queues.measurements.receive().await;
                    if let MeasurementPayload::Decibel(sample) = event.payload {
                        assert!(sample.laeq >= 90.0);
                        assert_eq!(event.seq, seen, "seq arrives in publish order");
                    } else {
                        panic!("unexpected payload kind");
                    }
                    seen += 1;
                }
                seen
            },
        ));
        assert_eq!(report.1, 3);
    }
}
