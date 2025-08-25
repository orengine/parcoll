//! This module provides a single-producer multi-consumer unbounded queue. Read more in
//! [`new_unbounded`].
#![allow(
    clippy::cast_possible_truncation,
    reason = "LongNumber should be synonymous to usize"
)]
use orengine_utils::light_arc::LightArc;
use orengine_utils::hints::unlikely;
use orengine_utils::hints::cold_path;
use crate::batch_receiver::BatchReceiver;
use crate::buffer_version::{
    pack_version_and_tail, unpack_version_and_tail, CachedVersion, Version,
};
use crate::cache_padded::{CachePaddedAtomicU32, CachePaddedAtomicU64};
use crate::loom_bindings::sync::atomic::{AtomicU32, AtomicU64};
use crate::naive_rw_lock::NaiveRWLock;
use crate::number_types::{NotCachePaddedAtomicU32, NotCachePaddedAtomicU64};
use crate::suspicious_orders::SUSPICIOUS_RELAXED_ACQUIRE;
use crate::sync_cell::{LockFreeSyncCell, SyncCell};
use std::cell::{Cell, UnsafeCell};
use std::marker::PhantomData;
use std::mem::{needs_drop, MaybeUninit};
use std::ops::Deref;
use std::sync::atomic::Ordering;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::{ptr, slice};

/// The single-producer, multi-consumer ring-based _unbounded_ queue.
///
/// It is safe to use when and only when only one thread is writing to the queue at the same time.
///
/// You can call `producer_` methods for the producer and `consumer_` methods for the consumers.
///
/// It accepts two atomic wrappers as generic parameters.
/// It allows using cache-padded atomics or not.
/// You should create type aliases not to write this large type name.
///
/// # Why it is private?
///
/// It is private because it needs [`CachedVersion`] to work,
/// and it is useless to use [`CachedVersion`] without separate consumers.
/// It is too expensive to load the [`Version`] for any consumer method.
/// This behavior may be changed in the future.
///
/// It doesn't implement the [`Producer`] and [`Consumer`] traits because all producer methods
/// are unsafe (can be called only by one thread).
///
/// # `SyncCell`
///
/// It accepts the [`SyncCell`] as a generic.
/// If it is [`LockFreeSyncCell`], the queue is fully lock-free.
/// Else, the producer's methods are not lock-free on slow paths.
#[repr(C)]
pub(crate) struct SPMCUnboundedQueue<
    T,
    SC,
    AtomicU32Wrapper = NotCachePaddedAtomicU32,
    AtomicU64Wrapper = NotCachePaddedAtomicU64,
> where
    T: Send,
    SC: SyncCell<LightArc<Version<T>>>,
    AtomicU32Wrapper: Deref<Target = AtomicU32> + Default,
    AtomicU64Wrapper: Deref<Target = AtomicU64> + Default,
{
    /// First the producer updates the real version,
    /// and next sets a new id. The version id is monotonic.
    tail_and_version: AtomicU64Wrapper,
    head: AtomicU32Wrapper,
    cached_head: Cell<u32>,
    last_version: SC,
    phantom_data: PhantomData<T>,
}

impl<T: Send, SC, AtomicU32Wrapper, AtomicU64Wrapper>
    SPMCUnboundedQueue<T, SC, AtomicU32Wrapper, AtomicU64Wrapper>
where
    SC: SyncCell<LightArc<Version<T>>>,
    AtomicU32Wrapper: Deref<Target = AtomicU32> + Default,
    AtomicU64Wrapper: Deref<Target = AtomicU64> + Default,
{
    /// Creates a new queue with the given capacity.
    fn with_capacity(capacity: usize) -> Self {
        Self {
            tail_and_version: AtomicU64Wrapper::default(),
            head: AtomicU32Wrapper::default(),
            cached_head: Cell::new(0),
            last_version: SC::from_value(Version::alloc_new(capacity, 0)),
            phantom_data: PhantomData,
        }
    }

    /// Creates a new queue with the default capacity.
    fn new() -> Self {
        Self::with_capacity(4)
    }

    /// Updates the version of the queue or returns `false`.
    ///
    /// If it returned `false`, then we should guess that the producer has been preempted,
    /// and we should retry the operation after some time.
    #[must_use]
    fn update_version(&self, version: &mut CachedVersion<T>) -> bool {
        let updated = self.last_version.try_with(|new_version| {
            // We shouldn't check the version id, because the producer first updates the version
            // and only then next updates the version id.
            // The version_id in `tail_and_version`
            // can mismatch with the version id
            // only if the producer has already updated the version but not the version id,
            // and the consumer tries to load the new version.
            //
            // We can represent this as:
            // 1. The consumer loads the version A.
            // 2. The producer updates the version and the version id to B.
            // 3. The producer updates the version to C, but is preempted.
            // 4. The consumer loads the version id B and the version C.
            //
            // Because the producer copies all values before updating the version,
            // the consumer can read B or C.
            // But obviously, we should return the version C not to load it again.

            *version = CachedVersion::from_arc_version(new_version.clone());
        });

        // If it is `None`, then we should guess that the producer has been preempted.
        // It is too expensive to wait.
        // It is very unlikely to happen because the consumer tries to update the version
        // only after the producer updates the version id;
        // therefore, we can be here only
        // if the producer updates the version and the version id from A to B
        // and then locks the version to update from B to C,
        // and the consumer tries to update from A to B.

        // It is always true when the `SC` is lock-free.

        updated.is_some()
    }

    /// Returns the length of the queue by the given `head` and `tail`.
    #[inline]
    fn len(head: u32, tail: u32) -> usize {
        tail.wrapping_sub(head) as usize
    }

    /// Unsynchronously loads the tail.
    ///
    /// # Safety
    ///
    /// It is called only by the producer.
    unsafe fn unsync_load_tail(&self) -> u32 {
        let tail_and_version = unsafe { self.tail_and_version.unsync_load() };

        tail_and_version as u32
    }

    /// Synchronously loads the tail and version.
    fn sync_load_version_and_tail(&self, ordering: Ordering) -> (u32, u32) {
        let tail_and_version = self.tail_and_version.load(ordering);

        unpack_version_and_tail(tail_and_version)
    }

    /// Synchronously loads the version.
    fn sync_load_version(&self, ordering: Ordering) -> u32 {
        let tail_and_version = self.tail_and_version.load(ordering);

        (tail_and_version >> 32) as u32
    }
}

// Producer
impl<T: Send, SC, AtomicU32Wrapper, AtomicU64Wrapper>
    SPMCUnboundedQueue<T, SC, AtomicU32Wrapper, AtomicU64Wrapper>
where
    SC: SyncCell<LightArc<Version<T>>>,
    AtomicU32Wrapper: Deref<Target = AtomicU32> + Default,
    AtomicU64Wrapper: Deref<Target = AtomicU64> + Default,
{
    /// Returns the length of the queue.
    ///
    /// # Safety
    ///
    /// It is called only by the producer.
    #[inline]
    unsafe fn producer_len(&self) -> usize {
        let tail = unsafe { self.unsync_load_tail() }; // only the producer can change tail
        self.cached_head
            .set(self.head.load(SUSPICIOUS_RELAXED_ACQUIRE));

        // We can avoid checking the version
        // because the producer always has the latest version.

        Self::len(self.cached_head.get(), tail)
    }

    /// Returns the capacity of the queue.
    ///
    /// # Safety
    ///
    /// It is called only by the producer.
    #[inline]
    unsafe fn producer_capacity(version: &CachedVersion<T>) -> usize {
        // The producer always has the latest version.
        version.capacity()
    }

    /// Pushes a slice into the queue. Returns a new tail (not index).
    fn copy_slice(
        buffer_ptr: *mut T,
        start_tail: u32,
        slice: &[T],
        version: &CachedVersion<T>,
    ) -> u32 {
        let tail_idx = version.physical_index(start_tail) as usize;

        if tail_idx + slice.len() <= version.capacity() {
            unsafe {
                ptr::copy_nonoverlapping(slice.as_ptr(), buffer_ptr.add(tail_idx), slice.len());
            };
        } else {
            let right = version.capacity() - tail_idx;

            unsafe {
                ptr::copy_nonoverlapping(slice.as_ptr(), buffer_ptr.add(tail_idx), right);
                ptr::copy_nonoverlapping(
                    slice.as_ptr().add(right),
                    buffer_ptr,
                    slice.len() - right,
                );
            }
        }

        start_tail.wrapping_add(slice.len() as u32)
    }

    /// Creates a new version and writes it but not updates the tail.
    /// Returns the new version and the new tail.
    fn create_new_version_and_write_it_but_not_update_tail(
        &self,
        head: u32,
        mut tail: u32,
        new_capacity: usize,
        old_version: &CachedVersion<T>,
    ) -> (CachedVersion<T>, u32) {
        let new_version: LightArc<Version<T>> =
            Version::alloc_new(new_capacity, old_version.id() + 1);

        // The key idea is to transform the buffer viewed as:
        // [ 7 8 1 2 3 4 5 6 ]
        //       ^ head_idx
        //       ^ tail_idx
        // into:
        // [ X X 1 2 3 4 5 6 7 8 X X X X X X ]
        //       ^ head_idx      ^ tail_idx
        // It keeps the order
        // and allows consumers to read the value from the loaded head.

        let (src_right, src_left): (&[T], &[T]) = unsafe {
            if unlikely(head == tail) {
                (&[], &[])
            } else {
                let old_head_idx = old_version.physical_index(head) as usize;
                let old_tail_idx = old_version.physical_index(tail) as usize;

                if old_head_idx < old_tail_idx {
                    (
                        slice::from_raw_parts(
                            old_version.thin_ptr().add(old_head_idx).cast(),
                            old_tail_idx - old_head_idx,
                        ),
                        &[],
                    )
                } else {
                    (
                        slice::from_raw_parts(
                            old_version.thin_ptr().add(old_head_idx).cast(),
                            old_version.capacity() - old_head_idx,
                        ),
                        slice::from_raw_parts(old_version.thin_ptr().cast(), old_tail_idx),
                    )
                }
            }
        };

        let cached_version = CachedVersion::from_arc_version(new_version.clone());

        tail = Self::copy_slice(
            unsafe { cached_version.thin_mut_ptr() }.cast::<T>(),
            head,
            src_right,
            &cached_version,
        );
        tail = Self::copy_slice(
            unsafe { cached_version.thin_mut_ptr() }.cast::<T>(),
            tail,
            src_left,
            &cached_version,
        );

        self.last_version.swap(new_version);

        (cached_version, tail)
    }

    /// Updates the capacity of the queue.
    ///
    /// # Safety
    ///
    /// It is called only by the producer,
    /// and the provided capacity should be more than the current capacity
    /// and less than `u32::MAX`
    /// and be a power of two when the "`unbounded_slices_always_pow2`" is enabled.
    unsafe fn producer_reserve(&self, new_capacity: usize, version: &mut CachedVersion<T>) {
        debug_assert!(
            new_capacity > version.capacity(),
            "new_capacity should be more than version.capacity()"
        );
        debug_assert!(
            u32::try_from(new_capacity).is_ok(),
            "new_capacity should be less than u32::MAX"
        );
        debug_assert!(
            !cfg!(feature = "unbounded_slices_always_pow2") || new_capacity.is_power_of_two(),
            "new_capacity should be power of two"
        );

        let tail = unsafe { self.unsync_load_tail() }; // only the producer can change tail
        let (cached_version, tail) = self.create_new_version_and_write_it_but_not_update_tail(
            self.cached_head.get(),
            tail,
            new_capacity,
            version,
        );

        self.tail_and_version
            .store(pack_version_and_tail(cached_version.id(), tail), Release);

        *version = cached_version;
    }

    /// Pops a value from the queue.
    ///
    /// # Safety
    ///
    /// It should be called only by the producer.
    #[inline]
    unsafe fn producer_pop(&self, version: &CachedVersion<T>) -> Option<T> {
        // The producer always has the latest version.

        let tail = unsafe { self.unsync_load_tail() }; // only the producer can change tail
        let mut head = self.cached_head.get();

        loop {
            if unlikely(head == tail) {
                self.cached_head
                    .set(self.head.load(SUSPICIOUS_RELAXED_ACQUIRE));

                if unlikely(head == self.cached_head.get()) {
                    return None;
                }

                head = self.cached_head.get();
            }

            let new_head = head.wrapping_add(1);

            match self
                .head
                .compare_exchange_weak(head, new_head, Release, Relaxed)
            {
                Ok(_) => {
                    // We are the only producer,
                    // so we can don't worry
                    // about someone overwriting the value before we read it

                    self.cached_head.set(new_head);

                    return Some(unsafe {
                        version
                            .thin_ptr()
                            .add(version.physical_index(head) as usize)
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
    /// Returns the number of popped values.
    ///
    /// # Safety
    ///
    /// It should be called only by the producer.
    #[inline]
    unsafe fn producer_pop_many(
        &self,
        dst: &mut [MaybeUninit<T>],
        version: &CachedVersion<T>,
    ) -> usize {
        // The producer always has the latest version.

        let tail = unsafe { self.unsync_load_tail() }; // only the producer can change tail
        let mut head = self.cached_head.get();

        loop {
            let mut available = Self::len(head, tail);
            let mut n = dst.len().min(available);

            if n == 0 {
                self.cached_head
                    .set(self.head.load(SUSPICIOUS_RELAXED_ACQUIRE));

                if unlikely(head == self.cached_head.get()) {
                    return 0;
                }

                head = self.cached_head.get();
                available = Self::len(head, tail);
                n = dst.len().min(available);
            }

            debug_assert!(n <= version.capacity(), "Bug occurred, please report it.");

            match self.head.compare_exchange_weak(
                head,
                head.wrapping_add(n as u32),
                Release,
                Relaxed,
            ) {
                Ok(_) => {
                    // We are the only producer,
                    // so we can don't worry
                    // about someone overwriting the value before we read it.

                    let dst_ptr = dst.as_mut_ptr();
                    let head_idx = version.physical_index(head) as usize;
                    let right = version.capacity() - head_idx;

                    if n <= right {
                        // No wraparound, copy in one shot
                        unsafe {
                            ptr::copy_nonoverlapping(version.thin_ptr().add(head_idx), dst_ptr, n);
                        }
                    } else {
                        unsafe {
                            // Wraparound: copy right half then left half
                            ptr::copy_nonoverlapping(
                                version.thin_ptr().add(head_idx),
                                dst_ptr,
                                right,
                            );
                            ptr::copy_nonoverlapping(
                                version.thin_ptr(),
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
    unsafe fn push_unchecked(&self, value: T, tail: u32, version: &CachedVersion<T>) {
        // The producer always has the latest version.

        unsafe {
            version
                .thin_ptr()
                .add(version.physical_index(tail) as usize)
                .cast_mut()
                .write(MaybeUninit::new(value));
        }

        self.tail_and_version.store(
            pack_version_and_tail(version.id(), tail.wrapping_add(1)),
            Release,
        );
    }

    /// Updates the version and resizes the queue.
    /// Then it insets the provided slice.
    ///
    /// # Safety
    ///
    /// It should be called only by the producer.
    #[inline(never)]
    #[cold]
    unsafe fn handle_overflow(
        &self,
        head: u32,
        tail: u32,
        version: &mut CachedVersion<T>,
        values: &[T],
    ) {
        let mut new_capacity = Version::<T>::greater_capacity(version.capacity());
        while new_capacity <= version.capacity() + values.len() {
            new_capacity = Version::<T>::greater_capacity(new_capacity);
        }

        let (cached_version, tail) = self.create_new_version_and_write_it_but_not_update_tail(
            head,
            tail,
            new_capacity,
            version,
        );

        let new_tail = Self::copy_slice(
            unsafe { cached_version.thin_mut_ptr().cast() },
            tail,
            values,
            &cached_version,
        );
        self.tail_and_version.store(
            pack_version_and_tail(cached_version.id(), new_tail),
            Release,
        );

        // Here we don't need the previous version anymore.
        *version = cached_version;
    }

    /// Pushes a value to the queue.
    /// Because the queue is unbounded, this method always succeeds.
    ///
    /// # Safety
    ///
    /// It should be called only by the producer.
    #[inline]
    unsafe fn producer_push(&self, value: T, version: &mut CachedVersion<T>) {
        let tail = unsafe { self.unsync_load_tail() }; // only the producer can change tail
        let head = self.cached_head.get();

        if unlikely(Self::len(head, tail) >= version.capacity()) {
            self.cached_head
                .set(self.head.load(SUSPICIOUS_RELAXED_ACQUIRE));

            if unlikely(head == self.cached_head.get()) {
                unsafe { self.handle_overflow(head, tail, version, &[value]) };

                return;
            }

            // The queue is not full
        }

        unsafe { self.push_unchecked(value, tail, version) };
    }

    /// Pushes many values to the queue.
    ///
    /// # Safety
    ///
    /// It should be called only by the producer, and the space is enough.
    #[inline]
    unsafe fn producer_push_many_unchecked(
        &self,
        first: &[T],
        last: &[T],
        version: &CachedVersion<T>,
    ) {
        if cfg!(debug_assertions) {
            let tail = unsafe { self.unsync_load_tail() }; // only the producer can change tail
            let head = self.head.load(SUSPICIOUS_RELAXED_ACQUIRE);

            debug_assert!(Self::len(head, tail) + first.len() + last.len() <= version.capacity());
        }

        // It is SPMC, and it is expected that the capacity is enough.

        let mut tail = unsafe { self.unsync_load_tail() }; // only the producer can change tail

        tail = Self::copy_slice(
            unsafe { version.thin_mut_ptr().cast() },
            tail,
            first,
            version,
        );
        tail = Self::copy_slice(
            unsafe { version.thin_mut_ptr().cast() },
            tail,
            last,
            version,
        );

        self.tail_and_version
            .store(pack_version_and_tail(version.id(), tail), Release);
    }

    /// Pushes many values to the queue.
    ///
    /// # Safety
    ///
    /// It should be called only by the producer.
    #[inline]
    unsafe fn producer_push_many(&self, slice: &[T], version: &mut CachedVersion<T>) {
        let mut tail = unsafe { self.unsync_load_tail() }; // only the producer can change tail

        if unlikely(Self::len(self.cached_head.get(), tail) + slice.len() > version.capacity()) {
            self.cached_head
                .set(self.head.load(SUSPICIOUS_RELAXED_ACQUIRE));

            if unlikely(Self::len(self.cached_head.get(), tail) + slice.len() > version.capacity())
            {
                unsafe { self.handle_overflow(self.cached_head.get(), tail, version, slice) };

                return;
            }

            // The queue is not full
        }

        tail = Self::copy_slice(
            unsafe { version.thin_mut_ptr().cast() },
            tail,
            slice,
            version,
        );

        self.tail_and_version
            .store(pack_version_and_tail(version.id(), tail), Release);
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
        version: &mut CachedVersion<T>,
    ) -> Result<FSuccess, FError> {
        debug_assert!(left.len() + right.len() + self.producer_len() <= version.capacity());

        let mut new_tail = Self::copy_slice(
            unsafe { version.thin_mut_ptr().cast() },
            unsafe { self.unsync_load_tail() }, // only the producer can change tail
            right,
            version,
        );
        new_tail = Self::copy_slice(
            unsafe { version.thin_mut_ptr().cast() },
            new_tail,
            left,
            version,
        );

        let should_commit = condition();
        match should_commit {
            Ok(res) => {
                self.tail_and_version
                    .store(pack_version_and_tail(version.id(), new_tail), Release);

                Ok(res)
            }
            Err(err) => Err(err),
        }
    }
}

// Consumers
impl<T: Send, SC, AtomicU32Wrapper, AtomicU64Wrapper>
    SPMCUnboundedQueue<T, SC, AtomicU32Wrapper, AtomicU64Wrapper>
where
    SC: SyncCell<LightArc<Version<T>>>,
    AtomicU32Wrapper: Deref<Target = AtomicU32> + Default,
    AtomicU64Wrapper: Deref<Target = AtomicU64> + Default,
{
    /// Returns the capacity of the queue.
    #[inline]
    fn consumer_capacity(&self, version: &mut CachedVersion<T>) -> usize {
        let last_version_id = self.sync_load_version(SUSPICIOUS_RELAXED_ACQUIRE);
        if version.id() == last_version_id {
            return version.capacity();
        }

        cold_path();

        let _was_updated = self.update_version(version);

        // If was_updated, the capacity is valid.
        // If !was_updated we should guess that the producer has been preempted,
        // and for some time the capacity of the current version is valid.
        version.capacity()
    }

    /// Returns the length of the queue.
    #[inline]
    fn consumer_len(&self, version: &mut CachedVersion<T>) -> usize {
        loop {
            let (last_version_id, tail) =
                self.sync_load_version_and_tail(SUSPICIOUS_RELAXED_ACQUIRE);
            let head = self.head.load(SUSPICIOUS_RELAXED_ACQUIRE);
            let len = Self::len(head, tail);

            if unlikely(len > version.capacity()) {
                // Three possible reasons:
                // 1. Inconsistent state (this thread has been preempted
                //    after we have loaded `tail`,
                //    and before we have loaded `head`);
                // 2. The new version was created and has more capacity;
                // 3. The first reason and the second reason simultaneously.

                if unlikely(last_version_id == version.id()) {
                    // Case 1, we can retry.
                    continue;
                }

                let was_updated = self.update_version(version);
                if unlikely(!was_updated) {
                    // We can't reliably return the length in this situation.
                    // But it is not a problem.
                    // This method can be used for two purposes:
                    // 1. In tests to check if all values were pushed;
                    // 2. To check if the queue is empty -> the reading is possible.
                    //
                    // The first case is impossible, because not to fail the test accidentally,
                    // this method can be called
                    // only when concurrent work with the queue is impossible;
                    // therefore, in this case we can't be here
                    // (the update_version method always returns `true`
                    // without the concurrent work).
                    //
                    // For the second case, we can return zero
                    // because the reading is impossible.
                    return 0;
                }
            }

            return len;
        }
    }

    /// Pops many values from the queue to the `dst`.
    /// Returns the number of values popped and whether the operation failed
    /// because it should wait.
    ///
    /// It is lock-free.
    #[inline]
    fn consumer_lock_free_pop_many(
        &self,
        dst: &mut [MaybeUninit<T>],
        version: &mut CachedVersion<T>,
    ) -> (usize, bool) {
        let mut head = self.head.load(SUSPICIOUS_RELAXED_ACQUIRE);
        let (mut last_version_id, mut tail) = self.sync_load_version_and_tail(Acquire);

        loop {
            if unlikely(version.id() < last_version_id) {
                if unlikely(!self.update_version(version)) {
                    // We can't reliably calculate the length in this situation.
                    return (0, true);
                }

                (last_version_id, tail) = self.sync_load_version_and_tail(Acquire);

                continue;
            }

            let available = Self::len(head, tail);
            let n = dst.len().min(available);

            if n == 0 {
                return (0, false);
            }

            if unlikely(n > version.capacity()) {
                // Inconsistent state (this thread has been preempted
                // after we have loaded `head`,
                // and before we have loaded `tail`).

                head = self.head.load(SUSPICIOUS_RELAXED_ACQUIRE);
                (last_version_id, tail) = self.sync_load_version_and_tail(Acquire);

                continue;
            }

            let dst_ptr = dst.as_mut_ptr();
            let head_idx = version.physical_index(head) as usize;
            let right = version.capacity() - head_idx;

            // We optimistically copy the values from the buffer into the dst.
            // On CAS failure, we forget the copied values and try again.
            // It is safe because we can concurrently read from the head.

            if n <= right {
                // No wraparound, copy in one shot
                unsafe {
                    ptr::copy_nonoverlapping(version.thin_mut_ptr().add(head_idx), dst_ptr, n);
                }
            } else {
                unsafe {
                    // Wraparound: copy right half then left half
                    ptr::copy_nonoverlapping(version.thin_ptr().add(head_idx), dst_ptr, right);
                    ptr::copy_nonoverlapping(version.thin_ptr(), dst_ptr.add(right), n - right);
                }
            }

            // Now claim ownership
            // CAS is strong because we don't want to recopy the values
            match self
                .head
                .compare_exchange(head, head.wrapping_add(n as u32), Release, Relaxed)
            {
                Ok(_) => return (n, false),
                Err(actual_head) => {
                    // CAS failed, forget read values (they're MaybeUninit, so it's fine)
                    // But don't try to drop, just retry

                    head = actual_head;
                    (last_version_id, tail) = self.sync_load_version_and_tail(Acquire);
                }
            }
        }
    }

    /// Pops many values from the queue to the `dst`.
    /// Returns the number of values popped.
    ///
    /// It can return zero even if the queue is not empty,
    /// if the producer is preempted while pushing.
    #[inline]
    fn consumer_pop_many(
        &self,
        dst: &mut [MaybeUninit<T>],
        version: &mut CachedVersion<T>,
    ) -> usize {
        self.consumer_lock_free_pop_many(dst, version).0
    }

    /// Steals many values from the consumer to the `dst`.
    /// Returns the number of values stolen and whether the operation failed
    /// because it should wait.
    ///
    /// It is lock-free when the provided producer is lock-free.
    #[inline]
    fn steal_into(
        &self,
        dst: &impl crate::single_producer::SingleProducer<T>,
        src_version: &mut CachedVersion<T>,
    ) -> (usize, bool) {
        let mut src_head = self.head.load(SUSPICIOUS_RELAXED_ACQUIRE);
        let (mut src_last_version_id, mut src_tail) = self.sync_load_version_and_tail(Acquire);

        loop {
            if unlikely(src_version.id() < src_last_version_id) {
                if unlikely(!self.update_version(src_version)) {
                    // We can't reliably calculate the length in this situation.
                    return (0, true);
                }

                (src_last_version_id, src_tail) = self.sync_load_version_and_tail(Acquire);

                continue;
            }

            if cfg!(debug_assertions) {
                assert!(dst.is_empty(), "dst must be empty when stealing");
            }

            let n = Self::len(src_head, src_tail) / 2;
            if n > src_version.capacity() / 2 {
                // Inconsistent state (this thread has been preempted
                // after we have loaded `src_head`,
                // and before we have loaded `src_tail`);

                src_head = self.head.load(SUSPICIOUS_RELAXED_ACQUIRE);
                (src_last_version_id, src_tail) = self.sync_load_version_and_tail(Acquire);

                continue;
            }

            if !cfg!(feature = "always_steal") && n < 4 || n == 0 {
                // we don't steal less than 4 by default
                // because else we may lose more because of cache locality and NUMA awareness
                return (0, false);
            }

            let n = n.min(dst.capacity()); // dst is empty, so the capacity is the number of free slots

            let src_head_idx = src_version.physical_index(src_head) as usize;

            let (src_right, src_left): (&[T], &[T]) = unsafe {
                let right_occupied = src_version.capacity() - src_head_idx;
                if n <= right_occupied {
                    (
                        slice::from_raw_parts(src_version.thin_ptr().add(src_head_idx).cast(), n),
                        &[],
                    )
                } else {
                    (
                        slice::from_raw_parts(
                            src_version.thin_ptr().add(src_head_idx).cast(),
                            right_occupied,
                        ),
                        slice::from_raw_parts(src_version.thin_ptr().cast(), n - right_occupied),
                    )
                }
            };

            let cas_closure = || {
                // CAS is strong because we don't want to recopy the values
                self.head.compare_exchange(
                    src_head,
                    src_head.wrapping_add(n as u32),
                    Release,
                    Relaxed,
                )
            };

            let res = unsafe { dst.copy_and_commit_if(src_right, src_left, cas_closure) };
            match res {
                Ok(_) => {
                    return (n, false);
                }
                Err(current_head) => {
                    // Another thread has read the same values,
                    // forget them and full retry

                    src_head = current_head;
                    (src_last_version_id, src_tail) = self.sync_load_version_and_tail(Acquire);
                }
            }
        }
    }
}

#[allow(clippy::non_send_fields_in_send_ty, reason = "We guarantee it is Send")]
unsafe impl<T: Send, SC, AtomicU32Wrapper, AtomicU64Wrapper> Send
    for SPMCUnboundedQueue<T, SC, AtomicU32Wrapper, AtomicU64Wrapper>
where
    SC: SyncCell<LightArc<Version<T>>>,
    AtomicU32Wrapper: Deref<Target = AtomicU32> + Default,
    AtomicU64Wrapper: Deref<Target = AtomicU64> + Default,
{
}
unsafe impl<T: Send, SC, AtomicU32Wrapper, AtomicU64Wrapper> Sync
    for SPMCUnboundedQueue<T, SC, AtomicU32Wrapper, AtomicU64Wrapper>
where
    SC: SyncCell<LightArc<Version<T>>>,
    AtomicU32Wrapper: Deref<Target = AtomicU32> + Default,
    AtomicU64Wrapper: Deref<Target = AtomicU64> + Default,
{
}

impl<T: Send, SC, AtomicU32Wrapper, AtomicU64Wrapper> Drop
    for SPMCUnboundedQueue<T, SC, AtomicU32Wrapper, AtomicU64Wrapper>
where
    SC: SyncCell<LightArc<Version<T>>>,
    AtomicU32Wrapper: Deref<Target = AtomicU32> + Default,
    AtomicU64Wrapper: Deref<Target = AtomicU64> + Default,
{
    fn drop(&mut self) {
        // While dropping, there is no concurrency

        if needs_drop::<T>() {
            let tail = unsafe { self.unsync_load_tail() };
            let version = self.last_version.get_mut();
            let mut head = unsafe { self.head.unsync_load() };

            while head != tail {
                unsafe {
                    ptr::drop_in_place(
                        version
                            .thin_mut_ptr()
                            .add(version.physical_index(head) as usize)
                            .cast::<T>(),
                    );
                }

                head = head.wrapping_add(1);
            }
        }
    }
}

/// Generates SPMC producer and consumer.
macro_rules! generate_spmc_producer_and_consumer {
    ($producer_name:ident, $consumer_name:ident, $atomic_u32_wrapper:ty, $long_atomic_wrapper:ty, $sync_cell:ty) => {
        /// The producer of the [`SPMCUnboundedQueue`].
        pub struct $producer_name<T: Send, SC: SyncCell<LightArc<Version<T>>> = $sync_cell>
        {
            inner: LightArc<SPMCUnboundedQueue<T, SC, $atomic_u32_wrapper, $long_atomic_wrapper>>,
            cached_version: UnsafeCell<CachedVersion<T>>, // The producer is not Sync, it needs only shared references and it never gets two mutable references to this field
            _non_sync: PhantomData<*const ()>,
        }

        impl<T: Send, SC: SyncCell<LightArc<Version<T>>>> $producer_name<T, SC> {
            /// Returns a mutable reference to the cached version.
            #[allow(clippy::mut_from_ref, reason = "It improves readability")]
            #[inline]
            fn cached_version(&self) -> &mut CachedVersion<T> {
                unsafe { &mut *self.cached_version.get() }
            }

            /// Updates the capacity of the queue to the given value.
            ///
            /// # Safety
            ///
            /// The provided capacity should be more than the current capacity
            /// and less than `u32::MAX`
            /// and be a power of two when the `unbounded_slices_always_pow2` is enabled.
            pub fn reserve(&self, capacity: usize) {
                unsafe {
                    self
                        .inner
                        .producer_reserve(capacity, self.cached_version())
                };
            }
        }

        impl<T: Send, SC: SyncCell<LightArc<Version<T>>>> $crate::Producer<T> for $producer_name<T, SC> {
             #[inline]
            fn capacity(&self) -> usize {
                // The producer always has the latest version.
                unsafe { SPMCUnboundedQueue::<T, SC, $atomic_u32_wrapper, $long_atomic_wrapper>::producer_capacity(self.cached_version()) }
            }

            #[inline]
            fn len(&self) -> usize {
                unsafe { self.inner.producer_len() }
            }

            #[inline]
            fn maybe_push(&self, value: T) -> Result<(), T> {
                unsafe { self.inner.producer_push(value, self.cached_version()) };

                Ok(())
            }
        }

        impl<T: Send, SC> $crate::LockFreeProducer<T> for $producer_name<T, SC>
        where
            SC: SyncCell<LightArc<Version<T>>> + LockFreeSyncCell<LightArc<Version<T>>>
        {
            fn lock_free_maybe_push(&self, value: T) -> Result<(), $crate::lock_free_errors::LockFreePushErr<T>> {
                unsafe { self.inner.producer_push(value, self.cached_version()) };

                Ok(())
            }
        }

        impl<T: Send, SC: SyncCell<LightArc<Version<T>>>> $crate::single_producer::SingleProducer<T> for $producer_name<T, SC> {
            unsafe fn push_many_unchecked(&self, first: &[T], last: &[T]) {
                unsafe {
                    self
                    .inner
                        .producer_push_many_unchecked(first, last, self.cached_version())
                }
            }

            unsafe fn maybe_push_many(&self, slice: &[T]) -> Result<(), ()> {
                unsafe {
                    self
                    .inner
                        .producer_push_many(slice, self.cached_version())
                };

                Ok(())
            }

            unsafe fn copy_and_commit_if<F, FSuccess, FError>(&self, right: &[T], left: &[T], f: F) -> Result<FSuccess, FError>
            where
                F: FnOnce() -> Result<FSuccess, FError>
            {
                unsafe { self.inner.producer_copy_and_commit_if(right, left, f, self.cached_version()) }
            }
        }

        impl<T: Send, SC> $crate::single_producer::SingleLockFreeProducer<T> for $producer_name<T, SC>
        where
            SC: SyncCell<LightArc<Version<T>>> + LockFreeSyncCell<LightArc<Version<T>>>
        {
            unsafe fn lock_free_maybe_push_many(&self, slice: &[T]) -> Result<(), $crate::lock_free_errors::LockFreePushManyErr> {
                unsafe { self.inner.producer_push_many(slice, self.cached_version()) };

                Ok(())
            }
        }

        impl<T: Send, SC> $crate::spmc_producer::SPMCProducer<T> for $producer_name<T, SC>
        where
            SC: SyncCell<LightArc<Version<T>>>
        {
            #[inline]
            unsafe fn push_many<BR: BatchReceiver<T>>(
                &self,
                slice: &[T],
                _batch_receiver: &BR,
            ) {
                unsafe { self.inner.producer_push_many(slice, self.cached_version()) };
            }

            #[inline]
            fn push<BR: BatchReceiver<T>>(&self, value: T, _batch_receiver: &BR) {
                unsafe { self.inner.producer_push(value, self.cached_version()) };
            }

            fn pop_many(&self, dst: &mut [MaybeUninit<T>]) -> usize {
                unsafe { self.inner.producer_pop_many(dst, self.cached_version()) }
            }

            fn pop(&self) -> Option<T> {
                unsafe { self.inner.producer_pop(self.cached_version()) }
            }
        }

        impl<T: Send, SC> $crate::spmc_producer::SPMCLockFreeProducer<T> for $producer_name<T, SC>
        where
            SC: SyncCell<LightArc<Version<T>>> + LockFreeSyncCell<LightArc<Version<T>>>
        {
            unsafe fn lock_free_push_many<BR: $crate::batch_receiver::LockFreeBatchReceiver<T>>(
                &self,
                slice: &[T],
                _batch_receiver: &BR
            ) -> Result<(), ()> {
                unsafe { self.inner.producer_push_many(slice, self.cached_version()) };

                Ok(())
            }

            fn lock_free_push<BR: $crate::batch_receiver::LockFreeBatchReceiver<T>>(&self, value: T, _batch_receiver: &BR) -> Result<(), T> {
                unsafe { self.inner.producer_push(value, self.cached_version()) };

                Ok(())
            }

            fn lock_free_pop_many(&self, dst: &mut [MaybeUninit<T>]) -> (usize, bool) {
                unsafe { (self.inner.producer_pop_many(dst, self.cached_version()), false) }
            }

            fn lock_free_pop(&self) -> Result<T, $crate::lock_free_errors::LockFreePopErr> {
                unsafe { self.inner.producer_pop(self.cached_version()).ok_or($crate::lock_free_errors::LockFreePopErr::Empty) }
            }
        }

        impl<T: Send, SC> $crate::multi_consumer::MultiConsumerSpawner<T> for $producer_name<T, SC>
        where
            SC: SyncCell<LightArc<Version<T>>>
        {
            type SpawnedConsumer = $consumer_name<T, SC>;

            fn spawn_multi_consumer(&self) -> Self::SpawnedConsumer {
                $consumer_name {
                    inner: self.inner.clone(),
                    cached_version: UnsafeCell::new(self.cached_version().clone()),
                    _non_sync: PhantomData,
                }
            }
        }

        impl<T: Send, SC> $crate::multi_consumer::MultiLockFreeConsumerSpawner<T> for $producer_name<T, SC>
        where
            SC: SyncCell<LightArc<Version<T>>> + LockFreeSyncCell<LightArc<Version<T>>>
        {
            type SpawnedLockFreeConsumer = $consumer_name<T, SC>;

            fn spawn_multi_lock_free_consumer(&self) -> Self::SpawnedLockFreeConsumer {
                $consumer_name {
                    inner: self.inner.clone(),
                    cached_version: UnsafeCell::new(self.cached_version().clone()),
                    _non_sync: PhantomData,
                }
            }
        }

        #[allow(clippy::non_send_fields_in_send_ty, reason = "We guarantee it is Send")]
        unsafe impl<T: Send, SC: SyncCell<LightArc<Version<T>>>> Send for $producer_name<T, SC>
        where
            SC: SyncCell<LightArc<Version<T>>>
        {}

        /// The consumer of the [`SPMCUnboundedQueue`].
        pub struct $consumer_name<T: Send, SC: SyncCell<LightArc<Version<T>>> = $sync_cell>
        where
            SC: SyncCell<LightArc<Version<T>>>
        {
            inner: LightArc<SPMCUnboundedQueue<T, SC, $atomic_u32_wrapper, $long_atomic_wrapper>>,
            cached_version: UnsafeCell<CachedVersion<T>>,
            _non_sync: PhantomData<*const ()>,
        }

        impl<T: Send, SC: SyncCell<LightArc<Version<T>>>> $consumer_name<T, SC>
        where
            SC: SyncCell<LightArc<Version<T>>>
        {
            /// Returns a mutable reference to the cached version.
            #[allow(clippy::mut_from_ref, reason = "It improves readability")]
            #[inline]
            fn cached_version(&self) -> &mut CachedVersion<T> {
                unsafe { &mut *self.cached_version.get() }
            }
        }

        impl<T: Send, SC> $crate::Consumer<T> for $consumer_name<T, SC>
        where
            SC: SyncCell<LightArc<Version<T>>>
        {
            #[inline]
            fn capacity(&self) -> usize {
                self.inner.consumer_capacity(self.cached_version())
            }

            #[inline]
            fn len(&self) -> usize {
                self.inner.consumer_len(self.cached_version())
            }

            #[inline]
            fn pop_many(&self, dst: &mut [MaybeUninit<T>]) -> usize {
                self.inner.consumer_pop_many(dst, self.cached_version())
            }

            #[inline(never)]
            fn steal_into(&self, dst: &impl $crate::single_producer::SingleProducer<T>) -> usize {
                self.inner.steal_into(
                    dst,
                    self.cached_version(),
                ).0
            }
        }

        impl<T: Send, SC> $crate::LockFreeConsumer<T> for $consumer_name<T, SC>
        where
            SC: SyncCell<LightArc<Version<T>>> + LockFreeSyncCell<LightArc<Version<T>>>
        {
            #[inline]
            fn lock_free_pop_many(&self, dst: &mut [MaybeUninit<T>]) -> (usize, bool) {
                self.inner.consumer_lock_free_pop_many(dst, self.cached_version())
            }

            #[inline(never)]
            fn lock_free_steal_into(&self, dst: &impl $crate::single_producer::SingleLockFreeProducer<T>) -> (usize, bool) {
                self.inner.steal_into(
                    dst,
                    self.cached_version(),
                )
            }
        }

        impl<T: Send, SC> $crate::single_consumer::SingleConsumer<T> for $consumer_name<T, SC>
        where
            SC: SyncCell<LightArc<Version<T>>>
        {}

        impl<T: Send, SC> $crate::single_consumer::SingleLockFreeConsumer<T> for $consumer_name<T, SC>
        where
            SC: SyncCell<LightArc<Version<T>>> + LockFreeSyncCell<LightArc<Version<T>>>
        {}

        impl<T: Send, SC> $crate::multi_consumer::MultiConsumer<T> for $consumer_name<T, SC>
        where
            SC: SyncCell<LightArc<Version<T>>>
        {}

        impl<T: Send, SC> $crate::multi_consumer::MultiLockFreeConsumer<T> for $consumer_name<T, SC>
        where
            SC: SyncCell<LightArc<Version<T>>> + LockFreeSyncCell<LightArc<Version<T>>>
        {}

        impl<T: Send, SC> Clone for $consumer_name<T, SC>
        where
            SC: SyncCell<LightArc<Version<T>>>
        {
            fn clone(&self) -> Self {
                Self {
                    cached_version: UnsafeCell::new(self.cached_version().clone()),
                    inner: self.inner.clone(),
                    _non_sync: PhantomData,
                }
            }
        }

        #[allow(clippy::non_send_fields_in_send_ty, reason = "We guarantee it is Send")]
        unsafe impl<T: Send, SC> Send for $consumer_name<T, SC>
        where
            SC: SyncCell<LightArc<Version<T>>>
        {}
    };

    ($producer_name:ident, $consumer_name:ident) => {
        generate_spmc_producer_and_consumer!(
            $producer_name,
            $consumer_name,
            NotCachePaddedAtomicU32,
            NotCachePaddedAtomicU64,
            NaiveRWLock<LightArc<Version<T>>>
        );
    };
}

generate_spmc_producer_and_consumer!(SPMCUnboundedProducer, SPMCUnboundedConsumer);

/// Creates a new single-producer, multi-consumer unbounded queue.
/// Returns [`producer`](SPMCUnboundedProducer) and [`consumer`](SPMCUnboundedConsumer).
///
/// The producer __should__ be only one while consumers can be cloned.
///
/// # Unbounded queue vs. [`bounded queue`](crate::spmc::new_bounded).
///
/// - [`maybe_push`](crate::Producer::maybe_push),
///   [`maybe_push_many`](crate::single_producer::SingleProducer::maybe_push_many)
///   can return an error only for `bounded` queue.
/// - [`push`](crate::spmc_producer::SPMCProducer::push),
///   [`push_many`](crate::spmc_producer::SPMCProducer::push_many)
///   writes to the [`BatchReceiver`] only for `bounded` queue.
/// - [`Consumer::steal_into`](crate::Consumer)
///   and [`Consumer::pop_many`](crate::Consumer) can pop zero values even if the source
///   queue is not empty for `unbounded` queue.
/// - [`Consumer::capacity`](crate::Consumer)
///   and [`Consumer::len`](crate::Consumer) can return old values for `unbounded` queue.
/// - All methods of `bounded` queue work much faster than all methods of `unbounded` queue.
///
/// # Cache padding
///
/// Cache padding can improve the performance of the queue many times, but it also requires
/// much more memory (likely 128 or 256 more bytes for the queue).
/// If you can sacrifice some memory for the performance, use [`new_cache_padded_unbounded`].
///
/// # `SyncCell`
///
/// It accepts the [`SyncCell`] as a generic.
/// If it is [`LockFreeSyncCell`], the queue is fully lock-free.
/// Else, the producer's methods are not lock-free on slow paths.
///
/// # Examples
///
/// ```
/// use parcoll::spmc::new_unbounded_with_sync_cell;
/// use parcoll::{Producer, Consumer, LightArc};
/// use parcoll::buffer_version::Version;
/// use parcoll::naive_rw_lock::NaiveRWLock;
///
/// let (producer, consumer) = new_unbounded_with_sync_cell::<_, NaiveRWLock<LightArc<Version<_>>>>();
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
pub fn new_unbounded_with_sync_cell<T: Send, SC: SyncCell<LightArc<Version<T>>>>(
) -> (SPMCUnboundedProducer<T, SC>, SPMCUnboundedConsumer<T, SC>) {
    let mut queue: SPMCUnboundedQueue<T, SC, NotCachePaddedAtomicU32, NotCachePaddedAtomicU64> =
        SPMCUnboundedQueue::new();
    let version = queue.last_version.get_mut().clone();
    let queue = LightArc::new(queue);

    (
        SPMCUnboundedProducer {
            inner: queue.clone(),
            cached_version: UnsafeCell::new(CachedVersion::from_arc_version(version.clone())),
            _non_sync: PhantomData,
        },
        SPMCUnboundedConsumer {
            cached_version: UnsafeCell::new(CachedVersion::from_arc_version(version)),
            inner: queue,
            _non_sync: PhantomData,
        },
    )
}

generate_spmc_producer_and_consumer!(
    CachePaddedSPMCUnboundedProducer,
    CachePaddedSPMCUnboundedConsumer,
    CachePaddedAtomicU32,
    CachePaddedAtomicU64,
    NaiveRWLock<LightArc<Version<T>>>
);

/// Creates a new single-producer, multi-consumer unbounded queue.
/// Returns [`producer`](SPMCUnboundedProducer) and [`consumer`](SPMCUnboundedConsumer).
///
/// The producer __should__ be only one while consumers can be cloned.
///
/// # Unbounded queue vs. [`bounded queue`](crate::spmc::new_bounded).
///
/// - [`maybe_push`](crate::Producer::maybe_push),
///   [`maybe_push_many`](crate::single_producer::SingleProducer::maybe_push_many)
///   can return an error only for `bounded` queue.
/// - [`push`](crate::spmc_producer::SPMCProducer::push),
///   [`push_many`](crate::spmc_producer::SPMCProducer::push_many)
///   writes to the [`BatchReceiver`] only for `bounded` queue.
/// - [`Consumer::steal_into`](crate::Consumer)
///   and [`Consumer::pop_many`](crate::Consumer) can pop zero values even if the source
///   queue is not empty for `unbounded` queue.
/// - [`Consumer::capacity`](crate::Consumer)
///   and [`Consumer::len`](crate::Consumer) can return old values for `unbounded` queue.
/// - All methods of `bounded` queue work much faster than all methods of `unbounded` queue.
///
/// # Cache padding
///
/// Cache padding can improve the performance of the queue many times, but it also requires
/// much more memory (likely 128 or 256 more bytes for the queue).
/// If you can't sacrifice some memory for the performance, use [`new_unbounded`].
///
/// # `SyncCell`
///
/// It accepts the [`SyncCell`] as a generic.
/// If it is [`LockFreeSyncCell`], the queue is fully lock-free.
/// Else, the producer's methods are not lock-free on slow paths.
///
/// # Examples
///
/// ```
/// use parcoll::spmc::new_cache_padded_unbounded_with_sync_cell;
/// use parcoll::{Producer, Consumer, LightArc};
/// use parcoll::buffer_version::Version;
/// use parcoll::naive_rw_lock::NaiveRWLock;
///
/// let (producer, consumer) = new_cache_padded_unbounded_with_sync_cell::<_, NaiveRWLock<LightArc<Version<_>>>>();
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
pub fn new_cache_padded_unbounded_with_sync_cell<T: Send, SC: SyncCell<LightArc<Version<T>>>>() -> (
    CachePaddedSPMCUnboundedProducer<T, SC>,
    CachePaddedSPMCUnboundedConsumer<T, SC>,
) {
    let mut queue: SPMCUnboundedQueue<T, SC, CachePaddedAtomicU32, CachePaddedAtomicU64> =
        SPMCUnboundedQueue::new();
    let version = queue.last_version.get_mut().clone();
    let queue = LightArc::new(queue);

    (
        CachePaddedSPMCUnboundedProducer {
            inner: queue.clone(),
            cached_version: UnsafeCell::new(CachedVersion::from_arc_version(version.clone())),
            _non_sync: PhantomData,
        },
        CachePaddedSPMCUnboundedConsumer {
            cached_version: UnsafeCell::new(CachedVersion::from_arc_version(version)),
            inner: queue,
            _non_sync: PhantomData,
        },
    )
}

/// Calls [`new_unbounded_with_sync_cell`] with [`NaiveRWLock`].
///
/// It is not lock-free: on slow paths producer's methods may lock.
///
/// For more information, see [`new_unbounded_with_sync_cell`].
pub fn new_unbounded<T: Send>() -> (SPMCUnboundedProducer<T>, SPMCUnboundedConsumer<T>) {
    new_unbounded_with_sync_cell::<T, NaiveRWLock<LightArc<Version<T>>>>()
}

/// Calls [`new_cache_padded_unbounded_with_sync_cell`] with [`NaiveRWLock`].
///
/// It is not lock-free: on slow paths producer's methods may lock.
///
/// For more information, see [`new_cache_padded_unbounded_with_sync_cell`].
pub fn new_cache_padded_unbounded<T: Send>() -> (
    CachePaddedSPMCUnboundedProducer<T>,
    CachePaddedSPMCUnboundedConsumer<T>,
) {
    new_cache_padded_unbounded_with_sync_cell::<T, NaiveRWLock<LightArc<Version<T>>>>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutex_vec_queue::MutexVecQueue;
    use crate::single_producer::{SingleLockFreeProducer, SingleProducer};
    use crate::spmc_producer::{SPMCLockFreeProducer, SPMCProducer};
    use crate::{Consumer, LockFreeConsumer, Producer};
    use std::collections::VecDeque;
    use crate::sync_cell::LockFreeSyncCellForTests;

    const N: usize = 16000;
    const BATCH_SIZE: usize = 10;

    #[test]
    fn test_spmc_unbounded_seq_insertions() {
        let global_queue = MutexVecQueue::new();
        let (producer, _) = new_unbounded();

        for i in 0..N {
            producer.push(i, &global_queue);
        }

        assert!(global_queue.is_empty());

        for i in 0..N {
            assert_eq!(producer.pop().unwrap(), i);
        }

        let (producer, consumer) = new_unbounded();

        for i in 0..N {
            producer.maybe_push(i).unwrap();
        }

        for i in 0..N / BATCH_SIZE {
            let mut slice = [MaybeUninit::uninit(); BATCH_SIZE];

            assert_eq!(consumer.pop_many(slice.as_mut_slice()), BATCH_SIZE);

            for (j, item) in slice.iter().enumerate().take(BATCH_SIZE) {
                assert_eq!(unsafe { item.assume_init() }, i * BATCH_SIZE + j);
            }
        }
    }

    #[test]
    fn test_spmc_unbounded_stealing() {
        const TRIES: usize = 100;

        let global_queue = MutexVecQueue::new();
        let mut stolen = VecDeque::new();
        let (producer1, consumer) = new_unbounded();
        let (producer2, _) = new_unbounded();

        producer2.reserve(512);

        for _ in 0..TRIES * 2 {
            for i in 0..N / 2 {
                producer1.push(i, &global_queue);
            }

            consumer.steal_into(&producer2);

            while let Some(task) = producer2.pop() {
                stolen.push_back(task);
            }

            assert!(global_queue.is_empty());
        }

        assert!(producer2.is_empty());

        let mut count = 0;

        while producer1.pop().is_some() {
            count += 1;
        }

        assert_eq!(count + stolen.len(), N * TRIES);
    }

    #[test]
    fn test_spmc_unbounded_many() {
        const BATCH_SIZE: usize = 30;
        const N: usize = BATCH_SIZE * 100;

        let global_queue = MutexVecQueue::new();
        let (producer, consumer) = new_unbounded();

        for i in 0..N / BATCH_SIZE / 2 {
            let slice = (0..BATCH_SIZE)
                .map(|j| i * BATCH_SIZE + j)
                .collect::<Vec<_>>();

            unsafe {
                producer.maybe_push_many(&slice).unwrap();
            }

            let mut slice = [MaybeUninit::uninit(); BATCH_SIZE];
            producer.pop_many(slice.as_mut_slice());

            for (j, item) in slice.iter().enumerate().take(BATCH_SIZE) {
                let index = i * BATCH_SIZE + j;

                assert_eq!(unsafe { item.assume_init() }, index);
            }
        }

        for i in 0..N / BATCH_SIZE / 2 {
            let slice = (0..BATCH_SIZE)
                .map(|j| i * BATCH_SIZE + j)
                .collect::<Vec<_>>();

            unsafe {
                producer.push_many(&slice, &global_queue);
            }

            assert!(global_queue.is_empty());

            let mut slice = [MaybeUninit::uninit(); BATCH_SIZE];

            consumer.pop_many(slice.as_mut_slice());

            for (j, item) in slice.iter().enumerate().take(BATCH_SIZE) {
                let index = i * BATCH_SIZE + j;

                assert_eq!(unsafe { item.assume_init() }, index);
            }
        }
    }

    #[test]
    fn test_spmc_lock_free_unbounded_seq_insertions() {
        #[cfg(feature = "with-light-qsbr")]
        let shared_manager = light_qsbr::SharedManager::new();
        #[cfg(feature = "with-light-qsbr")]
        shared_manager.register_new_executor();

        let global_queue = MutexVecQueue::new();
        let (producer, _) = new_cache_padded_unbounded_with_sync_cell::<
            _,
            LockFreeSyncCellForTests<LightArc<Version<usize>>>,
        >();

        for i in 0..N {
            producer.push(i, &global_queue);
        }

        assert!(global_queue.is_empty());

        for i in 0..N {
            assert_eq!(producer.pop().unwrap(), i);
        }

        let (producer, consumer) = new_unbounded();

        for i in 0..N {
            producer.maybe_push(i).unwrap();
        }

        for i in 0..N / BATCH_SIZE {
            let mut slice = [MaybeUninit::uninit(); BATCH_SIZE];

            assert_eq!(consumer.pop_many(slice.as_mut_slice()), BATCH_SIZE);

            for (j, item) in slice.iter().enumerate().take(BATCH_SIZE) {
                assert_eq!(unsafe { item.assume_init() }, i * BATCH_SIZE + j);
            }
        }

        #[cfg(feature = "with-light-qsbr")]
        unsafe { light_qsbr::LocalManager::deregister() };
    }

    #[test]
    fn test_spmc_lock_free_unbounded_stealing() {
        const TRIES: usize = 100;

        #[cfg(feature = "with-light-qsbr")]
        let shared_manager = light_qsbr::SharedManager::new();
        #[cfg(feature = "with-light-qsbr")]
        shared_manager.register_new_executor();

        let global_queue = MutexVecQueue::new();
        let mut stolen = VecDeque::new();
        let (producer1, consumer) =
            new_unbounded_with_sync_cell::<_, LockFreeSyncCellForTests<LightArc<Version<usize>>>>();
        let (producer2, _) =
            new_unbounded_with_sync_cell::<_, LockFreeSyncCellForTests<LightArc<Version<usize>>>>();

        producer2.reserve(512);

        for _ in 0..TRIES * 2 {
            for i in 0..N / 2 {
                producer1.lock_free_push(i, &global_queue).unwrap();
            }

            assert!(!consumer.lock_free_steal_into(&producer2).1);

            while let Ok(task) = producer2.lock_free_pop() {
                stolen.push_back(task);
            }

            assert!(global_queue.is_empty());
        }

        assert!(producer2.is_empty());

        let mut count = 0;

        while producer1.lock_free_pop().is_ok() {
            count += 1;
        }

        assert_eq!(count + stolen.len(), N * TRIES);

        #[cfg(feature = "with-light-qsbr")]
        unsafe { light_qsbr::LocalManager::deregister() };
    }

    #[test]
    fn test_spmc_lock_free_unbounded_many() {
        const BATCH_SIZE: usize = 30;
        const N: usize = BATCH_SIZE * 100;

        #[cfg(feature = "with-light-qsbr")]
        let shared_manager = light_qsbr::SharedManager::new();
        #[cfg(feature = "with-light-qsbr")]
        shared_manager.register_new_executor();

        let global_queue = MutexVecQueue::new();
        let (producer, consumer) =
            new_unbounded_with_sync_cell::<_, LockFreeSyncCellForTests<LightArc<Version<usize>>>>();

        for i in 0..N / BATCH_SIZE / 2 {
            let slice = (0..BATCH_SIZE)
                .map(|j| i * BATCH_SIZE + j)
                .collect::<Vec<_>>();

            unsafe {
                producer.lock_free_maybe_push_many(&slice).unwrap();
            }

            let mut slice = [MaybeUninit::uninit(); BATCH_SIZE];
            assert_eq!(
                producer.lock_free_pop_many(slice.as_mut_slice()),
                (BATCH_SIZE, false)
            );

            for (j, item) in slice.iter().enumerate().take(BATCH_SIZE) {
                let index = i * BATCH_SIZE + j;

                assert_eq!(unsafe { item.assume_init() }, index);
            }
        }

        for i in 0..N / BATCH_SIZE / 2 {
            let slice = (0..BATCH_SIZE)
                .map(|j| i * BATCH_SIZE + j)
                .collect::<Vec<_>>();

            unsafe {
                producer
                    .lock_free_push_many(&slice, &global_queue)
                    .unwrap();
            }

            assert!(global_queue.is_empty());

            let mut slice = [MaybeUninit::uninit(); BATCH_SIZE];

            assert_eq!(
                consumer.lock_free_pop_many(slice.as_mut_slice()),
                (BATCH_SIZE, false)
            );

            for (j, item) in slice.iter().enumerate().take(BATCH_SIZE) {
                let index = i * BATCH_SIZE + j;

                assert_eq!(unsafe { item.assume_init() }, index);
            }
        }

        #[cfg(feature = "with-light-qsbr")]
        unsafe { light_qsbr::LocalManager::deregister() };
    }
}
