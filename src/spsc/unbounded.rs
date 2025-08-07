//! This module provides a single-producer single-consumer unbounded queue. Read more in
//! [`new_unbounded`].
#![allow(
    clippy::cast_possible_truncation,
    reason = "LongNumber should be synonymous to usize"
)]
use crate::cache_padded::{CachePaddedAtomicU32, CachePaddedAtomicU64};
use crate::hints::{cold_path, unlikely};
use crate::light_arc::LightArc;
use crate::loom_bindings::sync::atomic::{AtomicU32, AtomicU64};
use crate::naive_rw_lock::NaiveRWLock;
use crate::number_types::{NotCachePaddedAtomicU32, NotCachePaddedAtomicU64};
use crate::spsc::{Consumer, Producer};
use std::alloc::{alloc, Layout};
use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem::{needs_drop, MaybeUninit};
use std::ops::Deref;
use std::sync::atomic::Ordering;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::{ptr, slice};

/// Packs the version and the tail into a single 64-bit value.
#[inline(always)]
fn pack_version_and_tail(version: u32, tail: u32) -> u64 {
    (u64::from(version) << 32) | u64::from(tail)
}

/// Unpacks the version and the tail from a single 64-bit value.
#[inline(always)]
fn unpack_version_and_tail(value: u64) -> (u32, u32) {
    ((value >> 32) as u32, value as u32)
}

/// A version of the ring-based queue.
#[repr(C)]
struct Version<T> {
    ptr: *mut [MaybeUninit<T>],
    mask: u32,
    id: u32,
}

impl<T> Version<T> {
    /// Returns the mask for the capacity of the underlying buffer.
    #[inline(always)]
    fn mask(&self) -> u32 {
        self.mask
    }

    /// Allocates a new version with the given `capacity` and `id`.
    fn alloc_new(capacity: usize, id: u32) -> LightArc<Self> {
        debug_assert!(
            capacity > 0 && u32::try_from(capacity).is_ok() && capacity.is_power_of_two()
        );

        let slice_ptr = unsafe {
            slice::from_raw_parts_mut(
                alloc(Layout::array::<MaybeUninit<T>>(capacity).unwrap_unchecked())
                    .cast::<MaybeUninit<T>>(),
                capacity,
            )
        };

        LightArc::new(Self {
            ptr: slice_ptr,
            mask: (capacity - 1) as u32,
            id,
        })
    }

    /// Returns a raw pointer to the underlying buffer.
    #[inline(always)]
    unsafe fn thin_mut_ptr(&self) -> *mut T {
        unsafe { (*self.ptr).as_ptr().cast_mut().cast() }
    }
}

impl<T> Drop for Version<T> {
    fn drop(&mut self) {
        unsafe { drop(Box::from_raw(self.ptr)) };
    }
}

/// A cached [`Version`].
#[repr(C)]
struct CachedVersion<T> {
    ptr: *const [MaybeUninit<T>],
    mask: u32,
    id: u32,
    /// Needs to be dropped to release the memory.
    real: LightArc<Version<T>>,
}

impl<T> CachedVersion<T> {
    /// Returns a cached version from the given `arc` version.
    fn from_arc_version(arc: LightArc<Version<T>>) -> Self {
        Self {
            ptr: arc.ptr,
            mask: arc.mask,
            id: arc.id,
            real: arc,
        }
    }

    /// Returns the capacity of the underlying buffer.
    #[inline(always)]
    fn capacity(&self) -> usize {
        self.ptr.len()
    }

    /// Returns the version id.
    #[inline(always)]
    fn id(&self) -> u32 {
        self.id
    }

    /// Returns the mask for the capacity of the underlying buffer.
    #[inline(always)]
    fn mask(&self) -> u32 {
        self.mask
    }

    /// Returns a raw pointer to the underlying buffer.
    #[inline(always)]
    fn thin_ptr(&self) -> *const MaybeUninit<T> {
        unsafe { &*self.ptr }.as_ptr()
    }

    /// Returns a mutable raw pointer to the underlying buffer.
    #[inline(always)]
    unsafe fn thin_mut_ptr(&self) -> *mut MaybeUninit<T> {
        unsafe { &mut *self.ptr.cast_mut() }.as_mut_ptr()
    }
}

impl<T> Clone for CachedVersion<T> {
    fn clone(&self) -> Self {
        Self {
            ptr: self.ptr,
            mask: self.mask,
            id: self.id,
            real: self.real.clone(),
        }
    }
}

/// The single-producer, single-consumer ring-based _unbounded_ queue.
///
/// It is safe to use when and only when only one thread is writing to the queue at the same time,
/// and only one thread is reading from the queue at the same time.
///
/// It accepts two atomic wrappers as generic parameters.
/// It allows using cache-padded atomics or not.
/// You should create types aliases not to write this large type name.
///
/// # Why it is private?
///
/// It is private because it needs [`CachedVersion`] to work,
/// and it is useless to use [`CachedVersion`] without separate consumers.
/// It is too expansive to load the [`Version`] for any consumer method.
/// This behavior may be changed in the future.
///
/// It doesn't implement the [`Producer`] and [`Consumer`] traits because all producer methods
/// are unsafe (can be called only by one thread).
#[repr(C)]
pub(crate) struct SPSCUnboundedQueue<
    T,
    AtomicU32Wrapper = NotCachePaddedAtomicU32,
    AtomicU64Wrapper = NotCachePaddedAtomicU64,
> where
    AtomicU32Wrapper: Deref<Target = AtomicU32> + Default,
    AtomicU64Wrapper: Deref<Target = AtomicU64> + Default,
{
    /// First the producer updates the real version,
    /// and next sets a new id. The version id is monotonic.
    tail_and_version: AtomicU64Wrapper,
    head: AtomicU32Wrapper,
    last_version: NaiveRWLock<LightArc<Version<T>>>,
}

impl<T, AtomicU32Wrapper, AtomicU64Wrapper>
SPSCUnboundedQueue<T, AtomicU32Wrapper, AtomicU64Wrapper>
where
    AtomicU32Wrapper: Deref<Target = AtomicU32> + Default,
    AtomicU64Wrapper: Deref<Target = AtomicU64> + Default,
{
    /// Creates a new queue with the given capacity.
    fn with_capacity(capacity: usize) -> Self {
        Self {
            tail_and_version: AtomicU64Wrapper::default(),
            head: AtomicU32Wrapper::default(),
            last_version: NaiveRWLock::new(Version::alloc_new(capacity, 0)),
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
        let Some(new_version) = self.last_version.try_read() else {
            cold_path();

            // We should guess that the producer has been preempted.
            // It is too expansive to wait.
            // It is very unlikely to happen because the consumer tries to update the version
            // only after the producer updates the version id;
            // therefore, we can be here only
            // if the producer updates the version and the version id from A to B
            // and then locks the version to update from B to C,
            // and the consumer tries to update from A to B.
            return false;
        };

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
        // Because the producer copies all values before update the version,
        // the consumer can read B or C.
        // But obviously we should return the version C not to load it again.

        *version = CachedVersion::from_arc_version(new_version.clone());

        true
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
impl<T, AtomicU32Wrapper, AtomicU64Wrapper>
SPSCUnboundedQueue<T, AtomicU32Wrapper, AtomicU64Wrapper>
where
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
        let head = self.head.load(Acquire);
        let tail = unsafe { self.unsync_load_tail() }; // only producer can change tail

        // We can avoid checking the version,
        // because the producer always has the latest version.

        Self::len(head, tail)
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
        let tail_idx = (start_tail & version.mask) as usize;

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
                let old_head_idx = (head & old_version.mask) as usize;
                let old_tail_idx = (tail & old_version.mask) as usize;

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

        *self.last_version.write() = new_version;

        (cached_version, tail)
    }

    /// Updates the capacity of the queue.
    ///
    /// # Safety
    ///
    /// It is called only by the producer,
    /// and the provided capacity should be more than the current capacity,
    /// and less than `u32::MAX` and be a power of two.
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
            new_capacity.is_power_of_two(),
            "new_capacity should be power of two"
        );

        let tail = unsafe { self.unsync_load_tail() }; // only producer can change tail
        let (cached_version, tail) = self.create_new_version_and_write_it_but_not_update_tail(
            self.head.load(Acquire),
            tail,
            new_capacity,
            version,
        );

        self.tail_and_version
            .store(pack_version_and_tail(cached_version.id(), tail), Release);

        *version = cached_version;
    }

    /// Pushes a value to the queue.
    ///
    /// # Safety
    ///
    /// The called should be the only producer and the queue should not be full.
    #[inline(always)]
    unsafe fn push_unchecked(&self, value: T, tail: u32, version: &CachedVersion<T>) {
        // The producer always has the latest version.

        unsafe {
            version
                .thin_ptr()
                .add((tail & version.mask()) as usize)
                .cast_mut()
                .write(MaybeUninit::new(value));
        }

        self.tail_and_version.store(
            pack_version_and_tail(version.id, tail.wrapping_add(1)),
            Release,
        );
    }

    /// Updates the version and resizes the queue to the capacity * 2.
    /// Then it insets the provided slice.
    ///
    /// # Safety
    ///
    /// The called should be the only producer.
    #[inline(never)]
    #[cold]
    unsafe fn handle_overflow(
        &self,
        head: u32,
        tail: u32,
        version: &mut CachedVersion<T>,
        values: &[T],
    ) {
        let mut new_capacity = version.capacity() * 2;
        while new_capacity <= version.capacity() + values.len() {
            new_capacity *= 2;
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
    /// The called should be the only producer.
    #[inline]
    unsafe fn producer_push(&self, value: T, version: &mut CachedVersion<T>) {
        let head = self.head.load(Acquire);
        let tail = unsafe { self.unsync_load_tail() }; // only producer can change tail

        if unlikely(Self::len(head, tail) == version.capacity()) {
            unsafe { self.handle_overflow(head, tail, version, &[value]) };

            return;
        }

        unsafe { self.push_unchecked(value, tail, version) };
    }

    /// Pushes many values to the queue.
    ///
    /// # Safety
    ///
    /// The called should be the only producer and the space is enough.
    #[inline]
    unsafe fn producer_push_many_unchecked(
        &self,
        first: &[T],
        last: &[T],
        version: &CachedVersion<T>,
    ) {
        if cfg!(debug_assertions) {
            let head = self.head.load(Acquire);
            let tail = unsafe { self.unsync_load_tail() }; // only producer can change tail

            debug_assert!(Self::len(head, tail) + first.len() + last.len() <= version.capacity());
        }

        // It is SPSC, and it is expected that the capacity is enough.

        let mut tail = unsafe { self.unsync_load_tail() }; // only producer can change tail

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
    /// The called should be the only producer.
    #[inline]
    unsafe fn producer_push_many(&self, slice: &[T], version: &mut CachedVersion<T>) {
        let head = self.head.load(Acquire);
        let mut tail = unsafe { self.unsync_load_tail() }; // only producer can change tail

        if unlikely(Self::len(head, tail) + slice.len() > version.capacity()) {
            unsafe { self.handle_overflow(head, tail, version, slice) };

            return;
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
}

// Consumers
impl<T, AtomicU32Wrapper, AtomicU64Wrapper>
SPSCUnboundedQueue<T, AtomicU32Wrapper, AtomicU64Wrapper>
where
    AtomicU32Wrapper: Deref<Target = AtomicU32> + Default,
    AtomicU64Wrapper: Deref<Target = AtomicU64> + Default,
{
    /// Returns the capacity of the queue.
    #[inline]
    fn consumer_capacity(&self, version: &mut CachedVersion<T>) -> usize {
        let last_version_id = self.sync_load_version(Relaxed);
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
        let (last_version_id, tail) = self.sync_load_version_and_tail(Relaxed);
        let head = unsafe { self.head.unsync_load() }; // only consumer can change head
        let len = Self::len(head, tail);

        if unlikely(last_version_id > version.id()) {
            // The new version was created and has another capacity.

            let was_updated = self.update_version(version);
            if unlikely(!was_updated) {
                // We can't reliably return the length in this situation.
                // But it is not a problem.
                // This method can be used for two purposes:
                // 1. In tests to check if all values were pushed;
                // 2. To check if the queue is empty -> the reading is possible.
                //
                // The first case is impossible, because not to fail test accidentally,
                // this method can be called
                // only when concurrent work with the queue is impossible;
                // therefore, in this case we can't be here
                // (the update_version method always returns `true`
                // without the concurrent work).
                //
                // For the second case, we can return zero,
                // because the reading is impossible.
                return 0;
            }
        }

        len
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
        let (mut last_version_id, mut tail) = self.sync_load_version_and_tail(Acquire);
        let head = unsafe { self.head.unsync_load() }; // only consumer can change head

        loop {
            if unlikely(version.id() < last_version_id) {
                if unlikely(!self.update_version(version)) {
                    // We can't reliably calculate the length in this situation.
                    return 0;
                }

                (last_version_id, tail) = self.sync_load_version_and_tail(Acquire);

                continue;
            }

            let available = Self::len(head, tail);
            let n = dst.len().min(available);

            if n == 0 {
                return 0;
            }

            let dst_ptr = dst.as_mut_ptr();
            let head_idx = (head & version.mask()) as usize;
            let right = version.capacity() - head_idx;

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
            
            self.head.store(head.wrapping_add(n as u32), Release);
            
            return n;
        }
    }

    /// Steals many values from the consumer to the `dst`.
    /// Returns the number of values stolen.
    ///
    /// It can return zero even if the source queue is not empty,
    /// if the producer is preempted while pushing.
    fn steal_into(
        &self,
        dst: &Self,
        src_version: &mut CachedVersion<T>,
        dst_version: &mut CachedVersion<T>,
    ) -> usize {
        let (mut src_last_version_id, mut src_tail) = self.sync_load_version_and_tail(Acquire);
        let src_head = unsafe { self.head.unsync_load() }; // only producer can change head
        let dst_tail = unsafe { dst.unsync_load_tail() }; // only producer can change tail

        if cfg!(debug_assertions) {
            let dst_head = dst.head.load(Relaxed);

            assert_eq!(
                dst_head, dst_tail,
                "steal_into should not be called when dst is not empty"
            );
        }

        loop {
            if unlikely(src_version.id() < src_last_version_id) {
                if unlikely(!self.update_version(src_version)) {
                    // We can't reliably calculate the length in this situation.
                    return 0;
                }
                
                (src_last_version_id, src_tail) = self.sync_load_version_and_tail(Acquire);

                continue;
            }

            let n = Self::len(src_head, src_tail) / 2;
            if !cfg!(feature = "always_steal") && n < 4 || n == 0 {
                // we don't steal less than 4 by default
                // because else we may lose more because of cache locality and NUMA awareness
                return 0;
            }

            let n = n.min(dst_version.capacity());
            let src_head_idx = (src_head & src_version.mask()) as usize;

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

            Self::copy_slice(
                unsafe { dst_version.thin_mut_ptr() }.cast::<T>(),
                dst_tail,
                src_right,
                dst_version,
            );
            Self::copy_slice(
                unsafe { dst_version.thin_mut_ptr() }.cast::<T>(),
                dst_tail.wrapping_add(src_right.len() as u32),
                src_left,
                dst_version,
            );
            
            self.head.store(src_head.wrapping_add(n as u32), Release);

            // Success, we can move dst tail and return
            dst.tail_and_version.store(
                pack_version_and_tail(dst_version.id(), dst_tail.wrapping_add(n as u32)),
                Release,
            );

            return n;
        }
    }
}

#[allow(clippy::non_send_fields_in_send_ty, reason = "We guarantee it is Send")]
unsafe impl<T, AtomicU32Wrapper, AtomicU64Wrapper> Send
for SPSCUnboundedQueue<T, AtomicU32Wrapper, AtomicU64Wrapper>
where
    AtomicU32Wrapper: Deref<Target = AtomicU32> + Default,
    AtomicU64Wrapper: Deref<Target = AtomicU64> + Default,
{
}
unsafe impl<T, AtomicU32Wrapper, AtomicU64Wrapper> Sync
for SPSCUnboundedQueue<T, AtomicU32Wrapper, AtomicU64Wrapper>
where
    AtomicU32Wrapper: Deref<Target = AtomicU32> + Default,
    AtomicU64Wrapper: Deref<Target = AtomicU64> + Default,
{
}

impl<T, AtomicU32Wrapper, AtomicU64Wrapper> Drop
for SPSCUnboundedQueue<T, AtomicU32Wrapper, AtomicU64Wrapper>
where
    AtomicU32Wrapper: Deref<Target = AtomicU32> + Default,
    AtomicU64Wrapper: Deref<Target = AtomicU64> + Default,
{
    fn drop(&mut self) {
        // While dropping there is no concurrency

        if needs_drop::<T>() {
            let version = self.last_version.try_read().unwrap();
            let mut head = unsafe { self.head.unsync_load() };
            let tail = unsafe { self.unsync_load_tail() };

            while head != tail {
                unsafe {
                    ptr::drop_in_place(
                        version
                            .thin_mut_ptr()
                            .add((head & version.mask()) as usize)
                            .cast::<T>(),
                    );
                }

                head = head.wrapping_add(1);
            }
        }
    }
}

/// Generates SPSC producer and consumer.
macro_rules! generate_spsc_producer_and_consumer {
    ($producer_name:ident, $consumer_name:ident, $atomic_u32_wrapper:ty, $long_atomic_wrapper:ty) => {
        /// The producer of the [`SPSCUnboundedQueue`].
        pub struct $producer_name<T> {
            inner: LightArc<SPSCUnboundedQueue<T, $atomic_u32_wrapper, $long_atomic_wrapper>>,
            cached_version: UnsafeCell<CachedVersion<T>>, // The producer is not Sync, it needs only shared references and it never gets two mutable references to this field
            _non_sync: PhantomData<*const ()>,
        }

        impl<T> $producer_name<T> {
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
            /// The provided capacity must be greater than the current capacity,
            /// less than `u32::MAX` and be a power of two.
            pub fn reserve(&self, capacity: usize) {
                unsafe {
                    self
                        .inner
                        .producer_reserve(capacity, self.cached_version())
                };
            }
        }

        impl<T: Send> Producer<T> for $producer_name<T> {
            #[inline]
            fn capacity(&self) -> usize {
                // The producer always has the latest version.
                unsafe { SPSCUnboundedQueue::<T, $atomic_u32_wrapper, $long_atomic_wrapper>::producer_capacity(self.cached_version()) }
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

            #[inline]
            unsafe fn push_many_unchecked(&self, first: &[T], last: &[T]) {
                unsafe {
                    self
                    .inner
                        .producer_push_many_unchecked(first, last, self.cached_version())
                }
            }

            #[inline]
            unsafe fn maybe_push_many(&self, slice: &[T]) -> Result<(), ()> {
                unsafe {
                    self
                    .inner
                        .producer_push_many(slice, self.cached_version())
                };

                Ok(())
            }
        }

        #[allow(clippy::non_send_fields_in_send_ty, reason = "We guarantee it is Send")]
        unsafe impl<T: Send> Send for $producer_name<T> {}

        /// The consumer of the [`SPSCUnboundedQueue`].
        pub struct $consumer_name<T> {
            inner: LightArc<SPSCUnboundedQueue<T, $atomic_u32_wrapper, $long_atomic_wrapper>>,
            cached_version: UnsafeCell<CachedVersion<T>>,
            _non_sync: PhantomData<*const ()>,
        }

        impl<T> $consumer_name<T> {
            /// Returns a mutable reference to the cached version.
            #[allow(clippy::mut_from_ref, reason = "It improves readability")]
            #[inline]
            fn cached_version(&self) -> &mut CachedVersion<T> {
                unsafe { &mut *self.cached_version.get() }
            }
        }

        impl<T: Send> Consumer<T> for $consumer_name<T> {
            type AssociatedProducer = $producer_name<T>;

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

            #[inline]
            fn steal_into(&self, dst: &Self::AssociatedProducer) -> usize {
                self.inner.steal_into(
                    &*dst.inner,
                    self.cached_version(),
                    dst.cached_version(),
                )
            }
        }

        impl<T> Clone for $consumer_name<T> {
            fn clone(&self) -> Self {
                Self {
                    cached_version: UnsafeCell::new(self.cached_version().clone()),
                    inner: self.inner.clone(),
                    _non_sync: PhantomData,
                }
            }
        }

        #[allow(clippy::non_send_fields_in_send_ty, reason = "We guarantee it is Send")]
        unsafe impl<T: Send> Send for $consumer_name<T> {}
    };

    ($producer_name:ident, $consumer_name:ident) => {
        generate_spsc_producer_and_consumer!(
            $producer_name,
            $consumer_name,
            NotCachePaddedAtomicU32,
            NotCachePaddedAtomicU64
        );
    };
}

generate_spsc_producer_and_consumer!(SPSCUnboundedProducer, SPSCUnboundedConsumer);

/// Creates a new single-producer, single-consumer unbounded queue.
/// Returns [`producer`](SPSCUnboundedProducer) and [`consumer`](SPSCUnboundedConsumer).
///
/// The producer __should__ be only one while consumers can be cloned.
/// If you want to use more than one producer, don't use this queue.
///
/// # Unbounded queue vs. [`bounded queue`](crate::spsc::new_bounded).
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
/// If you can sacrifice some memory for the performance, use [`new_cache_padded_unbounded`].
///
/// # Examples
///
/// ```
/// use parcoll::spsc::{Producer, Consumer, new_unbounded};
///
/// let (mut producer, mut consumer) = new_unbounded();
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
pub fn new_unbounded<T>() -> (SPSCUnboundedProducer<T>, SPSCUnboundedConsumer<T>) {
    let mut queue = SPSCUnboundedQueue::new();
    let version = queue.last_version.get_mut().clone();
    let queue = LightArc::new(queue);

    (
        SPSCUnboundedProducer {
            inner: queue.clone(),
            cached_version: UnsafeCell::new(CachedVersion::from_arc_version(version.clone())),
            _non_sync: PhantomData,
        },
        SPSCUnboundedConsumer {
            cached_version: UnsafeCell::new(CachedVersion::from_arc_version(version)),
            inner: queue,
            _non_sync: PhantomData,
        },
    )
}

generate_spsc_producer_and_consumer!(
    CachePaddedSPSCUnboundedProducer,
    CachePaddedSPSCUnboundedConsumer,
    CachePaddedAtomicU32,
    CachePaddedAtomicU64
);

/// Creates a new single-producer, single-consumer unbounded queue.
/// Returns [`producer`](SPSCUnboundedProducer) and [`consumer`](SPSCUnboundedConsumer).
///
/// The producer __should__ be only one while consumers can be cloned.
/// If you want to use more than one producer, don't use this queue.
///
/// # Unbounded queue vs. [`bounded queue`](crate::spsc::new_bounded).
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
/// If you can't sacrifice some memory for the performance, use [`new_unbounded`].
///
/// # Examples
///
/// ```
/// use parcoll::spsc::{Producer, Consumer, new_cache_padded_unbounded};
///
/// let (mut producer, mut consumer) = new_cache_padded_unbounded();
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
pub fn new_cache_padded_unbounded<T>() -> (
    CachePaddedSPSCUnboundedProducer<T>,
    CachePaddedSPSCUnboundedConsumer<T>,
) {
    let mut queue = SPSCUnboundedQueue::new();
    let version = queue.last_version.get_mut().clone();
    let queue = LightArc::new(queue);

    (
        CachePaddedSPSCUnboundedProducer {
            inner: queue.clone(),
            cached_version: UnsafeCell::new(CachedVersion::from_arc_version(version.clone())),
            _non_sync: PhantomData,
        },
        CachePaddedSPSCUnboundedConsumer {
            cached_version: UnsafeCell::new(CachedVersion::from_arc_version(version)),
            inner: queue,
            _non_sync: PhantomData,
        },
    )
}

#[cfg(test)]
mod tests {
    use crate::mutex_vec_queue::VecQueue;
    use super::*;

    const N: usize = 16000;
    const BATCH_SIZE: usize = 10;

    #[test]
    fn test_spsc_unbounded_seq_insertions() {
        let (producer, consumer) = new_unbounded();

        for i in 0..N {
            producer.maybe_push(i).unwrap();
        }

        for i in 0..N {
            assert_eq!(consumer.pop().unwrap(), i);
        }

        let (producer, consumer) = new_unbounded();

        for i in 0..N {
            producer.maybe_push(i).unwrap();
        }

        for i in 0..N / BATCH_SIZE {
            let mut slice = [MaybeUninit::uninit(); BATCH_SIZE];

            assert_eq!(consumer.pop_many(slice.as_mut_slice()), BATCH_SIZE);

            for j in 0..BATCH_SIZE {
                assert_eq!(unsafe { slice[j].assume_init() }, i * BATCH_SIZE + j);
            }
        }
    }

    #[test]
    fn test_spsc_unbounded_stealing() {
        const TRIES: usize = 100;

        let (producer1, consumer) = new_unbounded();
        let (mut producer2, consumer2) = new_unbounded();
        let mut stolen = VecQueue::new();

        producer2.reserve(512);

        for _ in 0..TRIES * 2 {
            for i in 0..N / 2 {
                producer1.maybe_push(i).unwrap();
            }

            consumer.steal_into(&mut producer2);

            while let Some(task) = consumer2.pop() {
                stolen.push(task);
            }
        }

        assert!(producer2.is_empty());

        let mut count = 0;

        while let Some(_) = consumer.pop() {
            count += 1;
        }

        assert_eq!(count + stolen.len(), N * TRIES);
    }

    #[test]
    fn test_spsc_unbounded_many() {
        const BATCH_SIZE: usize = 30;
        const N: usize = BATCH_SIZE * 100;

        let (producer, consumer) = new_unbounded();

        for i in 0..N / BATCH_SIZE / 2 {
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

        for i in 0..N / BATCH_SIZE / 2 {
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
