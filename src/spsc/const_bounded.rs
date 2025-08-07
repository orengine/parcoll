//! This module provides a single-producer single-consumer queue.
//!
//! It is implemented as a const bounded ring buffer.
#![allow(
    clippy::cast_possible_truncation,
    reason = "LongNumber should be synonymous to usize"
)]
use crate::hints::unlikely;
use crate::light_arc::LightArc;
use crate::number_types::{
    CachePaddedLongAtomic, LongAtomic, LongNumber, NotCachePaddedLongAtomic,
};
use crate::spsc::{Consumer, Producer};
use std::marker::PhantomData;
use std::mem::{needs_drop, MaybeUninit};
use std::ops::Deref;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::{ptr, slice};

// Don't care about ABA because we can count that 16-bit and 32-bit processors never
// insert + read (2 ^ 16) - 1 or (2 ^ 32) - 1 values while some consumer is preempted.
// For 32-bit:
// If we guess, it always put and get 10 values by time and do it in
// 20 nanoseconds (it becomes slower by adding new threads), then the thread needs to be preempted
// for 8.5 seconds + all the time the other thread is preempted while the other thread only works with this queue.
// For 16-bit:
// We guess, it never has so much concurrency.
// For 64-bit it is unrealistic to have the ABA problem.

// Reads from the head, writes to the tail.

/// The single-producer, single-consumer ring-based _const bounded_ queue.
///
/// It is safe to use when and only when only one thread is writing to the queue at the same time,
/// and only one thread is reading from the queue at the same time.
///
/// You can call `producer_` methods for the producer and `consumer_` methods for the consumer.
///
/// It accepts the atomic wrapper as a generic parameter.
/// It allows using cache-padded atomics or not.
/// You should create types aliases not to write this large type name.
///
/// # Using directly the [`SPSCBoundedQueue`] vs. using [`new_bounded`] or [`new_cache_padded_bounded`].
///
/// Functions [`new_bounded`] and [`new_cache_padded_bounded`] allocate the
/// [`SPSCBoundedQueue`] on the heap in [`LightArc`] and provide separate producer and consumer.
/// It hurts the performance if you don't need to allocate the queue separately, but improve
/// the readability when you need to separate producer and consumer logic and share them.
///
/// It doesn't implement the [`Producer`] and [`Consumer`] traits because all producer and consumer
/// methods are unsafe (can be called only by one thread for each).
#[repr(C)]
pub struct SPSCBoundedQueue<
    T,
    const CAPACITY: usize,
    AtomicWrapper: Deref<Target = LongAtomic> + Default = NotCachePaddedLongAtomic,
> {
    tail: AtomicWrapper,
    head: AtomicWrapper,
    buffer: *mut [MaybeUninit<T>; CAPACITY],
}

impl<T, const CAPACITY: usize, AtomicWrapper: Deref<Target = LongAtomic> + Default>
    SPSCBoundedQueue<T, CAPACITY, AtomicWrapper>
{
    /// Creates a new [`SPSCBoundedQueue`].
    pub fn new() -> Self {
        debug_assert!(size_of::<MaybeUninit<T>>() == size_of::<T>()); // Assume that we can just cast it

        Self {
            buffer: Box::into_raw(Box::new([const { MaybeUninit::uninit() }; CAPACITY])),
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
        unsafe { &*self.buffer }.as_ptr()
    }

    /// Returns a mutable pointer to the buffer.
    fn buffer_mut_thin_ptr(&self) -> *mut MaybeUninit<T> {
        unsafe { &mut *self.buffer }.as_mut_ptr()
    }

    /// Returns the number of elements in the queue.
    #[inline]
    fn len(head: LongNumber, tail: LongNumber) -> usize {
        tail.wrapping_sub(head) as usize
    }
}

// Producer
impl<T, const CAPACITY: usize, AtomicWrapper: Deref<Target = LongAtomic> + Default>
    SPSCBoundedQueue<T, CAPACITY, AtomicWrapper>
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
    /// The called should be the only producer.
    #[inline]
    pub unsafe fn producer_len(&self) -> usize {
        let head = self.head.load(Relaxed);
        let tail = unsafe { self.tail.unsync_load() }; // only producer can change tail

        Self::len(head, tail)
    }

    /// Pushes a value to the queue.
    ///
    /// # Safety
    ///
    /// The called should be the only producer and the queue should not be full.
    #[inline(always)]
    pub unsafe fn push_unchecked(&self, value: T, tail: LongNumber) {
        unsafe {
            self.buffer_mut_thin_ptr()
                .add(tail as usize % CAPACITY)
                .write(MaybeUninit::new(value));
        }

        self.tail.store(tail.wrapping_add(1), Release);
    }

    /// Pushes a value to the queue or returns an error.
    ///
    /// # Safety
    ///
    /// The called should be the only producer.
    #[inline]
    pub unsafe fn producer_maybe_push(&self, value: T) -> Result<(), T> {
        let head = self.head.load(Acquire);
        let tail = unsafe { self.tail.unsync_load() }; // only producer can change tail

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
    /// The called should be the only producer and the space is enough.
    #[inline]
    pub unsafe fn producer_push_many_unchecked(&self, first: &[T], last: &[T]) {
        if cfg!(debug_assertions) {
            let head = self.head.load(Acquire);
            let tail = unsafe { self.tail.unsync_load() }; // only producer can change tail

            debug_assert!(Self::len(head, tail) + first.len() + last.len() <= CAPACITY);
        }

        // It is SPSC, and it is expected that the capacity is enough.

        let mut tail = unsafe { self.tail.unsync_load() }; // only producer can change tail

        tail = Self::copy_slice(self.buffer_mut_thin_ptr().cast(), tail, first);
        tail = Self::copy_slice(self.buffer_mut_thin_ptr().cast(), tail, last);

        self.tail.store(tail, Release);
    }

    /// Pushes many values to the queue or returns an error.
    ///
    /// # Safety
    ///
    /// The called should be the only producer.
    #[inline]
    pub unsafe fn producer_maybe_push_many(&self, slice: &[T]) -> Result<(), ()> {
        let head = self.head.load(Acquire);
        let mut tail = unsafe { self.tail.unsync_load() }; // only producer can change tail

        if unlikely(Self::len(head, tail) + slice.len() > CAPACITY) {
            return Err(()); // don't have enough space
        }

        debug_assert!(Self::len(head, tail) + slice.len() <= CAPACITY);

        tail = Self::copy_slice(self.buffer_mut_thin_ptr().cast(), tail, slice);

        self.tail.store(tail, Release);

        Ok(())
    }
}

// Consumers
impl<T, const CAPACITY: usize, AtomicWrapper: Deref<Target = LongAtomic> + Default>
    SPSCBoundedQueue<T, CAPACITY, AtomicWrapper>
{
    /// Returns the number of values in the queue.
    ///
    /// # Safety
    ///
    /// The called should be the only consumer.
    #[inline]
    pub unsafe fn consumer_len(&self) -> usize {
        let tail = self.tail.load(Relaxed);
        let head = unsafe { self.head.unsync_load() }; // only consumer can change head

        Self::len(head, tail)
    }

    /// Pops many values from the queue to the `dst`.
    /// Returns the number of values popped.
    ///
    /// # Safety
    ///
    /// The called should be the only consumer.
    #[inline]
    pub unsafe fn consumer_pop_many(&self, dst: &mut [MaybeUninit<T>]) -> usize {
        let tail = self.tail.load(Acquire);
        let head = unsafe { self.head.unsync_load() }; // only consumer can change head
        let available = Self::len(head, tail);
        let n = dst.len().min(available);

        if n == 0 {
            return 0;
        }

        let dst_ptr = dst.as_mut_ptr();
        let head_idx = head as usize % CAPACITY;
        let right = CAPACITY - head_idx;

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

        self.head.store(head.wrapping_add(n as LongNumber), Release);

        n
    }

    /// Steals many values from the consumer to the `dst`.
    /// Returns the number of values stolen.
    ///
    /// # Safety
    ///
    /// The called should be the only consumer.
    ///
    /// # Panics
    ///
    /// If `dst` is not empty.
    pub unsafe fn steal_into(&self, dst: &Self) -> usize {
        let src_tail = self.tail.load(Acquire);
        let src_head = unsafe { self.head.unsync_load() }; // only consumer can change head
        let dst_tail = unsafe { dst.tail.unsync_load() }; // only producer can change tail

        if cfg!(debug_assertions) {
            let dst_head = dst.head.load(Relaxed);

            assert_eq!(
                dst_head, dst_tail,
                "steal_into should not be called when dst is not empty"
            );
        }

        let n = Self::len(src_head, src_tail) / 2;
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

        Self::copy_slice(
            dst.buffer_mut_thin_ptr().cast::<T>(),
            dst_tail % CAPACITY as LongNumber,
            src_right,
        );
        Self::copy_slice(
            dst.buffer_mut_thin_ptr().cast::<T>(),
            (dst_tail.wrapping_add(src_right.len() as LongNumber)) % CAPACITY as LongNumber,
            src_left,
        );

        self.head
            .store(src_head.wrapping_add(n as LongNumber), Release);
        dst.tail
            .store(dst_tail.wrapping_add(n as LongNumber), Release);

        n
    }
}

impl<T, const CAPACITY: usize, AtomicWrapper: Deref<Target = LongAtomic> + Default> Default
    for SPSCBoundedQueue<T, CAPACITY, AtomicWrapper>
{
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl<T, const CAPACITY: usize, AtomicWrapper> Sync
    for SPSCBoundedQueue<T, CAPACITY, AtomicWrapper>
where
    AtomicWrapper: Deref<Target = LongAtomic> + Default,
{
}
#[allow(clippy::non_send_fields_in_send_ty, reason = "We guarantee it is Send")]
unsafe impl<T, const CAPACITY: usize, AtomicWrapper> Send
    for SPSCBoundedQueue<T, CAPACITY, AtomicWrapper>
where
    AtomicWrapper: Deref<Target = LongAtomic> + Default,
{
}

impl<T, const CAPACITY: usize, AtomicWrapper> Drop for SPSCBoundedQueue<T, CAPACITY, AtomicWrapper>
where
    AtomicWrapper: Deref<Target = LongAtomic> + Default,
{
    fn drop(&mut self) {
        // While dropping there is no concurrency

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

        unsafe { drop(Box::from_raw(self.buffer)) };
    }
}

/// Generates SPSC producer and consumer.
macro_rules! generate_spsc_producer_and_consumer {
    ($producer_name:ident, $consumer_name:ident, $atomic_wrapper:ty) => {
        /// The producer of the [`SPSCBoundedQueue`].
        pub struct $producer_name<T, const CAPACITY: usize> {
            inner: LightArc<SPSCBoundedQueue<T, CAPACITY, $atomic_wrapper>>,
            _non_sync: PhantomData<*const ()>,
        }

        impl<T: Send, const CAPACITY: usize> Producer<T> for $producer_name<T, CAPACITY> {
            #[inline]
            fn capacity(&self) -> usize {
                CAPACITY as usize
            }

            #[inline]
            fn len(&self) -> usize {
                unsafe { self.inner.producer_len() }
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
        }

        unsafe impl<T: Send, const CAPACITY: usize> Send for $producer_name<T, CAPACITY> {}

        /// The consumer of the [`SPSCBoundedQueue`].
        pub struct $consumer_name<T, const CAPACITY: usize> {
            inner: LightArc<SPSCBoundedQueue<T, CAPACITY, $atomic_wrapper>>,
            _non_sync: PhantomData<*const ()>,
        }

        impl<T: Send, const CAPACITY: usize> Consumer<T> for $consumer_name<T, CAPACITY> {
            type AssociatedProducer = $producer_name<T, CAPACITY>;

            #[inline]
            fn capacity(&self) -> usize {
                CAPACITY as usize
            }

            #[inline]
            fn len(&self) -> usize {
                unsafe { self.inner.consumer_len() }
            }

            #[inline]
            fn pop_many(&self, dst: &mut [MaybeUninit<T>]) -> usize {
                unsafe { self.inner.consumer_pop_many(dst) }
            }

            #[inline]
            fn steal_into(&self, dst: &Self::AssociatedProducer) -> usize {
                unsafe { self.inner.steal_into(&*dst.inner) }
            }
        }

        unsafe impl<T: Send, const CAPACITY: usize> Send for $consumer_name<T, CAPACITY> {}
    };

    ($producer_name:ident, $consumer_name:ident) => {
        generate_spsc_producer_and_consumer!(
            $producer_name,
            $consumer_name,
            NotCachePaddedLongAtomic
        );
    };
}

generate_spsc_producer_and_consumer!(SPSCProducer, SPSCConsumer);

/// Creates a new single-producer, single-consumer queue with the given capacity.
/// Returns [`producer`](SPSCProducer) and [`consumer`](SPSCConsumer).
///
/// It accepts the capacity as a const generic parameter.
/// We recommend using a power of two.
///
/// The producer __should__ be only one while consumers can be cloned.
/// If you want to use more than one producer, don't use this queue.
///
/// # Bounded queue vs. [`unbounded queue`](crate::spsc::new_unbounded)
///
/// - [`maybe_push`](Producer::maybe_push), [`maybe_push_many`](Producer::maybe_push_many)
///   can return an error only for `bounded` queue.
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
/// use parcoll::spsc::{new_bounded, Producer, Consumer};
/// use std::sync::Arc;
///
/// let (mut producer, mut consumer) = new_bounded::<_, 256>();
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
) -> (SPSCProducer<T, CAPACITY>, SPSCConsumer<T, CAPACITY>) {
    let queue = LightArc::new(SPSCBoundedQueue::new());

    (
        SPSCProducer {
            inner: queue.clone(),
            _non_sync: PhantomData,
        },
        SPSCConsumer {
            inner: queue,
            _non_sync: PhantomData,
        },
    )
}

generate_spsc_producer_and_consumer!(
    CachePaddedSPSCProducer,
    CachePaddedSPSCConsumer,
    CachePaddedLongAtomic
);

/// Creates a new single-producer, single-consumer queue with the given capacity.
/// Returns [`producer`](CachePaddedSPSCProducer) and [`consumer`](CachePaddedSPSCConsumer).
///
/// It accepts the capacity as a const generic parameter.
/// We recommend using a power of two.
///
/// The producer __should__ be only one while consumers can be cloned.
/// If you want to use more than one producer, don't use this queue.
///
/// # Bounded queue vs. [`unbounded queue`](crate::spsc::new_unbounded)
///
/// - [`maybe_push`](Producer::maybe_push), [`maybe_push_many`](Producer::maybe_push_many)
///   can return an error only for `bounded` queue.
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
/// use parcoll::spsc::{new_bounded, Producer, Consumer};
/// use std::sync::Arc;
///
/// let (mut producer, mut consumer) = new_bounded::<_, 256>();
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
    CachePaddedSPSCProducer<T, CAPACITY>,
    CachePaddedSPSCConsumer<T, CAPACITY>,
) {
    let queue = LightArc::new(SPSCBoundedQueue::new());

    (
        CachePaddedSPSCProducer {
            inner: queue.clone(),
            _non_sync: PhantomData,
        },
        CachePaddedSPSCConsumer {
            inner: queue,
            _non_sync: PhantomData,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    const CAPACITY: usize = 256;

    #[test]
    fn test_spsc_bounded_size() {
        let queue = SPSCBoundedQueue::<(), CAPACITY>::new();

        assert_eq!(
            size_of_val(&queue),
            size_of::<usize>() + size_of::<LongAtomic>() * 2
        );

        let cache_padded_queue = SPSCBoundedQueue::<(), CAPACITY, CachePaddedLongAtomic>::new();

        assert_eq!(
            size_of_val(&cache_padded_queue),
            size_of::<CachePaddedLongAtomic>() * 2 + size_of::<usize>()
        );
    }

    #[test]
    fn test_spsc_bounded_seq_insertions() {
        let (producer, consumer) = new_bounded::<_, CAPACITY>();

        for i in 0..CAPACITY * 100 {
            producer.maybe_push(i).unwrap();

            assert_eq!(consumer.pop().unwrap(), i);
        }

        for i in 0..CAPACITY {
            producer.maybe_push(i).unwrap();
        }

        assert_eq!(consumer.len(), CAPACITY);
        assert_eq!(producer.len(), CAPACITY);
        assert_eq!(consumer.capacity(), CAPACITY);
    }

    #[test]
    fn test_spsc_bounded_stealing() {
        const TRIES: usize = 10;

        let (producer1, consumer1) = new_bounded::<_, CAPACITY>();
        let (producer2, consumer2) = new_bounded::<_, CAPACITY>();

        let mut stolen = VecDeque::new();

        for _ in 0..TRIES * 2 {
            for i in 0..CAPACITY / 2 {
                producer1.maybe_push(i).unwrap();
            }

            consumer1.steal_into(&producer2);

            while let Some(task) = consumer2.pop() {
                stolen.push_back(task);
            }
        }

        assert!(producer2.is_empty());

        let mut count = 0;

        while let Some(_) = consumer1.pop() {
            count += 1;
        }

        assert_eq!(count + stolen.len(), CAPACITY * TRIES);
    }

    #[test]
    fn test_spsc_bounded_many() {
        const BATCH_SIZE: usize = 30;
        const N: usize = BATCH_SIZE * 100;

        let (producer, consumer) = new_bounded::<_, CAPACITY>();

        for i in 0..N / BATCH_SIZE {
            let slice = (0..BATCH_SIZE)
                .map(|j| i * BATCH_SIZE + j)
                .collect::<Vec<_>>();

            unsafe {
                producer.maybe_push_many(&*slice).unwrap();
            }

            let mut slice = [MaybeUninit::uninit(); BATCH_SIZE];
            consumer.pop_many(slice.as_mut_slice());

            for j in 0..BATCH_SIZE {
                let index = i * BATCH_SIZE + j;

                assert_eq!(unsafe { slice[j].assume_init() }, index);
            }
        }
    }
}
