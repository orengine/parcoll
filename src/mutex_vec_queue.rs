use crate::hints::{assert_hint, unlikely};
use crate::loom_bindings::sync::{Arc, Mutex};
use crate::batch_receiver::BatchReceiver;
use std::ptr::slice_from_raw_parts;
use std::{mem, ptr};
use crate::Producer;

pub(crate) struct VecQueue<T> {
    ptr: *mut T,
    head: usize,
    tail: usize,
    capacity: usize,
    mask: usize,
}

impl<T> VecQueue<T> {
    fn allocate(capacity: usize) -> *mut T {
        debug_assert!(capacity > 0 && capacity.is_power_of_two());

        unsafe {
            std::alloc::alloc(std::alloc::Layout::array::<T>(capacity).unwrap_unchecked()).cast()
        }
    }

    fn deallocate(ptr: *mut T, capacity: usize) {
        unsafe {
            std::alloc::dealloc(
                ptr.cast(),
                std::alloc::Layout::array::<T>(capacity).unwrap_unchecked(),
            );
        }
    }

    fn get_mask_for_capacity(capacity: usize) -> usize {
        debug_assert!(capacity.is_power_of_two());

        capacity - 1
    }

    #[inline(always)]
    fn get_physical_index(&self, index: usize) -> usize {
        debug_assert!(self.capacity.is_power_of_two());

        index & self.mask
    }

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

    pub(crate) fn len(&self) -> usize {
        self.tail.wrapping_sub(self.head)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.head == self.tail
    }

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

    #[inline]
    pub(crate) fn extend_from_slice(&mut self, slice: &[T]) {
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
                std::ptr::copy_nonoverlapping(slice.as_ptr(), self.ptr.add(phys_tail), slice.len());
            } else {
                // wraparound case
                std::ptr::copy_nonoverlapping(slice.as_ptr(), self.ptr.add(phys_tail), right_space);

                std::ptr::copy_nonoverlapping(
                    slice.as_ptr().add(right_space),
                    self.ptr,
                    slice.len() - right_space,
                );
            }
        }

        self.tail = self.tail.wrapping_add(slice.len());
    }

    pub(crate) fn move_batch_to_producer(
        &mut self,
        producer: &mut impl Producer<T>,
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

#[derive(Clone)]
pub struct MutexVecQueue<T> {
    inner: Arc<Mutex<VecQueue<T>>>,
}

impl<T> MutexVecQueue<T> {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecQueue::new())),
        }
    }

    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }

    pub fn push(&self, task: T) {
        self.inner.lock().push(task);
    }

    pub fn move_batch_to_producer(&self, producer: &mut impl Producer<T>, limit: usize) {
        self.inner.lock().move_batch_to_producer(producer, limit);
    }
}

impl<T> Default for MutexVecQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> BatchReceiver<T> for MutexVecQueue<T> {
    fn push_many_and_one(&self, first: &[T], last: &[T], value: T) {
        let mut inner = self.inner.lock();

        while inner.len() + first.len() + last.len() + 1 > inner.capacity {
            let new_capacity = inner.capacity * 2;

            inner.extend_to(new_capacity);
        }

        assert_hint(
            inner.len() + first.len() + last.len() < inner.capacity,
            "capacity was not updated",
        );

        inner.extend_from_slice(first);
        inner.extend_from_slice(last);

        inner.push(unsafe { ptr::read(&value) });

        // Clippy wants it
        drop(inner);
    }

    fn push_many_and_slice(&self, first: &[T], last: &[T], slice: &[T]) {
        let mut inner = self.inner.lock();

        while inner.len() + first.len() + last.len() + slice.len() > inner.capacity {
            let new_capacity = inner.capacity * 2;

            inner.extend_to(new_capacity);
        }

        assert_hint(
            inner.len() + first.len() + last.len() + slice.len() <= inner.capacity,
            "capacity was not updated",
        );

        inner.extend_from_slice(first);
        inner.extend_from_slice(last);
        inner.extend_from_slice(slice);
    }
}

#[cfg(test)]
mod tests {
    use crate::spmc_producer::SPMCProducer;
    use super::*;

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

            queue.extend_from_slice(&slice);
        }

        for i in 0..N {
            let task = queue.pop().unwrap();

            assert_eq!(task, i);
        }
    }

    #[test]
    fn test_global_queue_with_local() {
        let global_queue = MutexVecQueue::new();
        let (mut producer, _) = crate::spmc::new_bounded::<_, 256>();

        for i in 0..N / BATCH_SIZE {
            let slice = (0..BATCH_SIZE - 1)
                .map(|j| i * BATCH_SIZE + j)
                .collect::<Vec<_>>();
            let one_more_value = i * BATCH_SIZE + BATCH_SIZE - 1;

            let _ = global_queue.push_many_and_one(&slice[..2], &slice[2..], one_more_value);
        }

        for i in 0..N / BATCH_SIZE {
            global_queue.move_batch_to_producer(&mut producer, BATCH_SIZE);

            for j in 0..BATCH_SIZE {
                let index = i * BATCH_SIZE + j;

                assert_eq!(producer.pop().unwrap(), index);
            }
        }
    }
}
