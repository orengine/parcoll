//! This module provides [`NaiveRWLock`] and its guards.
use crate::backoff::Backoff;
use crate::loom_bindings::sync::atomic::AtomicI32;
use crate::number_types::NonCachePaddedAtomicI32;
use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::Ordering;

/// A RAII read guard for a [`NaiveRWLock`].
pub struct NaiveRWLockReadGuard<'rw_lock, T, AtomicWrapper = NonCachePaddedAtomicI32>
where
    AtomicWrapper: Deref<Target = AtomicI32> + Default,
{
    rw_lock: &'rw_lock NaiveRWLock<T, AtomicWrapper>,
}

impl<T, AtomicWrapper: Deref<Target = AtomicI32> + Default> Deref
    for NaiveRWLockReadGuard<'_, T, AtomicWrapper>
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.rw_lock.data.get() }
    }
}

impl<T, AtomicWrapper: Deref<Target = AtomicI32> + Default> Drop
    for NaiveRWLockReadGuard<'_, T, AtomicWrapper>
{
    fn drop(&mut self) {
        self.rw_lock.state.fetch_sub(1, Ordering::Release);
    }
}

/// A RAII write guard for a [`NaiveRWLock`].
pub struct NaiveRWLockWriteGuard<'rw_lock, T, AtomicWrapper = NonCachePaddedAtomicI32>
where
    AtomicWrapper: Deref<Target = AtomicI32> + Default,
{
    rw_lock: &'rw_lock NaiveRWLock<T, AtomicWrapper>,
}

impl<T, AtomicWrapper: Deref<Target = AtomicI32> + Default> Deref
    for NaiveRWLockWriteGuard<'_, T, AtomicWrapper>
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.rw_lock.data.get() }
    }
}

impl<T, AtomicWrapper: Deref<Target = AtomicI32> + Default> DerefMut
    for NaiveRWLockWriteGuard<'_, T, AtomicWrapper>
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.rw_lock.data.get() }
    }
}

impl<T, AtomicWrapper: Deref<Target = AtomicI32> + Default> Drop
    for NaiveRWLockWriteGuard<'_, T, AtomicWrapper>
{
    fn drop(&mut self) {
        self.rw_lock.state.store(0, Ordering::Release);
    }
}

/// A naive read-write lock.
/// It can when and only when write operations are rare.
/// In this case it works much faster than [`std::sync::RwLock`].
pub struct NaiveRWLock<
    T,
    AtomicWrapper: Deref<Target = AtomicI32> + Default = NonCachePaddedAtomicI32,
> {
    state: AtomicWrapper,
    data: UnsafeCell<T>,
}

impl<T, AtomicWrapper: Deref<Target = AtomicI32> + Default> NaiveRWLock<T, AtomicWrapper> {
    /// Creates a new [`NaiveRWLock`] with the given data.
    pub fn new(data: T) -> Self {
        Self {
            state: AtomicWrapper::default(),
            data: UnsafeCell::new(data),
        }
    }

    /// Tries to acquire a read lock. Returns `None` if a write lock is held.
    pub fn try_read(&self) -> Option<NaiveRWLockReadGuard<T, AtomicWrapper>> {
        let mut state = self.state.load(Ordering::Acquire);

        loop {
            if state != -1 {
                match self.state.compare_exchange_weak(
                    state,
                    state + 1,
                    Ordering::Acquire,
                    Ordering::Acquire,
                ) {
                    Ok(_) => break Some(NaiveRWLockReadGuard { rw_lock: self }),
                    Err(current_state) => {
                        state = current_state;
                    }
                }
            } else {
                break None;
            }
        }
    }

    /// Acquires a write lock. Blocks until the lock is available.
    pub fn write(&self) -> NaiveRWLockWriteGuard<T, AtomicWrapper> {
        let backoff = Backoff::new();

        loop {
            match self
                .state
                .compare_exchange(0, -1, Ordering::Acquire, Ordering::Relaxed)
            {
                Ok(_) => return NaiveRWLockWriteGuard { rw_lock: self },
                Err(_) => backoff.snooze(),
            }
        }
    }
}
