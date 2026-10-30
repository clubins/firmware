//! Async backpressure over the real channel types — the integration half of
//! the event-bus contract, above and beyond the unit tests: a producer whose
//! events exceed the queue depth must park in `send().await` (never drop,
//! never overwrite), resume as the consumer drains, and deliver everything
//! exactly once in publish order. Per-type queues must isolate measurement
//! traffic from status traffic, which is what lets one stalled concern slow
//! only its own pipeline.

use clubins_firmware_v3::channels::Queues;
use clubins_firmware_v3::config::{MEASUREMENT_QUEUE_DEPTH, STATUS_QUEUE_DEPTH};
use clubins_firmware_v3::events::{
    DecibelSample, MeasurementEvent, MeasurementPayload, PaxSample, StatusEvent,
    StatusResult,
};
use clubins_firmware_v3::session::Sequencer;
use embassy_futures::block_on;
use embassy_futures::join::{join, join4};

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

fn status_event(seq: u32, subsystem: &str) -> StatusEvent {
    let mut event = StatusEvent {
        time_ms: 0,
        seq,
        subsystem: heapless::String::new(),
        result: StatusResult::Success,
        message: heapless::String::new(),
    };
    event.subsystem.push_str(subsystem).unwrap();
    event
}

/// Three times the queue depth flows through a depth-N channel: the producer
/// must block at capacity, resume on drain, and the consumer must see every
/// event exactly once, in publish order, seq intact.
#[test]
fn backpressure_preserves_every_event_in_publish_order() {
    let queues = Queues::new();
    let total = MEASUREMENT_QUEUE_DEPTH * 3;

    let (_, received) = block_on(join(
        async {
            let mut seq = Sequencer::new();
            for i in 0..total {
                // Level carries the loop index so order is observable end to end.
                queues
                    .measurements
                    .send(decibel_event(seq.next_seq(), 90.0 + i as f32))
                    .await;
            }
        },
        async {
            let mut seen = heapless::Vec::<f32, { MEASUREMENT_QUEUE_DEPTH * 3 }>::new();
            for _ in 0..total {
                let event = queues.measurements.receive().await;
                let MeasurementPayload::Decibel(sample) = event.payload else {
                    panic!("only decibel events are published here");
                };
                seen.push(sample.laeq).unwrap();
            }
            seen
        },
    ));

    for (slot, laeq) in received.iter().enumerate() {
        assert_eq!(
            *laeq,
            90.0 + slot as f32,
            "slot {slot} out of publish order"
        );
    }
}

/// The two channels are independent end to end: a full measurement queue
/// (producer parked) must not stop status traffic, and vice versa. This is
/// the task-isolation guarantee that replaces the superloop's shared stall.
#[test]
fn stalled_measurement_traffic_does_not_stall_status() {
    let queues = Queues::new();

    let (_, measured, _, status_seen) = block_on(join4(
        // Measurement producer: floods past capacity, parking in send().
        async {
            let mut seq = Sequencer::new();
            for i in 0..(MEASUREMENT_QUEUE_DEPTH * 2) {
                queues
                    .measurements
                    .send(decibel_event(seq.next_seq(), 100.0 + i as f32))
                    .await;
            }
        },
        // Measurement consumer: drains everything the producer sent.
        async {
            let mut seen = 0;
            while seen < MEASUREMENT_QUEUE_DEPTH * 2 {
                let event = queues.measurements.receive().await;
                assert_eq!(event.seq, seen as u32, "measurement order");
                seen += 1;
            }
            seen
        },
        // Status producer: interleaved with the measurement flood.
        async {
            let mut seq = Sequencer::new();
            for _ in 0..(STATUS_QUEUE_DEPTH * 2) {
                queues
                    .status
                    .send(status_event(seq.next_seq(), "wifi"))
                    .await;
            }
        },
        // Status consumer.
        async {
            let mut seen = 0;
            while seen < STATUS_QUEUE_DEPTH * 2 {
                let event = queues.status.receive().await;
                assert_eq!(event.seq, seen as u32, "status order");
                assert_eq!(event.subsystem.as_str(), "wifi");
                seen += 1;
            }
            seen
        },
    ));

    assert_eq!(measured, MEASUREMENT_QUEUE_DEPTH * 2);
    assert_eq!(status_seen, STATUS_QUEUE_DEPTH * 2);
}

/// Different sensor tasks publish onto the shared measurement channel —
/// here decibel and pax — each stamping events from its own per-type
/// sequencer contract. The channel stays typed per event kind, and the
/// transport consumer receives both kinds in one FIFO stream.
#[test]
fn mixed_sensor_kinds_share_one_measurement_channel_in_order() {
    let queues = Queues::new();
    let decibel_count = 3u32;
    let pax_count = 4u32;

    let (_, ()) = block_on(join(
        async {
            let mut seq = Sequencer::new();
            for i in 0..decibel_count {
                queues
                    .measurements
                    .send(decibel_event(seq.next_seq(), 95.0 + i as f32))
                    .await;
            }
            for i in 0..pax_count {
                queues
                    .measurements
                    .send(MeasurementEvent {
                        payload: MeasurementPayload::Pax(PaxSample {
                            occupancy_count: 10 + i,
                        }),
                        time_ms: 0,
                        seq: seq.next_seq(),
                    })
                    .await;
            }
        },
        async {
            // First the decibel run in publish order…
            for i in 0..decibel_count {
                let event = queues.measurements.receive().await;
                assert_eq!(event.seq, i);
                let MeasurementPayload::Decibel(sample) = event.payload else {
                    panic!("expected the decibel run first");
                };
                assert_eq!(sample.laeq, 95.0 + i as f32);
            }
            // …then the pax run, still in publish order, seq continuing.
            for i in 0..pax_count {
                let event = queues.measurements.receive().await;
                assert_eq!(event.seq, decibel_count + i, "per-topic seq continues");
                assert_eq!(
                    event.payload,
                    MeasurementPayload::Pax(PaxSample {
                        occupancy_count: 10 + i,
                    })
                );
            }
        },
    ));
}
