//! This module provides the [`Consumer`] trait for the single-producer, multi-consumer queue.
use crate::spmc::Producer;
use std::mem::MaybeUninit;

/// A consumer of the single-producer, multi-consumer queue.
/// It can pop values and be cloned.
///
/// [`Producers`](Producer) doesn't implement this trait but also can pop values
/// and even do it faster.
pub trait Consumer<T>: Clone {
    /// Returns the capacity of the queue.
    fn capacity(&self) -> usize;

    /// Returns the length of the queue.
    fn len(&self) -> usize;

    /// Returns whether the queue is empty.
    #[inline]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Pops many values from the queue and returns the number of read values.
    fn pop_many(&self, dst: &mut [MaybeUninit<T>]) -> usize;

    /// Pops a value from the queue and returns it.
    fn pop(&self) -> Option<T> {
        let mut uninit_item = MaybeUninit::uninit();
        let n = self.pop_many(unsafe { &mut *(&raw mut uninit_item).cast::<[_; 1]>() });

        if n == 1 {
            Some(unsafe { uninit_item.assume_init() })
        } else {
            debug_assert_eq!(n, 0, "pop_many returned more than one value for [T; 1]");

            None
        }
    }

    /// Steals some values from the consumer and places them into `dst`.
    /// Returns the number of stolen values.
    ///
    /// It requires that the other queue to be empty.
    /// Expected to steal the half of the queue,
    /// but other implementations may steal another number of values.
    fn steal_into(&self, dst: &impl Producer<T>) -> usize;
}
