//! This module provides the [`MultiProducer`], the [`MultiProducerSpawner`], 
//! the [`MultiLockFreeProducer`] and the [`MultiLockFreeProducerSpawner`] traits.

use crate::producer::LockFreeProducer;
use crate::Producer;

/// A producer of a multi-producer queue.
///
/// Because it is the only producer, it can push values very quickly.
pub trait MultiProducer<T>: Producer<T> + Clone {}

/// A lock-free producer of a multi-producer queue.
///
/// Because it is the only producer, it can push values very quickly.
pub trait MultiLockFreeProducer<T>: MultiProducer<T> + LockFreeProducer<T> + Clone {}

/// A spawner of producers for a multi-consumer queue.
pub trait MultiProducerSpawner<T> {
    /// An associated [`MultiProducer`] type.
    type SpawnedProducer: MultiProducer<T>;

    /// Spawns a [`MultiProducer`].
    fn spawn_multi_producer(&self) -> Self::SpawnedProducer;
}
/// A spawner of lock-free producers for a single-consumer queue.
pub trait MultiLockFreeProducerSpawner<T>: MultiProducerSpawner<T> {
    /// An associated [`MultiLockFreeProducer`] type.
    type SpawnedLockFreeProducer: MultiLockFreeProducer<T>;

    /// Spawns a [`MultiLockFreeProducer`].
    fn spawn_multi_lock_free_producer(&self) -> Self::SpawnedLockFreeProducer;
}
