use crate::LockFreePushManyErr;
use crate::producer::{LockFreeProducer, Producer};

/// A producer of a single-producer queue.
///
/// Because it is the only producer, it can push values very quickly.
pub trait SingleProducer<T>: Producer<T> {
    /// Pushes multiple values into the queue.
    /// It accepts two slices to allow using it for ring-based queues.
    ///
    /// # Safety
    ///
    /// If the `T` is not `Copy`, the caller must [`forget`](core::mem::forget) the both slices;
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
    /// If the `T` is not `Copy`, the caller must [`forget`](core::mem::forget) the provided slice.
    unsafe fn maybe_push_many(&self, slice: &[T]) -> Result<(), ()>;
    
    /// Copies values, calls the provided function and commits the values if the function returns `true`.
    /// It returns an error if the function returns an error and doesn't commit the values
    /// (caller must ensure that their destructors are called).
    ///
    /// It first copies the `right` slice, next the `left` slice.
    ///
    /// This method is low level
    /// and is used for [`Consumer::steal_into`](crate::Consumer::steal_into).
    ///
    /// # Why it first copies the values and then commits them?
    ///
    /// Because it is better for performance to optimistically copy the values
    /// and only then use a CAS operation.
    /// It is possible because it is a single-producer queue,
    /// so we can read (no other writers and the concurrent read operation is allowed).
    /// If the CAS operation fails (provided function returns an error),
    /// then this method doesn't commit the values.
    ///
    /// # Safety
    ///
    /// If the `T` is not `Copy`, the caller must [`forget`](core::mem::forget) the both slices.
    /// The [`SingleProducer`] must have space for the values.
    ///
    /// # Panics
    ///
    /// If the [`SingleProducer`] doesn't have enough space to copy the values.
    unsafe fn copy_and_commit_if<F, FSuccess, FError>(
        &self,
        right: &[T],
        left: &[T],
        f: F,
    ) -> Result<FSuccess, FError>
    where
        F: FnOnce() -> Result<FSuccess, FError>;
}

/// A lock-free producer of a single-producer queue.
///
/// Because it is the only producer, it can push values very quickly.
pub trait SingleLockFreeProducer<T>: SingleProducer<T> + LockFreeProducer<T> {
    /// Pushes multiple values into the queue or returns
    /// an <code>Err([LockFreePushManyErr])</code>.
    ///
    /// It is lock-free.
    /// If you can lock, you can look at the [`SingleProducer::maybe_push_many`] method
    /// because if it is implemented not as lock-free, it should have better performance.
    ///
    /// # Safety
    ///
    /// If the `T` is not `Copy`, the caller must [`forget`](core::mem::forget) the provided slice.
    unsafe fn lock_free_maybe_push_many(&self, slice: &[T]) -> Result<(), LockFreePushManyErr>;
}
