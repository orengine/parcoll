use std::cell::UnsafeCell;
use std::fmt;
use std::ops;
use std::panic;

/// `AtomicUsize` providing an additional `unsync_load` function.
pub struct AtomicUsize {
    inner: UnsafeCell<std::sync::atomic::AtomicUsize>,
}

unsafe impl Send for AtomicUsize {}
unsafe impl Sync for AtomicUsize {}
impl panic::RefUnwindSafe for AtomicUsize {}
impl panic::UnwindSafe for AtomicUsize {}

impl AtomicUsize {
    pub const fn new(val: usize) -> Self {
        let inner = UnsafeCell::new(std::sync::atomic::AtomicUsize::new(val));

        Self { inner }
    }

    /// Performs an unsynchronized load.
    ///
    /// # Safety
    ///
    /// All mutations must have happened before the unsynchronized load.
    /// Additionally, there must be no concurrent mutations.
    pub unsafe fn unsync_load(&self) -> usize {
        core::ptr::read(self.inner.get() as *const usize)
    }

    pub fn with_mut<R>(&mut self, f: impl FnOnce(&mut usize) -> R) -> R {
        // safety: we have mutable access
        f(unsafe { (*self.inner.get()).get_mut() })
    }
}

impl Default for AtomicUsize {
    fn default() -> Self {
        Self::new(0)
    }
}

impl ops::Deref for AtomicUsize {
    type Target = std::sync::atomic::AtomicUsize;

    fn deref(&self) -> &Self::Target {
        // safety: it is always safe to access `&self` fns on the inner value as
        // we never perform unsafe mutations.
        unsafe { &*self.inner.get() }
    }
}

impl ops::DerefMut for AtomicUsize {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // safety: we hold `&mut self`
        unsafe { &mut *self.inner.get() }
    }
}

impl fmt::Debug for AtomicUsize {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(fmt)
    }
}

/// `AtomicIsize` providing an additional `unsync_load` function.
pub struct AtomicIsize {
    inner: UnsafeCell<std::sync::atomic::AtomicIsize>,
}

unsafe impl Send for AtomicIsize {}
unsafe impl Sync for AtomicIsize {}
impl panic::RefUnwindSafe for AtomicIsize {}
impl panic::UnwindSafe for AtomicIsize {}

impl AtomicIsize {
    pub const fn new(val: isize) -> Self {
        let inner = UnsafeCell::new(std::sync::atomic::AtomicIsize::new(val));

        Self { inner }
    }

    /// Performs an unsynchronized load.
    ///
    /// # Safety
    ///
    /// All mutations must have happened before the unsynchronized load.
    /// Additionally, there must be no concurrent mutations.
    pub unsafe fn unsync_load(&self) -> isize {
        core::ptr::read(self.inner.get() as *const isize)
    }

    pub fn with_mut<R>(&mut self, f: impl FnOnce(&mut isize) -> R) -> R {
        // safety: we have mutable access
        f(unsafe { (*self.inner.get()).get_mut() })
    }
}

impl Default for AtomicIsize {
    fn default() -> Self {
        Self::new(0)
    }
}

impl ops::Deref for AtomicIsize {
    type Target = std::sync::atomic::AtomicIsize;

    fn deref(&self) -> &Self::Target {
        // safety: it is always safe to access `&self` fns on the inner value as
        // we never perform unsafe mutations.
        unsafe { &*self.inner.get() }
    }
}

impl ops::DerefMut for AtomicIsize {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // safety: we hold `&mut self`
        unsafe { &mut *self.inner.get() }
    }
}

impl fmt::Debug for AtomicIsize {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(fmt)
    }
}