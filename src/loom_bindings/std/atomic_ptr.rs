use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Deref;
use std::sync::atomic::Ordering;

macro_rules! cfg_has_atomic_ptr {
    ($($item:item)*) => {
        $(
            #[cfg(target_has_atomic = "ptr")]
            $item
        )*
    }
}

macro_rules! cfg_not_has_atomic_ptr {
    ($($item:item)*) => {
        $(
            #[cfg(not(target_has_atomic = "ptr"))]
            $item
        )*
    }
}

cfg_has_atomic_ptr! {
    pub struct AtomicPtr<T> {
        inner: UnsafeCell<std::sync::atomic::AtomicPtr<T>>,
    }

    unsafe impl<T> Send for AtomicPtr<T> {}
    unsafe impl<T> Sync for AtomicPtr<T> {}

    impl<T> AtomicPtr<T> {
        pub const fn new(ptr: *mut T) -> Self {
            Self {
                inner: UnsafeCell::new(std::sync::atomic::AtomicPtr::new(ptr)),
            }
        }

        /// Performs an unsynchronized load.
        ///
        /// # Safety
        ///
        /// Caller must ensure no concurrent mutation.
        pub unsafe fn unsync_load(&self) -> *mut T {
            core::ptr::read(self.inner.get().cast())
        }
    }

    impl<T> Deref for AtomicPtr<T> {
        type Target = std::sync::atomic::AtomicPtr<T>;

        fn deref(&self) -> &Self::Target {
            // safety: it is always safe to access `&self` fns on the inner value as
            // we never perform unsafe mutations.
            unsafe { &*self.inner.get() }
        }
    }

    impl<T> Default for AtomicPtr<T> {
        fn default() -> Self {
            Self::new(core::ptr::null_mut())
        }
    }

    impl<T> fmt::Debug for AtomicPtr<T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("AtomicPtr")
                .field("ptr", &self.load(Ordering::SeqCst))
                .finish()
        }
    }
}

cfg_not_has_atomic_ptr! {
    // Fallback implementation using NaiveRWLock<*mut T>
    use crate::loom_bindings::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

    pub struct AtomicPtrFallback<T> {
        inner: RwLock<T>,
    }

    unsafe impl<T> Send for AtomicPtr<T> {}
    unsafe impl<T> Sync for AtomicPtr<T> {}

    impl<T> AtomicPtr<T> {
        pub fn new(ptr: *mut T) -> Self {
            Self {
                inner: NaiveRWLock::new(ptr),
            }
        }

        /// Performs an unsynchronized load.
        ///
        /// # Safety
        ///
        /// Caller must ensure no concurrent mutation.
        pub unsafe fn unsync_load(&self) -> *mut T {
            *self.inner.data.get()
        }

        pub fn load(&self, _order: Ordering) -> *mut T {
            let guard: RwLockReadGuard<*mut T> = self.inner.read();

            *guard
        }

        pub fn store(&self, ptr: *mut T, _order: Ordering) {
            let mut guard: RwLockWriteGuard<*mut T> = self.inner.write();

            *guard = ptr;
        }

        pub fn compare_exchange(
            &self,
            current: *mut T,
            new: *mut T,
            _success: Ordering,
            _failure: Ordering,
        ) -> Result<*mut T, *mut T> {
            let mut guard = self.inner.write();

            if *guard == current {
                *guard = new;

                Ok(current)
            } else {
                Err(*guard)
            }
        }

        pub fn compare_exchange_weak(
            &self,
            current: *mut T,
            new: *mut T,
            success: Ordering,
            failure: Ordering,
        ) -> Result<*mut T, *mut T> {
            self.compare_exchange(current, new, success, failure)
        }
    }

    impl<T> Default for AtomicPtr<T> {
        fn default() -> Self {
            Self::new(core::ptr::null_mut())
        }
    }

    impl<T> fmt::Debug for AtomicPtr<T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("AtomicPtrFallback")
                .field("ptr", &self.load(Ordering::SeqCst))
                .finish()
        }
    }
}
