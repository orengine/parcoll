//! This module provides the [`MutexVecQueue`] and the [`VecQueue`].
use orengine_utils::light_arc::LightArc;
use crate::batch_receiver::{BatchReceiver, LockFreeBatchReceiver, LockFreePushBatchErr};
use crate::loom_bindings::sync::Mutex;
use crate::single_producer::SingleProducer;
use crate::VecQueue;

/// A wrapper around `LightArc<Mutex<VecQueue<T>>>`.
/// It can be used as a global queue.
///
/// It implements [`BatchReceiver`] and [`LockFreeBatchReceiver`] with returning an error when it should wait.
#[derive(Clone)]
pub struct MutexVecQueue<T> {
    inner: LightArc<Mutex<VecQueue<T>>>,
}

impl<T> MutexVecQueue<T> {
    /// Creates a new `MutexVecQueue`.
    pub fn new() -> Self {
        Self {
            inner: LightArc::new(Mutex::new(VecQueue::new())),
        }
    }

    /// Returns the number of elements in the queue.
    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    /// Returns whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }

    /// Pushes a value to the queue.
    pub fn push(&self, task: T) {
        self.inner.lock().push(task);
    }

    /// Moves at most `limit` elements from the queue to the producer.
    pub fn move_batch_to_producer(&self, producer: &impl SingleProducer<T>, limit: usize) {
        unsafe {
            self.inner.lock().take_batch(|slice1, slice2| {
                producer.push_many_unchecked(slice1, slice2);
            }, limit);
        };
    }

    /// Pushes two slices and a value to the queue.
    fn push_many_and_one_with_inner(inner: &mut VecQueue<T>, first: &[T], last: &[T], value: T) {
        unsafe {
            inner.extend_from_slice(first);
            inner.extend_from_slice(last);
        }

        inner.push(value);
    }

    /// Pushes three slices to the queue.
    fn push_many_and_slice_with_inner(
        inner: &mut VecQueue<T>,
        first: &[T],
        last: &[T],
        slice: &[T],
    ) {
        unsafe {
            inner.extend_from_slice(first);
            inner.extend_from_slice(last);
            inner.extend_from_slice(slice);
        }
    }
}

impl<T> Default for MutexVecQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> BatchReceiver<T> for MutexVecQueue<T> {
    unsafe fn push_many_and_slice(&self, first: &[T], last: &[T], slice: &[T]) {
        Self::push_many_and_slice_with_inner(&mut self.inner.lock(), first, last, slice);
    }

    unsafe fn push_many_and_one(&self, first: &[T], last: &[T], value: T) {
        Self::push_many_and_one_with_inner(&mut self.inner.lock(), first, last, value);
    }
}

impl<T> LockFreeBatchReceiver<T> for MutexVecQueue<T> {
    unsafe fn lock_free_push_many_and_slice_and_commit_if<F, FSuccess, FError>(
        &self,
        first: &[T],
        last: &[T],
        slice: &[T],
        f: F,
    ) -> Result<FSuccess, LockFreePushBatchErr<(), FError>>
    where
        F: FnOnce() -> Result<FSuccess, FError>,
    {
        let Some(mut inner) = self.inner.try_lock() else {
            return Err(LockFreePushBatchErr::ShouldWait(()));
        };

        match f() {
            Ok(res) => {
                Self::push_many_and_slice_with_inner(&mut inner, first, last, slice);

                Ok(res)
            }
            Err(err) => Err(LockFreePushBatchErr::CondictionIsFalse(((), err))),
        }
    }

    unsafe fn push_many_and_one_and_commit_if<F, FSuccess, FError>(
        &self,
        first: &[T],
        last: &[T],
        value: T,
        f: F,
    ) -> Result<FSuccess, LockFreePushBatchErr<T, FError>>
    where
        F: FnOnce() -> Result<FSuccess, FError>,
    {
        let Some(mut inner) = self.inner.try_lock() else {
            return Err(LockFreePushBatchErr::ShouldWait(value));
        };

        match f() {
            Ok(res) => {
                Self::push_many_and_one_with_inner(&mut inner, first, last, value);

                Ok(res)
            }
            Err(err) => Err(LockFreePushBatchErr::CondictionIsFalse((value, err))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spmc_producer::SPMCProducer;

    const N: usize = 100_000;
    const BATCH_SIZE: usize = 10;

    #[test]
    fn test_vec_queue() {
        let mut queue = VecQueue::new_const();

        for i in 0..N {
            queue.push(i);
        }

        for i in 0..N {
            let task = queue.pop().unwrap();
            assert_eq!(task, i);
        }

        let mut queue = VecQueue::new();

        for i in 0..N / BATCH_SIZE {
            let slice = (0..BATCH_SIZE)
                .map(|j| i * BATCH_SIZE + j)
                .collect::<Vec<_>>();

            unsafe { queue.extend_from_slice(&slice) };
        }

        for i in 0..N {
            let task = queue.pop().unwrap();

            assert_eq!(task, i);
        }
    }

    #[test]
    fn test_global_queue_with_local() {
        let global_queue = MutexVecQueue::new();
        let (producer, _) = crate::spmc::new_bounded::<_, 256>();

        for i in 0..N / BATCH_SIZE {
            let slice = (0..BATCH_SIZE - 1)
                .map(|j| i * BATCH_SIZE + j)
                .collect::<Vec<_>>();
            let one_more_value = i * BATCH_SIZE + BATCH_SIZE - 1;

            unsafe {
                global_queue.push_many_and_one(
                    &slice[..2],
                    &slice[2..],
                    one_more_value
                );
            };
        }

        for i in 0..N / BATCH_SIZE {
            global_queue.move_batch_to_producer(&producer, BATCH_SIZE);

            for j in 0..BATCH_SIZE {
                let index = i * BATCH_SIZE + j;

                assert_eq!(producer.pop().unwrap(), index);
            }
        }
    }
}
