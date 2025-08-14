//! This module provides a single-producer multi-consumer queue.
//!
//! It is implemented as a const bounded ring buffer.
//! It is optimized for the work-stealing model.
#![allow(
    clippy::cast_possible_truncation,
    reason = "LongNumber should be synonymous to usize"
)]
use crate::hints::unlikely;
use crate::light_arc::LightArc;
use crate::number_types::{
    CachePaddedLongAtomic, LongAtomic, LongNumber, NotCachePaddedLongAtomic,
};
use crate::batch_receiver::BatchReceiver;
use std::marker::PhantomData;
use std::mem::{needs_drop, MaybeUninit};
use std::ops::Deref;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::{mem, ptr, slice};
use crate::{LockFreePopErr, LockFreePushErr, LockFreePushManyErr};

// Don't care about ABA because we can count that 16-bit and 32-bit processors never
// insert + read (2 ^ 16) - 1 or (2 ^ 32) - 1 values while some consumer is preempted.
// For 32-bit:
// If we guess, it always puts and gets 10 values by time and does it in
// 20 nanoseconds (it becomes slower by adding new threads), then the thread needs to be preempted
// for 8.5 seconds while the other thread only works with this queue.
// For 16-bit:
// We guess it never has so much concurrency.
// For 64-bit it is unrealistic to have the ABA problem.

// Reads from the head, writes to the tail.

/// The single-producer, single-consumer ring-based _const bounded_ queue.
///
/// It is safe to use when and only when only one thread is writing to the queue at the same time.
///
/// You can call `producer_` methods for the producer and `consumer_` methods for the consumers.
///
/// It accepts the atomic wrapper as a generic parameter.
/// It allows using cache-padded atomics or not.
/// You should create type aliases not to write this large type name.
///
/// # Using directly the [`SPMCBoundedQueue`] vs. using [`new_bounded`] or [`new_cache_padded_bounded`].
///
/// Functions [`new_bounded`] and [`new_cache_padded_bounded`] allocate the
/// [`SPMCUnboundedQueue`](crate::spmc::SPMCUnboundedQueue) on the heap in [`LightArc`]
/// and provide separate producer and consumer.
/// It hurts the performance if you don't need to allocate the queue separately but improves 
/// the readability when you need to separate producer and consumer logic and share them.
///
/// It doesn't implement the [`Producer`] and [`Consumer`] traits because all producer methods
/// are unsafe (can be called only by one thread).
#[repr(C)]
pub struct SPMCBoundedQueue<
    T,
    const CAPACITY: usize,
    AtomicWrapper: Deref<Target = LongAtomic> + Default = NotCachePaddedLongAtomic,
> {
    tail: AtomicWrapper,
    head: AtomicWrapper,
    buffer: [MaybeUninit<T>; CAPACITY],
}

impl<T, const CAPACITY: usize, AtomicWrapper: Deref<Target = LongAtomic> + Default>
    SPMCBoundedQueue<T, CAPACITY, AtomicWrapper>
{
    /// Indicates how many elements we are taking from the local queue.
    ///
    /// This is one less than the number of values pushed to the global
    /// queue (or any other `SyncBatchReceiver`) as we are also inserting the `value` argument.
    const NUM_VALUES_TAKEN: LongNumber = CAPACITY as LongNumber / 2;

    /// Creates a new [`SPMCBoundedQueue`].
    pub fn new() -> Self {
        debug_assert!(size_of::<MaybeUninit<T>>() == size_of::<T>()); // Assume that we can just cast it

        Self {
            buffer: [const { MaybeUninit::uninit() }; CAPACITY],
            tail: AtomicWrapper::default(),
            head: AtomicWrapper::default(),
        }
    }

    /// Returns the capacity of the queue.
    #[inline]
    pub fn capacity(&self) -> usize {
        CAPACITY
    }

    /// Returns a pointer to the buffer.
    fn buffer_thin_ptr(&self) -> *const MaybeUninit<T> {
        (&raw const self.buffer) as *const _
    }

    /// Returns a mutable pointer to the buffer.
    fn buffer_mut_thin_ptr(&self) -> *mut MaybeUninit<T> {
        (&raw const self.buffer).cast_mut() as *mut _
    }

    /// Returns the number of elements in the queue.
    #[inline]
    fn len(head: LongNumber, tail: LongNumber) -> usize {
        tail.wrapping_sub(head) as usize
    }
}

// Producer
impl<T, const CAPACITY: usize, AtomicWrapper: Deref<Target = LongAtomic> + Default>
    SPMCBoundedQueue<T, CAPACITY, AtomicWrapper>
{
    /// Pushes a slice into the queue. Returns a new tail (not index).
    fn copy_slice(buffer_ptr: *mut T, start_tail: LongNumber, slice: &[T]) -> LongNumber {
        let tail_idx = start_tail as usize % CAPACITY;

        if tail_idx + slice.len() <= CAPACITY {
            unsafe {
                ptr::copy_nonoverlapping(slice.as_ptr(), buffer_ptr.add(tail_idx), slice.len());
            };
        } else {
            let right = CAPACITY - tail_idx;

            unsafe {
                ptr::copy_nonoverlapping(slice.as_ptr(), buffer_ptr.add(tail_idx), right);
                ptr::copy_nonoverlapping(
                    slice.as_ptr().add(right),
                    buffer_ptr,
                    slice.len() - right,
                );
            }
        }

        start_tail.wrapping_add(slice.len() as LongNumber)
    }

    /// Return the number of elements in the queue.
    ///
    /// # Safety
    ///
    /// It should be called only by the producer.
    #[inline]
    pub unsafe fn producer_len(&self) -> usize {
        let head = self.head.load(Relaxed);
        let tail = unsafe { self.tail.unsync_load() }; // only the producer can change tail

        Self::len(head, tail)
    }

    /// Pops a value from the queue.
    /// Returns `None` if the queue is empty.
    ///
    /// # Safety
    ///
    /// It should be called only by the producer.
    #[inline]
    pub unsafe fn producer_pop(&self) -> Option<T> {
        let mut head = self.head.load(Acquire);
        let tail = unsafe { self.tail.unsync_load() }; // only the producer can change tail

        loop {
            if unlikely(head == tail) {
                return None;
            }

            match self
                .head
                .compare_exchange_weak(head, head.wrapping_add(1), Release, Relaxed)
            {
                Ok(_) => {
                    // We are the only producer,
                    // so we can don't worry about someone overwriting the value before we read it
                    return Some(unsafe {
                        self.buffer_thin_ptr()
                            .add(head as usize % CAPACITY)
                            .read()
                            .assume_init()
                    });
                }
                Err(new_head) => {
                    head = new_head;
                }
            }
        }
    }

    /// Pops many values from the queue.
    /// Returns the number of values popped.
    ///
    /// # Safety
    ///
    /// It should be called only by the producer.
    #[inline]
    pub unsafe fn producer_pop_many(&self, dst: &mut [MaybeUninit<T>]) -> usize {
        let mut head = self.head.load(Acquire);
        let tail = unsafe { self.tail.unsync_load() }; // only the producer can change tail

        loop {
            let available = Self::len(head, tail);
            let n = dst.len().min(available);

            if n == 0 {
                return 0;
            }

            debug_assert!(n <= CAPACITY, "Bug occurred, please report it.");

            match self.head.compare_exchange_weak(
                head,
                head.wrapping_add(n as LongNumber),
                Release,
                Relaxed,
            ) {
                Ok(_) => {
                    // We are the only producer,
                    // so we can don't worry about someone overwriting the value before we read it.

                    let dst_ptr = dst.as_mut_ptr();
                    let head_idx = head as usize % CAPACITY;
                    let right = CAPACITY - head_idx;

                    if n <= right {
                        // No wraparound, copy in one shot
                        unsafe {
                            ptr::copy_nonoverlapping(
                                self.buffer_thin_ptr().add(head_idx),
                                dst_ptr,
                                n,
                            );
                        }
                    } else {
                        unsafe {
                            // Wraparound: copy right half then left half
                            ptr::copy_nonoverlapping(
                                self.buffer_thin_ptr().add(head_idx),
                                dst_ptr,
                                right,
                            );
                            ptr::copy_nonoverlapping(
                                self.buffer_thin_ptr(),
                                dst_ptr.add(right),
                                n - right,
                            );
                        }
                    }

                    return n;
                }
                Err(new_head) => {
                    head = new_head;
                }
            }
        }
    }

    /// Pushes a value to the queue.
    ///
    /// # Safety
    ///
    /// It should be called only by the producer, and the queue should not be full.
    #[inline(always)]
    pub unsafe fn push_unchecked(&self, value: T, tail: LongNumber) {
        unsafe {
            self.buffer_mut_thin_ptr()
                .add(tail as usize % CAPACITY)
                .write(MaybeUninit::new(value));
        }

        self.tail.store(tail.wrapping_add(1), Release);
    }

    /// Likely moves a half of the queue and one value to the [`BatchReceiver`].
    #[inline(never)]
    #[cold]
    fn handle_overflow_one<BR: BatchReceiver<T>>(
        &self,
        tail: LongNumber,
        mut head: LongNumber,
        br: &BR,
        value: T,
    ) {
        debug_assert!(tail == head.wrapping_add(CAPACITY as LongNumber) && tail > head);

        loop {
            let head_idx = head as usize % CAPACITY;

            let (right, left): (&[MaybeUninit<T>], &[MaybeUninit<T>]) = if head_idx < Self::NUM_VALUES_TAKEN as usize {
                // we can return only the right half of the queue
                (
                    &self.buffer[head_idx..head_idx + Self::NUM_VALUES_TAKEN as usize],
                    &[],
                )
            } else {
                let left_part_len = head_idx - Self::NUM_VALUES_TAKEN as usize;

                (&self.buffer[head_idx..], &self.buffer[..left_part_len])
            };

            // We haven't read the value yet, so we can use `compare_exchange_weak`.
            //If it fails, we calculate two slices and try again; it is not a performance issue.
            let res = self.head.compare_exchange_weak(
                head,
                head.wrapping_add(Self::NUM_VALUES_TAKEN),
                Release,
                Relaxed,
            );

            match res {
                Ok(_) => {}
                Err(new_head) => {
                    head = new_head;

                    if Self::len(head, tail) < Self::NUM_VALUES_TAKEN as usize {
                        // Another thread concurrently
                        // stole from the queue.
                        // Because we are the one producer,
                        // we can just insert the value (it can't become full before we return).

                        unsafe { self.push_unchecked(value, tail) };

                        return;
                    }

                    continue;
                }
            }

            unsafe {
                br.push_many_and_one(
                    mem::transmute::<_, &[T]>(left),
                    mem::transmute::<_, &[T]>(right),
                    value
                );
            }

            return;
        }
    }

    /// Likely moves a half of the queue and many values to the [`BatchReceiver`].
    #[inline(never)]
    #[cold]
    fn handle_overflow_many<BR: BatchReceiver<T>>(
        &self,
        tail: LongNumber,
        mut head: LongNumber,
        br: &BR,
        slice: &[T],
    ) {
        debug_assert!(tail == head.wrapping_add(CAPACITY as LongNumber) && tail > head);

        loop {
            let head_idx = head as usize % CAPACITY;

            let (right, left): (&[MaybeUninit<T>], &[MaybeUninit<T>]) = if head_idx < Self::NUM_VALUES_TAKEN as usize {
                // we can return only the right half of the queue
                (
                    &self.buffer[head_idx..head_idx + Self::NUM_VALUES_TAKEN as usize],
                    &[],
                )
            } else {
                let left_part_len = head_idx - Self::NUM_VALUES_TAKEN as usize;

                (&self.buffer[head_idx..], &self.buffer[..left_part_len])
            };

            // We haven't read the value yet, so we can use `compare_exchange_weak`.
            //If it fails, we calculate two slices and try again; it is not a performance issue.
            let res = self.head.compare_exchange_weak(
                head,
                head.wrapping_add(Self::NUM_VALUES_TAKEN),
                Release,
                Relaxed,
            );

            match res {
                Ok(_) => {}
                Err(new_head) => {
                    head = new_head;

                    let len = Self::len(head, tail);

                    if (len < Self::NUM_VALUES_TAKEN as usize) && len + slice.len() <= CAPACITY {
                        // Another thread concurrently
                        // stole from the queue.
                        // Because we are the one producer,
                        // we can just insert the slice (it can't become full before we return).

                        let new_tail =
                            Self::copy_slice(self.buffer_mut_thin_ptr().cast(), tail, slice);
                        self.tail.store(new_tail, Release);

                        return;
                    }

                    continue;
                }
            }

            unsafe {
                br.push_many_and_slice(
                    mem::transmute::<_, &[T]>(left),
                    mem::transmute::<_, &[T]>(right),
                    slice
                );
            }

            return;
        }
    }

    /// Pushes a value to the queue or to the [`BatchReceiver`].
    ///
    /// # Safety
    ///
    /// It should be called only by the producer.
    #[inline]
    pub unsafe fn producer_push<BR: BatchReceiver<T>>(
        &self,
        value: T,
        batch_receiver: &BR,
    ) {
        let head = self.head.load(Acquire);
        let tail = unsafe { self.tail.unsync_load() }; // only the producer can change tail

        if unlikely(Self::len(head, tail) == CAPACITY) {
            self.handle_overflow_one(tail, head, batch_receiver, value);

            return;
        }

        unsafe { self.push_unchecked(value, tail) };
    }

    /// Pushes a value to the queue or returns an error.
    ///
    /// # Safety
    ///
    /// It should be called only by the producer.
    #[inline]
    pub unsafe fn producer_maybe_push(&self, value: T) -> Result<(), T> {
        let head = self.head.load(Acquire);
        let tail = unsafe { self.tail.unsync_load() }; // only the producer can change tail

        if unlikely(Self::len(head, tail) == CAPACITY) {
            return Err(value);
        }

        debug_assert!(Self::len(head, tail) < CAPACITY);

        unsafe { self.push_unchecked(value, tail) };

        Ok(())
    }

    /// Pushes many values to the queue.
    /// It accepts two slices to allow using ring-based src.
    ///
    /// # Safety
    ///
    /// It should be called only by the producer, and the space is enough.
    #[inline]
    pub unsafe fn producer_push_many_unchecked(&self, first: &[T], last: &[T]) {
        if cfg!(debug_assertions) {
            let head = self.head.load(Acquire);
            let tail = unsafe { self.tail.unsync_load() }; // only the producer can change tail

            debug_assert!(Self::len(head, tail) + first.len() + last.len() <= CAPACITY);
        }

        // It is SPMC, and it is expected that the capacity is enough.

        let mut tail = unsafe { self.tail.unsync_load() }; // only the producer can change tail

        tail = Self::copy_slice(self.buffer_mut_thin_ptr().cast(), tail, first);
        tail = Self::copy_slice(self.buffer_mut_thin_ptr().cast(), tail, last);

        self.tail.store(tail, Release);
    }

    /// Pushes many values to the queue or to the [`BatchReceiver`].
    ///
    /// # Safety
    ///
    /// It should be called only by the producer.
    #[inline]
    pub unsafe fn producer_push_many<BR: BatchReceiver<T>>(
        &self,
        slice: &[T],
        batch_receiver: &BR,
    ) {
        let head = self.head.load(Acquire);
        let mut tail = unsafe { self.tail.unsync_load() }; // only the producer can change tail

        if unlikely(Self::len(head, tail) + slice.len() > CAPACITY) {
            self.handle_overflow_many(tail, head, batch_receiver, slice);

            return;
        }

        tail = Self::copy_slice(self.buffer_mut_thin_ptr().cast(), tail, slice);

        self.tail.store(tail, Release);
    }

    /// Pushes many values to the queue or returns an error.
    ///
    /// # Safety
    ///
    /// It should be called only by the producer.
    #[inline]
    pub unsafe fn producer_maybe_push_many(&self, slice: &[T]) -> Result<(), ()> {
        let head = self.head.load(Acquire);
        let mut tail = unsafe { self.tail.unsync_load() }; // only the producer can change tail

        if unlikely(Self::len(head, tail) + slice.len() > CAPACITY) {
            return Err(()); // full
        }

        debug_assert!(Self::len(head, tail) + slice.len() <= CAPACITY);

        tail = Self::copy_slice(self.buffer_mut_thin_ptr().cast(), tail, slice);

        self.tail.store(tail, Release);

        Ok(())
    }

    /// Read the doc at [`Producer::copy_and_commit_if`].
    ///
    /// # Safety
    ///
    /// The called should be the only producer and the safety conditions
    /// from [`Producer::copy_and_commit_if`].
    ///
    /// # Panics
    ///
    /// Read the doc at [`Producer::copy_and_commit_if`].
    unsafe fn producer_copy_and_commit_if<FSuccess, FError>(
        &self,
        left: &[T],
        right: &[T],
        condition: impl FnOnce() -> Result<FSuccess, FError>,
    ) -> Result<FSuccess, FError> {
        debug_assert!(left.len() + right.len() + self.producer_len() <= CAPACITY);

        let mut new_tail = Self::copy_slice(
            self.buffer_mut_thin_ptr().cast(),
            unsafe { self.tail.unsync_load() }, // only the producer can change tail
            right,
        );
        new_tail = Self::copy_slice(
            self.buffer_mut_thin_ptr().cast(),
            new_tail,
            left,
        );

        let should_commit = condition();
        match should_commit {
            Ok(res) => {
                self
                    .tail
                    .store(new_tail, Release);

                Ok(res)
            }
            Err(err) => Err(err),
        }
    }
}

// Consumers
impl<T, const CAPACITY: usize, AtomicWrapper: Deref<Target = LongAtomic> + Default>
    SPMCBoundedQueue<T, CAPACITY, AtomicWrapper>
{
    /// Returns the number of values in the queue.
    #[inline]
    pub fn consumer_len(&self) -> usize {
        loop {
            let head = self.head.load(Relaxed);
            let tail = self.tail.load(Relaxed);
            let len = Self::len(head, tail);

            if unlikely(len > CAPACITY) {
                // Inconsistent state (this thread has been preempted
                // after we have loaded `head`,
                // and before we have loaded `tail`),
                // try again
                continue;
            }

            return len;
        }
    }

    /// Pops many values from the queue to the `dst`.
    /// Returns the number of values popped.
    #[inline]
    pub fn consumer_pop_many(&self, dst: &mut [MaybeUninit<T>]) -> usize {
        let mut head = self.head.load(Acquire);
        let mut tail = self.tail.load(Acquire);

        loop {
            let available = Self::len(head, tail);
            let n = dst.len().min(available);

            if n == 0 {
                return 0;
            }

            if unlikely(n > CAPACITY) {
                // Inconsistent state (this thread has been preempted
                // after we have loaded `head`,
                // and before we have loaded `tail`),
                // try again

                head = self.head.load(Acquire);
                tail = self.tail.load(Acquire);

                continue;
            }

            let dst_ptr = dst.as_mut_ptr();
            let head_idx = head as usize % CAPACITY;
            let right = CAPACITY - head_idx;

            // We optimistically copy the values from the buffer into the dst.
            // On CAS failure, we forget the copied values and try again.
            // It is safe because we can concurrently read from the head.

            if n <= right {
                // No wraparound, copy in one shot
                unsafe {
                    ptr::copy_nonoverlapping(self.buffer_thin_ptr().add(head_idx), dst_ptr, n);
                }
            } else {
                unsafe {
                    // Wraparound: copy right half then left half
                    ptr::copy_nonoverlapping(self.buffer_thin_ptr().add(head_idx), dst_ptr, right);
                    ptr::copy_nonoverlapping(self.buffer_thin_ptr(), dst_ptr.add(right), n - right);
                }
            }

            // Now claim ownership
            // CAS is strong because we don't want to recopy the values
            match self.head.compare_exchange(
                head,
                head.wrapping_add(n as LongNumber),
                Release,
                Relaxed,
            ) {
                Ok(_) => return n,
                Err(actual_head) => {
                    // CAS failed, forget read values (they're MaybeUninit, so it's fine)
                    // But don't try to drop, just retry

                    head = actual_head;
                    tail = self.tail.load(Acquire);
                }
            }
        }
    }

    /// Steals many values from the consumer to the `dst`.
    /// Returns the number of values stolen.
    ///
    /// # Panics
    ///
    /// If `dst` is not empty.
    pub fn steal_into(&self, dst: &impl crate::single_producer::SingleProducer<T>) -> usize {
        if cfg!(debug_assertions) {
            assert!(
                dst.is_empty(),
                "steal_into should not be called when dst is not empty"
            );
        }

        let mut src_head = self.head.load(Acquire);

        loop {
            let src_tail = self.tail.load(Acquire);
            let n = Self::len(src_head, src_tail) / 2;

            if n > CAPACITY / 2 {
                // Inconsistent state (this thread has been preempted
                // after we have loaded `src_head`,
                // and before we have loaded `src_tail`),
                // try again

                src_head = self.head.load(Acquire);

                continue;
            }

            if !cfg!(feature = "always_steal") && n < 4 || n == 0 {
                // we don't steal less than 4 by default
                // because else we may lose more because of cache locality and NUMA awareness
                return 0;
            }

            let src_head_idx = src_head as usize % CAPACITY;

            let (src_right, src_left): (&[T], &[T]) = unsafe {
                let right_occupied = CAPACITY - src_head_idx;
                if n <= right_occupied {
                    (
                        slice::from_raw_parts(self.buffer_thin_ptr().add(src_head_idx).cast(), n),
                        &[],
                    )
                } else {
                    (
                        slice::from_raw_parts(
                            self.buffer_thin_ptr().add(src_head_idx).cast(),
                            right_occupied,
                        ),
                        slice::from_raw_parts(self.buffer_thin_ptr().cast(), n - right_occupied),
                    )
                }
            };

            let cas_closure = || {
                // CAS is strong because we don't want to recopy the values
                self.head.compare_exchange(
                    src_head,
                    src_head.wrapping_add(n as LongNumber),
                    Release,
                    Relaxed,
                )
            };

            // CAS is strong because we don't want to recopy the values
            let res = unsafe { dst.copy_and_commit_if(src_right, src_left, cas_closure) };

            match res {
                Ok(_) => {
                    return n;
                }
                Err(current_head) => {
                    // another thread has read the same values, full retry
                    src_head = current_head;
                }
            }
        }
    }
}

impl<T, const CAPACITY: usize, AtomicWrapper: Deref<Target = LongAtomic> + Default> Default
    for SPMCBoundedQueue<T, CAPACITY, AtomicWrapper>
{
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl<T, const CAPACITY: usize, AtomicWrapper> Sync
    for SPMCBoundedQueue<T, CAPACITY, AtomicWrapper>
where
    AtomicWrapper: Deref<Target = LongAtomic> + Default,
{
}

#[allow(clippy::non_send_fields_in_send_ty, reason = "We guarantee it is Send")]
unsafe impl<T, const CAPACITY: usize, AtomicWrapper> Send
    for SPMCBoundedQueue<T, CAPACITY, AtomicWrapper>
where
    AtomicWrapper: Deref<Target = LongAtomic> + Default,
{
}

impl<T, const CAPACITY: usize, AtomicWrapper> Drop for SPMCBoundedQueue<T, CAPACITY, AtomicWrapper>
where
    AtomicWrapper: Deref<Target = LongAtomic> + Default,
{
    fn drop(&mut self) {
        // While dropping, there is no concurrency

        if needs_drop::<T>() {
            let mut head = unsafe { self.head.unsync_load() };
            let tail = unsafe { self.tail.unsync_load() };

            while head != tail {
                unsafe {
                    ptr::drop_in_place(
                        self.buffer_thin_ptr()
                            .add(head as usize % CAPACITY)
                            .cast::<T>()
                            .cast_mut(),
                    );
                }

                head = head.wrapping_add(1);
            }
        }
    }
}

/// Generates SPMC producer and consumer.
macro_rules! generate_spmc_producer_and_consumer {
    ($producer_name:ident, $consumer_name:ident, $atomic_wrapper:ty) => {
        /// The producer of the [`SPMCBoundedQueue`].
        pub struct $producer_name<T, const CAPACITY: usize> {
            inner: LightArc<SPMCBoundedQueue<T, CAPACITY, $atomic_wrapper>>,
            _non_sync: PhantomData<*const ()>,
        }

        impl<T: Send, const CAPACITY: usize> $crate::Producer<T> for $producer_name<T, CAPACITY> {
            #[inline]
            fn capacity(&self) -> usize {
                CAPACITY as usize
            }

            #[inline]
            fn len(&self) -> usize {
                unsafe { self.inner.producer_len() }
            }

            #[inline]
            fn push<BR: BatchReceiver<T>>(&self, value: T, batch_receiver: &BR) {
                unsafe { self.inner.producer_push(value, batch_receiver) };
            }

            #[inline]
            fn maybe_push(&self, value: T) -> Result<(), T> {
                unsafe { self.inner.producer_maybe_push(value) }
            }

            #[inline]
            unsafe fn push_many_unchecked(&self, first: &[T], last: &[T]) {
                unsafe { self.inner.producer_push_many_unchecked(first, last) };
            }

            #[inline]
            unsafe fn maybe_push_many(&self, slice: &[T]) -> Result<(), ()> {
                unsafe { self.inner.producer_maybe_push_many(slice) }
            }

            #[inline]
            unsafe fn push_many<BR: BatchReceiver<T>>(
                &self,
                slice: &[T],
                batch_receiver: &BR,
            ) {
                unsafe { self.inner.producer_push_many(slice, batch_receiver) };
            }
        }

        impl<T: Send, const CAPACITY: usize> $crate::LockFreeProducer<T> for $producer_name<T, CAPACITY> {
            unsafe fn lock_free_maybe_push_many(&self, slice: &[T]) -> Result<(), LockFreePushManyErr> {
                unsafe { self.maybe_push_many(slice).map_err(|_| LockFreePushManyErr::NotEnoughSpace) }
            }

            fn lock_free_maybe_push(&self, value: T) -> Result<(), LockFreePushErr<T>> {
                self.maybe_push(value).map_err(|value| LockFreePushErr::Full(value))
            }
        }

        impl<T: Send, const CAPACITY: usize> $crate::single_producer::SingleProducer<T> for $producer_name<T, CAPACITY> {
            #[inline]
            unsafe fn copy_and_commit_if<F, FSuccess, FError>(
                &self,
                right: &[T],
                left: &[T],
                f: F
            ) -> Result<FSuccess, FError>
            where
                F: FnOnce() -> Result<FSuccess, FError>
            {
                unsafe { self.inner.producer_copy_and_commit_if(right, left, f) }
            }
        }

        impl<T: Send, const CAPACITY: usize> $crate::single_producer::SingleLockFreeProducer<T> for $producer_name<T, CAPACITY> {}

        impl<T: Send, const CAPACITY: usize> $crate::multi_consumer::MultiConsumerSpawner<T> for $producer_name<T, CAPACITY> {
            fn spawn_multi_consumer(&self) -> impl $crate::multi_consumer::MultiConsumer<T> {
                 $consumer_name {
                    inner: self.inner.clone(),
                    _non_sync: PhantomData,
                }
            }
        }

        impl<T: Send, const CAPACITY: usize> $crate::multi_consumer::MultiLockFreeConsumerSpawner<T> for $producer_name<T, CAPACITY> {
            fn spawn_multi_lock_free_consumer(&self) -> impl $crate::multi_consumer::MultiLockFreeConsumer<T> {
                $consumer_name {
                    inner: self.inner.clone(),
                    _non_sync: PhantomData,
                }
            }
        }

        impl<T: Send, const CAPACITY: usize> $crate::spmc_producer::SPMCProducer<T> for $producer_name<T, CAPACITY> {
            #[inline]
            fn pop(&self) -> Option<T> {
                unsafe { self.inner.producer_pop() }
            }

            #[inline]
            fn pop_many(&self, dst: &mut [MaybeUninit<T>]) -> usize {
                unsafe { self.inner.producer_pop_many(dst) }
            }
        }

        unsafe impl<T: Send, const CAPACITY: usize> Send for $producer_name<T, CAPACITY> {}

        /// The consumer of the [`SPMCBoundedQueue`].
        pub struct $consumer_name<T, const CAPACITY: usize> {
            inner: LightArc<SPMCBoundedQueue<T, CAPACITY, $atomic_wrapper>>,
            _non_sync: PhantomData<*const ()>,
        }

        impl<T: Send, const CAPACITY: usize> $crate::Consumer<T> for $consumer_name<T, CAPACITY> {
            #[inline]
            fn capacity(&self) -> usize {
                CAPACITY as usize
            }

            #[inline]
            fn len(&self) -> usize {
                self.inner.consumer_len()
            }

            #[inline]
            fn pop_many(&self, dst: &mut [MaybeUninit<T>]) -> usize {
                self.inner.consumer_pop_many(dst)
            }

            #[inline(never)]
            fn steal_into(&self, dst: &impl Producer<T>) -> usize {
                self.inner.steal_into(dst)
            }
        }

        impl<T: Send, const CAPACITY: usize> $crate::LockFreeConsumer<T> for $consumer_name<T, CAPACITY> {
            #[inline]
            unsafe fn lock_free_pop_many(&self, dst: &mut [MaybeUninit<T>]) -> (usize, bool) {
                (self.pop_many(dst), false)
            }

            #[inline]
            unsafe fn lock_free_pop(&self) -> Result<T, LockFreePopErr> {
                self.pop().ok_or(LockFreePopErr::Empty)
            }

            #[inline(never)]
            fn lock_free_steal(&self, dst: &impl Producer<T>) -> (usize, bool) {
                (self.steal_into(dst), false)
            }
        }

        impl<T: Send, const CAPACITY: usize> $crate::multi_consumer::MultiConsumer<T> for $consumer_name<T, CAPACITY> {}

        impl<T: Send, const CAPACITY: usize> $crate::multi_consumer::MultiLockFreeConsumer<T> for $consumer_name<T, CAPACITY> {}

        impl<T, const CAPACITY: usize> Clone for $consumer_name<T, CAPACITY> {
            fn clone(&self) -> Self {
                Self {
                    inner: self.inner.clone(),
                    _non_sync: PhantomData,
                }
            }
        }

        unsafe impl<T: Send, const CAPACITY: usize> Send for $consumer_name<T, CAPACITY> {}
    };

    ($producer_name:ident, $consumer_name:ident) => {
        generate_spmc_producer_and_consumer!(
            $producer_name,
            $consumer_name,
            NotCachePaddedLongAtomic
        );
    };
}

generate_spmc_producer_and_consumer!(SPMCProducer, SPMCConsumer);

/// Creates a new single-producer, multi-consumer queue with the given capacity.
/// Returns [`producer`](SPMCProducer) and [`consumer`](SPMCConsumer).
///
/// It accepts the capacity as a const generic parameter.
/// We recommend using a power of two.
///
/// The producer __should__ be only one while consumers can be cloned.
/// If you want to use more than one producer, don't use this queue.
///
/// If you want to use only one consumer, look at the single-producer, single-consumer queue.
///
/// # Bounded queue vs. [`unbounded queue`](crate::spmc::new_unbounded)
///
/// - [`maybe_push`](Producer::maybe_push), [`maybe_push_many`](Producer::maybe_push_many)
///   can return an error only for `bounded` queue.
/// - [`push`](Producer::push), [`push_many`](Producer::push_many)
///   writes to the [`BatchReceiver`] only for `bounded` queue.
/// - [`Consumer::steal_into`] and [`Consumer::pop_many`] can pop zero values even if the source
///   queue is not empty for `unbounded` queue.
/// - [`Consumer::capacity`] and [`Consumer::len`] can return old values for `unbounded` queue.
/// - All methods of `bounded` queue work much faster than all methods of `unbounded` queue.
///
/// # Cache padding
///
/// Cache padding can improve the performance of the queue many times, but it also requires
/// much more memory (likely 128 or 256 more bytes for the queue).
/// If you can sacrifice some memory for the performance, use [`new_cache_padded_bounded`].
///
/// # Examples
///
/// ```
/// use parcoll::spmc::new_bounded;
/// use parcoll::{Consumer, Producer};
///
/// let (mut producer, mut consumer) = new_bounded::<_, 256>();
/// let consumer2 = consumer.clone(); // You can clone the consumer
///
/// producer.maybe_push(1).unwrap();
/// producer.maybe_push(2).unwrap();
///
/// let mut slice = [std::mem::MaybeUninit::uninit(); 3];
/// let popped = consumer.pop_many(&mut slice);
///
/// assert_eq!(popped, 2);
/// assert_eq!(unsafe { slice[0].assume_init() }, 1);
/// assert_eq!(unsafe { slice[1].assume_init() }, 2);
/// ```
pub fn new_bounded<T, const CAPACITY: usize>(
) -> (SPMCProducer<T, CAPACITY>, SPMCConsumer<T, CAPACITY>) {
    let queue = LightArc::new(SPMCBoundedQueue::new());

    (
        SPMCProducer {
            inner: queue.clone(),
            _non_sync: PhantomData,
        },
        SPMCConsumer {
            inner: queue,
            _non_sync: PhantomData,
        },
    )
}

generate_spmc_producer_and_consumer!(
    CachePaddedSPMCProducer,
    CachePaddedSPMCConsumer,
    CachePaddedLongAtomic
);

/// Creates a new single-producer, multi-consumer queue with the given capacity.
/// Returns [`producer`](CachePaddedSPMCProducer) and [`consumer`](CachePaddedSPMCConsumer).
///
/// It accepts the capacity as a const generic parameter.
/// We recommend using a power of two.
///
/// The producer __should__ be only one while consumers can be cloned.
/// If you want to use more than one producer, don't use this queue.
///
/// If you want to use only one consumer, look at the single-producer, single-consumer queue.
///
/// # Bounded queue vs. [`unbounded queue`](crate::spmc::new_unbounded)
///
/// - [`maybe_push`](Producer::maybe_push), [`maybe_push_many`](Producer::maybe_push_many)
///   can return an error only for `bounded` queue.
/// - [`push`](Producer::push), [`push_many`](Producer::push_many)
///   writes to the [`BatchReceiver`] only for `bounded` queue.
/// - [`Consumer::steal_into`] and [`Consumer::pop_many`] can pop zero values even if the source
///   queue is not empty for `unbounded` queue.
/// - [`Consumer::capacity`] and [`Consumer::len`] can return old values for `unbounded` queue.
/// - All methods of `bounded` queue work much faster than all methods of `unbounded` queue.
///
/// # Cache padding
///
/// Cache padding can improve the performance of the queue many times, but it also requires
/// much more memory (likely 128 or 256 more bytes for the queue).
/// If you can't sacrifice some memory for the performance, use [`new_bounded`].
///
/// # Examples
///
/// ```
/// use parcoll::spmc::new_bounded;
/// use parcoll::{Consumer, Producer};
///
/// let (mut producer, mut consumer) = new_bounded::<_, 256>();
/// let consumer2 = consumer.clone(); // You can clone the consumer
///
/// producer.maybe_push(1).unwrap();
/// producer.maybe_push(2).unwrap();
///
/// let mut slice = [std::mem::MaybeUninit::uninit(); 3];
/// let popped = consumer.pop_many(&mut slice);
///
/// assert_eq!(popped, 2);
/// assert_eq!(unsafe { slice[0].assume_init() }, 1);
/// assert_eq!(unsafe { slice[1].assume_init() }, 2);
/// ```
pub fn new_cache_padded_bounded<T, const CAPACITY: usize>() -> (
    CachePaddedSPMCProducer<T, CAPACITY>,
    CachePaddedSPMCConsumer<T, CAPACITY>,
) {
    let queue = LightArc::new(SPMCBoundedQueue::new());

    (
        CachePaddedSPMCProducer {
            inner: queue.clone(),
            _non_sync: PhantomData,
        },
        CachePaddedSPMCConsumer {
            inner: queue,
            _non_sync: PhantomData,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutex_vec_queue::MutexVecQueue;
    use std::collections::VecDeque;
    use crate::{Consumer, Producer};
    use crate::spmc_producer::SPMCProducer;

    const CAPACITY: usize = 256;

    #[test]
    fn test_spmc_bounded_size() {
        let queue = SPMCBoundedQueue::<u8, CAPACITY>::new();

        assert_eq!(
            size_of_val(&queue),
            CAPACITY + size_of::<LongAtomic>() * 2
        );

        let cache_padded_queue = SPMCBoundedQueue::<u8, CAPACITY, CachePaddedLongAtomic>::new();

        assert_eq!(
            size_of_val(&cache_padded_queue),
            size_of::<CachePaddedLongAtomic>() * 2 + CAPACITY
        );
    }

    #[test]
    fn test_spmc_bounded_seq_insertions() {
        let global_queue = MutexVecQueue::new();
        let (producer, _) = new_bounded::<_, CAPACITY>();

        for i in 0..CAPACITY * 100 {
            producer.push(i, &global_queue);
        }

        let (mut new_producer, _) = new_bounded::<_, CAPACITY>();

        global_queue
            .move_batch_to_producer(&mut new_producer, producer.capacity() - producer.len());

        assert_eq!(
            producer.len() + new_producer.len() + global_queue.len(),
            CAPACITY * 100
        );

        for _ in 0..producer.len() {
            assert!(producer.pop().is_some());
        }

        for _ in 0..new_producer.len() {
            assert!(new_producer.pop().is_some());
        }
    }

    #[test]
    fn test_spmc_bounded_stealing() {
        const TRIES: usize = 10;

        let global_queue = MutexVecQueue::new();
        let (producer1, consumer) = new_bounded::<_, CAPACITY>();
        let (mut producer2, _) = new_bounded::<_, CAPACITY>();

        let mut stolen = VecDeque::new();

        for _ in 0..TRIES * 2 {
            for i in 0..CAPACITY / 2 {
                producer1.push(i, &global_queue);
            }

            consumer.steal_into(&mut producer2);

            while let Some(task) = producer2.pop() {
                stolen.push_back(task);
            }

            assert!(global_queue.is_empty());
        }

        assert!(producer2.is_empty());

        let mut count = 0;

        while let Some(_) = producer1.pop() {
            count += 1;
        }

        assert_eq!(count + stolen.len() + global_queue.len(), CAPACITY * TRIES);
    }

    #[test]
    fn test_spmc_bounded_many() {
        const BATCH_SIZE: usize = 30;
        const N: usize = BATCH_SIZE * 100;

        let global_queue = MutexVecQueue::new();
        let (producer, consumer) = new_bounded::<_, CAPACITY>();

        for i in 0..N / BATCH_SIZE / 2 {
            let slice = (0..BATCH_SIZE)
                .map(|j| i * BATCH_SIZE + j)
                .collect::<Vec<_>>();

            unsafe {
                producer.maybe_push_many(&*slice).unwrap();
            }

            let mut slice = [MaybeUninit::uninit(); BATCH_SIZE];
            producer.pop_many(slice.as_mut_slice());

            for j in 0..BATCH_SIZE {
                let index = i * BATCH_SIZE + j;

                assert_eq!(unsafe { slice[j].assume_init() }, index);
            }
        }

        for i in 0..N / BATCH_SIZE / 2 {
            let slice = (0..BATCH_SIZE)
                .map(|j| i * BATCH_SIZE + j)
                .collect::<Vec<_>>();

            unsafe {
                producer.push_many(&*slice, &global_queue);
            }

            assert!(global_queue.is_empty());

            let mut slice = [MaybeUninit::uninit(); BATCH_SIZE];
            consumer.pop_many(slice.as_mut_slice());

            for j in 0..BATCH_SIZE {
                let index = i * BATCH_SIZE + j;

                assert_eq!(unsafe { slice[j].assume_init() }, index);
            }
        }
    }
}
