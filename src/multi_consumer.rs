//! This module provides the [`MultiConsumer`], the [`MultiConsumerSpawner`],
//! the [`MultiLockFreeConsumerSpawner`] and the [`MultiLockFreeConsumer`] traits for
//! multi-consumer queues.

use crate::consumer::LockFreeConsumer;
use crate::Consumer;

/// A consumer of a multi-consumer queue.
///
/// If the queue is spmc, the [`SPMCProducer`](crate::spmc_producer::SPMCProducer)
/// read methods work faster, and you should use them when
/// it is possible.
pub trait MultiConsumer<T>: Consumer<T> + Clone {}

/// A lock-free consumer of a single-consumer queue.
///
/// If the queue is spmc, the [`SPMCProducer`](crate::spmc_producer::SPMCProducer)
/// read methods work faster, and you should use them when
/// it is possible.
pub trait MultiLockFreeConsumer<T>: MultiConsumer<T> + LockFreeConsumer<T> + Clone {}

/// A spawner of consumers for a multi-consumer queue.
pub trait MultiConsumerSpawner<T> {
    /// Spawns a [`MultiConsumer`].
    fn spawn_multi_consumer(&self) -> impl MultiConsumer<T>;
}
/// A spawner of lock-free consumer for a single-consumer queue.
pub trait MultiLockFreeConsumerSpawner<T> {
    /// Spawns a [`MultiLockFreeConsumer`].
    fn spawn_multi_lock_free_consumer(&self) -> impl MultiLockFreeConsumer<T>;
}
