//! Implementation of an atomic u64 cell. On 64 bit platforms, this is a
//! re-export of `AtomicU64`. On 32 bit platforms, this is implemented using a
//! `Mutex`.

macro_rules! cfg_has_atomic_u64 {
    ($($item:item)*) => {
        $(
            #[cfg(target_has_atomic = "64")]
            $item
        )*
    }
}

macro_rules! cfg_not_has_atomic_u64 {
    ($($item:item)*) => {
        $(
            #[cfg(not(target_has_atomic = "64"))]
            $item
        )*
    }
}

// `AtomicU64` can only be used on targets with `target_has_atomic` is 64 or greater.
// Once `cfg_target_has_atomic` feature is stable, we can replace it with
// `#[cfg(target_has_atomic = "64")]`.
// Refs: https://github.com/rust-lang/rust/tree/master/src/librustc_target
cfg_has_atomic_u64! {
    use std::cell::UnsafeCell;
    use std::fmt;
    use std::ops::Deref;
    use std::panic;

    /// `AtomicU64` providing an additional `unsync_load` function.
    pub struct AtomicU64 {
        inner: UnsafeCell<std::sync::atomic::AtomicU64>,
    }

    unsafe impl Send for AtomicU64 {}
    unsafe impl Sync for AtomicU64 {}
    impl panic::RefUnwindSafe for AtomicU64 {}
    impl panic::UnwindSafe for AtomicU64 {}

    impl AtomicU64 {
        pub const fn new(val: u64) -> Self {
            let inner = UnsafeCell::new(std::sync::atomic::AtomicU64::new(val));

            Self { inner }
        }

        /// Performs an unsynchronized load.
        ///
        /// # Safety
        ///
        /// All mutations must have happened before the unsynchronized load.
        /// Additionally, there must be no concurrent mutations.
        pub unsafe fn unsync_load(&self) -> u64 {
            core::ptr::read(self.inner.get() as *const u64)
        }
    }

    impl Default for AtomicU64 {
        fn default() -> AtomicU64 {
            Self {
                inner: UnsafeCell::new(std::sync::atomic::AtomicU64::new(0)),
            }
        }
    }

    impl Deref for AtomicU64 {
        type Target = std::sync::atomic::AtomicU64;

        fn deref(&self) -> &Self::Target {
            // safety: it is always safe to access `&self` fns on the inner value as
            // we never perform unsafe mutations.
            unsafe { &*self.inner.get() }
        }
    }

    impl fmt::Debug for AtomicU64 {
        fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.deref().fmt(fmt)
        }
    }
}

cfg_not_has_atomic_u64! {
    use crate::loom_bindings::sync::Mutex;
    use std::sync::atomic::Ordering;

    #[derive(Debug)]
    pub(crate) struct AtomicU64 {
        inner: Mutex<u64>,
    }

    impl AtomicU64 {
        pub(crate) fn new(val: u64) -> Self {
            Self {
                inner: Mutex::new(val),
            }
        }

        pub(crate) fn unsync_load(&self) -> u64 {
            *self.inner.try_lock().unwrap()
        }

        pub(crate) fn load(&self, _: Ordering) -> u64 {
            *self.inner.lock()
        }

        pub(crate) fn store(&self, val: u64, _: Ordering) {
            *self.inner.lock() = val;
        }

        pub(crate) fn fetch_add(&self, val: u64, _: Ordering) -> u64 {
            let mut lock = self.inner.lock();
            let prev = *lock;
            *lock = prev + val;
            prev
        }

        pub(crate) fn fetch_or(&self, val: u64, _: Ordering) -> u64 {
            let mut lock = self.inner.lock();
            let prev = *lock;
            *lock = prev | val;
            prev
        }

        pub(crate) fn compare_exchange(
            &self,
            current: u64,
            new: u64,
            _success: Ordering,
            _failure: Ordering,
        ) -> Result<u64, u64> {
            let mut lock = self.inner.lock();

            if *lock == current {
                *lock = new;
                Ok(current)
            } else {
                Err(*lock)
            }
        }

        pub(crate) fn compare_exchange_weak(
            &self,
            current: u64,
            new: u64,
            success: Ordering,
            failure: Ordering,
        ) -> Result<u64, u64> {
            self.compare_exchange(current, new, success, failure)
        }
    }

    impl Default for AtomicU64 {
        fn default() -> AtomicU64 {
            Self {
                inner: Mutex::new(0),
            }
        }
    }
}
