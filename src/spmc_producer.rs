use std::mem::MaybeUninit;
use crate::single_producer::{SingleLockFreeProducer, SingleProducer};

/// A producer of a single-producer, multi-consumer queue.
/// 
/// It can push values and pop them.
/// `SPMCProuder's` pop methods are faster than consumers' pop methods. 
pub trait SPMCProducer<T>: SingleProducer<T> {
    /// Pops a value from the queue.
    /// 
    /// It may be non-lock-free.
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

    /// Pops multiple values from the queue and returns the number of read values.
    /// 
    /// It may be non-lock-free.
    fn pop_many(&self, dst: &mut [MaybeUninit<T>]) -> usize;
}

/// A lock-free of a single-producer, multi-consumer queue.
/// 
/// It can push values and pop them.
/// `SPMCProuder's` pop methods are faster than consumers' pop methods. 
pub trait SPMCLockFreeProducer<T>: SingleProducer<T> + SingleLockFreeProducer<T> {
    /// Pops a value from the queue.
    ///
    /// It is lock-free.
    /// If you can lock, you can look at the [`SPMCProducer::maybe_push_many`] method
    /// because if it is implemented not as lock-free, it should have better performance.
    fn lock_free_pop(&self) -> Option<T>;

    /// Pops multiple values from the queue and returns the number of read values.
    ///
    /// It is lock-free.
    /// If you can lock, you can look at the [`SPMCProducer::maybe_push_many`] method
    /// because if it is implemented not as lock-free, it should have better performance.
    fn lock_free_pop_many(&self, dst: &mut [MaybeUninit<T>]) -> usize;
}