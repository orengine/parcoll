//! This module provides the [`Consumer`] and the [`LockFreeConsumer`] traits.
use orengine_utils::hints::{unreachable_hint, unwrap_or_bug_hint};
use orengine_utils::hints::unlikely;
use crate::single_producer::{SingleLockFreeProducer, SingleProducer};
use crate::LockFreePopErr;
use std::mem::MaybeUninit;

/// A consumer of a queue.
pub trait Consumer<T> {
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
    ///
    /// It may be non-lock-free.
    fn pop_many(&self, dst: &mut [MaybeUninit<T>]) -> usize;

    /// Pops a value from the queue and returns it.
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

    /// Steals some values from the consumer and places them into `dst`.
    /// Returns the number of stolen values.
    ///
    /// It requires that the other queue to be empty.
    /// Expected to steal the half of the queue,
    /// but other implementations may steal another number of values.
    ///
    /// It may be non-lock-free.
    ///
    /// # Panics
    ///
    /// Panics if the other queue is not empty.
    fn steal_into(&self, dst: &impl SingleProducer<T>) -> usize {
        debug_assert!(
            dst.is_empty(),
            "steal_into requires the other queue to be empty"
        );

        let max_stolen = self.len() / 2;
        if !cfg!(feature = "always_steal") && max_stolen < 4 || max_stolen == 0 {
            // we don't steal less than 4 by default
            // because else we may lose more because of cache locality and NUMA awareness
            return 0;
        }

        let mut stolen = 0;
        let dst_capacity = dst.capacity();

        while stolen < max_stolen && stolen < dst_capacity {
            if let Some(item) = self.pop() {
                unwrap_or_bug_hint(dst.maybe_push(item));

                stolen += 1;
            } else {
                break;
            }
        }

        stolen
    }
}

/// A lock-free consumer of a queue.
pub trait LockFreeConsumer<T>: Consumer<T> {
    /// Pops many values from the queue.
    /// Returns the number of popped values and whether the operation failed
    /// because it should wait.
    ///
    /// It is lock-free.
    /// If you can lock, you can look at the [`Consumer::pop_many`] method
    /// because if it is implemented not as lock-free, it should have better performance.
    fn lock_free_pop_many(&self, dst: &mut [MaybeUninit<T>]) -> (usize, bool);

    /// Pops a value from the queue. On failure, returns <code>Err([`LockFreePopErr`])</code>.
    ///
    /// It is lock-free.
    /// If you can lock, you can look at the [`Consumer::pop`] method
    /// because if it is implemented not as lock-free, it should have better performance.
    fn lock_free_pop(&self) -> Result<T, LockFreePopErr> {
        let mut uninit_item = MaybeUninit::uninit();
        let (n, should_wait) =
            self.lock_free_pop_many(unsafe { &mut *(&raw mut uninit_item).cast::<[_; 1]>() });

        if unlikely(should_wait) {
            return Err(LockFreePopErr::ShouldWait);
        }

        if n == 1 {
            Ok(unsafe { uninit_item.assume_init() })
        } else {
            debug_assert_eq!(
                n, 0,
                "lock_free_pop_many returned more than one value for [T; 1]"
            );

            Err(LockFreePopErr::Empty)
        }
    }

    /// Steals some values from the consumer and places them into `dst`.
    /// Returns the number of stolen values and whether the operation failed
    /// because it should wait.
    ///
    /// It requires that the other queue to be empty.
    /// Expected to steal the half of the queue,
    /// but other implementations may steal another number of values.
    ///
    /// It is lock-free.
    /// If you can lock, you can look at the [`Consumer::steal_into`] method
    /// because if it is implemented not as lock-free, it should have better performance.
    ///
    /// # Panics
    ///
    /// Panics if the other queue is not empty.
    fn lock_free_steal_into(&self, dst: &impl SingleLockFreeProducer<T>) -> (usize, bool) {
        debug_assert!(
            dst.is_empty(),
            "steal_into requires the other queue to be empty"
        );

        let max_stolen = self.len() / 2;
        if !cfg!(feature = "always_steal") && max_stolen < 4 || max_stolen == 0 {
            // we don't steal less than 4 by default
            // because else we may lose more because of cache locality and NUMA awareness
            return (0, false);
        }

        let mut stolen = 0;
        let dst_capacity = dst.capacity();

        while stolen < max_stolen && stolen < dst_capacity {
            match self.lock_free_pop() {
                Ok(item) => {
                    unwrap_or_bug_hint(dst.lock_free_maybe_push(item));

                    stolen += 1;
                }
                Err(LockFreePopErr::Empty) => return (stolen, false),
                Err(LockFreePopErr::ShouldWait) => return (stolen, true),
            }
        }

        unreachable_hint()
    }
}
