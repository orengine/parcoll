//! This module provides the [`Producer`] trait.
use crate::sync_batch_receiver::SyncBatchReceiver;
use std::mem::MaybeUninit;

/// A producer of the single-producer, multi-consumer queue.
/// It can push values and pop them.
///
/// Because it is the only producer, it can pop and push values very quickly.
pub trait Producer<T>: Send + Sync {
    /// Returns the capacity of the queue.
    fn capacity(&self) -> usize;

    /// Returns the length of the queue.
    ///
    /// It accepts a mutable reference to the self to forbit using this method from multiple threads.
    fn len(&mut self) -> usize;

    /// Returns whether the queue is empty.
    ///
    /// It accepts a mutable reference to the self to forbit using this method from multiple threads.
    #[inline]
    fn is_empty(&mut self) -> bool {
        self.len() == 0
    }

    /// Returns the number of free slots in the queue.
    ///
    /// It accepts a mutable reference to the self to forbit using this method from multiple threads.
    #[inline]
    fn free_slots(&mut self) -> usize {
        self.capacity() - self.len()
    }

    /// Pushes a value into the queue. If the queue is full, up to half of the queue values
    /// are pushed into the global queue (or any other [`SyncBatchReceiver`]).
    ///
    /// It accepts a mutable reference to the self to forbit using this method from multiple threads.
    fn push<SBR: SyncBatchReceiver<T>>(&mut self, value: T, sync_batch_receiver: &SBR);

    /// Pushes a value only if the queue is not full.
    /// It returns an error if the queue is full.
    ///
    /// It accepts a mutable reference to the self to forbit using this method from multiple threads.
    fn maybe_push(&mut self, value: T) -> Result<(), T>;

    /// Pops a value from the queue.
    ///
    /// It accepts a mutable reference to the self to forbit using this method from multiple threads.
    fn pop(&mut self) -> Option<T>;

    /// Pops multiple values from the queue and returns the number of read values.
    ///
    /// It accepts a mutable reference to the self to forbit using this method from multiple threads.
    fn pop_many(&mut self, dst: &mut [MaybeUninit<T>]) -> usize;

    /// Pushes multiple values into the queue.
    /// It accepts two slices to allow using it for ring-based queues.
    ///
    /// It accepts a mutable reference to the self to forbit using this method from multiple threads.
    ///
    /// # Safety
    ///
    /// If the `T` is not `Copy`, the caller must [`forget`](core::mem::forget) the both slices;
    /// It should be called only when the `Producer` has space for the values.
    /// It doesn't check if it has space and expected that the caller does.
    unsafe fn push_many_unchecked(&mut self, first: &[T], last: &[T]);

    /// Pushes multiple values into the queue or returns an error if the queue doesn't have enough space.
    fn maybe_push_many(&mut self, slice: &[T]) -> Result<(), ()>;

    /// Pushes a slice of value into the queue.
    /// If the queue doesn't have enough space, up to half of the queue values
    /// are pushed into the global queue (or any other [`SyncBatchReceiver`]).
    ///
    /// It accepts a mutable reference to the self to forbit using this method from multiple threads.
    fn push_many<SBR: SyncBatchReceiver<T>>(&mut self, values: &[T], sync_batch_receiver: &SBR);
}
