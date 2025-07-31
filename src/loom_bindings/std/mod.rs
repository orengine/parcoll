#![cfg_attr(parcoll_loom, allow(unused_imports, dead_code))]

mod atomic16;
mod atomic32;
mod atomic64;
mod atomic_usize;
mod barrier;
mod mutex;
mod rwlock;
mod unsafe_cell;
mod atomic8;
mod atomic_ptr;

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

        let mut hasher = rand_state.build_hasher();

        // Hash some unique-ish data to generate some new state
        COUNTER.fetch_add(1, Relaxed).hash(&mut hasher);

        // Get the seed
        hasher.finish()
    }
}

pub mod sync {
    pub use std::sync::Arc;

    // Below, make sure all the feature-influenced types are exported for
    // internal use. Note however that some are not _currently_ named by
    // consuming code.

    #[allow(unused_imports)]
    pub use std::sync::{Condvar, MutexGuard, RwLockReadGuard, WaitTimeoutResult, RwLockWriteGuard};

    pub use crate::loom_bindings::std::mutex::Mutex;
    pub use crate::loom_bindings::std::rwlock::RwLock;

    pub mod atomic {
        pub use crate::loom_bindings::std::atomic8::{AtomicU8, AtomicI8};
        pub use crate::loom_bindings::std::atomic16::{AtomicU16, AtomicI16};
        pub use crate::loom_bindings::std::atomic32::{AtomicU32, AtomicI32};
        pub use crate::loom_bindings::std::atomic64::{AtomicU64, AtomicI64};
        pub use crate::loom_bindings::std::atomic_usize::{AtomicUsize, AtomicIsize};
        pub use crate::loom_bindings::std::atomic_ptr::AtomicPtr;
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

    #[allow(unused_imports)]
    pub use std::thread::{
        current, panicking, park, park_timeout, sleep, spawn, AccessError, Builder, JoinHandle,
        LocalKey, Result, Thread, ThreadId,
    };
}
