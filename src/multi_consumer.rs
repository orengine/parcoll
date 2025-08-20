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
    /// An associated [`MultiConsumer`] type.
    type SpawnedConsumer: MultiConsumer<T>;

    /// Spawns a [`MultiConsumer`].
    fn spawn_multi_consumer(&self) -> Self::SpawnedConsumer;
}
/// A spawner of lock-free consumer for a single-consumer queue.
pub trait MultiLockFreeConsumerSpawner<T> {
    /// An associated [`MultiLockFreeConsumer`] type.
    type SpawnedLockFreeConsumer: MultiLockFreeConsumer<T>;

    /// Spawns a [`MultiLockFreeConsumer`].
    fn spawn_multi_lock_free_consumer(&self) -> Self::SpawnedLockFreeConsumer;
}
