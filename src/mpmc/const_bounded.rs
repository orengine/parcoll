//! This module provides a multi-producer, multi-consumer bounded queue. Read more in
//! [`new_bounded`].
#![allow(clippy::cast_possible_truncation, reason = "LongNumber is always at least usize")]
use crate::backoff::Backoff;
use crate::number_types::{CachePaddedLongAtomic, LongAtomic, LongNumber, NotCachePaddedLongAtomic};
use crate::{Consumer, LockFreeConsumer, LockFreePopErr, LockFreeProducer, LockFreePushErr, Producer};
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::sync::atomic::Ordering;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use crate::hints::unlikely;
use crate::light_arc::LightArc;
use std::marker::PhantomData;
use crate::single_producer::{SingleLockFreeProducer, SingleProducer};

// Implementation notes for an MPMC (multi-producer, multi-consumer) bounded queue.
//
// This design builds on simpler queue types. If you have not read the SPMC
// (single-producer, multi-consumer) implementation, start there because it is much
// simpler because it has only one producer. In fact, if you compare SPMC and
// SPSC (single-producer, single-consumer) queues, you’ll see that their producer
// code is identical, while the consumer side is only slightly more complex.
// The reason is that concurrent reading is safe, but concurrent writing is not.
//
// In an MPMC queue, multiple producers cannot safely update the tail pointer at
// the same time. We cannot advance `tail` before writing finishes (consumers might
// read uninitialized data), and we cannot advance it after writing finishes (two
// producers could overwrite each other’s slots). This is why a fully lock-free
// ring-buffer MPMC queue is tricky. However, we can make it *almost* lock-free
// and still very fast.
//
// The core idea is to give each slot its own atomic state (conceptually similar
// to semaphore). You could reduce this to one state per group of slots, but
// that hurts performance.
//
// First attempt:
// - Use an atomic boolean per slot for `empty` / `has value`.
// - Producers: update `tail`, write the value, then set the flag.
//   This avoids concurrent writing into the same slot.
// - Consumers: check the flag before reading.
//   Problem: the consumer may have to wait for a writing operation at the head,
//   so `pop` is no longer fully lock-free.
//   Advantage: `push` can be lock-free — producers can write into the next slot
//   while previous producers finish writing.
//
// Step-by-step model for the first attempt:
// Push: load the tail; load the head; compute a length; if full, return;
//       load a slot flag; if `has value`, retry;
//       CAS on tail; if CAS succeeds: write value, set the flag, return;
//       else retry.
// Pop:  load tail; load head; compute a length; if empty, return;
//       load a slot flag; if `empty`, retry;
//       CAS on head; if CAS succeeds: read value, set the flag to empty, return;
//       else retry.
//
// In the first approach, writers checked the slot's flag before writing, and
// readers checked it before reading. This means both sides communicate not
// only through the head and tail indexes but also through these per-slot flags.
// Writers rely on the head index to avoid overwriting unread data, and readers
// rely on the tail index to avoid reading unwritten data.
//
// However, head/tail alone do not tell us *when* a slot was last written or
// read, especially when the queue wraps around. To distinguish "still contains
// unread data from a previous pass" from "empty and ready to write," we need an
// additional concept: laps.
//
// A lap counter increments each time an index (head or tail) wraps around the
// buffer. By associating each slot with the lap number of its last writing, we
// can tell whether the data is from the current pass (safe to read) or from an
// earlier pass (stale for readers, full for writers). This allows us to stop
// checking head for writing safety and tail for reading safety, relying instead
// on per-slot state derived from lap information.
// In the `push` method load only the tail and the lap,
// in the `pop` method load only the head and the lap.
//
// Next optimization:
// Instead of a boolean, store an atomic *state* per slot:
//   - `empty`
//   - `value written this lap` (available for reading, not full for writers)
//   - `full` (value not yet read since a previous lap)
// The “full” state occurs when the slot still holds data from the previous lap
// of writing.
//
// We can encode these states using lap counters and slot indices:
//   - `lap + slot`             -> empty
//   - `current lap + slot + 1` -> value written this lap
//   - `old lap + slot + 1`     -> full (previous lap’s value)
// This "+1" encoding forbids queues of size 1 but allows inexpensive `wrapping_add`
// and simple comparisons (`== number + 1` or `< number`) without special cases.

struct State(LongAtomic);

impl State {
    #[inline(always)]
    fn new(slot_index: LongNumber) -> Self {
        Self(LongAtomic::new(slot_index))
    }

    #[inline(always)]
    fn load(&self, ordering: Ordering) -> LongNumber {
        self.0.load(ordering)
    }

    #[inline(always)]
    fn mark_as_occupied(&self, current_tail: LongNumber, ordering: Ordering) {
        self.0.store(current_tail.wrapping_add(1), ordering);
    }

    #[inline(always)]
    fn mark_as_free(&self, current_head: LongNumber, capacity: LongNumber, ordering: Ordering) {
        self.0.store(current_head.wrapping_add(capacity), ordering);
    }

    #[inline(always)]
    fn is_slot_free(current_tail: LongNumber, state_value: LongNumber) -> bool {
        current_tail == state_value
    }

    #[inline(always)]
    fn is_slot_occupied(current_head: LongNumber, state_value: LongNumber) -> bool {
        current_head.wrapping_add(1) == state_value
    }

    #[inline(always)]
    fn is_queue_empty(current_head: LongNumber, state_value: LongNumber) -> bool {
        current_head == state_value
    }

    #[inline(always)]
    fn is_queue_full(
        current_tail: LongNumber,
        state_value: LongNumber,
        capacity: LongNumber,
    ) -> bool {
        current_tail == state_value.wrapping_add(capacity - 1)
    }
}

struct Slot<T> {
    state: State,
    value: UnsafeCell<MaybeUninit<T>>,
}

impl<T> Slot<T> {
    fn new_without_value(slot_index: LongNumber) -> Self {
        Self {
            state: State::new(slot_index),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    fn state(&self) -> &State {
        &self.state
    }

    unsafe fn read_value(&self) -> T {
        unsafe { self.value.get().cast::<T>().read() }
    }

    unsafe fn write_value(&self, value: T) {
        unsafe { self.value.get().cast::<T>().write(value) }
    }
}

/// A multi-producer, multi-consumer bounded queue.
///
/// It is safe to use when and only when only one thread is writing to the queue at the same time.
///
/// It accepts the atomic wrapper as a generic parameter.
/// It allows using cache-padded atomics or not.
/// You should create type aliases not to write this large type name.
///
/// # Using directly the [`MPMCBoundedQueue`] vs. using [`new_bounded`] or [`new_cache_padded_bounded`].
///
/// Functions [`new_bounded`] and [`new_cache_padded_bounded`] allocate the
/// [`MPMCBoundedQueue`] on the heap in [`LightArc`]
/// and provide separate producer's and consumer's parts.
/// It hurts the performance if you don't need to allocate the queue separately but improves
/// the readability when you need to separate producer and consumer logic and share them.
#[repr(C)]
pub struct MPMCBoundedQueue<
    T,
    const CAPACITY: usize,
    AtomicWrapper: Deref<Target = LongAtomic> + Default = NotCachePaddedLongAtomic,
> {
    tail: AtomicWrapper,
    head: AtomicWrapper,
    // There were two decisions how to store the slots.
    // The first one is to store the slots in one array (it was chosen).
    // The second one is to store `states` and `values` in separate arrays.
    //
    // The first one is more efficient for single `push`/`pop` operations
    // because it provides better single slot cache distributing.
    //
    // The second one is more efficient for `push_many`/`pop_many` operations
    // because it allows loading more `states` at once.
    //
    // But based on observations, it is more likely to use MPMC queue for
    // single `push`/`pop` operations.
    slots: [Slot<T>; CAPACITY],
}

impl<T, const CAPACITY: usize, AtomicWrapper> MPMCBoundedQueue<T, CAPACITY, AtomicWrapper>
where
    AtomicWrapper: Deref<Target = LongAtomic> + Default,
{
    /// Creates a new [`MPMCBoundedQueue`].
    pub fn new() -> Self {
        debug_assert!(
            CAPACITY >= 2,
            "MPMCBoundedQueue should have at least 2 slots"
        );
        debug_assert!(size_of::<MaybeUninit<T>>() == size_of::<T>()); // Assume that we can just cast it

        let mut buffer: MaybeUninit<[Slot<T>; CAPACITY]> = MaybeUninit::uninit();
        let buffer_ptr = unsafe { buffer.assume_init_mut() }.as_mut_ptr();

        for i in 0..CAPACITY {
            unsafe {
                buffer_ptr
                    .add(i)
                    .write(Slot::new_without_value(i as LongNumber));
            };
        }

        Self {
            slots: unsafe { buffer.assume_init() },
            tail: AtomicWrapper::default(),
            head: AtomicWrapper::default(),
        }
    }
}

impl<T, const CAPACITY: usize, AtomicWrapper> Default for MPMCBoundedQueue<T, CAPACITY, AtomicWrapper>
where
    AtomicWrapper: Deref<Target = LongAtomic> + Default,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const CAPACITY: usize, AtomicWrapper> Producer<T> for MPMCBoundedQueue<T, CAPACITY, AtomicWrapper>
where
    AtomicWrapper: Deref<Target = LongAtomic> + Default,
{
    fn capacity(&self) -> usize {
        CAPACITY
    }

    fn len(&self) -> usize {
        let mut head = self.head.load(Relaxed);
        let mut tail = self.tail.load(Relaxed);

        loop {
            let len = tail.wrapping_sub(head) as usize;

            if unlikely(len > CAPACITY) {
                // Inconsistent state (this thread has been preempted
                // after we have loaded `head`,
                // and before we have loaded `tail`),
                // try again

                head = self.head.load(Relaxed);
                tail = self.tail.load(Relaxed);

                continue;
            }

            return len;
        }
    }

    fn is_empty(&self) -> bool {
        self.head.load(Relaxed) == self.tail.load(Relaxed)
    }

    fn maybe_push(&self, value: T) -> Result<(), T> {
        let mut tail = self.tail.load(Relaxed);
        let mut slot: &Slot<T>;

        loop {
            slot = &self.slots[tail as usize % CAPACITY];
            let state = slot.state().load(Acquire);

            if State::is_slot_free(tail, state) {
                // Let the compiler or PU to cache it into the registers
                // and use in inlined functions
                let new_tail = tail.wrapping_add(1);
                let cas_result =
                    self
                        .tail
                        .compare_exchange_weak(tail, new_tail, Relaxed, Relaxed);

                match cas_result {
                    Ok(_) => {
                        unsafe { slot.write_value(value) };

                        slot.state().mark_as_occupied(tail, Release);

                        break Ok(());
                    }
                    Err(current_tail) => {
                        tail = current_tail;
                    }
                }
            } else if State::is_queue_full(tail, state, CAPACITY as LongNumber) {
                // Some value has occupied the slot since the previous lap.
                //
                // It may mean two things:
                // 1. The queue is full;
                // 2. The reader has been preempted,
                //    but another has read the next value.
                //
                // But in both cases, we should not push.
                // We can't try to detect it and push in the next slot.

                break Err(value);
            } else {
                // We lose the race. Try to push again.
                tail = self.tail.load(Relaxed);
                // Likely we were preempted, we should not call Backoff::snooze here.
            }
        }
    }
}

impl<T, const CAPACITY: usize, AtomicWrapper> LockFreeProducer<T> for MPMCBoundedQueue<T, CAPACITY, AtomicWrapper>
where
    AtomicWrapper: Deref<Target = LongAtomic> + Default,
{
    fn lock_free_maybe_push(&self, value: T) -> Result<(), LockFreePushErr<T>> {
        self.maybe_push(value).map_err(LockFreePushErr::Full)
    }
}

macro_rules! generate_pop_body {
    (
        $self:ident,
        $backoff_ident:ident,
        $backoff_init:expr,
        $on_writer_block:block,
        $success_value_ident:ident,
        $on_success_value:block,
        $on_empty:block
    ) => {{
        let mut head = $self.head.load(Relaxed);
        let mut slot: &Slot<T>;
        #[allow(unused, reason = "It improve readability")]
        let $backoff_ident = $backoff_init;

        loop {
            slot = &$self.slots[head as usize % CAPACITY];
            let state = slot.state().load(Acquire);
            // Let the compiler or PU to cache it into the registers and use in inlined functions
            let new_head = head.wrapping_add(1);

            if State::is_slot_occupied(head, state) {
                let cas_result =
                    $self
                        .head
                        .compare_exchange_weak(head, new_head, Relaxed, Relaxed);

                match cas_result {
                    Ok(_) => {
                        let $success_value_ident = unsafe { slot.read_value() };

                        slot
                            .state()
                            .mark_as_free(head, CAPACITY as LongNumber, Release);

                        $on_success_value
                    }
                    Err(current_head) => {
                        head = current_head;
                    }
                }
            } else if State::is_queue_empty(head, state) {
                $on_empty
            } else {
                let old_head = head;
                // We lose the race or the value is still not written. Try to pop again.
                head = $self.head.load(Relaxed);

                if head == old_head {
                    $on_writer_block // Maybe the writer has been preempted.
                }
            }
        }
    }};
}

impl<T, const CAPACITY: usize, AtomicWrapper> Consumer<T> for MPMCBoundedQueue<T, CAPACITY, AtomicWrapper>
where
    AtomicWrapper: Deref<Target = LongAtomic> + Default,
{
    fn capacity(&self) -> usize {
        CAPACITY
    }

    fn len(&self) -> usize {
        Producer::len(self)
    }

    fn pop(&self) -> Option<T> {
        generate_pop_body!(
            self,
            backoff,
            Backoff::new(),
            { backoff.snooze() },
            value,
            {
                return Some(value);
            },
            {
                return None;
            }
        )
    }

    fn pop_many(&self, dst: &mut [MaybeUninit<T>]) -> usize {
        let mut popped = 0;

        while popped < dst.len() {
            if let Some(value) = self.pop() {
                dst[popped].write(value);

                popped += 1;
            } else {
                break;
            }
        }

        popped
    }

    fn steal_into(&self, dst: &impl SingleProducer<T>) -> usize {
        let capacity = dst.capacity() as LongNumber;
        let start_len = Producer::len(self) as LongNumber;
        let to_steal = (start_len / 2).min(capacity);
        let mut i = 0;

        while i < to_steal {
            if let Some(value) = self.pop() {
                unsafe { dst.push_many_unchecked(&[value], &[]) };

                i += 1;
            } else {
                break;
            }
        }

        i as usize
    }
}

impl<T, const CAPACITY: usize, AtomicWrapper> LockFreeConsumer<T> for MPMCBoundedQueue<T, CAPACITY, AtomicWrapper>
where
    AtomicWrapper: Deref<Target = LongAtomic> + Default,
{
    fn lock_free_pop(&self) -> Result<T, LockFreePopErr> {
        generate_pop_body!(
            self,
            _backoff,
            (),
            {
                return Err(LockFreePopErr::ShouldWait);
            },
            value,
            {
                return Ok(value);
            },
            {
                return Err(LockFreePopErr::Empty);
            }
        )
    }

    fn lock_free_pop_many(&self, dst: &mut [MaybeUninit<T>]) -> (usize, bool) {
        let mut popped = 0;

        while popped < dst.len() {
            match self.lock_free_pop() {
                Ok(value) => {
                    dst[popped].write(value);

                    popped += 1;
                }
                Err(LockFreePopErr::Empty) => {
                    return (popped, false);
                }
                Err(LockFreePopErr::ShouldWait) => {
                    return (popped, true);
                }
            }
        }

        (popped, false)
    }

    fn lock_free_steal_into(&self, dst: &impl SingleLockFreeProducer<T>) -> (usize, bool) {
        let capacity = dst.capacity() as LongNumber;
        let start_len = Producer::len(self) as LongNumber;
        let to_steal = (start_len / 2).min(capacity);
        let mut i = 0;

        while i < to_steal {
            let res = self.lock_free_pop();
            match res {
                Ok(value) => {
                    unsafe { dst.push_many_unchecked(&[value], &[]) };

                    i += 1;
                }
                Err(LockFreePopErr::Empty) => break,
                Err(LockFreePopErr::ShouldWait) => return (i as usize, true),
            }
        }

        (i as usize, false)
    }
}

impl<T, const CAPACITY: usize, AtomicWrapper> Drop for MPMCBoundedQueue<T, CAPACITY, AtomicWrapper>
where
    AtomicWrapper: Deref<Target = LongAtomic> + Default,
{
    fn drop(&mut self) {
        let mut head = unsafe { self.head.unsync_load() };
        let tail = unsafe { self.tail.unsync_load() };

        while head != tail {
            unsafe {
                self.slots.as_mut_ptr().add(head as usize % CAPACITY).drop_in_place();
            }
            
            head += 1;
        }
    }
}

/// Generates MPMC producer and consumer.
macro_rules! generate_mpmc_producer_and_consumer {
    ($producer_name:ident, $consumer_name:ident, $atomic_wrapper:ty) => {
        /// The producer of the [`MPMCBoundedQueue`].
        pub struct $producer_name<T: Send, const CAPACITY: usize> {
            inner: LightArc<MPMCBoundedQueue<T, CAPACITY, $atomic_wrapper>>,
            _non_sync: PhantomData<*const ()>,
        }

        impl<T: Send, const CAPACITY: usize> $producer_name<T, CAPACITY> {
            /// Returns a reference to the inner [`MPMCBoundedQueue`].
            pub fn queue(&self) -> &MPMCBoundedQueue<T, CAPACITY, $atomic_wrapper> {
                &*self.inner
            }
        }

        impl<T: Send, const CAPACITY: usize> $crate::Producer<T> for $producer_name<T, CAPACITY> {
            #[inline]
            fn capacity(&self) -> usize {
                CAPACITY as usize
            }

            #[inline]
            fn len(&self) -> usize {
                $crate::Producer::len(&*self.inner)
            }

            #[inline]
            fn maybe_push(&self, value: T) -> Result<(), T> {
                $crate::Producer::maybe_push(&*self.inner, value)
            }
        }

        impl<T: Send, const CAPACITY: usize> $crate::LockFreeProducer<T>
            for $producer_name<T, CAPACITY>
        {
            fn lock_free_maybe_push(&self, value: T) -> Result<(), $crate::lock_free_errors::LockFreePushErr<T>> {
                $crate::LockFreeProducer::lock_free_maybe_push(&*self.inner, value)
            }
        }

        impl<T: Send, const CAPACITY: usize> $crate::multi_producer::MultiProducer<T>
            for $producer_name<T, CAPACITY>
        {}

        impl<T: Send, const CAPACITY: usize> $crate::multi_producer::MultiLockFreeProducer<T>
            for $producer_name<T, CAPACITY>
        {
        }

        impl<T: Send, const CAPACITY: usize> $crate::multi_consumer::MultiConsumerSpawner<T>
            for $producer_name<T, CAPACITY>
        {
            type SpawnedConsumer = $consumer_name<T, CAPACITY>;

            fn spawn_multi_consumer(&self) -> Self::SpawnedConsumer {
                $consumer_name {
                    inner: self.inner.clone(),
                    _non_sync: PhantomData,
                }
            }
        }

        impl<T: Send, const CAPACITY: usize> $crate::multi_consumer::MultiLockFreeConsumerSpawner<T>
            for $producer_name<T, CAPACITY>
        {
            type SpawnedLockFreeConsumer = $consumer_name<T, CAPACITY>;

            fn spawn_multi_lock_free_consumer(
                &self,
            ) -> Self::SpawnedLockFreeConsumer {
                $consumer_name {
                    inner: self.inner.clone(),
                    _non_sync: PhantomData,
                }
            }
        }

        impl<T: Send, const CAPACITY: usize> Clone for $producer_name<T, CAPACITY> {
            fn clone(&self) -> Self {
                Self {
                    inner: self.inner.clone(),
                    _non_sync: PhantomData,
                }
            }
        }

        #[allow(clippy::non_send_fields_in_send_ty, reason = "We guarantee it is safe")]
        unsafe impl<T: Send, const CAPACITY: usize> Send for $producer_name<T, CAPACITY> {}

        /// The consumer of the [`MPMCBoundedQueue`].
        pub struct $consumer_name<T: Send, const CAPACITY: usize> {
            inner: LightArc<MPMCBoundedQueue<T, CAPACITY, $atomic_wrapper>>,
            _non_sync: PhantomData<*const ()>,
        }

        impl<T: Send, const CAPACITY: usize> $consumer_name<T, CAPACITY> {
            /// Returns a reference to the inner [`MPMCBoundedQueue`].
            pub fn queue(&self) -> &MPMCBoundedQueue<T, CAPACITY, $atomic_wrapper> {
                &*self.inner
            }
        }

        impl<T: Send, const CAPACITY: usize> $crate::Consumer<T> for $consumer_name<T, CAPACITY> {
            #[inline]
            fn capacity(&self) -> usize {
                CAPACITY as usize
            }

            #[inline]
            fn len(&self) -> usize {
                $crate::Consumer::len(&*self.inner)
            }

            #[inline]
            fn pop_many(&self, dst: &mut [MaybeUninit<T>]) -> usize {
                $crate::Consumer::pop_many(&*self.inner, dst)
            }

            #[inline(never)]
            fn steal_into(&self, dst: &impl $crate::single_producer::SingleProducer<T>) -> usize {
                $crate::Consumer::steal_into(&*self.inner, dst)
            }
        }

        impl<T: Send, const CAPACITY: usize> $crate::LockFreeConsumer<T>
            for $consumer_name<T, CAPACITY>
        {
            #[inline]
            fn lock_free_pop_many(&self, dst: &mut [MaybeUninit<T>]) -> (usize, bool) {
                $crate::LockFreeConsumer::lock_free_pop_many(&*self.inner, dst)
            }

            #[inline(never)]
            fn lock_free_steal_into(
                &self,
                dst: &impl $crate::single_producer::SingleLockFreeProducer<T>,
            ) -> (usize, bool) {
                $crate::LockFreeConsumer::lock_free_steal_into(&*self.inner, dst)
            }
        }

        impl<T: Send, const CAPACITY: usize> $crate::multi_consumer::MultiConsumer<T>
            for $consumer_name<T, CAPACITY>
        {
        }

        impl<T: Send, const CAPACITY: usize> $crate::multi_consumer::MultiLockFreeConsumer<T>
            for $consumer_name<T, CAPACITY>
        {
        }

        impl<T: Send, const CAPACITY: usize> Clone for $consumer_name<T, CAPACITY> {
            fn clone(&self) -> Self {
                Self {
                    inner: self.inner.clone(),
                    _non_sync: PhantomData,
                }
            }
        }

        impl<T: Send, const CAPACITY: usize> $crate::multi_producer::MultiProducerSpawner<T>
            for $consumer_name<T, CAPACITY>
        {
            type SpawnedProducer = $producer_name<T, CAPACITY>;

            fn spawn_multi_producer(&self) -> Self::SpawnedProducer {
                $producer_name {
                    inner: self.inner.clone(),
                    _non_sync: PhantomData,
                }
            }
        }

        impl<T: Send, const CAPACITY: usize> $crate::multi_producer::MultiLockFreeProducerSpawner<T>
            for $consumer_name<T, CAPACITY>
        {
            type SpawnedLockFreeProducer = $producer_name<T, CAPACITY>;

            fn spawn_multi_lock_free_producer(
                &self,
            ) -> Self::SpawnedLockFreeProducer {
                $producer_name {
                    inner: self.inner.clone(),
                    _non_sync: PhantomData,
                }
            }
        }

        #[allow(clippy::non_send_fields_in_send_ty, reason = "We guarantee it is safe")]
        unsafe impl<T: Send, const CAPACITY: usize> Send for $consumer_name<T, CAPACITY> {}
    };

    ($producer_name:ident, $consumer_name:ident) => {
        generate_mpmc_producer_and_consumer!(
            $producer_name,
            $consumer_name,
            NotCachePaddedLongAtomic
        );
    };
}

generate_mpmc_producer_and_consumer!(MPMCProducer, MPMCConsumer);

/// Creates a new multi-producer, multi-consumer queue with the given capacity.
/// Returns [`producer`](MPMCProducer) and [`consumer`](MPMCConsumer).
///
/// It accepts the capacity as a const generic parameter.
/// We recommend using a power of two.
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
/// use parcoll::mpmc::new_bounded;
/// use parcoll::{Consumer, Producer};
///
/// let (producer, consumer) = new_bounded::<_, 256>();
/// let producer2 = producer.clone();
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
pub fn new_bounded<T: Send, const CAPACITY: usize>() -> (
    MPMCProducer<T, CAPACITY>,
    MPMCConsumer<T, CAPACITY>
) {
    let queue = LightArc::new(MPMCBoundedQueue::new());

    (
        MPMCProducer {
            inner: queue.clone(),
            _non_sync: PhantomData,
        },
        MPMCConsumer {
            inner: queue,
            _non_sync: PhantomData,
        },
    )
}

generate_mpmc_producer_and_consumer!(
    CachePaddedMPMCProducer,
    CachePaddedMPMCConsumer,
    CachePaddedLongAtomic
);

/// Creates a new multi-producer, multi-consumer queue with the given capacity.
/// Returns [`producer`](MPMCProducer) and [`consumer`](MPMCConsumer).
///
/// It accepts the capacity as a const generic parameter.
/// We recommend using a power of two.
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
/// use parcoll::mpmc::new_cache_padded_bounded;
/// use parcoll::{Consumer, Producer};
///
/// let (producer, consumer) = new_cache_padded_bounded::<_, 256>();
/// let producer2 = producer.clone();
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
pub fn new_cache_padded_bounded<T: Send, const CAPACITY: usize>() -> (
    CachePaddedMPMCProducer<T, CAPACITY>,
    CachePaddedMPMCConsumer<T, CAPACITY>,
) {
    let queue = LightArc::new(MPMCBoundedQueue::new());

    (
        CachePaddedMPMCProducer {
            inner: queue.clone(),
            _non_sync: PhantomData,
        },
        CachePaddedMPMCConsumer {
            inner: queue,
            _non_sync: PhantomData,
        },
    )
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use crate::{spsc, Consumer, LockFreeConsumer, LockFreeProducer, Producer};
    use super::*;

    const CAPACITY: usize = 16;

    #[test]
    fn test_mpmc_bounded_seq_insertions() {
        let (producer, consumer) = new_bounded::<_, CAPACITY>();

        for i in 0..CAPACITY  {
            producer.maybe_push(i).unwrap();
        }

        assert!(producer.maybe_push(0).is_err());

        assert_eq!(
            producer.len(),
            CAPACITY
        );

        assert_eq!(
            consumer.len(),
            CAPACITY
        );

        for i in 0..producer.len() {
            assert_eq!(consumer.pop(), Some(i));
        }
    }

    #[test]
    fn test_mpmc_bounded_stealing() {
        const TRIES: usize = 10;

        let (producer1, consumer) = new_bounded::<_, CAPACITY>();
        let (producer2, consumer2) = spsc::new_bounded::<_, CAPACITY>();

        let mut stolen = VecDeque::new();

        for _ in 0..TRIES * 2 {
            for i in 0..CAPACITY / 2 {
                producer1.maybe_push(i).unwrap();
            }

            consumer.steal_into(&producer2);

            while let Some(task) = consumer2.pop() {
                stolen.push_back(task);
            }
        }

        assert!(producer2.is_empty());

        let mut count = 0;

        while let Some(_) = consumer.pop() {
            count += 1;
        }

        assert_eq!(count + stolen.len(), CAPACITY * TRIES);
    }

    #[test]
    fn test_mpmc_lock_free_bounded_seq_insertions() {
        let (producer, consumer) = new_bounded::<_, CAPACITY>();

        for i in 0..CAPACITY  {
            producer.lock_free_maybe_push(i).unwrap();
        }

        assert!(producer.lock_free_maybe_push(0).is_err());

        assert_eq!(
            producer.len(),
            CAPACITY
        );

        assert_eq!(
            consumer.len(),
            CAPACITY
        );

        for i in 0..producer.len() {
            assert_eq!(consumer.lock_free_pop().unwrap(), i);
        }
    }

    #[test]
    fn test_mpmc_lock_free_bounded_stealing() {
        const TRIES: usize = 10;

        let (producer1, consumer) = new_bounded::<_, CAPACITY>();
        let (producer2, consumer2) = spsc::new_bounded::<_, CAPACITY>();

        let mut stolen = VecDeque::new();

        for _ in 0..TRIES * 2 {
            for i in 0..CAPACITY / 2 {
                producer1.lock_free_maybe_push(i).unwrap();
            }

            consumer.lock_free_steal_into(&producer2);

            while let Ok(task) = consumer2.lock_free_pop() {
                stolen.push_back(task);
            }
        }

        assert!(producer2.is_empty());

        let mut count = 0;

        while let Ok(_) = consumer.lock_free_pop() {
            count += 1;
        }

        assert_eq!(count + stolen.len(), CAPACITY * TRIES);
    }
}