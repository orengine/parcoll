//! This module provides the [`SingleConsumer`], the [`SingleConsumerSpawner`],
//! the [`SingleLockFreeConsumerSpawner`] and the [`SingleLockFreeConsumer`] traits for
//! single-consumer queues.

use crate::consumer::Consumer;

/// A consumer of a single-consumer queue.
///
/// Because it is the only consumer, it can push and pop values very quickly.
pub trait SingleConsumer<T>: Consumer<T> {}

/// A lock-free consumer of a single-consumer queue.
///
/// Because it is the only consumer, it can push and pop values very quickly.
pub trait SingleLockFreeConsumer<T>: SingleConsumer<T> {}

/// A spawner of consumer for a single-consumer queue.
pub trait SingleConsumerSpawner<T> {
    /// Spawns a [`SingleConsumer`].
    fn spawn_single_consumer(&self) -> impl SingleConsumer<T>;
}

/// A spawner of lock-free consumer for a single-consumer queue.
pub trait SingleLockFreeConsumerSpawner<T> {
    /// Spawns a [`SingleLockFreeConsumer`].
    fn spawn_single_lock_free_consumer(&self) -> impl SingleLockFreeConsumer<T>;
}
