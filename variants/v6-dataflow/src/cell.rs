//! One-time static initialization cell — no allocator, no move after init.
//!
//! The embedded-dataflow idiom for "statically allocated, initialized at
//! boot": the cell is a `static` (its storage cost is the payload's), and
//! `init` writes the value during bring-up, handing out the one `&'static
//! mut` the rest of the system pins. V6 uses it to give the graph a
//! link-time-fixed home (`.bss`/`.data`), which is what makes the budget
//! check in [`crate::budget`] honest.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, Ordering};

/// A cell that hands out one `&'static mut T`, exactly once.
pub struct StaticCell<T> {
    value: UnsafeCell<MaybeUninit<T>>,
    taken: AtomicBool,
}

// SAFETY: `init` hands out at most one `&'static mut T` (guarded by the
// `taken` swap) and never hands out another; the caller pins the reference
// for the program's life. `T` must be `Send` because the reference may be
// used from any context.
unsafe impl<T: Send> Sync for StaticCell<T> {}

impl<T> StaticCell<T> {
    /// An empty cell. Const, for `static`s.
    pub const fn new() -> Self {
        Self {
            value: UnsafeCell::new(MaybeUninit::uninit()),
            taken: AtomicBool::new(false),
        }
    }

    /// Write `value` and hand out the one exclusive `'static` reference.
    ///
    /// # Panics
    /// If called twice — a second init is a boot bug, not a fallback path.
    #[allow(clippy::mut_from_ref)] // once-only: the `taken` swap makes the
    // slot unaliased, so deriving one `&mut` from `&'static self` is sound
    pub fn init(&'static self, value: T) -> &'static mut T {
        if self.taken.swap(true, Ordering::AcqRel) {
            panic!("StaticCell initialized twice");
        }
        // SAFETY: exactly one call gets past the swap (it is the only
        // writer of `taken`), so the slot is unaliased from here on.
        unsafe { &mut *self.value.get() }.write(value)
    }
}

impl<T> Default for StaticCell<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_hands_out_the_value_once() {
        static CELL: StaticCell<u32> = StaticCell::new();
        let v = CELL.init(42);
        assert_eq!(*v, 42);
        *v += 1;
        assert_eq!(*v, 43);
    }

    #[test]
    #[should_panic(expected = "initialized twice")]
    fn second_init_panics() {
        static CELL: StaticCell<u32> = StaticCell::new();
        let _ = CELL.init(1);
        let _ = CELL.init(2); // boot bug: exactly one init is the contract
    }
}
