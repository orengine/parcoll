//! This module provides the [`SyncCell`] and the [`LockFreeSyncCell`] traits.
use orengine_utils::backoff::Backoff;
use crate::naive_rw_lock::NaiveRWLock;

/// A thread-safe cell that can be used as `atomic` for any type.
///
/// It is already implemented for [`NaiveRWLock`], [`std::sync::Mutex`], [`std::sync::RwLock`].
pub trait SyncCell<T> {
    /// Creates a new [`SyncCell`] with the given value.
    fn from_value(value: T) -> Self;

    /// Returns a mutable reference to the inner value.
    fn get_mut(&mut self) -> &mut T;

    /// Tries to load the value and call the provided function with it.
    /// Returns `None` if the [`SyncCell`] is locked.
    ///
    /// For [`LockFreeSyncCell`], this method always returns `Some`.
    fn try_with<U>(&self, f: impl FnOnce(&T) -> U) -> Option<U>;

    /// Tries to store the value.
    /// It returns `Ok(())` or `Err(value)` if the [`SyncCell`] is locked.
    ///
    /// For [`LockFreeSyncCell`], this method always returns `Ok`.
    fn try_store(&self, value: T) -> Result<(), T>;

    /// Tries to store the value using busy-waiting with [`Backoff`].
    ///
    /// For [`LockFreeSyncCell`], this method never blocks.
    fn swap(&self, mut value: T) {
        let backoff = 
            Backoff::new();

        loop {
            match self.try_store(value) {
                Ok(()) => return,
                Err(value_) => {
                    value = value_;

                    backoff.snooze();
                }
            }
        }
    }
}

/// A lock-free [`SyncCell`]. Read methods of [`SyncCell`] for more information.
pub trait LockFreeSyncCell<T>: SyncCell<T> {}

// It is unsafe to implement [`SyncCell`] for [`AtomicPtr`] because after loading the pointer and
// before reading the value, it can be deallocated.
// But it becomes safe with the Epoch-Based Memory Reclamation or other techniques.

impl<T> SyncCell<T> for NaiveRWLock<T> {
    fn from_value(value: T) -> Self {
        Self::new(value)
    }

    fn get_mut(&mut self) -> &mut T {
        Self::get_mut(self)
    }

    fn try_with<U>(&self, f: impl FnOnce(&T) -> U) -> Option<U> {
        self.try_read().map(|guard| f(&*guard))
    }

    fn try_store(&self, value: T) -> Result<(), T> {
        let Some(mut guard) = self.try_write() else {
            return Err(value);
        };

        drop(std::mem::replace(&mut *guard, value));

        Ok(())
    }
}

impl<T> SyncCell<T> for std::sync::RwLock<T> {
    fn from_value(value: T) -> Self {
        Self::new(value)
    }

    fn get_mut(&mut self) -> &mut T {
        Self::get_mut(self).unwrap()
    }

    fn try_with<U>(&self, f: impl FnOnce(&T) -> U) -> Option<U> {
        self.read().map(|guard| f(&*guard)).ok()
    }

    fn try_store(&self, value: T) -> Result<(), T> {
        let Some(mut guard) = self.try_write().ok() else {
            return Err(value);
        };

        drop(std::mem::replace(&mut *guard, value));

        Ok(())
    }
}

impl<T> SyncCell<T> for std::sync::Mutex<T> {
    fn from_value(value: T) -> Self {
        Self::new(value)
    }

    fn get_mut(&mut self) -> &mut T {
        Self::get_mut(self).unwrap()
    }

    fn try_with<U>(&self, f: impl FnOnce(&T) -> U) -> Option<U> {
        self.lock().map(|guard| f(&*guard)).ok()
    }

    fn try_store(&self, value: T) -> Result<(), T> {
        let Some(mut guard) = self.lock().ok() else {
            return Err(value);
        };

        drop(std::mem::replace(&mut *guard, value));

        Ok(())
    }
}

/// A mocking implementation of [`LockFreeSyncCell`]. But it is not lock-free.
#[cfg(all(test, not(feature = "with-light-qsbr")))]
pub(crate) struct LockFreeSyncCellMock<T> {
    inner: NaiveRWLock<T>,
}

#[cfg(all(test, not(feature = "with-light-qsbr")))]
impl<T> SyncCell<T> for LockFreeSyncCellMock<T> {
    fn from_value(value: T) -> Self {
        Self {
            inner: NaiveRWLock::new(value),
        }
    }

    fn get_mut(&mut self) -> &mut T {
        self.inner.get_mut()
    }

    fn try_with<U>(&self, f: impl FnOnce(&T) -> U) -> Option<U> {
        self.inner.try_with(f)
    }

    fn try_store(&self, value: T) -> Result<(), T> {
        self.inner.try_store(value)
    }
}

#[cfg(all(test, not(feature = "with-light-qsbr")))]
impl<T> LockFreeSyncCell<T> for LockFreeSyncCellMock<T> {}

#[cfg(feature = "with-light-qsbr")]
pub struct LightQSBRSyncCell<T> {
    inner: crate::loom_bindings::sync::atomic::AtomicPtr<T>,
}

#[cfg(feature = "with-light-qsbr")]
impl<T> SyncCell<T> for LightQSBRSyncCell<T> {
    fn from_value(value: T) -> Self {
        Self {
            inner: crate::loom_bindings::sync::atomic::AtomicPtr::new(Box::into_raw(Box::new(value))),
        }
    }

    fn get_mut(&mut self) -> &mut T {
        unsafe { &mut **self.inner.get_mut() }
    }

    fn try_with<U>(&self, f: impl FnOnce(&T) -> U) -> Option<U> {
        let value = self.inner.load(std::sync::atomic::Ordering::Acquire);
        
        Some(f(unsafe { &*value }))
    }

    fn try_store(&self, value: T) -> Result<(), T> {
        let new = Box::into_raw(Box::new(value));
        let prev = self.inner.swap(new, std::sync::atomic::Ordering::AcqRel);

        if std::mem::needs_drop::<T>() {
            unsafe {
                light_qsbr::local_manager().schedule_drop(move || {
                    drop(Box::from_raw(prev));
                });
            };
        } else {
            unsafe { light_qsbr::local_manager().schedule_deallocate(prev); };
        }

        Ok(())
    }

    fn swap(&self, value: T) {
        orengine_utils::hints::unwrap_or_bug_hint(self.try_store(value));
    }
}

#[cfg(feature = "with-light-qsbr")]
impl<T> LockFreeSyncCell<T> for LightQSBRSyncCell<T> {}

#[cfg(all(test, not(feature = "with-light-qsbr")))]
pub(crate) type LockFreeSyncCellForTests<T> = LockFreeSyncCellMock<T>;

#[cfg(all(test, feature = "with-light-qsbr"))]
pub(crate) type LockFreeSyncCellForTests<T> = LightQSBRSyncCell<T>;
