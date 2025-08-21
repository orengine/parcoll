use crate::hints::unreachable_hint;
use crate::LightArc;
use std::alloc::{alloc, Layout};
use std::mem::MaybeUninit;
use std::slice;

/// Packs the version and the tail into a single 64-bit value.
#[inline(always)]
pub(crate) fn pack_version_and_tail(version: u32, tail: u32) -> u64 {
    (u64::from(version) << 32) | u64::from(tail)
}

/// Unpacks the version and the tail from a single 64-bit value.
#[inline(always)]
#[allow(
    clippy::cast_possible_truncation,
    reason = "u64 can be divided into two u32"
)]
pub(crate) fn unpack_version_and_tail(value: u64) -> (u32, u32) {
    ((value >> 32) as u32, value as u32)
}

/// A version of the ring-based queue.
#[repr(C)]
pub struct Version<T> {
    ptr: *mut [MaybeUninit<T>],
    id: u32,
}

impl<T> Version<T> {
    /// Returns a greater capacity than the provided one.
    pub(crate) fn greater_capacity(capacity: usize) -> usize {
        const MAX_CAPACITY: usize = u32::MAX as usize;

        if cfg!(feature = "unbounded_slices_always_pow2") {
            debug_assert!(capacity.is_power_of_two());

            if capacity > 0 {
                capacity * 2
            } else {
                4
            }
        } else {
            match capacity {
                1..512 => capacity * 2,
                512..1024 => capacity * 3 / 2,
                1024..4096 => capacity * 4 / 3,
                4096..MAX_CAPACITY => capacity * 5 / 4,
                0 => 4,
                MAX_CAPACITY.. => unreachable_hint(),
            }
        }
    }

    /// Allocates a new version with the given `capacity` and `id`.
    ///
    /// # Panics
    ///
    /// Panics if the provided `capacity` is not a power of two when the feature `unbounded_slices_always_pow2` is enabled.
    /// Or if the provided `capacity` is zero or greater than `u32::MAX`.
    pub(crate) fn alloc_new(capacity: usize, id: u32) -> LightArc<Self> {
        debug_assert!(capacity > 0 && u32::try_from(capacity).is_ok());
        if cfg!(feature = "unbounded_slices_always_pow2") {
            debug_assert!(
                capacity.is_power_of_two(),
                "feature \"unbounded_slices_always_pow2\" is enabled but the provided capacity is not a power of two"
            );
        }

        let slice_ptr = unsafe {
            slice::from_raw_parts_mut(
                alloc(Layout::array::<MaybeUninit<T>>(capacity).unwrap_unchecked())
                    .cast::<MaybeUninit<T>>(),
                capacity,
            )
        };

        LightArc::new(Self { ptr: slice_ptr, id })
    }

    /// Returns the capacity of the underlying buffer.
    #[inline(always)]
    pub(crate) fn capacity(&self) -> usize {
        self.ptr.len()
    }

    /// Returns a physical index of the given logical index.
    #[inline(always)]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "It is not allowed to have more than u32::MAX elements"
    )]
    pub(crate) fn physical_index(&self, logical_index: u32) -> u32 {
        #[cfg(feature = "unbounded_slices_always_pow2")]
        {
            logical_index & (self.capacity() as u32 - 1)
        }

        #[cfg(not(feature = "unbounded_slices_always_pow2"))]
        {
            logical_index % self.capacity() as u32
        }
    }

    /// Returns a raw pointer to the underlying buffer.
    #[inline(always)]
    pub(crate) unsafe fn thin_mut_ptr(&self) -> *mut T {
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
pub(crate) struct CachedVersion<T> {
    ptr: *const [MaybeUninit<T>],
    #[cfg(feature = "unbounded_slices_always_pow2")]
    mask: u32,
    id: u32,
    /// Needs to be dropped to release the memory.
    real: LightArc<Version<T>>,
}

impl<T> CachedVersion<T> {
    /// Returns a cached version from the given `arc` version.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "It is not allowed to have more than u32::MAX elements"
    )]
    pub(crate) fn from_arc_version(arc: LightArc<Version<T>>) -> Self {
        Self {
            ptr: arc.ptr,
            #[cfg(feature = "unbounded_slices_always_pow2")]
            mask: (arc.ptr.len() - 1) as u32,
            id: arc.id,
            real: arc,
        }
    }

    /// Returns the capacity of the underlying buffer.
    #[inline(always)]
    pub(crate) fn capacity(&self) -> usize {
        self.ptr.len()
    }

    /// Returns the version id.
    #[inline(always)]
    pub(crate) fn id(&self) -> u32 {
        self.id
    }

    /// Returns a physical index of the given logical index.
    #[inline(always)]
    pub(crate) fn physical_index(&self, logical_index: u32) -> u32 {
        #[cfg(feature = "unbounded_slices_always_pow2")]
        {
            logical_index & self.mask
        }

        #[cfg(not(feature = "unbounded_slices_always_pow2"))]
        {
            logical_index % self.capacity() as u32
        }
    }

    /// Returns a raw pointer to the underlying buffer.
    #[inline(always)]
    pub(crate) fn thin_ptr(&self) -> *const MaybeUninit<T> {
        unsafe { &*self.ptr }.as_ptr()
    }

    /// Returns a mutable raw pointer to the underlying buffer.
    #[inline(always)]
    pub(crate) unsafe fn thin_mut_ptr(&self) -> *mut MaybeUninit<T> {
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
