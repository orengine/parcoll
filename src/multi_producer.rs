//! This module provides the [`MultiProducer`] and the [`MultiLockFreeProducer`] traits.

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
