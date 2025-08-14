//! This module provides the [`BatchReceiver`] and the [`LockFreeBatchReceiver`] traits.

use std::mem;

/// A batch receiver of the multi-consumer queue.
/// It is used to move half of the values from the queue to this receiver on overflow.
///
/// This library provides the [`MutexVecQueue`](crate::MutexVecQueue) that implements this trait.
pub trait BatchReceiver<T> {
    /// Pushes a batch of values to the receiver.
    ///
    /// It first pushes the first slice, then the last slice and finally the `slice`.
    ///
    /// It has such an interesting signature because it can be used in ring-based queues.
    ///
    /// It may be non-lock-free.
    ///
    /// # Safety
    ///
    /// If `T` is not [`Copy`], the caller should [`forget`](mem::forget) the provided slices.
    unsafe fn push_many_and_slice(&self, first: &[T], last: &[T], slice: &[T]);

    /// Pushes a batch of values to the receiver.
    ///
    /// It first pushes the first slice, then the last slice and finally the `value`.
    ///
    /// It has such an interesting signature because it can be used in ring-based queues.
    ///
    /// It may be non-lock-free.
    ///
    /// # Safety
    ///
    /// If `T` is not [`Copy`], the caller should [`forget`](mem::forget) the provided slices
    /// and the provided value.
    unsafe fn push_many_and_one(&self, first: &[T], last: &[T], value: T) {
        unsafe { self.push_many_and_slice(first, last, &[value]); }
    }
}

/// A lock-free batch receiver of the multi-consumer queue.
/// It is used to move half of the values from the queue to this receiver on overflow.
///
/// This library provides the [`MutexVecQueue`](crate::MutexVecQueue) that implements this trait.
pub trait LockFreeBatchReceiver<T> {
    /// Pushes a batch of values to the receiver.
    /// It returns an error if the operation should wait.
    ///
    /// It first pushes the first slice, then the last slice and finally the `slice`.
    ///
    /// It has such an interesting signature because it can be used in ring-based queues.
    ///
    /// It is lock-free.
    ///
    /// # Safety
    ///
    /// If `T` is not [`Copy`] and the operation succeed,
    /// the caller should [`forget`](mem::forget) the provided slices.
    unsafe fn lock_free_push_many_and_slice(&self, first: &[T], last: &[T], slice: &[T]) -> Result<(), ()>;

    /// Pushes a batch of values to the receiver.
    /// It returns an error if the operation should wait.
    ///
    /// It first pushes the first slice, then the last slice and finally the `value`.
    ///
    /// It has such an interesting signature because it can be used in ring-based queues.
    ///
    /// It is lock-free.
    ///
    /// # Safety
    ///
    /// If `T` is not [`Copy`] and the operation succeed, the caller should [`forget`](mem::forget)
    /// the provided slices and the provided value.
    unsafe fn push_many_and_one(&self, first: &[T], last: &[T], value: T) -> Result<(), T> {
        let res = unsafe {
            self.lock_free_push_many_and_slice(first, last, &*(&raw mut value).cast::<[_; 1]>())
        };

        match res {
            Ok(()) => Ok(mem::forget(value)),
            Err(()) => Err(value),
        }
    }
}