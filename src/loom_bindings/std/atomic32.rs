//! Implementation of an atomic u32 cell.
//! On platforms that support `target_has_atomic = "32"`, this is a
//! re-export of `AtomicU32`.
//! On other platforms, this is implemented using a `Mutex`.

macro_rules! cfg_has_atomic_u32 {
    ($($item:item)*) => {
        $(
            #[cfg(target_has_atomic = "32")]
            $item
        )*
    }
}

macro_rules! cfg_not_has_atomic_u32 {
    ($($item:item)*) => {
        $(
            #[cfg(not(target_has_atomic = "32"))]
            $item
        )*
    }
}

// `AtomicU32` can only be used on targets with `target_has_atomic` is 32 or greater.
// Once `cfg_target_has_atomic` feature is stable, we can replace it with
// `#[cfg(target_has_atomic = "32")]`.
// Refs: https://github.com/rust-lang/rust/tree/master/src/librustc_target
cfg_has_atomic_u32! {
    use std::cell::UnsafeCell;
    use std::fmt;
    use std::ops::Deref;
    use std::panic;

    /// `AtomicU32` providing an additional `unsync_load` function.
    pub struct AtomicU32 {
        inner: UnsafeCell<std::sync::atomic::AtomicU32>,
    }

    unsafe impl Send for AtomicU32 {}
    unsafe impl Sync for AtomicU32 {}
    impl panic::RefUnwindSafe for AtomicU32 {}
    impl panic::UnwindSafe for AtomicU32 {}

    impl AtomicU32 {
        pub const fn new(val: u32) -> Self {
            let inner = UnsafeCell::new(std::sync::atomic::AtomicU32::new(val));

            Self { inner }
        }

        /// Performs an unsynchronized load.
        ///
        /// # Safety
        ///
        /// All mutations must have happened before the unsynchronized load.
        /// Additionally, there must be no concurrent mutations.
        pub unsafe fn unsync_load(&self) -> u32 {
            unsafe { core::ptr::read(self.inner.get() as *const u32) }
        }
    }

    impl Default for AtomicU32 {
        fn default() -> Self {
            Self {
                inner: UnsafeCell::new(std::sync::atomic::AtomicU32::new(0)),
            }
        }
    }

    impl Deref for AtomicU32 {
        type Target = std::sync::atomic::AtomicU32;

        fn deref(&self) -> &Self::Target {
            // safety: it is always safe to access `&self` fns on the inner value as
            // we never perform unsafe mutations.
            unsafe { &*self.inner.get() }
        }
    }

    impl fmt::Debug for AtomicU32 {
        fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.deref().fmt(fmt)
        }
    }

    /// `AtomicU32` providing an additional `unsync_load` function.
    pub struct AtomicI32 {
        inner: UnsafeCell<std::sync::atomic::AtomicI32>,
    }

    unsafe impl Send for AtomicI32 {}
    unsafe impl Sync for AtomicI32 {}
    impl panic::RefUnwindSafe for AtomicI32 {}
    impl panic::UnwindSafe for AtomicI32 {}

    impl AtomicI32 {
        pub const fn new(val: i32) -> Self {
            let inner = UnsafeCell::new(std::sync::atomic::AtomicI32::new(val));

            Self { inner }
        }

        /// Performs an unsynchronized load.
        ///
        /// # Safety
        ///
        /// All mutations must have happened before the unsynchronized load.
        /// Additionally, there must be no concurrent mutations.
        pub unsafe fn unsync_load(&self) -> i32 {
            unsafe { core::ptr::read(self.inner.get() as *const i32) }
        }
    }

    impl Default for AtomicI32 {
        fn default() -> Self {
            Self {
                inner: UnsafeCell::new(std::sync::atomic::AtomicI32::new(0)),
            }
        }
    }

    impl Deref for AtomicI32 {
        type Target = std::sync::atomic::AtomicI32;

        fn deref(&self) -> &Self::Target {
            // safety: it is always safe to access `&self` fns on the inner value as
            // we never perform unsafe mutations.
            unsafe { &*self.inner.get() }
        }
    }

    impl fmt::Debug for AtomicI32 {
        fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.deref().fmt(fmt)
        }
    }
}

cfg_not_has_atomic_u32! {
    use crate::loom_bindings::sync::Mutex;
    use std::sync::atomic::Ordering;

    #[derive(Debug)]
    pub(crate) struct AtomicU32 {
        inner: Mutex<u32>,
    }

    impl AtomicU32 {
        pub(crate) fn new(val: u32) -> Self {
            Self {
                inner: Mutex::new(val),
            }
        }

        pub(crate) fn unsync_load(&self) -> u32 {
            *self.inner.try_lock().unwrap()
        }

        pub(crate) fn load(&self, _: Ordering) -> u32 {
            *self.inner.lock()
        }

        pub(crate) fn store(&self, val: u32, _: Ordering) {
            *self.inner.lock() = val;
        }

        pub(crate) fn fetch_add(&self, val: u32, _: Ordering) -> u32 {
            let mut lock = self.inner.lock();
            let prev = *lock;
            *lock = prev + val;
            prev
        }

        pub(crate) fn fetch_or(&self, val: u32, _: Ordering) -> u32 {
            let mut lock = self.inner.lock();
            let prev = *lock;
            *lock = prev | val;
            prev
        }

        pub(crate) fn compare_exchange(
            &self,
            current: u32,
            new: u32,
            _success: Ordering,
            _failure: Ordering,
        ) -> Result<u32, u32> {
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
            current: u32,
            new: u32,
            success: Ordering,
            failure: Ordering,
        ) -> Result<u32, u32> {
            self.compare_exchange(current, new, success, failure)
        }
    }

    impl Default for AtomicU32 {
        fn default() -> AtomicU32 {
            Self {
                inner: Mutex::new(0),
            }
        }
    }

    #[derive(Debug)]
    pub(crate) struct AtomicI32 {
        inner: Mutex<i32>,
    }

    impl AtomicI32 {
        pub(crate) fn new(val: i32) -> Self {
            Self {
                inner: Mutex::new(val),
            }
        }

        pub(crate) fn unsync_load(&self) -> i32 {
            *self.inner.try_lock().unwrap()
        }

        pub(crate) fn load(&self, _: Ordering) -> i32 {
            *self.inner.lock()
        }

        pub(crate) fn store(&self, val: i32, _: Ordering) {
            *self.inner.lock() = val;
        }

        pub(crate) fn fetch_add(&self, val: i32, _: Ordering) -> i32 {
            let mut lock = self.inner.lock();
            let prev = *lock;
            *lock = prev + val;
            prev
        }

        pub(crate) fn fetch_or(&self, val: i32, _: Ordering) -> i32 {
            let mut lock = self.inner.lock();
            let prev = *lock;
            *lock = prev | val;
            prev
        }

        pub(crate) fn compare_exchange(
            &self,
            current: i32,
            new: i32,
            _success: Ordering,
            _failure: Ordering,
        ) -> Result<i32, i32> {
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
            current: i32,
            new: i32,
            success: Ordering,
            failure: Ordering,
        ) -> Result<i32, i32> {
            self.compare_exchange(current, new, success, failure)
        }
    }

    impl Default for AtomicI32 {
        fn default() -> AtomicI32 {
            Self {
                inner: Mutex::new(0),
            }
        }
    }
}
