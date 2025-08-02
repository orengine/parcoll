#![cfg_attr(parcoll_loom, allow(unused_imports, dead_code))]

mod atomic16;
mod atomic32;
mod atomic64;
mod atomic8;
mod atomic_ptr;
mod atomic_usize;
mod barrier;
mod mutex;
mod rwlock;
mod unsafe_cell;

pub mod cell {}

pub mod hint {
    pub use std::hint::spin_loop;
}

pub mod rand {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hash, Hasher};
    use std::sync::atomic::AtomicU32;
    use std::sync::atomic::Ordering::Relaxed;

    static COUNTER: AtomicU32 = AtomicU32::new(1);

    pub fn seed() -> u64 {
        let rand_state = RandomState::new();

        // Get the seed
        rand_state.hash_one(COUNTER.fetch_add(1, Relaxed))
    }
}

pub mod sync {
    pub use std::sync::Arc;

    // Below, make sure all the feature-influenced types are exported for
    // internal use. Note however that some are not _currently_ named by
    // consuming code.

    pub use std::sync::{
        Condvar, MutexGuard, RwLockReadGuard, RwLockWriteGuard, WaitTimeoutResult,
    };

    pub use crate::loom_bindings::std::mutex::Mutex;
    pub use crate::loom_bindings::std::rwlock::RwLock;

    pub mod atomic {
        pub use crate::loom_bindings::std::atomic_ptr::AtomicPtr;
        pub use crate::loom_bindings::std::atomic_usize::{AtomicIsize, AtomicUsize};
        pub use crate::loom_bindings::std::atomic8::{AtomicI8, AtomicU8};
        pub use crate::loom_bindings::std::atomic16::{AtomicI16, AtomicU16};
        pub use crate::loom_bindings::std::atomic32::{AtomicI32, AtomicU32};
        pub use crate::loom_bindings::std::atomic64::{AtomicI64, AtomicU64};
    }
}

pub mod sys {
    use std::num::NonZeroUsize;

    pub(crate) fn num_cpus() -> usize {
        std::thread::available_parallelism().map_or(1, NonZeroUsize::get)
    }
}

pub mod thread {
    #[inline]
    pub fn yield_now() {
        std::thread::yield_now();
    }

    pub use std::thread::{
        AccessError, Builder, JoinHandle, LocalKey, Result, Thread, ThreadId, current, panicking,
        park, park_timeout, sleep, spawn,
    };
}
