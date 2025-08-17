use crate::batch_receiver::LockFreeBatchReceiver;
use crate::hints::unlikely;
use crate::single_producer::{SingleLockFreeProducer, SingleProducer};
use crate::{BatchReceiver, LockFreePopErr};
use std::mem;
use std::mem::MaybeUninit;

/// A producer of a single-producer, multi-consumer queue.
///
/// It can push values and pop them.
/// `SPMCProuder's` pop methods are faster than consumers' pop methods.
pub trait SPMCProducer<T>: SingleProducer<T> {
    /// Pushes many values to the queue or to the provided [`BatchReceiver`].
    ///
    /// Likely it is implemented to move a half of the values from the queue to the [`BatchReceiver`]
    /// on overflow.
    ///
    /// It may be non-lock-free.
    ///
    /// # Safety
    ///
    /// If the `T` is not `Copy`, the caller must [`forget`](mem::forget) the provided slice.
    unsafe fn push_many<BR: BatchReceiver<T>>(&self, slice: &[T], batch_receiver: &BR);

    /// Pushes a value to the queue or to the provided [`BatchReceiver`].
    ///
    /// Likely it is implemented to move a half of the values from the queue to the [`BatchReceiver`]
    /// on overflow.
    ///
    /// It may be non-lock-free.
    #[inline]
    fn push<BR: BatchReceiver<T>>(&self, mut value: T, batch_receiver: &BR) {
        unsafe { self.push_many(&*(&raw mut value).cast::<[_; 1]>(), batch_receiver) };

        mem::forget(value);
    }

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
    /// Pushes many values to the queue or to the provided [`BatchReceiver`].
    /// Returns an error if the operation failed because it should wait.
    ///
    /// Likely it is implemented
    /// to move a half of the values from the queue to the [`BatchReceiver`]
    /// on overflow.
    ///
    /// It is lock-free.
    ///
    /// If you can lock, you can look at the [`SPMCProducer::push_many`] method
    /// because if it is implemented not as lock-free, it should have better performance.
    ///
    /// # Safety
    ///
    /// If the `T` is not `Copy`, the caller must [`forget`](mem::forget) the provided slice.
    unsafe fn lock_free_push_many<BR: LockFreeBatchReceiver<T>>(
        &self,
        slice: &[T],
        batch_receiver: &BR,
    ) -> Result<(), ()>;

    /// Pushes a value to the queue or to the provided [`BatchReceiver`].
    /// It fails only if the operation should wait.
    ///
    /// Likely it is implemented
    /// to move a half of the values from the queue to the [`BatchReceiver`]
    /// on overflow.
    ///
    /// It is lock-free.
    ///
    /// If you can lock, you can look at the [`SPMCProducer::push`] method
    /// because if it is implemented not as lock-free, it should have better performance.
    #[inline]
    fn lock_free_push<BR: LockFreeBatchReceiver<T>>(
        &self,
        mut value: T,
        batch_receiver: &BR,
    ) -> Result<(), T> {
        let res = unsafe {
            self.lock_free_push_many(&*(&raw mut value).cast::<[_; 1]>(), batch_receiver)
        };

        match res {
            Ok(()) => Ok(mem::forget(value)),
            Err(()) => Err(value),
        }
    }

    /// Pops multiple values from the queue and returns the number of read values.
    /// Returns the number of popped values and whether the operation failed
    /// because it should wait.
    ///
    /// It is lock-free.
    ///
    /// If you can lock, you can look at the [`SPMCProducer::pop_many`] method
    /// because if it is implemented not as lock-free, it should have better performance.
    fn lock_free_pop_many(&self, dst: &mut [MaybeUninit<T>]) -> (usize, bool);

    /// Pops a value from the queue.
    /// On success, it returns the value.
    /// On failure, it returns [`LockFreePopErr`].
    ///
    /// It is lock-free.
    ///
    /// If you can lock, you can look at the [`SPMCProducer::pop`] method
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
}
