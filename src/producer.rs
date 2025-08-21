//! Provides the [`Producer`] and the [`LockFreeProducer`] traits.
use crate::LockFreePushErr;

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

    /// Pushes a value only if the queue is not full.
    /// It returns an error if the queue is full.
    ///
    /// It may be non-lock-free.
    fn maybe_push(&self, value: T) -> Result<(), T>;
}

/// A lock-free producer of a queue.
pub trait LockFreeProducer<T>: Producer<T> {
    /// Pushes a value only if the queue is not full.
    /// On failure, returns <code>Err([LockFreePushErr])</code>.
    ///
    /// It is lock-free.
    /// If you can lock, you can look at the [`Producer::maybe_push`] method
    /// because if it is implemented not as lock-free, it should have better performance.
    fn lock_free_maybe_push(&self, value: T) -> Result<(), LockFreePushErr<T>>;
}
