//! Generic traits for queue benchmarking.

use parcoll::{spmc, Consumer, LightArc, Producer};
use st3;
use parcoll::multi_consumer::MultiConsumerSpawner;

/// Error returned on stealing failure.
pub enum GenericStealError {
    Empty,
    Busy,
}

/// Generic interface for a queue worker.
pub trait GenericWorker<T>: Send {
    fn new() -> Self;
    fn push(&self, item: T) -> Result<(), T>;
    fn pop(&self) -> Option<T>;
    fn stealer(&self) -> impl GenericStealer<T, W = Self>;
}

/// Generic interface for a queue stealer.
pub trait GenericStealer<T>: Clone + Send {
    type W: GenericWorker<T>;

    fn steal_batch(&self, worker: &Self::W) -> Result<(), GenericStealError>;
}

// region st3

/// Generic work-stealing queue traits implementation for St3 (LIFO).
impl<T: Send> GenericWorker<T> for st3::lifo::Worker<T> {
    fn new() -> Self {
        Self::new(256)
    }

    fn push(&self, item: T) -> Result<(), T> {
        self.push(item)
    }

    fn pop(&self) -> Option<T> {
        self.pop()
    }

    fn stealer(&self) -> impl GenericStealer<T, W = Self> {
        self.stealer()
    }
}
impl<T: Send> GenericStealer<T> for st3::lifo::Stealer<T> {
    type W = st3::lifo::Worker<T>;

    fn steal_batch(&self, worker: &Self::W) -> Result<(), GenericStealError> {
        // The maximum number of tasks to be stolen is limited in order to match
        // the behavior of `crossbeam-dequeue`.
        const MAX_BATCH_SIZE: usize = 32;

        self.steal_and_pop(worker, |n| (n - n / 2).min(MAX_BATCH_SIZE))
            .map(|_| ())
            .map_err(|e| match e {
                st3::StealError::Empty => GenericStealError::Empty,
                st3::StealError::Busy => GenericStealError::Busy,
            })
    }
}

/// Generic work-stealing queue traits implementation for St3 (FIFO).
impl<T: Send> GenericWorker<T> for st3::fifo::Worker<T> {
    fn new() -> Self {
        Self::new(256)
    }

    fn push(&self, item: T) -> Result<(), T> {
        self.push(item)
    }

    fn pop(&self) -> Option<T> {
        self.pop()
    }

    fn stealer(&self) -> impl GenericStealer<T, W = Self> {
        self.stealer()
    }
}
impl<T: Send> GenericStealer<T> for st3::fifo::Stealer<T> {
    type W = st3::fifo::Worker<T>;

    fn steal_batch(&self, worker: &Self::W) -> Result<(), GenericStealError> {
        // The maximum number of tasks to be stolen is limited in order to match
        // the behavior of `crossbeam-dequeue`.
        const MAX_BATCH_SIZE: usize = 32;

        self.steal_and_pop(worker, |n| (n - n / 2).min(MAX_BATCH_SIZE))
            .map(|_| ())
            .map_err(|e| match e {
                st3::StealError::Empty => GenericStealError::Empty,
                st3::StealError::Busy => GenericStealError::Busy,
            })
    }
}

// endregion

// region crossbeam

/// New types distinguishing between FIFO and LIFO crossbeam queues.
pub struct CrossbeamFifoWorker<T>(crossbeam_deque::Worker<T>);
pub struct CrossbeamFifoStealer<T>(crossbeam_deque::Stealer<T>);
pub struct CrossbeamLifoWorker<T>(crossbeam_deque::Worker<T>);
pub struct CrossbeamLifoStealer<T>(crossbeam_deque::Stealer<T>);

/// Generic work-stealing queue traits implementation for crossbeam-deque (FIFO).
impl<T: Send> GenericWorker<T> for CrossbeamFifoWorker<T> {
    fn new() -> Self {
        Self(crossbeam_deque::Worker::new_fifo())
    }

    fn push(&self, item: T) -> Result<(), T> {
        self.0.push(item);

        Ok(())
    }

    fn pop(&self) -> Option<T> {
        self.0.pop()
    }

    fn stealer(&self) -> impl GenericStealer<T, W = Self> {
        CrossbeamFifoStealer(self.0.stealer())
    }
}
impl<T> Clone for CrossbeamFifoStealer<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T: Send> GenericStealer<T> for CrossbeamFifoStealer<T> {
    type W = CrossbeamFifoWorker<T>;

    fn steal_batch(&self, worker: &Self::W) -> Result<(), GenericStealError> {
        match self.0.steal_batch_and_pop(&worker.0) {
            crossbeam_deque::Steal::Empty => Err(GenericStealError::Empty),
            crossbeam_deque::Steal::Retry => Err(GenericStealError::Busy),
            crossbeam_deque::Steal::Success(_) => Ok(()),
        }
    }
}

/// Generic work-stealing queue traits implementation for crossbeam-deque (LIFO).
impl<T: Send> GenericWorker<T> for CrossbeamLifoWorker<T> {
    fn new() -> Self {
        Self(crossbeam_deque::Worker::new_lifo())
    }

    fn push(&self, item: T) -> Result<(), T> {
        self.0.push(item);

        Ok(())
    }

    fn pop(&self) -> Option<T> {
        self.0.pop()
    }

    fn stealer(&self) -> impl GenericStealer<T, W = Self> {
        CrossbeamLifoStealer(self.0.stealer())
    }
}
impl<T> Clone for CrossbeamLifoStealer<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T: Send> GenericStealer<T> for CrossbeamLifoStealer<T> {
    type W = CrossbeamLifoWorker<T>;

    fn steal_batch(&self, worker: &Self::W) -> Result<(), GenericStealError> {
        match self.0.steal_batch_and_pop(&worker.0) {
            crossbeam_deque::Steal::Empty => Err(GenericStealError::Empty),
            crossbeam_deque::Steal::Retry => Err(GenericStealError::Busy),
            crossbeam_deque::Steal::Success(_) => Ok(()),
        }
    }
}

impl<T: Send> GenericWorker<T> for LightArc<crossbeam_queue::ArrayQueue<T>> {
    fn new() -> Self {
        LightArc::new(crossbeam_queue::ArrayQueue::new(256))
    }

    fn push(&self, item: T) -> Result<(), T> {
        crossbeam_queue::ArrayQueue::push(self, item)
    }

    fn pop(&self) -> Option<T> {
        crossbeam_queue::ArrayQueue::pop(self)
    }

    fn stealer(&self) -> impl GenericStealer<T, W = Self> {
        self.clone()
    }
}

impl<T: Send> GenericStealer<T> for LightArc<crossbeam_queue::ArrayQueue<T>> {
    type W = LightArc<crossbeam_queue::ArrayQueue<T>>;

    fn steal_batch(&self, worker: &Self::W) -> Result<(), GenericStealError> {
        let src_len = crossbeam_queue::ArrayQueue::len(self);

        if src_len == 0 {
            return Err(GenericStealError::Empty);
        }

        let dst_vacant = crossbeam_queue::ArrayQueue::capacity(worker)
            - crossbeam_queue::ArrayQueue::len(worker);

        for _ in 0..(src_len - src_len / 2).min(dst_vacant) {
            let _ = worker.push(crossbeam_queue::ArrayQueue::pop(self).unwrap());
        }

        Ok(())
    }
}

impl<T: Send> GenericWorker<T> for LightArc<crossbeam_queue::SegQueue<T>> {
    fn new() -> Self {
        LightArc::new(crossbeam_queue::SegQueue::new())
    }

    fn push(&self, item: T) -> Result<(), T> {
        crossbeam_queue::SegQueue::push(self, item);

        Ok(())
    }

    fn pop(&self) -> Option<T> {
        crossbeam_queue::SegQueue::pop(self)
    }

    fn stealer(&self) -> impl GenericStealer<T, W = Self> {
        self.clone()
    }
}

impl<T: Send> GenericStealer<T> for LightArc<crossbeam_queue::SegQueue<T>> {
    type W = LightArc<crossbeam_queue::SegQueue<T>>;

    fn steal_batch(&self, worker: &Self::W) -> Result<(), GenericStealError> {
        let src_len = crossbeam_queue::SegQueue::len(self);

        if src_len == 0 {
            return Err(GenericStealError::Empty);
        }

        for _ in 0..src_len - src_len / 2 {
            let _ = worker.push(crossbeam_queue::SegQueue::pop(self).unwrap());
        }

        Ok(())
    }
}

// endregion

// region parcoll

impl<T: Send, const CAPACITY: usize> GenericWorker<T>
    for spmc::CachePaddedSPMCProducer<T, CAPACITY>
{
    fn new() -> Self {
        let (producer, _) = spmc::new_cache_padded_bounded();

        producer
    }

    fn push(&self, item: T) -> Result<(), T> {
        parcoll::Producer::maybe_push(self, item)
    }

    fn pop(&self) -> Option<T> {
        parcoll::spmc_producer::SPMCProducer::pop(self)
    }

    fn stealer(&self) -> impl GenericStealer<T, W = Self> {
        self.spawn_multi_consumer()
    }
}

impl<T: Send, const CAPACITY: usize> GenericStealer<T>
    for spmc::CachePaddedSPMCConsumer<T, CAPACITY>
{
    type W = spmc::CachePaddedSPMCProducer<T, CAPACITY>;

    fn steal_batch(&self, worker: &Self::W) -> Result<(), GenericStealError> {
        let n = self.steal_into(worker);

        if n != 0 {
            Ok(())
        } else {
            Err(GenericStealError::Empty)
        }
    }
}

impl<T: Send> GenericWorker<T> for spmc::CachePaddedSPMCUnboundedProducer<T> {
    fn new() -> Self {
        let (producer, _) = spmc::new_cache_padded_unbounded();

        producer
    }

    fn push(&self, item: T) -> Result<(), T> {
        self.maybe_push(item)
    }

    fn pop(&self) -> Option<T> {
        parcoll::spmc_producer::SPMCProducer::pop(self)
    }

    fn stealer(&self) -> impl GenericStealer<T, W = Self> {
        parcoll::multi_consumer::MultiConsumerSpawner::spawn_multi_consumer(self)
    }
}

impl<T: Send> GenericStealer<T> for spmc::CachePaddedSPMCUnboundedConsumer<T> {
    type W = spmc::CachePaddedSPMCUnboundedProducer<T>;

    fn steal_batch(&self, worker: &Self::W) -> Result<(), GenericStealError> {
        let n = self.steal_into(worker);

        if n != 0 {
            Ok(())
        } else {
            Err(GenericStealError::Empty) // But we can't be sure
        }
    }
}

// endregion
