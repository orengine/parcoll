//! Implementation of an atomic u8 cell.
//! On platforms that support `target_has_atomic = "8"`, this is a
//! re-export of `AtomicU8`.
//! On other platforms, this is implemented using a `Mutex`.

macro_rules! cfg_has_atomic_u8 {
    ($($item:item)*) => {
        $(
            #[cfg(target_has_atomic = "8")]
            $item
        )*
    }
}

macro_rules! cfg_not_has_atomic_u8 {
    ($($item:item)*) => {
        $(
            #[cfg(not(target_has_atomic = "8"))]
            $item
        )*
    }
}

// `AtomicU8` can only be used on targets with `target_has_atomic` is 8 or greater.
// Once `cfg_target_has_atomic` feature is stable, we can replace it with
// `#[cfg(target_has_atomic = "8")]`.
// Refs: https://github.com/rust-lang/rust/tree/master/src/librustc_target
cfg_has_atomic_u8! {
    use std::cell::UnsafeCell;
    use std::fmt;
    use std::ops::Deref;
    use std::panic;

    /// `AtomicU8` providing an additional `unsync_load` function.
    pub struct AtomicU8 {
        inner: UnsafeCell<std::sync::atomic::AtomicU8>,
    }

    unsafe impl Send for AtomicU8 {}
    unsafe impl Sync for AtomicU8 {}
    impl panic::RefUnwindSafe for AtomicU8 {}
    impl panic::UnwindSafe for AtomicU8 {}

    impl AtomicU8 {
        pub const fn new(val: u8) -> Self {
            let inner = UnsafeCell::new(std::sync::atomic::AtomicU8::new(val));

            Self { inner }
        }

        /// Performs an unsynchronized load.
        ///
        /// # Safety
        ///
        /// All mutations must have happened before the unsynchronized load.
        /// Additionally, there must be no concurrent mutations.
        pub unsafe fn unsync_load(&self) -> u8 {
            core::ptr::read(self.inner.get() as *const u8)
        }
    }

    impl Default for AtomicU8 {
        fn default() -> AtomicU8 {
            Self {
                inner: UnsafeCell::new(std::sync::atomic::AtomicU8::new(0)),
            }
        }
    }

    impl Deref for AtomicU8 {
        type Target = std::sync::atomic::AtomicU8;

        fn deref(&self) -> &Self::Target {
            // safety: it is always safe to access `&self` fns on the inner value as
            // we never perform unsafe mutations.
            unsafe { &*self.inner.get() }
        }
    }

    impl fmt::Debug for AtomicU8 {
        fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.deref().fmt(fmt)
        }
    }

    /// `AtomicI8` providing an additional `unsync_load` function.
    pub struct AtomicI8 {
        inner: UnsafeCell<std::sync::atomic::AtomicI8>,
    }

    unsafe impl Send for AtomicI8 {}
    unsafe impl Sync for AtomicI8 {}
    impl panic::RefUnwindSafe for AtomicI8 {}
    impl panic::UnwindSafe for AtomicI8 {}

    impl AtomicI8 {
        pub const fn new(val: i8) -> Self {
            let inner = UnsafeCell::new(std::sync::atomic::AtomicI8::new(val));

            Self { inner }
        }

        /// Performs an unsynchronized load.
        ///
        /// # Safety
        ///
        /// All mutations must have happened before the unsynchronized load.
        /// Additionally, there must be no concurrent mutations.
        pub unsafe fn unsync_load(&self) -> i8 {
            core::ptr::read(self.inner.get() as *const i8)
        }
    }

    impl Default for AtomicI8 {
        fn default() -> AtomicI8 {
            Self {
                inner: UnsafeCell::new(std::sync::atomic::AtomicI8::new(0)),
            }
        }
    }

    impl Deref for AtomicI8 {
        type Target = std::sync::atomic::AtomicI8;

        fn deref(&self) -> &Self::Target {
            // safety: it is always safe to access `&self` fns on the inner value as
            // we never perform unsafe mutations.
            unsafe { &*self.inner.get() }
        }
    }

    impl fmt::Debug for AtomicI8 {
        fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.deref().fmt(fmt)
        }
    }
}

cfg_not_has_atomic_u8! {
    use crate::loom_bindings::sync::Mutex;
    use std::sync::atomic::Ordering;

    #[derive(Debug)]
    pub(crate) struct AtomicU8 {
        inner: Mutex<u8>,
    }

    impl AtomicU8 {
        pub(crate) fn new(val: u8) -> Self {
            Self {
                inner: Mutex::new(val),
            }
        }

        pub(crate) fn unsync_load(&self) -> u8 {
            *self.inner.try_lock().unwrap()
        }

        pub(crate) fn load(&self, _: Ordering) -> u8 {
            *self.inner.lock()
        }

        pub(crate) fn store(&self, val: u8, _: Ordering) {
            *self.inner.lock() = val;
        }

        pub(crate) fn fetch_add(&self, val: u8, _: Ordering) -> u8 {
            let mut lock = self.inner.lock();
            let prev = *lock;
            *lock = prev + val;
            prev
        }

        pub(crate) fn fetch_or(&self, val: u8, _: Ordering) -> u8 {
            let mut lock = self.inner.lock();
            let prev = *lock;
            *lock = prev | val;
            prev
        }

        pub(crate) fn compare_exchange(
            &self,
            current: u8,
            new: u8,
            _success: Ordering,
            _failure: Ordering,
        ) -> Result<u8, u8> {
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
            current: u8,
            new: u8,
            success: Ordering,
            failure: Ordering,
        ) -> Result<u8, u8> {
            self.compare_exchange(current, new, success, failure)
        }
    }

    impl Default for AtomicU8 {
        fn default() -> AtomicU8 {
            Self {
                inner: Mutex::new(0),
            }
        }
    }

    #[derive(Debug)]
    pub(crate) struct AtomicI8 {
        inner: Mutex<i8>,
    }

    impl AtomicI8 {
        pub(crate) fn new(val: i8) -> Self {
            Self {
                inner: Mutex::new(val),
            }
        }

        pub(crate) fn unsync_load(&self) -> i8 {
            *self.inner.try_lock().unwrap()
        }

        pub(crate) fn load(&self, _: Ordering) -> i8 {
            *self.inner.lock()
        }

        pub(crate) fn store(&self, val: i8, _: Ordering) {
            *self.inner.lock() = val;
        }

        pub(crate) fn fetch_add(&self, val: i8, _: Ordering) -> i8 {
            let mut lock = self.inner.lock();
            let prev = *lock;
            *lock = prev + val;
            prev
        }

        pub(crate) fn fetch_or(&self, val: i8, _: Ordering) -> i8 {
            let mut lock = self.inner.lock();
            let prev = *lock;
            *lock = prev | val;
            prev
        }

        pub(crate) fn compare_exchange(
            &self,
            current: i8,
            new: i8,
            _success: Ordering,
            _failure: Ordering,
        ) -> Result<i8, i8> {
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
            current: i8,
            new: i8,
            success: Ordering,
            failure: Ordering,
        ) -> Result<i8, i8> {
            self.compare_exchange(current, new, success, failure)
        }
    }

    impl Default for AtomicI8 {
        fn default() -> AtomicI8 {
            Self {
                inner: Mutex::new(0),
            }
        }
    }
}
