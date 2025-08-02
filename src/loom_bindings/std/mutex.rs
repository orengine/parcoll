use std::sync::{self, MutexGuard, TryLockError};

/// Adapter for `std::Mutex` that removes the poisoning aspects
/// from its API.
#[derive(Debug)]
pub struct Mutex<T: ?Sized>(sync::Mutex<T>);

impl<T> Mutex<T> {
    #[inline]
    pub const fn new(t: T) -> Self {
        Self(sync::Mutex::new(t))
    }

    #[inline]
    pub const fn const_new(t: T) -> Self {
        Self(sync::Mutex::new(t))
    }

    #[inline]
    pub fn lock(&self) -> MutexGuard<'_, T> {
        self.0.lock().unwrap_or_else(sync::PoisonError::into_inner)
    }

    #[inline]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        match self.0.try_lock() {
            Ok(guard) => Some(guard),
            Err(TryLockError::Poisoned(p_err)) => Some(p_err.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        }
    }
}
