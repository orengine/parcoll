//! This module provides the [`SyncCell`] and the [`LockFreeSyncCell`] traits.
use crate::backoff::Backoff;
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

    /// Tries to swap the value.
    /// It returns `Ok(old_value)` or `Err(value)` if the [`SyncCell`] is locked.
    ///
    /// For [`LockFreeSyncCell`], this method always returns `Ok`.
    fn try_swap(&self, value: T) -> Result<T, T>;

    /// Swaps the value using busy-waiting with [`Backoff`].
    /// It returns the old value.
    ///
    /// For [`LockFreeSyncCell`], this method never blocks.
    fn swap(&self, mut value: T) -> T {
        let backoff = Backoff::new();

        loop {
            match self.try_swap(value) {
                Ok(value) => return value,
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

    fn try_swap(&self, value: T) -> Result<T, T> {
        let Some(mut guard) = self.try_write() else {
            return Err(value);
        };

        Ok(std::mem::replace(&mut *guard, value))
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

    fn try_swap(&self, value: T) -> Result<T, T> {
        let Some(mut guard) = self.try_write().ok() else {
            return Err(value);
        };

        Ok(std::mem::replace(&mut *guard, value))
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

    fn try_swap(&self, value: T) -> Result<T, T> {
        let Some(mut guard) = self.lock().ok() else {
            return Err(value);
        };

        Ok(std::mem::replace(&mut *guard, value))
    }
}

/// A mocking implementation of [`LockFreeSyncCell`]. But it is not lock-free.
#[cfg(test)]
pub(crate) struct LockFreeSyncCellMock<T> {
    inner: NaiveRWLock<T>,
}

#[cfg(test)]
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

    fn try_swap(&self, value: T) -> Result<T, T> {
        self.inner.try_swap(value)
    }
}

#[cfg(test)]
impl<T> LockFreeSyncCell<T> for LockFreeSyncCellMock<T> {}
