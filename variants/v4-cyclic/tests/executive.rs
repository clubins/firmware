//! Host tests for the V4 scheduler primitive.
//!
//! These are the executive's first-class guarantees, tested on the host: phase
//! ordering within a tick, bounded catch-up with honest lateness, ISR→phase
//! queue handoff, timer-wheel cadence math, and jitter accounting. The same
//! code runs on the target — only the tick source differs.

use core::num::NonZeroU32;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use v4_cyclic::exec::{
    ExecError, Executive, Full, MAX_CATCHUP, PhaseCtx, PhaseDef, PhaseId, PhaseTable, SpscRing,
    TableError, TimerWheel,
};

// --- handler-visible statics (reset per test) ------------------------------

static SENSOR_RUNS: AtomicU32 = AtomicU32::new(0);
static COMMS_RUNS: AtomicU32 = AtomicU32::new(0);
static OPS_RUNS: AtomicU32 = AtomicU32::new(0);
static LAST_TICK_SEEN: AtomicU64 = AtomicU64::new(u64::MAX);
/// Set if a comms run observes zero sensor runs — the ordering guarantee broken.
static ORDERING_VIOLATED: AtomicBool = AtomicBool::new(false);

fn reset_handler_state() {
    SENSOR_RUNS.store(0, Ordering::SeqCst);
    COMMS_RUNS.store(0, Ordering::SeqCst);
    OPS_RUNS.store(0, Ordering::SeqCst);
    LAST_TICK_SEEN.store(u64::MAX, Ordering::SeqCst);
    ORDERING_VIOLATED.store(false, Ordering::SeqCst);
}

/// The handler statics are process-global and the test harness runs tests in
/// parallel threads — state-sensitive tests serialize on this lock.
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn lock_shared_state() -> MutexGuard<'static, ()> {
    // A panicking test poisons the lock; the next test takes over anyway —
    // its reset_handler_state() hands it a clean slate.
    TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn sensor_handler(_ctx: &mut PhaseCtx) {
    SENSOR_RUNS.fetch_add(1, Ordering::SeqCst);
}

fn comms_handler(_ctx: &mut PhaseCtx) {
    // Ordering guarantee: sensor precedes comms in the same tick.
    if SENSOR_RUNS.load(Ordering::SeqCst) == 0 {
        ORDERING_VIOLATED.store(true, Ordering::SeqCst);
    }
    COMMS_RUNS.fetch_add(1, Ordering::SeqCst);
}

fn ops_handler(ctx: &mut PhaseCtx) {
    OPS_RUNS.fetch_add(1, Ordering::SeqCst);
    LAST_TICK_SEEN.store(ctx.now, Ordering::SeqCst);
}

/// The standard three-phase schedule: sensor every tick, comms every 3rd,
/// ops every 6th offset by 2. Table order = execution order.
fn three_phase_table() -> Result<PhaseTable, TableError> {
    let mut table = PhaseTable::new();
    table.push(PhaseDef::every_tick(PhaseId::SENSOR))?;
    table.push(PhaseDef::every(
        PhaseId::COMMS,
        NonZeroU32::new(3).unwrap(),
        0,
    ))?;
    table.push(PhaseDef::every(
        PhaseId::OPS,
        NonZeroU32::new(6).unwrap(),
        2,
    ))?;
    Ok(table)
}

// --- phase ordering & due math ---------------------------------------------

#[test]
fn phases_run_in_table_order_within_a_tick() {
    let _guard = lock_shared_state();
    reset_handler_state();
    let mut exec = Executive::new(three_phase_table().unwrap());
    exec.register(PhaseId::SENSOR, sensor_handler).unwrap();
    exec.register(PhaseId::COMMS, comms_handler).unwrap();
    exec.register(PhaseId::OPS, ops_handler).unwrap();

    exec.tick();
    assert!(
        !ORDERING_VIOLATED.load(Ordering::SeqCst),
        "comms ran before sensor"
    );
    assert_eq!(SENSOR_RUNS.load(Ordering::SeqCst), 1);
    assert_eq!(COMMS_RUNS.load(Ordering::SeqCst), 1); // tick 0: 0 % 3 == 0
    assert_eq!(OPS_RUNS.load(Ordering::SeqCst), 0); // tick 0: 0 % 6 != 2
}

#[test]
fn cadences_fire_on_their_scheduled_ticks_only() {
    let _guard = lock_shared_state();
    reset_handler_state();
    let mut exec = Executive::new(three_phase_table().unwrap());
    exec.register(PhaseId::SENSOR, sensor_handler).unwrap();
    exec.register(PhaseId::COMMS, comms_handler).unwrap();
    exec.register(PhaseId::OPS, ops_handler).unwrap();

    // Six on-time ticks.
    for _ in 0..6 {
        exec.tick();
    }
    assert_eq!(SENSOR_RUNS.load(Ordering::SeqCst), 6);
    assert_eq!(COMMS_RUNS.load(Ordering::SeqCst), 2); // ticks 0 and 3
    assert_eq!(OPS_RUNS.load(Ordering::SeqCst), 1); // tick 2 (2 % 6 == 2)
    assert_eq!(LAST_TICK_SEEN.load(Ordering::SeqCst), 2);
    assert!(!ORDERING_VIOLATED.load(Ordering::SeqCst));
}

#[test]
fn a_phase_never_runs_twice_in_one_batch() {
    let _guard = lock_shared_state();
    reset_handler_state();
    let mut exec = Executive::new(three_phase_table().unwrap());
    exec.register(PhaseId::SENSOR, sensor_handler).unwrap();

    // Catch-up batch of 3 ticks: sensor runs exactly once per serviced tick.
    let report = exec.advance(3);
    assert_eq!(report.ticks_ran, 3);
    assert_eq!(SENSOR_RUNS.load(Ordering::SeqCst), 3);
}

// --- bounded catch-up and honest lateness -----------------------------------

#[test]
fn catchup_batches_record_lateness_per_slot() {
    let _guard = lock_shared_state();
    reset_handler_state();
    let mut exec = Executive::new(three_phase_table().unwrap());
    exec.register(PhaseId::SENSOR, sensor_handler).unwrap();

    // Foreground stalled for 3 ticks: slot 0 runs 2 late, slot 1 one late,
    // slot 2 on time. The ledger must show the worst, not hide it.
    let report = exec.advance(3);
    assert_eq!(SENSOR_RUNS.load(Ordering::SeqCst), 3);
    assert_eq!(report.per_phase[0].max_lateness, 2);
    assert_eq!(exec.jitter().max(0), 2);
    assert_eq!(exec.jitter().last(0), 0, "the final slot ran on time");
    assert_eq!(exec.jitter().samples(0), 3);
    // Mean = (2 + 1 + 0) / 3, truncated.
    assert_eq!(exec.jitter().mean(0), 1);
}

#[test]
fn on_time_execution_keeps_jitter_at_zero() {
    let _guard = lock_shared_state();
    reset_handler_state();
    let mut exec = Executive::new(three_phase_table().unwrap());
    exec.register(PhaseId::SENSOR, sensor_handler).unwrap();

    for _ in 0..10 {
        exec.tick();
    }
    assert_eq!(exec.jitter().max_overall(), 0);
    assert!(
        exec.jitter().within_bound(0),
        "on-time ticks must satisfy a zero bound"
    );
}

#[test]
fn foreground_stalls_past_catchup_drop_ticks_visibly() {
    let _guard = lock_shared_state();
    reset_handler_state();
    let mut exec = Executive::new(three_phase_table().unwrap());
    exec.register(PhaseId::SENSOR, sensor_handler).unwrap();

    let report = exec.advance(MAX_CATCHUP + 5);
    assert_eq!(report.ticks_ran, MAX_CATCHUP);
    assert_eq!(
        report.dropped_ticks, 5,
        "unserviceable slots are reported, never swallowed"
    );
    assert_eq!(SENSOR_RUNS.load(Ordering::SeqCst), MAX_CATCHUP);
}

// --- handler registration is total ------------------------------------------

#[test]
fn registration_rejects_unknown_and_duplicate_handlers() {
    let _guard = lock_shared_state();
    let mut exec = Executive::new(three_phase_table().unwrap());
    exec.register(PhaseId::SENSOR, sensor_handler).unwrap();

    assert_eq!(
        exec.register(PhaseId::SENSOR, sensor_handler),
        Err(ExecError::DuplicateHandler)
    );
    assert_eq!(
        exec.register(PhaseId(9), sensor_handler),
        Err(ExecError::PhaseNotInTable)
    );
    // Out-of-range ids are rejected even if not in the table.
    assert_eq!(
        exec.register(PhaseId(u8::MAX), sensor_handler),
        Err(ExecError::PhaseIdOutOfRange)
    );
}

#[test]
fn phases_without_handlers_still_advance_the_schedule() {
    let _guard = lock_shared_state();
    reset_handler_state();
    let mut exec = Executive::new(three_phase_table().unwrap());
    exec.register(PhaseId::SENSOR, sensor_handler).unwrap();
    // COMMS and OPS deliberately unregistered — the tick still completes.

    let report = exec.tick();
    assert_eq!(report.ticks_ran, 1);
    assert_eq!(report.per_phase[0].runs, 1);
}

// --- ISR → phase queue handoff (SPSC ring) ----------------------------------

#[test]
fn ring_preserves_fifo_order_across_wraps() {
    let ring: SpscRing<u32, 4> = SpscRing::new();
    // Push and pop interleaved well past capacity: 3 full wraps.
    let mut next_expected = 0u32;
    for i in 0..12u32 {
        ring.push(i)
            .expect("interleaved pushes never fill a drained ring");
        if let Some(v) = ring.pop() {
            assert_eq!(v, next_expected, "FIFO order across wraparound");
            next_expected += 1;
        }
    }
    assert_eq!(ring.overflow_count(), 0);
}

#[test]
fn full_ring_returns_the_value_and_counts_overflow() {
    let ring: SpscRing<u32, 4> = SpscRing::new();
    for i in 0..4 {
        ring.push(i).expect("fills to capacity");
    }
    assert_eq!(ring.len(), 4);

    let Err(Full(rejected)) = ring.push(99) else {
        panic!("push to a full ring must fail");
    };
    assert_eq!(rejected, 99, "the value comes back to the producer");
    assert_eq!(ring.overflow_count(), 1);
    assert_eq!(ring.len(), 4, "the ring is unchanged by a rejected push");

    // Freeing one slot makes room exactly once.
    assert_eq!(ring.pop(), Some(0));
    ring.push(99).expect("slot freed by the pop");
    assert_eq!(ring.len(), 4);
}

#[test]
fn ring_drains_in_push_order_after_batch_fill() {
    let ring: SpscRing<u64, 8> = SpscRing::new();
    for i in 0..8u64 {
        ring.push(i * 1_000).expect("capacity fill");
    }
    for i in 0..8u64 {
        assert_eq!(ring.pop(), Some(i * 1_000));
    }
    assert!(ring.is_empty());
    assert_eq!(ring.pop(), None);
}

#[test]
fn ring_len_tracks_producer_minus_consumer() {
    let ring: SpscRing<u8, 4> = SpscRing::new();
    assert!(ring.is_empty());
    ring.push(1).unwrap();
    ring.push(2).unwrap();
    assert_eq!(ring.len(), 2);
    ring.pop();
    assert_eq!(ring.len(), 1);
}

// --- timer wheel cadence math -----------------------------------------------

fn collect_fired(wheel: &mut TimerWheel<64, 8>, ticks: u64) -> Vec<u16> {
    let mut fired = Vec::new();
    for _ in 0..ticks {
        let report = wheel.advance();
        for id in report.ids[..report.len].iter().flatten() {
            fired.push(id.0);
        }
    }
    fired
}

#[test]
fn periodic_timer_fires_every_period_ticks() {
    let mut wheel: TimerWheel<64, 8> = TimerWheel::new();
    let id = wheel.schedule(5, NonZeroU32::new(5)).unwrap();

    let fired = collect_fired(&mut wheel, 15);
    assert_eq!(
        fired,
        vec![id.0; 3],
        "fires at ticks 5, 10, 15 — and only there"
    );
}

#[test]
fn periodic_timer_rearms_across_slot_wraparound() {
    // Period longer than the wheel: the re-armed entry must survive sitting
    // in its slot while the wheel laps it — the classic timer-wheel bug.
    let mut wheel: TimerWheel<64, 8> = TimerWheel::new();
    let id = wheel.schedule(70, NonZeroU32::new(70)).unwrap();

    let fired = collect_fired(&mut wheel, 140);
    assert_eq!(
        fired,
        vec![id.0; 2],
        "fires at ticks 70 and 140, across the wrap"
    );
    assert!(
        !collect_fired(&mut wheel, 63).contains(&id.0),
        "silent in between"
    );
}

#[test]
fn one_shot_timer_fires_once_and_stays_dead() {
    let mut wheel: TimerWheel<64, 8> = TimerWheel::new();
    let id = wheel.schedule(3, None).unwrap();

    assert!(collect_fired(&mut wheel, 100).contains(&id.0));
    assert_eq!(
        collect_fired(&mut wheel, 100).len(),
        0,
        "one-shot never refires"
    );
}

#[test]
fn cancelled_timers_never_fire() {
    let mut wheel: TimerWheel<64, 8> = TimerWheel::new();
    let id = wheel.schedule(4, None).unwrap();
    wheel.cancel(id).expect("cancel a pending timer");
    assert_eq!(
        wheel.cancel(id),
        Err(v4_cyclic::exec::WheelError::NoSuchTimer)
    );
    assert!(collect_fired(&mut wheel, 10).is_empty());
}

#[test]
fn wheel_ticks_advance_absolute_grid_time() {
    let mut wheel: TimerWheel<64, 8> = TimerWheel::new();
    assert_eq!(wheel.now(), 0);
    collect_fired(&mut wheel, 7);
    assert_eq!(wheel.now(), 7);
}

#[test]
fn slow_periodic_cadence_matches_hand_computed_schedule() {
    // A 30 s heartbeat on a 1 ms grid, started 2 s in — the classic ops
    // cadence. On a shrunk wheel the math is identical: fire, re-arm, fire.
    let mut wheel: TimerWheel<64, 8> = TimerWheel::new();
    let id = wheel.schedule(2, NonZeroU32::new(3)).unwrap();
    // The fire lands when the wheel reaches the expiry: ticks 2, 5, 8 fire;
    // 1, 3, 4, 6, 7 do not.
    assert_eq!(collect_fired(&mut wheel, 2), vec![id.0]); // ticks 1, 2
    assert_eq!(collect_fired(&mut wheel, 2).len(), 0); // ticks 3, 4
    assert_eq!(collect_fired(&mut wheel, 1), vec![id.0]); // tick 5
    assert_eq!(collect_fired(&mut wheel, 2).len(), 0); // ticks 6, 7
    assert_eq!(collect_fired(&mut wheel, 1), vec![id.0]); // tick 8
}

// --- jitter ledger directly ---------------------------------------------------

#[test]
fn jitter_ledger_tracks_last_max_mean_and_bound() {
    let mut tracker = v4_cyclic::exec::JitterTracker::default();
    tracker.record(0, 3);
    tracker.record(0, 1);
    tracker.record(0, 7);
    assert_eq!(tracker.last(0), 7);
    assert_eq!(tracker.max(0), 7);
    assert_eq!(tracker.mean(0), 3); // (3+1+7)/3
    assert_eq!(tracker.samples(0), 3);
    assert!(!tracker.within_bound(5));
    assert!(tracker.within_bound(7));
    assert_eq!(tracker.max_overall(), 7);
}

// --- scheduling from a phase (the wheel/phase integration) --------------------

static SCHEDULED_ID: AtomicU16 = AtomicU16::new(u16::MAX);

fn scheduling_ops_handler(ctx: &mut PhaseCtx) {
    OPS_RUNS.fetch_add(1, Ordering::SeqCst);
    if OPS_RUNS.load(Ordering::SeqCst) == 1 {
        // First ops run schedules a one-shot 10 ticks out.
        if let Ok(id) = ctx.wheel.schedule(10, None) {
            SCHEDULED_ID.store(id.0, Ordering::SeqCst);
        }
    }
}

#[test]
fn phases_can_schedule_slow_cadences_on_the_wheel() {
    let _guard = lock_shared_state();
    reset_handler_state();
    SCHEDULED_ID.store(u16::MAX, Ordering::SeqCst);

    // Ops every 6th tick: first run at tick 2, schedules one-shot for tick 12.
    let mut exec = Executive::new(three_phase_table().unwrap());
    exec.register(PhaseId::OPS, scheduling_ops_handler).unwrap();

    let report = exec.advance(13);
    let scheduled = SCHEDULED_ID.load(Ordering::SeqCst);
    assert_ne!(scheduled, u16::MAX, "the handler scheduled a timer");
    assert_eq!(report.wheel.len, 1, "exactly one timer fired in the batch");
    assert_eq!(
        report.wheel.ids[0],
        Some(v4_cyclic::exec::TimerId(scheduled))
    );
    assert!(!report.wheel.truncated);
    assert!(!report.wheel.rearm_failed);
}

// --- table construction rules -------------------------------------------------

#[test]
fn table_rejects_duplicate_ids_and_impossible_offsets() {
    let mut table = PhaseTable::new();
    table.push(PhaseDef::every_tick(PhaseId::SENSOR)).unwrap();
    assert_eq!(
        table.push(PhaseDef::every_tick(PhaseId::SENSOR)),
        Err(TableError::DuplicateId)
    );
    let period = NonZeroU32::new(3).unwrap();
    assert_eq!(
        table.push(PhaseDef::every(PhaseId::COMMS, period, 3)),
        Err(TableError::OffsetBeyondPeriod)
    );
}

#[test]
fn table_due_slots_iterate_in_table_order() {
    let table = three_phase_table().unwrap();
    // Tick 0: sensor (slot 0) then comms (slot 1) — ops (slot 2) not due.
    let due: Vec<usize> = table.due_slots(0).collect();
    assert_eq!(due, vec![0, 1]);
    // Tick 2: sensor and ops.
    assert_eq!(table.due_slots(2).collect::<Vec<usize>>(), vec![0, 2]);
    // Tick 1: sensor only.
    assert_eq!(table.due_slots(1).collect::<Vec<usize>>(), vec![0]);
    assert!(table.contains(PhaseId::OPS));
    assert!(!table.contains(PhaseId(9)));
}
