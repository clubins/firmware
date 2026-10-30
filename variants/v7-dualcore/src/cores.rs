//! Core launch + runtimes — the device dual-core wiring and its host
//! mirror.
//!
//! The launch path uses esp-hal 1.2.2's `CpuControl::start_app_core`
//! (`src/system/multi_core.rs` in the pinned source; the API is gated by
//! esp-hal's `unstable` feature, which this variant enables). On the
//! target, [`boot`] splits the boundary, starts core 1 on the sensing
//! stack, and never returns from core 0. On the host, the same
//! composition runs cooperatively ([`Systems::runtimes`]) so the entire
//! partition system is unit-testable without hardware.

use crate::boundary::{OpsPartition, SamplingPartition, leak_split};
use crate::core0::Core0System;
use crate::core1::Core1System;

/// Firmware version — status heartbeats report it.
pub const FIRMWARE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The sensing partition's runtime: link + system. On the target this
/// lives on core 1 only; nothing else holds it.
pub struct Core1Runtime {
    link: crate::boundary::Core1Link,
    system: Core1System,
}

impl Core1Runtime {
    pub fn new(link: crate::boundary::Core1Link) -> Self {
        Core1Runtime {
            link,
            system: Core1System::build_registry(),
        }
    }

    /// One sensing step (core 1). Deterministic: fixed driver order, no
    /// allocation, no blocking.
    pub fn tick(&mut self) -> u32 {
        self.system.tick(&mut self.link)
    }

    pub fn system(&self) -> &Core1System {
        &self.system
    }
}

/// The network/ops partition's runtime.
pub struct Core0Runtime {
    link: crate::boundary::Core0Link,
    system: Core0System,
}

impl Core0Runtime {
    pub fn new(link: crate::boundary::Core0Link) -> Self {
        Core0Runtime {
            link,
            system: Core0System::new(),
        }
    }

    /// One ops step (core 0). Latency-tolerant: may back off, retry,
    /// wait.
    pub fn tick(&mut self) -> u32 {
        self.system.tick(&mut self.link)
    }

    pub fn system(&self) -> &Core0System {
        &self.system
    }
}

/// Both runtimes over one boundary — the host test harness and the
/// cooperative single-core fallback share this shape.
pub struct Systems {
    pub core1: Core1Runtime,
    pub core0: Core0Runtime,
}

impl Systems {
    /// Builds both partitions over a freshly split boundary.
    pub fn new() -> Self {
        let (core1_link, core0_link) = leak_split();
        Systems {
            core1: Core1Runtime::new(core1_link),
            core0: Core0Runtime::new(core0_link),
        }
    }

    /// Borrows both runtimes for a cooperative two-partition round: core
    /// 1 samples, core 0 publishes. On the target the two loops run
    /// concurrently on their own cores; this sequence is the host mirror.
    pub fn runtimes(&mut self) -> (&mut Core1Runtime, &mut Core0Runtime) {
        (&mut self.core1, &mut self.core0)
    }
}

impl Default for Systems {
    fn default() -> Self {
        Self::new()
    }
}

/// Device entry: split the boundary, start core 1, run core 0. Never
/// returns. Target-only — the host has no esp-hal types; the cooperative
/// mirror ([`Systems`]) is exercised in tests instead. Path note: in
/// esp-hal 1.2.2 the dual-core API lives at `esp_hal::system` (src/system.rs
/// re-exports src/system/multi_core.rs under the `unstable` feature).
#[cfg(target_arch = "xtensa")]
pub fn boot(cpu_control: esp_hal::system::CpuControl<'_>) -> ! {
    use esp_hal::system::Stack;

    /// The APP CPU stack, in static memory: esp-hal requires a `&'static
    /// mut Stack<N>` for the app core; N is bytes (`[u8; N]` — 32 KiB
    /// here, a multiple of 16 as `Stack::new` requires).
    static mut APP_CORE_STACK: Stack<32768> = Stack::new();

    let (core1_link, core0_link) = leak_split();
    let mut core1_runtime = Core1Runtime::new(core1_link);

    let mut cores = cpu_control;
    // The single allowed unsafe in the crate (lib.rs: deny(unsafe_code)):
    // esp-hal requires `&'static mut Stack` to launch the app core, and a
    // static-mutable borrow is the only way to provide one. The stack is
    // handed to core 1 exactly once, at boot, and never touched from
    // core 0 again — see the loop comment below.
    #[allow(unsafe_code)]
    let core1_started = cores.start_app_core(
        unsafe { &mut *core::ptr::addr_of_mut!(APP_CORE_STACK) },
        move || {
            // CORE 1 — sensing + DSP: apply control, poll drivers, queue
            // measurements. Fixed order, no allocation, no blocking; at
            // worst the queue back-pressures and drops are counted.
            loop {
                let _queued = core1_runtime.tick();
            }
        },
    );

    if let Err(_error) = core1_started {
        // The APP CPU failed to start (not observed on S3 in practice).
        // Sampling is lost; core 0 keeps operating — the network-side
        // partition outliving sensing matches the charter's failure
        // model (a network fault can never stall sampling, and neither
        // can its absence).
    }

    // CORE 0 — Wi-Fi/transports/ops: latency-tolerant loop.
    let mut core0_runtime = Core0Runtime::new(core0_link);
    loop {
        let _drained = core0_runtime.tick();
    }
}
