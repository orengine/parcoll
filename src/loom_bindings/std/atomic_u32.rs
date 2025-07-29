use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Deref;
use std::panic;

/// `AtomicU32` providing an additional `unsync_load` function.
pub struct AtomicU32 {
    inner: UnsafeCell<std::sync::atomic::AtomicU32>,
}

unsafe impl Send for AtomicU32 {}
unsafe impl Sync for AtomicU32 {}
impl panic::RefUnwindSafe for AtomicU32 {}
impl panic::UnwindSafe for AtomicU32 {}

impl AtomicU32 {
    pub const fn new(val: u32) -> Self {
        let inner = UnsafeCell::new(std::sync::atomic::AtomicU32::new(val));

        Self { inner }
    }

    /// Performs an unsynchronized load.
    ///
    /// # Safety
    ///
    /// All mutations must have happened before the unsynchronized load.
    /// Additionally, there must be no concurrent mutations.
    pub(crate) unsafe fn unsync_load(&self) -> u32 {
        core::ptr::read(self.inner.get() as *const u32)
    }
}

impl Default for AtomicU32 {
    fn default() -> AtomicU32 {
        Self {
            inner: UnsafeCell::new(std::sync::atomic::AtomicU32::new(0)),
        }
    }
}

impl Deref for AtomicU32 {
    type Target = std::sync::atomic::AtomicU32;

    fn deref(&self) -> &Self::Target {
        // safety: it is always safe to access `&self` fns on the inner value as
        // we never perform unsafe mutations.
        unsafe { &*self.inner.get() }
    }
}

impl fmt::Debug for AtomicU32 {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.deref().fmt(fmt)
    }
}
