//! This module contains a lock for tests.
use crate::loom_bindings::sync::Mutex;

/// Lock for tests.
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());