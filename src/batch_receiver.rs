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
        unsafe {
            self.push_many_and_slice(first, last, &[value]);
        }
    }
}

/// An error type
/// returned by the [`LockFreeBatchReceiver::lock_free_push_many_and_slice_and_commit_if`]
/// and by the [`BatchReceiver::push_many_and_slice_and_commit_if`].
///
/// It may be [`LockFreePushBatchErr::ShouldWait`] or [`LockFreePushBatchErr::CondictionIsFalse`].
pub enum LockFreePushBatchErr<T, U> {
    /// The operation should wait.
    ShouldWait(T),
    /// Provided condition is false, not committed.
    CondictionIsFalse((T, U)),
}

/// A lock-free batch receiver of the multi-consumer queue.
/// It is used to move half of the values from the queue to this receiver on overflow.
///
/// This library provides the [`MutexVecQueue`](crate::MutexVecQueue) that implements this trait.
pub trait LockFreeBatchReceiver<T> {
    /// Pushes a batch of values to the receiver but commits it only if the function returns `true`.
    ///
    /// It first pushes the first slice, then the last slice and finally the `slice`.
    ///
    /// It has such an interesting signature because it can be used in ring-based queues.
    ///
    /// It is lock-free.
    ///
    /// # Safety
    ///
    /// If `T` is not [`Copy`] and the operation succeeds,
    /// the caller should [`forget`](mem::forget) the provided slices.
    unsafe fn lock_free_push_many_and_slice_and_commit_if<F, FSuccess, FError>(
        &self,
        first: &[T],
        last: &[T],
        slice: &[T],
        f: F,
    ) -> Result<FSuccess, LockFreePushBatchErr<(), FError>>
    where
        F: FnOnce() -> Result<FSuccess, FError>;

    /// Pushes a batch of values to the receiver but commits it only if the function returns `true`.
    ///
    /// It first pushes the first slice, then the last slice and finally the `value`.
    ///
    /// It has such an interesting signature because it can be used in ring-based queues.
    ///
    /// It is lock-free.
    ///
    /// # Safety
    ///
    /// If `T` is not [`Copy`] and the operation succeeds,
    /// the caller should [`forget`](mem::forget)
    /// the provided slices and the provided value.
    unsafe fn push_many_and_one_and_commit_if<F, FSuccess, FError>(
        &self,
        first: &[T],
        last: &[T],
        mut value: T,
        f: F,
    ) -> Result<FSuccess, LockFreePushBatchErr<T, FError>>
    where
        F: FnOnce() -> Result<FSuccess, FError>,
    {
        let res = unsafe {
            self.lock_free_push_many_and_slice_and_commit_if(
                first,
                last,
                &*(&raw mut value).cast::<[_; 1]>(),
                f,
            )
        };

        match res {
            Ok(res) => {
                mem::forget(value);

                Ok(res)
            }
            Err(LockFreePushBatchErr::ShouldWait(())) => {
                Err(LockFreePushBatchErr::ShouldWait(value))
            }
            Err(LockFreePushBatchErr::CondictionIsFalse(((), err))) => {
                Err(LockFreePushBatchErr::CondictionIsFalse((value, err)))
            }
        }
    }
}
