//! This module provides the [`Producer`] trait for the single-producer, single-consumer queue.

/// A producer of the single-producer, single-consumer queue.
///
/// Because it is the only producer, it can push values very quickly.
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

    /// Pushes a value only if the queue is not full.
    /// It returns an error if the queue is full.
    fn maybe_push(&self, value: T) -> Result<(), T>;

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
    /// This method is low lovel,
    /// and is used for [`Consumer::steal_into`](spsc::Consumer::steal_into).
    ///
    /// # Why it first copies the values and then commits them?
    ///
    /// Because it is better for performance to optimistically copy the values,
    /// and only then use a CAS operation.
    /// It is possible because it is single-producer, multi-consumer queue,
    /// so we can read (no other writers and the concurrent read operation is allowed).
    /// If the CAS operation fails (provided function returns an error),
    /// then this method don't commit the values.
    ///
    /// # Safety
    ///
    /// If the `T` is not `Copy`, the caller must [`forget`](core::mem::forget) the both slices.
    /// The [`Producer`] must have space for the values.
    ///
    /// # Panics
    ///
    /// If the [`Producer`] doesn't have enough space to copy the values.
    unsafe fn copy_and_commit_if<F, FSuccess, FError>(&self, right: &[T], left: &[T], f: F) -> Result<FSuccess, FError>
    where 
        F: FnOnce() -> Result<FSuccess, FError>;
}
