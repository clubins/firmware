//! End-to-end PIR debounce fixtures: the event-driven task loop over the
//! scripted GPIO front end.
//!
//! Where the in-module unit tests pin one driver call at a time, these
//! run `pir::run` against `ScriptedEdges` with the device wiring's own
//! configuration constants (the warm-up settle and the debounce window
//! from `config`), drain the real measurement channel, and pin what the
//! ingest topic receives per motion episode: one pax event per rise/fall
//! pair, sequenced from zero, with every observed rising edge counting.
//! The scripted front end only advances on demand, so these model clean
//! hold periods; a front end stuck high publishes nothing (documented in
//! `sensors::pir` — not fixture-testable without a virtual clock).
//!
//! Compiled with the PAX sensor task (`just test-drivers`).

#![cfg(feature = "sensor-pax")]

use embassy_futures::block_on;
use embassy_futures::select::{Either, select};

use clubins_firmware_v3::channels::Queues;
use clubins_firmware_v3::config;
use clubins_firmware_v3::events::{MeasurementEvent, MeasurementPayload, PaxSample};
use clubins_firmware_v3::sensor::fakes::MockClock;
use clubins_firmware_v3::sensors::gpio::{Edge, fakes::ScriptedEdges};
use clubins_firmware_v3::sensors::pir::{Pir, run};

/// The task wired exactly as the device spawn path wires it, over the
/// scripted front end and the mock clock.
fn wired(script: &[Edge]) -> (Pir<ScriptedEdges, MockClock>, Queues) {
    let queues = Queues::new();
    let driver = Pir::new(
        ScriptedEdges::new(script),
        MockClock::new(),
        config::PIR_SETTLE_MS,
        config::PIR_DEBOUNCE_MS,
    );
    (driver, queues)
}

/// One rise/fall pair is one detection published as one pax event, after
/// the warm-up settle and one debounce window — the wiring constants,
/// not the test's, order the waits.
#[test]
fn settles_then_publishes_one_detection_per_episode() {
    block_on(async {
        let (mut driver, queues) = wired(&[Edge::Rising, Edge::Falling]);
        let mut sender = queues.measurements.sender();

        let drain = async { queues.measurements.receive().await };
        // The script exhausts after the first episode and the loop parks
        // on the edge seam; the drain is what completes, dropping the
        // loop mid-wait — which the edge seam survives by construction.
        let Either::Second(event) = select(run(&mut driver, &mut sender), drain).await
        else {
            unreachable!("the drain future completes while the run loop parks")
        };

        assert_eq!(event.seq, 0);
        assert_eq!(event.time_ms, 0); // unsynchronised until RTC sync
        assert_eq!(
            event.payload,
            MeasurementPayload::Pax(PaxSample { occupancy_count: 1 })
        );
    });
}

/// Three episodes are three events, in order, with boot-locally
/// increasing sequence numbers.
#[test]
fn three_episodes_publish_three_sequenced_events() {
    block_on(async {
        let (mut driver, queues) = wired(&[
            Edge::Rising,
            Edge::Falling,
            Edge::Rising,
            Edge::Falling,
            Edge::Rising,
            Edge::Falling,
        ]);
        let mut sender = queues.measurements.sender();

        let drain = async {
            (
                queues.measurements.receive().await,
                queues.measurements.receive().await,
                queues.measurements.receive().await,
            )
        };
        let Either::Second((first, second, third)) =
            select(run(&mut driver, &mut sender), drain).await
        else {
            unreachable!("the drain future completes while the run loop parks")
        };

        let count_of = |event: &MeasurementEvent| match &event.payload {
            MeasurementPayload::Pax(sample) => sample.occupancy_count,
            _ => 0,
        };
        assert_eq!(count_of(&first), 1);
        assert_eq!(count_of(&second), 1);
        assert_eq!(count_of(&third), 1);
        assert_eq!([first.seq, second.seq, third.seq], [0, 1, 2]);
    });
}

/// A falling edge before any detection is an idle glitch: the first real
/// episode still publishes exactly one detection.
#[test]
fn idle_glitch_before_motion_counts_one_detection() {
    block_on(async {
        let (mut driver, queues) = wired(&[Edge::Falling, Edge::Rising, Edge::Falling]);
        let mut sender = queues.measurements.sender();

        let drain = async { queues.measurements.receive().await };
        let Either::Second(event) = select(run(&mut driver, &mut sender), drain).await
        else {
            unreachable!("the drain future completes while the run loop parks")
        };

        assert_eq!(
            event.payload,
            MeasurementPayload::Pax(PaxSample { occupancy_count: 1 })
        );
    });
}

/// Every observed rising edge counts, even several in a row: one event
/// carrying the whole episode's count when the closing fall arrives. On
/// hardware this script shape is what the device adapter's pad resync
/// produces when a transition was missed while the task was publishing.
#[test]
fn every_observed_rise_counts_within_one_episode() {
    block_on(async {
        let (mut driver, queues) =
            wired(&[Edge::Rising, Edge::Rising, Edge::Rising, Edge::Falling]);
        let mut sender = queues.measurements.sender();

        let drain = async { queues.measurements.receive().await };
        let Either::Second(event) = select(run(&mut driver, &mut sender), drain).await
        else {
            unreachable!("the drain future completes while the run loop parks")
        };

        assert_eq!(
            event.payload,
            MeasurementPayload::Pax(PaxSample { occupancy_count: 3 })
        );
    });
}
