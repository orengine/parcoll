//! Implementation of an atomic u16 cell.
//! On platforms that support `target_has_atomic = "16"`, this is a
//! re-export of `AtomicU16`.
//! On other platforms, this is implemented using a `Mutex`.

macro_rules! cfg_has_atomic_u16 {
    ($($item:item)*) => {
        $(
            #[cfg(target_has_atomic = "16")]
            $item
        )*
    }
}

macro_rules! cfg_not_has_atomic_u16 {
    ($($item:item)*) => {
        $(
            #[cfg(not(target_has_atomic = "16"))]
            $item
        )*
    }
}

// `AtomicU16` can only be used on targets with `target_has_atomic` is 16 or greater.
// Once `cfg_target_has_atomic` feature is stable, we can replace it with
// `#[cfg(target_has_atomic = "16")]`.
// Refs: https://github.com/rust-lang/rust/tree/master/src/librustc_target
cfg_has_atomic_u16! {
    use std::cell::UnsafeCell;
    use std::fmt;
    use std::ops::Deref;
    use std::panic;

    /// `AtomicU16` providing an additional `unsync_load` function.
    pub struct AtomicU16 {
        inner: UnsafeCell<std::sync::atomic::AtomicU16>,
    }

    unsafe impl Send for AtomicU16 {}
    unsafe impl Sync for AtomicU16 {}
    impl panic::RefUnwindSafe for AtomicU16 {}
    impl panic::UnwindSafe for AtomicU16 {}

    impl AtomicU16 {
        pub const fn new(val: u16) -> Self {
            let inner = UnsafeCell::new(std::sync::atomic::AtomicU16::new(val));

            Self { inner }
        }

        /// Performs an unsynchronized load.
        ///
        /// # Safety
        ///
        /// All mutations must have happened before the unsynchronized load.
        /// Additionally, there must be no concurrent mutations.
        pub unsafe fn unsync_load(&self) -> u16 {
            core::ptr::read(self.inner.get() as *const u16)
        }
    }

    impl Default for AtomicU16 {
        fn default() -> AtomicU16 {
            Self {
                inner: UnsafeCell::new(std::sync::atomic::AtomicU16::new(0)),
            }
        }
    }

    impl Deref for AtomicU16 {
        type Target = std::sync::atomic::AtomicU16;

        fn deref(&self) -> &Self::Target {
            // safety: it is always safe to access `&self` fns on the inner value as
            // we never perform unsafe mutations.
            unsafe { &*self.inner.get() }
        }
    }

    impl fmt::Debug for AtomicU16 {
        fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.deref().fmt(fmt)
        }
    }

    /// `AtomicI16` providing an additional `unsync_load` function.
    pub struct AtomicI16 {
        inner: UnsafeCell<std::sync::atomic::AtomicI16>,
    }

    unsafe impl Send for AtomicI16 {}
    unsafe impl Sync for AtomicI16 {}
    impl panic::RefUnwindSafe for AtomicI16 {}
    impl panic::UnwindSafe for AtomicI16 {}

    impl AtomicI16 {
        pub const fn new(val: i16) -> Self {
            let inner = UnsafeCell::new(std::sync::atomic::AtomicI16::new(val));

            Self { inner }
        }

        /// Performs an unsynchronized load.
        ///
        /// # Safety
        ///
        /// All mutations must have happened before the unsynchronized load.
        /// Additionally, there must be no concurrent mutations.
        pub unsafe fn unsync_load(&self) -> i16 {
            core::ptr::read(self.inner.get() as *const i16)
        }
    }

    impl Default for AtomicI16 {
        fn default() -> AtomicI16 {
            Self {
                inner: UnsafeCell::new(std::sync::atomic::AtomicI16::new(0)),
            }
        }
    }

    impl Deref for AtomicI16 {
        type Target = std::sync::atomic::AtomicI16;

        fn deref(&self) -> &Self::Target {
            // safety: it is always safe to access `&self` fns on the inner value as
            // we never perform unsafe mutations.
            unsafe { &*self.inner.get() }
        }
    }

    impl fmt::Debug for AtomicI16 {
        fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.deref().fmt(fmt)
        }
    }
}

cfg_not_has_atomic_u16! {
    use crate::loom_bindings::sync::Mutex;
    use std::sync::atomic::Ordering;

    #[derive(Debug)]
    pub(crate) struct AtomicU16 {
        inner: Mutex<u16>,
    }

    impl AtomicU16 {
        pub(crate) fn new(val: u16) -> Self {
            Self {
                inner: Mutex::new(val),
            }
        }

        pub(crate) fn unsync_load(&self) -> u16 {
            *self.inner.try_lock().unwrap()
        }

        pub(crate) fn load(&self, _: Ordering) -> u16 {
            *self.inner.lock()
        }

        pub(crate) fn store(&self, val: u16, _: Ordering) {
            *self.inner.lock() = val;
        }

        pub(crate) fn fetch_add(&self, val: u16, _: Ordering) -> u16 {
            let mut lock = self.inner.lock();
            let prev = *lock;
            *lock = prev + val;
            prev
        }

        pub(crate) fn fetch_or(&self, val: u16, _: Ordering) -> u16 {
            let mut lock = self.inner.lock();
            let prev = *lock;
            *lock = prev | val;
            prev
        }

        pub(crate) fn compare_exchange(
            &self,
            current: u16,
            new: u16,
            _success: Ordering,
            _failure: Ordering,
        ) -> Result<u16, u16> {
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
            current: u16,
            new: u16,
            success: Ordering,
            failure: Ordering,
        ) -> Result<u16, u16> {
            self.compare_exchange(current, new, success, failure)
        }
    }

    impl Default for AtomicU16 {
        fn default() -> AtomicU16 {
            Self {
                inner: Mutex::new(0),
            }
        }
    }

    #[derive(Debug)]
    pub(crate) struct AtomicI16 {
        inner: Mutex<i16>,
    }

    impl AtomicI16 {
        pub(crate) fn new(val: i16) -> Self {
            Self {
                inner: Mutex::new(val),
            }
        }

        pub(crate) fn unsync_load(&self) -> i16 {
            *self.inner.try_lock().unwrap()
        }

        pub(crate) fn load(&self, _: Ordering) -> i16 {
            *self.inner.lock()
        }

        pub(crate) fn store(&self, val: i16, _: Ordering) {
            *self.inner.lock() = val;
        }

        pub(crate) fn fetch_add(&self, val: i16, _: Ordering) -> i16 {
            let mut lock = self.inner.lock();
            let prev = *lock;
            *lock = prev + val;
            prev
        }

        pub(crate) fn fetch_or(&self, val: i16, _: Ordering) -> i16 {
            let mut lock = self.inner.lock();
            let prev = *lock;
            *lock = prev | val;
            prev
        }

        pub(crate) fn compare_exchange(
            &self,
            current: i16,
            new: i16,
            _success: Ordering,
            _failure: Ordering,
        ) -> Result<i16, i16> {
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
            current: i16,
            new: i16,
            success: Ordering,
            failure: Ordering,
        ) -> Result<i16, i16> {
            self.compare_exchange(current, new, success, failure)
        }
    }

    impl Default for AtomicI16 {
        fn default() -> AtomicI16 {
            Self {
                inner: Mutex::new(0),
            }
        }
    }
}
