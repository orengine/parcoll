//! This module provides the [`Consumer`] trait.
use crate::spmc::Producer;
use std::mem::MaybeUninit;

/// A consumer of the single-producer, multi-consumer queue.
/// It can pop values and be cloned.
/// 
/// [`Producers`](Producer) doesn't implement this trait but also can pop values
/// and even do it faster.
pub trait Consumer<T> {
    /// An associated [`producer`](Producer) with this consumer.
    type AssociatedProducer: Producer<T>;

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

    /// Steals some values from the consumer and places them into `dst`.
    ///
    /// It requires that the other queue to be empty.
    /// Expected to steal the half of the queue,
    /// but other implementations may steal another number of values.
    ///
    /// It accepts a mutable reference to the producer that guarantees exclusivity.
    fn steal_into(&self, dst: &mut Self::AssociatedProducer) -> usize;
}
