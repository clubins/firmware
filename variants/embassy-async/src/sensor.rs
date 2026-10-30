//! The sensor task framework — the pattern every sensor task follows.
//!
//! Three moving parts, all host-testable:
//!
//! 1. [`SensorDriver`] — the async driver contract: bring the hardware up,
//!    tune it for the unit, then produce one measurement per call. Hardware
//!    drivers implement this over I2C/UART/GPIO/I2S; every driver task in
//!    `device/tasks.rs` runs through the same wrapper below.
//! 2. [`SensorTask`] — the task state machine: one [`SensorTask::step`]
//!    brings the driver up (or re-up after a fault), samples once, publishes
//!    one [`MeasurementEvent`](crate::events::MeasurementEvent) to the event
//!    bus, and sleeps the remaining cadence. A failed step backs off
//!    exponentially instead of busy-looping, and the task keeps running —
//!    a faulted sensor is that task's problem, never the executor's.
//! 3. [`run`] — the forever-loop that stitches the two together. This is
//!    what a device task body calls; it is four lines and holds no state,
//!    which is why the tests drive [`SensorTask::step`] directly.
//!
//! Time is injected through the [`Sleep`] trait: on the device the adapter
//! in `device/tasks.rs` sleeps with `embassy-time` (esp-hal timers via
//! esp-rtos); host tests inject [`fakes::MockClock`], which records the
//! requested durations — cadence is verified, never wall-clock-flaky.
//!
//! The publish side is the real measurement channel
//! ([`MeasurementSender`](crate::channels::MeasurementSender)) even in
//! tests, so backpressure semantics are the production ones: a full queue
//! parks the sensor task in `send().await` until the transport drains.
//!
//! Isolation contract: a faulted sensor task must not stall the executor.
//! Drivers honour that by *returning* errors (`Result`) instead of panicking
//! — a panic in any task aborts the whole executor — and the wrapper honours
//! it by treating every error the same way: record, back off, retry. An
//! erroring task spends its life in `sleep().await`, so the executor keeps
//! scheduling every other task. A driver whose future never resolves stalls
//! only its own task; the executor stays cooperative.

use core::fmt;

use crate::channels::MeasurementSender;
use crate::config;
use crate::events::{MeasurementEvent, MeasurementPayload};
use crate::session::Sequencer;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Why a driver call failed — the closed vocabulary the framework acts on.
///
/// The wrapper's policy is deliberately uniform (every fault backs off the
/// same way); the kind exists so logs and status messages can say what
/// failed without string formatting on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorFault {
    /// The bus to the sensor failed: I2C NACK, UART framing or timeout,
    /// a stuck GPIO front-end.
    Bus,
    /// The sensor answered, but the reading is unusable: bad checksum, NaN,
    /// out of the physical range.
    Reading,
    /// Bringing the hardware up failed: init sequence, self-test.
    Hardware,
    /// Per-unit calibration did not complete (e.g. the decibel calibration
    /// offset from `decibel.md` is out of range).
    Calibration,
}

impl fmt::Display for SensorFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Bus => "bus",
            Self::Reading => "reading",
            Self::Hardware => "hardware",
            Self::Calibration => "calibration",
        };
        f.write_str(name)
    }
}

/// A driver failure, as a value. `fault` is the class the framework and
/// status reporting speak; `code` is the driver's own cause number for logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorError {
    pub fault: SensorFault,
    /// Driver-specific cause code; `0` = unspecified.
    pub code: u8,
}

impl SensorError {
    #[must_use]
    pub const fn new(fault: SensorFault, code: u8) -> Self {
        Self { fault, code }
    }
}

impl fmt::Display for SensorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sensor {} fault (code {})", self.fault, self.code)
    }
}

// ---------------------------------------------------------------------------
// Driver contract
// ---------------------------------------------------------------------------

/// The async sensor-driver contract: every sensor driver in this firmware
/// implements exactly these three operations.
///
/// All three are async so slow hardware (I2C transactions, a PDM capture
/// window, a warm-up delay) yields the executor while it waits. Drivers
/// report failure as [`SensorError`] — never by panicking; see the
/// isolation contract in the module docs.
// The futures stay inside one task on the embassy executor — they never
// cross threads, so `Send` bounds are meaningless here. Same choice as
// embedded-hal-async.
#[allow(async_fn_in_trait)]
pub trait SensorDriver {
    /// Bring the hardware up. Called before the first calibration and again
    /// after any bring-up failure (init and calibrate are re-run together:
    /// a sensor that half-initialised must start over).
    async fn init(&mut self) -> Result<(), SensorError>;

    /// Tune the unit: per-unit calibration such as the decibel offset from
    /// `decibel.md`. Runs after every successful [`Self::init`].
    async fn calibrate(&mut self) -> Result<(), SensorError>;

    /// Produce one measurement. One call = one event on the bus.
    async fn sample(&mut self) -> Result<MeasurementPayload, SensorError>;
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// The task's time source: sleeps between steps. Dependency inversion is the
/// whole point — `embassy-time` stays a device-only dependency, and the host
/// tests get a deterministic clock ([`fakes::MockClock`]).
// See the note on `SensorDriver`: task-local futures, never `Send`.
#[allow(async_fn_in_trait)]
pub trait Sleep {
    /// Sleep for `ms` milliseconds. Implementations may return a future
    /// that resolves immediately (the mock does); the wrapper only orders
    /// time, it never reads it.
    async fn sleep(&mut self, ms: u64);
}

/// A borrowed sleeper delegates: a driver holding `&mut S` sleeps through
/// the same clock its owner still observes — the host tests hand a driver
/// a borrowed [`fakes::MockClock`] and read its recorded sleeps afterwards.
impl<S: Sleep> Sleep for &mut S {
    async fn sleep(&mut self, ms: u64) {
        <S as Sleep>::sleep(&mut **self, ms).await;
    }
}

// ---------------------------------------------------------------------------
// Task parameters
// ---------------------------------------------------------------------------

/// Cadence and fault policy for one sensor task. Architecture constants
/// come from [`crate::config`] via [`SensorTaskParams::built_in`]; tests
/// pass explicit values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorTaskParams {
    /// Sleep between completed steps — the sampling cadence.
    pub period_ms: u64,
    /// Upper bound on the error backoff, so a permanently faulted sensor
    /// retries a few times a minute instead of spinning.
    pub max_backoff_ms: u64,
}

impl SensorTaskParams {
    #[must_use]
    pub const fn new(period_ms: u64, max_backoff_ms: u64) -> Self {
        Self {
            period_ms,
            max_backoff_ms,
        }
    }

    /// The built-in cadence: the poll interval the skeletons used and the
    /// capped backoff. Real drivers tighten their cadence in their own
    /// tasks; until then every sensor task runs this.
    #[must_use]
    pub const fn built_in() -> Self {
        Self::new(
            config::SENSOR_POLL_INTERVAL_MS,
            config::MAX_SENSOR_BACKOFF_MS,
        )
    }
}

// ---------------------------------------------------------------------------
// Task state machine
// ---------------------------------------------------------------------------

/// One sensor task's mutable state: bring-up status, the fault counter
/// driving the backoff, the last error for supervision, and the sequencer
/// this task owns.
///
/// Sequencer ownership follows `session::Sequencer`: one sensor task per
/// measurement type owns that type's topic counter, so per-topic seq never
/// races and never gaps by construction.
#[derive(Debug)]
pub struct SensorTask {
    /// `true` once init + calibrate have both succeeded.
    ready: bool,
    consecutive_failures: u32,
    last_error: Option<SensorError>,
    seq: Sequencer,
}

impl Default for SensorTask {
    fn default() -> Self {
        Self::new()
    }
}

impl SensorTask {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            ready: false,
            consecutive_failures: 0,
            last_error: None,
            seq: Sequencer::new(),
        }
    }

    /// Fault state for supervision: is the task's driver up, how many
    /// samples failed in a row, and what was the last error. The ops
    /// tasks' supervision reads this; it is never consulted by `step`.
    #[must_use]
    pub const fn health(&self) -> SensorHealth {
        SensorHealth {
            ready: self.ready,
            consecutive_failures: self.consecutive_failures,
            last_error: self.last_error,
        }
    }

    /// One step of the sensor task: bring the driver up if needed, sample,
    /// publish, then sleep — the cadence sleep after a good sample, the
    /// backoff sleep after a faulted one. Never returns an error to the
    /// caller: there is no caller to tell. A faulted task sleeps and tries
    /// again, which is the isolation property.
    pub async fn step<D: SensorDriver, C: Sleep>(
        &mut self,
        driver: &mut D,
        clock: &mut C,
        publisher: &mut MeasurementSender<'_>,
        params: &SensorTaskParams,
    ) {
        if !self.ready {
            match driver.init().await {
                Err(error) => return self.fault(clock, params, error).await,
                Ok(()) => match driver.calibrate().await {
                    Err(error) => return self.fault(clock, params, error).await,
                    Ok(()) => self.ready = true,
                },
            }
        }

        match driver.sample().await {
            Ok(payload) => {
                self.consecutive_failures = 0;
                self.last_error = None;
                // time_ms stays 0 until a time-sync task lands (NTP
                // discipline): "clock not synchronised" is the honest value,
                // and the server must never mistake monotonic ms since boot
                // for a Unix epoch.
                let event = MeasurementEvent {
                    payload,
                    time_ms: 0,
                    seq: self.seq.next_seq(),
                };
                publisher.send(event).await;
                clock.sleep(params.period_ms).await;
            }
            Err(error) => self.fault(clock, params, error).await,
        }
    }

    /// Record a fault and sleep the backoff: exponential in the consecutive
    /// failure count, capped at `max_backoff_ms`. The sleep is what keeps a
    /// dead sensor from becoming a busy loop that starves the executor.
    async fn fault<C: Sleep>(
        &mut self,
        clock: &mut C,
        params: &SensorTaskParams,
        error: SensorError,
    ) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.last_error = Some(error);
        clock.sleep(self.backoff_ms(params)).await;
    }

    /// Backoff after the Nth consecutive failure (N >= 1):
    /// `period · 2^(N-1)`, capped. Pure and unit-tested below.
    fn backoff_ms(&self, params: &SensorTaskParams) -> u64 {
        let doubling = (self.consecutive_failures - 1).min(62) as u64;
        params
            .period_ms
            .saturating_mul(1u64 << doubling)
            .min(params.max_backoff_ms)
    }
}

/// Snapshot of one sensor task's fault state, for ops supervision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorHealth {
    /// `true` once init + calibrate have succeeded (sampling may still
    /// fault; see `consecutive_failures`).
    pub ready: bool,
    pub consecutive_failures: u32,
    pub last_error: Option<SensorError>,
}

/// The sensor task body: [`SensorTask::step`] forever. This is what the
/// device's `#[embassy_executor::task]` sensor functions call, one driver
/// each; the executor gets a task that only ever runs hardware, a channel
/// send, or a sleep.
pub async fn run<D: SensorDriver, C: Sleep>(
    driver: &mut D,
    clock: &mut C,
    publisher: &mut MeasurementSender<'_>,
    params: &SensorTaskParams,
) {
    let mut task = SensorTask::new();
    loop {
        task.step(driver, clock, publisher, params).await;
    }
}

// ---------------------------------------------------------------------------
// Host-testable fakes
// ---------------------------------------------------------------------------

/// Fakes for testing against the sensor task pattern without hardware —
/// usable from this crate's tests and from any future integration test of
/// tasks downstream of the sensor tasks (transports, ops supervision).
pub mod fakes {
    use core::fmt;

    use heapless::Vec as HeapVec;

    use crate::events::MeasurementPayload;

    use super::{SensorDriver, SensorError, Sleep};

    /// Maximum scripted outcomes per driver operation.
    const SCRIPT_DEPTH: usize = 64;

    /// A script of outcomes: walks front-to-back while entries remain, then
    /// either falls back to a caller-provided default (init/calibrate: a
    /// healthy sensor) or repeats its final entry (sample: an unbounded task
    /// loop fed a finite script settles instead of panicking).
    #[derive(Debug)]
    struct Script<T> {
        steps: HeapVec<T, SCRIPT_DEPTH>,
        cursor: usize,
    }

    // Manual impl: the derive would wrongly require T: Default.
    impl<T> Default for Script<T> {
        fn default() -> Self {
            Self {
                steps: HeapVec::new(),
                cursor: 0,
            }
        }
    }

    impl<T: Copy + fmt::Debug> Script<T> {
        fn new() -> Self {
            Self {
                steps: HeapVec::new(),
                cursor: 0,
            }
        }

        fn push(&mut self, outcome: T) {
            self.steps
                .push(outcome)
                .expect("script depth is a test size");
        }

        /// The scripted entry for this call, or `default` once the script
        /// has run out (and when it was never written).
        fn or_default(&self, default: T) -> T {
            match self.steps.get(self.cursor) {
                Some(outcome) => *outcome,
                None => default,
            }
        }

        /// The scripted entry for this call; an exhausted script repeats
        /// its final entry. An empty script is a misconfiguration.
        fn repeat_last(&self) -> T {
            assert!(
                !self.steps.is_empty(),
                "scripted sample called with an empty script — test misconfiguration"
            );
            let last = self.steps.len() - 1;
            self.steps[self.cursor.min(last)]
        }

        fn advance(&mut self) {
            self.cursor = self.cursor.saturating_add(1);
        }
    }

    /// A driver whose `init`/`calibrate`/`sample` outcomes are scripted per
    /// call, with per-operation call counters for assertions. Init and
    /// calibrate default to `Ok` — script only the deviations; every sample
    /// must be scripted.
    #[derive(Debug, Default)]
    pub struct ScriptedDriver {
        init: Script<Result<(), SensorError>>,
        calibrate: Script<Result<(), SensorError>>,
        sample: Script<Result<MeasurementPayload, SensorError>>,
    }

    impl ScriptedDriver {
        #[must_use]
        pub fn new() -> Self {
            Self {
                init: Script::new(),
                calibrate: Script::new(),
                sample: Script::new(),
            }
        }

        pub fn then_init(mut self, outcome: Result<(), SensorError>) -> Self {
            self.init.push(outcome);
            self
        }

        pub fn then_calibrate(mut self, outcome: Result<(), SensorError>) -> Self {
            self.calibrate.push(outcome);
            self
        }

        pub fn then_sample(
            mut self,
            outcome: Result<MeasurementPayload, SensorError>,
        ) -> Self {
            self.sample.push(outcome);
            self
        }

        #[must_use]
        pub const fn init_calls(&self) -> usize {
            self.init.cursor
        }

        #[must_use]
        pub const fn calibrate_calls(&self) -> usize {
            self.calibrate.cursor
        }

        #[must_use]
        pub const fn sample_calls(&self) -> usize {
            self.sample.cursor
        }
    }

    impl SensorDriver for ScriptedDriver {
        async fn init(&mut self) -> Result<(), SensorError> {
            let outcome = self.init.or_default(Ok(()));
            self.init.advance();
            outcome
        }

        async fn calibrate(&mut self) -> Result<(), SensorError> {
            let outcome = self.calibrate.or_default(Ok(()));
            self.calibrate.advance();
            outcome
        }

        async fn sample(&mut self) -> Result<MeasurementPayload, SensorError> {
            let outcome = self.sample.repeat_last();
            self.sample.advance();
            outcome
        }
    }

    /// A clock that records every requested sleep and resolves immediately.
    /// Cadence assertions read `sleeps`: the durations the task asked for,
    /// in order.
    #[derive(Debug, Default)]
    pub struct MockClock {
        pub sleeps: HeapVec<u64, 128>,
    }

    impl MockClock {
        #[must_use]
        pub const fn new() -> Self {
            Self {
                sleeps: HeapVec::new(),
            }
        }
    }

    impl Sleep for MockClock {
        async fn sleep(&mut self, ms: u64) {
            let _ = self.sleeps.push(ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use embassy_futures::block_on;
    use embassy_futures::join::join;

    use super::fakes::{MockClock, ScriptedDriver};
    use super::*;
    use crate::channels::Queues;
    use crate::config::MEASUREMENT_QUEUE_DEPTH;
    use crate::events::{DecibelSample, MeasurementPayload};

    /// One scripted sample with an observable level: the LAeq carries the
    /// sample index, so publish order is verifiable end to end.
    fn laeq_payload(laeq: f32) -> MeasurementPayload {
        MeasurementPayload::Decibel(DecibelSample {
            window_ms: 1000,
            overload: false,
            laeq,
            la_fmin: laeq - 10.0,
            la_fmax: laeq + 10.0,
            lceq: laeq + 5.0,
            lc_fmax: laeq + 12.0,
        })
    }

    fn params() -> SensorTaskParams {
        SensorTaskParams::new(1_000, 30_000)
    }

    #[test]
    fn a_healthy_driver_samples_on_cadence_and_publishes_in_order() {
        let queues = Queues::new();
        let mut driver = ScriptedDriver::new();
        for i in 0..3 {
            driver = driver.then_sample(Ok(laeq_payload(90.0 + i as f32)));
        }
        let mut clock = MockClock::new();
        let mut publisher = queues.measurements.sender();
        let mut task = SensorTask::new();

        for _ in 0..3 {
            block_on(task.step(&mut driver, &mut clock, &mut publisher, &params()));
        }

        for i in 0..3 {
            let event = queues.measurements.try_receive().expect("queued event");
            assert_eq!(event.seq, i as u32, "seq counts published events from 0");
            assert_eq!(event.time_ms, 0, "no clock sync yet: time_ms is 0");
            assert_eq!(event.payload, laeq_payload(90.0 + i as f32));
        }

        // Cadence: each completed step slept exactly one period, in order.
        assert_eq!(clock.sleeps.as_slice(), [1_000, 1_000, 1_000]);
        assert_eq!(driver.sample_calls(), 3);
    }

    #[test]
    fn a_failing_sample_backs_off_then_recovers() {
        let queues = Queues::new();
        let fault = SensorError::new(SensorFault::Reading, 7);
        let mut driver = ScriptedDriver::new()
            .then_sample(Err(fault))
            .then_sample(Err(fault))
            .then_sample(Ok(laeq_payload(92.0)));
        let mut clock = MockClock::new();
        let mut publisher = queues.measurements.sender();
        let mut task = SensorTask::new();

        block_on(task.step(&mut driver, &mut clock, &mut publisher, &params()));
        block_on(task.step(&mut driver, &mut clock, &mut publisher, &params()));
        assert!(
            queues.measurements.try_receive().is_err(),
            "faults publish nothing"
        );
        assert_eq!(clock.sleeps.as_slice(), [1_000, 2_000], "backoff doubles");
        let health = task.health();
        assert_eq!(health.consecutive_failures, 2);
        assert_eq!(health.last_error, Some(fault));

        // Recovery: the next success publishes — with the seq the failed
        // samples never consumed, because seq counts messages, not attempts
        // — resets the fault state, and returns to the plain cadence.
        block_on(task.step(&mut driver, &mut clock, &mut publisher, &params()));
        let event = queues
            .measurements
            .try_receive()
            .expect("recovery published");
        assert_eq!(event.seq, 0);
        let health = task.health();
        assert!(health.ready);
        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.last_error, None);
        assert_eq!(clock.sleeps.as_slice(), [1_000, 2_000, 1_000]);
    }

    #[test]
    fn backoff_doubles_and_caps_at_the_maximum() {
        let queues = Queues::new();
        let mut driver = ScriptedDriver::new();
        // Seven faults: the doubling runs into the cap and stays there.
        for _ in 0..7 {
            driver = driver.then_sample(Err(SensorError::new(SensorFault::Bus, 1)));
        }
        let mut clock = MockClock::new();
        let mut publisher = queues.measurements.sender();
        let mut task = SensorTask::new();

        for _ in 0..7 {
            block_on(task.step(&mut driver, &mut clock, &mut publisher, &params()));
        }
        assert_eq!(
            clock.sleeps.as_slice(),
            [1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000],
            "backoff doubles to the cap, never past it"
        );
        assert!(queues.measurements.try_receive().is_err());
    }

    #[test]
    fn a_failed_init_retries_the_whole_bring_up() {
        let queues = Queues::new();
        let mut driver = ScriptedDriver::new()
            .then_init(Err(SensorError::new(SensorFault::Hardware, 3)))
            .then_sample(Ok(laeq_payload(90.0)));
        let mut clock = MockClock::new();
        let mut publisher = queues.measurements.sender();
        let mut task = SensorTask::new();

        block_on(task.step(&mut driver, &mut clock, &mut publisher, &params()));
        assert!(queues.measurements.try_receive().is_err());

        block_on(task.step(&mut driver, &mut clock, &mut publisher, &params()));
        let event = queues
            .measurements
            .try_receive()
            .expect("published after retry");
        assert_eq!(event.seq, 0);
        // The failed bring-up re-runs init; calibration follows a
        // successful init, so it has run exactly once.
        assert_eq!(driver.init_calls(), 2);
        assert_eq!(driver.calibrate_calls(), 1);
        assert_eq!(driver.sample_calls(), 1);
    }

    #[test]
    fn calibrate_failure_restarts_from_init() {
        let queues = Queues::new();
        let mut driver = ScriptedDriver::new()
            .then_calibrate(Err(SensorError::new(SensorFault::Calibration, 1)))
            .then_sample(Ok(laeq_payload(90.0)));
        let mut clock = MockClock::new();
        let mut publisher = queues.measurements.sender();
        let mut task = SensorTask::new();

        block_on(task.step(&mut driver, &mut clock, &mut publisher, &params()));
        assert!(
            queues.measurements.try_receive().is_err(),
            "no sample after failed calibration"
        );

        block_on(task.step(&mut driver, &mut clock, &mut publisher, &params()));
        assert!(queues.measurements.try_receive().is_ok());
        // A sensor that half-initialised starts over: init runs again.
        assert_eq!(driver.init_calls(), 2);
        assert_eq!(driver.calibrate_calls(), 2);
    }

    /// The publish-ordering test under real backpressure: more events than
    /// the queue is deep, producer and consumer joined — every event
    /// arrives exactly once, in sample order, seq intact.
    #[test]
    fn publishes_keep_flowing_past_queue_depth() {
        let queues = Queues::new();
        let total = MEASUREMENT_QUEUE_DEPTH + 4;
        let mut driver = ScriptedDriver::new();
        for i in 0..total {
            driver = driver.then_sample(Ok(laeq_payload(90.0 + i as f32)));
        }

        let (clock, laeqs) = block_on(join(
            async {
                let mut clock = MockClock::new();
                let mut publisher = queues.measurements.sender();
                let mut task = SensorTask::new();
                for _ in 0..total {
                    task.step(&mut driver, &mut clock, &mut publisher, &params())
                        .await;
                }
                clock
            },
            async {
                let mut laeqs = heapless::Vec::<f32, 64>::new();
                while laeqs.len() < total {
                    let event = queues.measurements.receive().await;
                    let MeasurementPayload::Decibel(sample) = event.payload else {
                        panic!("unexpected payload kind");
                    };
                    let _ = laeqs.push(sample.laeq);
                }
                laeqs
            },
        ));

        // The producer's own cadence held: one period sleep per completed
        // step, including the steps that parked in send().await.
        assert_eq!(clock.sleeps.len(), total);
        assert!(clock.sleeps.iter().all(|ms| *ms == 1_000));

        for (i, laeq) in laeqs.iter().enumerate() {
            assert_eq!(*laeq, 90.0 + i as f32, "event {i} arrived in sample order");
        }
    }

    #[test]
    fn scripted_driver_repeats_its_final_outcome() {
        let mut driver = ScriptedDriver::new()
            .then_sample(Ok(laeq_payload(1.0)))
            .then_sample(Ok(laeq_payload(2.0)));
        // A finite script settling on its last entry is what lets an
        // unbounded task loop run forever without panicking.
        assert_eq!(block_on(driver.sample()), Ok(laeq_payload(1.0)));
        assert_eq!(block_on(driver.sample()), Ok(laeq_payload(2.0)));
        assert_eq!(block_on(driver.sample()), Ok(laeq_payload(2.0)));
        assert_eq!(driver.sample_calls(), 3);
    }
}
