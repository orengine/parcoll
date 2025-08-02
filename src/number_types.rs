use std::ops::Deref;
use crate::loom_bindings::sync::atomic::{AtomicI32, AtomicU32, AtomicU64};

#[cfg(target_has_atomic = "64")]
/// Synonym for the unsigned number with the size of the longest atomic.
pub type LongNumber = u64;
#[cfg(all(target_has_atomic = "32", not(target_pointer_width = "64")))]
/// Synonym for the unsigned number with the size of the longest atomic.
pub type LongNumber = u32;
#[cfg(all(
    target_has_atomic = "16",
    all(not(target_has_atomic = "32"), not(target_pointer_width = "64"))
))]
/// Synonym for the unsigned number with the size of the longest atomic.
pub type LongNumber = u16;

#[cfg(target_has_atomic = "64")]
/// Synonym for the longest atomic.
pub type LongAtomic = AtomicU64;
#[cfg(all(target_has_atomic = "32", not(target_pointer_width = "64")))]
/// Synonym for the longest atomic.
pub type LongAtomic = AtomicU32;
#[cfg(all(
    target_has_atomic = "16",
    all(not(target_has_atomic = "32"), not(target_pointer_width = "64"))
))]
/// Synonym for the longest atomic.
pub type LongAtomic = loom_bindings::sync::atomic::AtomicU16;

#[cfg(target_has_atomic = "64")]
/// Synonym for the cache padded longest atomic.
pub type CachePaddedLongAtomic = crate::cache_padded::CachePaddedAtomicU64;
#[cfg(all(target_has_atomic = "32", not(target_pointer_width = "64")))]
/// Synonym for the cache padded longest atomic.
pub type CachePaddedLongAtomic = crate::cache_padded::CachePaddedAtomicU32;
#[cfg(all(
    target_has_atomic = "16",
    all(not(target_has_atomic = "32"), not(target_pointer_width = "64"))
))]
/// Synonym for the longest atomic.
pub type CachePaddedLongAtomic = crate::cache_padded::CachePaddedAtomicU16;

/// Synonym for the non-cache padded longest atomic.
pub struct NotCachePaddedLongAtomic(LongAtomic);

impl Deref for NotCachePaddedLongAtomic {
    type Target = LongAtomic;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Default for NotCachePaddedLongAtomic {
    fn default() -> Self {
        Self(LongAtomic::new(0))
    }
}

/// Synonym for the non-cache padded [`AtomicU32`].
pub struct NotCachePaddedAtomicU32(AtomicU32);

impl Deref for NotCachePaddedAtomicU32 {
    type Target = AtomicU32;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Default for NotCachePaddedAtomicU32 {
    fn default() -> Self {
        Self(AtomicU32::new(0))
    }
}

/// Synonym for the non-cache padded [`AtomicU64`].
pub struct NotCachePaddedAtomicU64(AtomicU64);

impl Deref for NotCachePaddedAtomicU64 {
    type Target = AtomicU64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Default for NotCachePaddedAtomicU64 {
    fn default() -> Self {
        Self(AtomicU64::new(0))
    }
}

pub struct NonCachePaddedAtomicI32 {
    atomic: AtomicI32,
}

impl Deref for NonCachePaddedAtomicI32 {
    type Target = AtomicI32;

    fn deref(&self) -> &Self::Target {
        &self.atomic
    }
}

impl Default for NonCachePaddedAtomicI32 {
    fn default() -> Self {
        NonCachePaddedAtomicI32 { atomic: AtomicI32::new(0) }
    }
}