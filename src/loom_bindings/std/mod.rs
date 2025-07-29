#![cfg_attr(parcoll_loom, allow(unused_imports, dead_code))]

mod atomic_u16;
mod atomic_u32;
mod atomic_u64;
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
    pub use std::sync::{Condvar, MutexGuard, RwLockReadGuard, WaitTimeoutResult};

    pub use crate::loom_bindings::std::mutex::Mutex;

    pub mod atomic {
        pub use crate::loom_bindings::std::atomic_u16::AtomicU16;
        pub use crate::loom_bindings::std::atomic_u32::AtomicU32;
        pub use crate::loom_bindings::std::atomic_u64::AtomicU64;
        pub use crate::loom_bindings::std::atomic_usize::AtomicUsize;
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
