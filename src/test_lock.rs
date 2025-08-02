//! This module contains a lock for tests.
#[cfg(all(not(parcoll_loom), test))]
use crate::loom_bindings::sync::Mutex;

/// Lock for tests.
#[cfg(all(not(parcoll_loom), test))]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());
