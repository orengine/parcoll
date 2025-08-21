//! This module provides the [`MutexVecQueue`].
use crate::batch_receiver::{BatchReceiver, LockFreeBatchReceiver, LockFreePushBatchErr};
use crate::hints::{assert_hint, unlikely};
use crate::loom_bindings::sync::Mutex;
use crate::single_producer::SingleProducer;
use crate::LightArc;
use std::ptr::slice_from_raw_parts;
use std::{mem, ptr};

/// A queue that uses a vector to store the elements.
pub(crate) struct VecQueue<T> {
    ptr: *mut T,
    head: usize,
    tail: usize,
    capacity: usize,
    mask: usize,
}

impl<T> VecQueue<T> {
    /// Allocates a new vector with the given capacity.
    #[cold]
    fn allocate(capacity: usize) -> *mut T {
        debug_assert!(capacity > 0 && capacity.is_power_of_two());

        unsafe {
            std::alloc::alloc(std::alloc::Layout::array::<T>(capacity).unwrap_unchecked()).cast()
        }
    }

    /// Deallocates a vector with the given capacity.
    #[cold]
    fn deallocate(ptr: *mut T, capacity: usize) {
        unsafe {
            std::alloc::dealloc(
                ptr.cast(),
                std::alloc::Layout::array::<T>(capacity).unwrap_unchecked(),
            );
        }
    }

    /// Returns the mask for the given capacity.
    fn get_mask_for_capacity(capacity: usize) -> usize {
        debug_assert!(capacity.is_power_of_two());

        capacity - 1
    }

    /// Returns the physical index for the given index.
    #[inline(always)]
    fn get_physical_index(&self, index: usize) -> usize {
        debug_assert!(self.capacity.is_power_of_two());

        index & self.mask
    }

    /// Creates a new vector with the default capacity.
    pub(crate) fn new() -> Self {
        const DEFAULT_CAPACITY: usize = 16;

        Self {
            ptr: Self::allocate(DEFAULT_CAPACITY),
            head: 0,
            tail: 0,
            capacity: DEFAULT_CAPACITY,
            mask: Self::get_mask_for_capacity(DEFAULT_CAPACITY),
        }
    }

    /// Returns the number of elements in the queue.
    pub(crate) fn len(&self) -> usize {
        self.tail.wrapping_sub(self.head)
    }

    /// Returns whether the queue is empty.
    pub(crate) fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    /// Extends the vector to the given capacity.
    #[inline(never)]
    #[cold]
    #[track_caller]
    pub(crate) fn extend_to(&mut self, capacity: usize) {
        debug_assert!(capacity > self.capacity);

        let new_ptr = Self::allocate(capacity);
        let len = self.len();

        unsafe {
            let phys_head = self.get_physical_index(self.head);
            let phys_tail = self.get_physical_index(self.tail);
            let src = self.ptr.add(phys_head);
            let dst = new_ptr;

            if phys_head < phys_tail {
                ptr::copy(src, dst, len);
            } else {
                ptr::copy(src, dst, self.capacity - phys_head);

                let src = self.ptr;
                let dst = new_ptr.add(self.capacity - phys_head);

                ptr::copy(src, dst, phys_tail);
            }
        }

        Self::deallocate(self.ptr, self.capacity);

        self.head = 0;
        self.tail = len;
        self.ptr = new_ptr;
        self.capacity = capacity;
        self.mask = Self::get_mask_for_capacity(capacity);
    }

    /// Pushes a value to the queue.
    #[inline]
    pub(crate) fn push(&mut self, value: T) {
        if unlikely(self.len() == self.capacity) {
            self.extend_to(self.capacity * 2);
        }

        unsafe {
            let index = self.get_physical_index(self.tail);

            self.ptr.add(index).write(value);
        }

        self.tail = self.tail.wrapping_add(1);
    }

    /// Pops a value from the queue.
    #[inline]
    pub(crate) fn pop(&mut self) -> Option<T> {
        if self.is_empty() {
            return None;
        }

        let index = self.get_physical_index(self.head);
        let value = unsafe { self.ptr.add(index).read() };

        self.head = self.head.wrapping_add(1);

        Some(value)
    }

    /// Pushes a slice to the queue.
    ///
    /// # Safety
    ///
    /// It `T` is not `Copy`, the caller should [`forget`](mem::forget) the values.
    #[inline]
    pub(crate) unsafe fn extend_from_slice(&mut self, slice: &[T]) {
        let needed = self.len() + slice.len();

        if unlikely(needed > self.capacity) {
            let mut new_capacity = self.capacity;
            while new_capacity < needed {
                new_capacity *= 2;
            }

            self.extend_to(new_capacity);
        }

        let phys_tail = self.get_physical_index(self.tail);
        let right_space = self.capacity - phys_tail;

        unsafe {
            if slice.len() <= right_space {
                // fits in one memcpy
                ptr::copy_nonoverlapping(slice.as_ptr(), self.ptr.add(phys_tail), slice.len());
            } else {
                // wraparound case
                ptr::copy_nonoverlapping(slice.as_ptr(), self.ptr.add(phys_tail), right_space);

                ptr::copy_nonoverlapping(
                    slice.as_ptr().add(right_space),
                    self.ptr,
                    slice.len() - right_space,
                );
            }
        }

        self.tail = self.tail.wrapping_add(slice.len());
    }

    /// Moves at most `limit` elements from the queue to the [`single producer`](SingleProducer).
    pub(crate) fn move_batch_to_producer(
        &mut self,
        producer: &impl SingleProducer<T>,
        mut limit: usize,
    ) {
        limit = self.len().min(limit);

        let phys_head = self.get_physical_index(self.head);
        let right_occupied = self.capacity - phys_head;

        self.head = self.head.wrapping_add(limit);

        if limit <= right_occupied {
            // We can copy from the head to the head + limit.
            unsafe {
                producer.push_many_unchecked(
                    &*slice_from_raw_parts(self.ptr.add(phys_head), limit),
                    &[],
                );
            }

            // The head is already updated.

            return;
        }

        let slice1 = unsafe { &*slice_from_raw_parts(self.ptr.add(phys_head), right_occupied) };
        let slice2 = unsafe { &*slice_from_raw_parts(self.ptr, limit - right_occupied) };

        unsafe { producer.push_many_unchecked(slice1, slice2) };

        // The head is already updated.
    }
}

impl<T> Drop for VecQueue<T> {
    fn drop(&mut self) {
        if mem::needs_drop::<T>() {
            while let Some(val) = self.pop() {
                drop(val);
            }
        }

        Self::deallocate(self.ptr, self.capacity);
    }
}

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
        self.inner.lock().move_batch_to_producer(producer, limit);
    }

    /// Pushes two slices and a value to the queue.
    fn push_many_and_one_with_inner(inner: &mut VecQueue<T>, first: &[T], last: &[T], value: T) {
        while inner.len() + first.len() + last.len() + 1 > inner.capacity {
            let new_capacity = inner.capacity * 2;

            inner.extend_to(new_capacity);
        }

        assert_hint(
            inner.len() + first.len() + last.len() < inner.capacity,
            "capacity was not updated",
        );

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
        while inner.len() + first.len() + last.len() + slice.len() > inner.capacity {
            let new_capacity = inner.capacity * 2;

            inner.extend_to(new_capacity);
        }

        assert_hint(
            inner.len() + first.len() + last.len() + slice.len() <= inner.capacity,
            "capacity was not updated",
        );

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
        let mut queue = VecQueue::new();

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
