//! Lock-free single-producer/single-consumer ring for ISR → phase handoff.
//!
//! ISRs must never block and never allocate, so deferred work travels through
//! this ring: the producer half runs in interrupt context, the consumer half
//! runs in the phase that owns the data. Capacity is fixed at compile time
//! (power of two); overflow is counted and the value handed back, never
//! silently dropped.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Returned by [`SpscRing::push`] when the ring is full; the value comes back
/// so the producer can decide (count it, retry, or overwrite) instead of
/// losing it silently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Full<T>(pub T);

pub struct SpscRing<T, const N: usize> {
    buf: UnsafeCell<MaybeUninit<[T; N]>>,
    /// Next slot to write. Only the producer writes this.
    head: AtomicUsize,
    /// Next slot to read. Only the consumer writes this.
    tail: AtomicUsize,
    /// Failed pushes since construction — honest overflow accounting.
    overflow: AtomicUsize,
}

// SPSC discipline: exactly one producer and one consumer context. The two
// atomic cursors are each owned by one side, and slots are handed over
// head→tail with acquire/release ordering, so a value is fully written before
// the reader can observe it. `T` must be Send because values cross interrupt
// and task contexts.
unsafe impl<T: Send, const N: usize> Sync for SpscRing<T, N> {}

impl<T, const N: usize> SpscRing<T, N> {
    const MASK: usize = N - 1;

    /// `const` so rings can live in statics, shared with interrupt handlers.
    pub const fn new() -> Self {
        const {
            assert!(
                N.is_power_of_two(),
                "ring capacity N must be a power of two"
            );
        }
        Self {
            buf: UnsafeCell::new(MaybeUninit::uninit()),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            overflow: AtomicUsize::new(0),
        }
    }

    /// Producer side (ISR context). Never blocks, never allocates.
    pub fn push(&self, value: T) -> Result<(), Full<T>> {
        let head = self.head.load(Ordering::Relaxed);
        let used = head.wrapping_sub(self.tail.load(Ordering::Acquire));
        if used >= N {
            self.overflow.fetch_add(1, Ordering::Relaxed);
            return Err(Full(value));
        }
        // Safety: slot `head & MASK` is empty — the consumer only frees a slot
        // by releasing `tail`, which the occupancy check above observed.
        unsafe {
            (*self.buf.get())
                .as_mut_ptr()
                .cast::<T>()
                .add(head & Self::MASK)
                .write(value);
        }
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Consumer side (phase context). `None` when empty.
    pub fn pop(&self) -> Option<T> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        // Safety: slot `tail & MASK` holds a value the producer published by
        // releasing `head`, which the empty check above observed; no other
        // reader exists.
        let value = unsafe {
            (*self.buf.get())
                .as_ptr()
                .cast::<T>()
                .add(tail & Self::MASK)
                .read()
        };
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(value)
    }

    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Acquire) == self.tail.load(Ordering::Relaxed)
    }

    pub fn len(&self) -> usize {
        self.head
            .load(Ordering::Acquire)
            .wrapping_sub(self.tail.load(Ordering::Relaxed))
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    /// Failed pushes since construction.
    pub fn overflow_count(&self) -> usize {
        self.overflow.load(Ordering::Relaxed)
    }
}

impl<T, const N: usize> Default for SpscRing<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> Drop for SpscRing<T, N> {
    fn drop(&mut self) {
        let buf = self.buf.get_mut().as_mut_ptr().cast::<T>();
        let head = *self.head.get_mut();
        let tail = *self.tail.get_mut();
        for i in tail..head {
            // Safety: slots in [tail, head) hold values that were written and
            // not yet read.
            unsafe { core::ptr::drop_in_place(buf.add(i & Self::MASK)) }
        }
    }
}
