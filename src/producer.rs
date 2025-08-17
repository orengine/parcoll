//! Provides the [`Producer`] and the [`LockFreeProducer`] traits.
use crate::{LockFreePushErr, LockFreePushManyErr};
use std::mem;

/// A producer of a queue.
pub trait Producer<T> {
    /// Returns the capacity of the queue.
    fn capacity(&self) -> usize;

    /// Returns the length of the queue.
    fn len(&self) -> usize;

    /// Returns whether the queue is empty.
    #[inline]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the number of free slots in the queue.
    #[inline]
    fn free_slots(&self) -> usize {
        self.capacity() - self.len()
    }

    /// Pushes multiple values into the queue.
    /// It accepts two slices to allow using it for ring-based queues.
    ///
    /// # Safety
    ///
    /// If the `T` is not `Copy`, the caller must [`forget`](mem::forget) the both slices;
    /// It should be called only when the `Producer` has space for the values.
    /// It doesn't check if it has space and expected that the caller does.
    unsafe fn push_many_unchecked(&self, first: &[T], last: &[T]);

    /// Pushes multiple values into the queue or returns an error if
    /// the queue doesn't have enough space.
    ///
    /// It may be non-lock-free.
    ///
    /// # Safety
    ///
    /// If the `T` is not `Copy`, the caller must [`forget`](mem::forget) the provided slice.
    unsafe fn maybe_push_many(&self, slice: &[T]) -> Result<(), ()>;

    /// Pushes a value only if the queue is not full.
    /// It returns an error if the queue is full.
    ///
    /// It may be non-lock-free.
    fn maybe_push(&self, mut value: T) -> Result<(), T> {
        if unsafe { self.maybe_push_many(&*(&raw mut value).cast::<[_; 1]>()) }.is_ok() {
            mem::forget(value);

            Ok(())
        } else {
            Err(value)
        }
    }
}

/// A lock-free producer of a queue.
pub trait LockFreeProducer<T> {
    /// Pushes multiple values into the queue or returns an `Err(`[`LockFreePushManyErr`]`)`.
    ///
    /// It is lock-free.
    /// If you can lock, you can look at the [`Producer::maybe_push_many`] method
    /// because if it is implemented not as lock-free, it should have better performance.
    ///
    /// # Safety
    ///
    /// If the `T` is not `Copy`, the caller must [`forget`](mem::forget) the provided slice.
    unsafe fn lock_free_maybe_push_many(&self, slice: &[T]) -> Result<(), LockFreePushManyErr>;

    /// Pushes a value only if the queue is not full.
    /// On failure, returns `Err(`[`LockFreePushErr`]`)`.
    ///
    /// It is lock-free.
    /// If you can lock, you can look at the [`Producer::maybe_push`] method
    /// because if it is implemented not as lock-free, it should have better performance.
    fn lock_free_maybe_push(&self, mut value: T) -> Result<(), LockFreePushErr<T>> {
        match unsafe { self.lock_free_maybe_push_many(&*(&raw mut value).cast::<[_; 1]>()) } {
            Ok(()) => Ok(()),
            Err(LockFreePushManyErr::NotEnoughSpace) => Err(LockFreePushErr::Full(value)),
            Err(LockFreePushManyErr::ShouldWait) => Err(LockFreePushErr::ShouldWait(value)),
        }
    }
}
