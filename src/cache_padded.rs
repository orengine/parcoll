//! Provides cache-padded atomic types.
use crate::loom_bindings::sync::atomic::{
    AtomicI16, AtomicI32, AtomicI64, AtomicI8, AtomicIsize, AtomicU16, AtomicU32, AtomicU64,
    AtomicU8, AtomicUsize,
};

macro_rules! generate_cache_padded_type {
    ($wrapper:ident, $atomic:ident, $init:expr) => {
        pub struct $wrapper($atomic);

        impl core::ops::Deref for $wrapper {
            type Target = $atomic;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl core::ops::DerefMut for $wrapper {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }

        impl Default for $wrapper {
            fn default() -> Self {
                Self($init)
            }
        }
    };
}

generate_cache_padded_type!(CachePaddedAtomicU8, AtomicU8, AtomicU8::new(0));
generate_cache_padded_type!(CachePaddedAtomicU16, AtomicU16, AtomicU16::new(0));
generate_cache_padded_type!(CachePaddedAtomicU32, AtomicU32, AtomicU32::new(0));
generate_cache_padded_type!(CachePaddedAtomicU64, AtomicU64, AtomicU64::new(0));
generate_cache_padded_type!(CachePaddedAtomicUsize, AtomicUsize, AtomicUsize::new(0));

generate_cache_padded_type!(CachePaddedAtomicI8, AtomicI8, AtomicI8::new(0));
generate_cache_padded_type!(CachePaddedAtomicI16, AtomicI16, AtomicI16::new(0));
generate_cache_padded_type!(CachePaddedAtomicI32, AtomicI32, AtomicI32::new(0));
generate_cache_padded_type!(CachePaddedAtomicI64, AtomicI64, AtomicI64::new(0));
generate_cache_padded_type!(CachePaddedAtomicIsize, AtomicIsize, AtomicIsize::new(0));
