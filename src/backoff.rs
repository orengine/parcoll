//! This module provides a [`Backoff`] that can be used to busy-wait with
//! preemptive yield when it is necessary.
//!
//! It has the same API as [`crossbeam::Backoff`].
use crate::hints::likely;
use core::cell::Cell;
use core::fmt;

const SPIN_LIMIT: u32 = 6;

/// Performs exponential backoff in spin loops.
///
/// Backing off in spin loops reduces contention and improves overall performance.
///
/// This primitive can execute *YIELD* and *PAUSE* instructions, yield the current thread to the OS
/// scheduler, and tell when is a good time to block the thread using a different synchronization
/// mechanism. Each step of the back off procedure takes roughly twice as long as the previous
/// step.
///
/// # Examples
///
/// Backing off in a lock-free loop:
///
/// ```
/// use std::sync::atomic::AtomicUsize;
/// use std::sync::atomic::Ordering::SeqCst;
/// use parcoll::Backoff;
///
/// fn fetch_mul(a: &AtomicUsize, b: usize) -> usize {
///     let backoff = Backoff::new();
///
///     loop {
///         let val = a.load(SeqCst);
///         if a.compare_exchange(val, val.wrapping_mul(b), SeqCst, SeqCst).is_ok() {
///             return val;
///         }
///
///         backoff.spin();
///     }
/// }
/// ```
///
/// Waiting for an [`AtomicBool`] to become `true`:
///
/// ```
/// use std::sync::atomic::AtomicBool;
/// use std::sync::atomic::Ordering::SeqCst;
/// use parcoll::Backoff;
///
/// fn spin_wait(ready: &AtomicBool) {
///     let backoff = Backoff::new();
///
///     while !ready.load(SeqCst) {
///         backoff.snooze();
///     }
/// }
/// ```
///
/// Waiting for an [`AtomicBool`] to become `true` and parking the thread after a long wait.
/// Note that whoever sets the atomic variable to `true` must notify the parked thread by calling
/// [`unpark()`]:
///
/// ```
/// use parcoll::Backoff;
/// use std::sync::atomic::AtomicBool;
/// use std::sync::atomic::Ordering::SeqCst;
/// use std::thread;
///
/// fn blocking_wait(ready: &AtomicBool) {
///     let backoff = Backoff::new();
///
///     while !ready.load(SeqCst) {
///         if backoff.is_completed() {
///             thread::park();
///         } else {
///             backoff.snooze();
///         }
///     }
/// }
/// ```
///
/// [`std::thread::park()`]: std::thread::park
/// [`AtomicBool`]: std::sync::atomic::AtomicBool
/// [`unpark()`]: std::thread::Thread::unpark
pub struct Backoff {
    step: Cell<u32>,
}

impl Backoff {
    /// Creates a new `Backoff` instance.
    #[inline]
    pub fn new() -> Self {
        Self { step: Cell::new(0) }
    }

    /// Resets the backoff state.
    #[inline]
    pub fn reset(&self) {
        self.step.set(0);
    }

    /// Backs off in a lock-free loop.
    ///
    /// This method should be used when we need to retry an operation because another thread made
    /// progress.
    ///
    /// The processor may yield using the *YIELD* or *PAUSE* instruction.
    ///
    /// # Examples
    ///
    /// Backing off in a lock-free loop:
    ///
    /// ```
    /// use parcoll::Backoff;
    /// use std::sync::atomic::AtomicUsize;
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// fn fetch_mul(a: &AtomicUsize, b: usize) -> usize {
    ///     let backoff = Backoff::new();
    ///
    ///     loop {
    ///         let val = a.load(SeqCst);
    ///         if a.compare_exchange(val, val.wrapping_mul(b), SeqCst, SeqCst).is_ok() {
    ///             return val;
    ///         }
    ///
    ///         backoff.spin();
    ///     }
    /// }
    ///
    /// let a = AtomicUsize::new(7);
    /// assert_eq!(fetch_mul(&a, 8), 7);
    /// assert_eq!(a.load(SeqCst), 56);
    /// ```
    #[inline]
    #[allow(
        dead_code,
        reason = "It is fork, therefore it is more convenient to keep all original methods"
    )]
    pub fn spin(&self) {
        for _ in 0..1 << self.step.get().min(SPIN_LIMIT) {
            crate::loom_bindings::hint::spin_loop();
        }

        self.step.set(self.step.get() + 1);
    }

    /// Backs off in a blocking loop.
    ///
    /// This method should be used when we need to wait for another thread to make progress.
    ///
    /// The processor may yield using the *YIELD* or *PAUSE* instruction, and the current thread
    /// may yield by giving up a timeslice to the OS scheduler.
    ///
    /// In `#[no_std]` environments, this method is equivalent to [`spin`].
    ///
    /// If possible, use [`is_completed`] to check when it is advised to stop using backoff and
    /// block the current thread using a different synchronization mechanism instead.
    ///
    /// [`spin`]: Backoff::spin
    /// [`is_completed`]: Backoff::is_completed
    ///
    /// # Examples
    ///
    /// Waiting for an [`AtomicBool`] to become `true`:
    ///
    /// ```
    /// use parcoll::Backoff;
    /// use std::sync::Arc;
    /// use std::sync::atomic::AtomicBool;
    /// use std::sync::atomic::Ordering::SeqCst;
    /// use std::thread;
    /// use std::time::Duration;
    ///
    /// fn spin_wait(ready: &AtomicBool) {
    ///     let backoff = Backoff::new();
    ///
    ///     while !ready.load(SeqCst) {
    ///         backoff.snooze();
    ///     }
    /// }
    ///
    /// let ready = Arc::new(AtomicBool::new(false));
    /// let ready2 = ready.clone();
    ///
    /// # let t =
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_millis(100));
    ///
    ///     ready2.store(true, SeqCst);
    /// });
    ///
    /// assert_eq!(ready.load(SeqCst), false);
    ///
    /// spin_wait(&ready);
    ///
    /// assert_eq!(ready.load(SeqCst), true);
    ///
    /// # t.join().unwrap(); // join thread to avoid https://github.com/rust-lang/miri/issues/1371
    /// ```
    ///
    /// [`AtomicBool`]: std::sync::atomic::AtomicBool
    #[inline]
    pub fn snooze(&self) {
        if likely(self.step.get() <= SPIN_LIMIT) {
            for _ in 0..1 << self.step.get().min(SPIN_LIMIT) {
                crate::loom_bindings::hint::spin_loop();
            }
        } else {
            crate::loom_bindings::thread::yield_now();
        }

        self.step.set(self.step.get() + 1);
    }

    /// Returns `true` if exponential backoff has completed and blocking the thread is advised.
    ///
    /// # Examples
    ///
    /// Waiting for an [`AtomicBool`] to become `true` and parking the thread after a long wait:
    ///
    /// ```
    /// use parcoll::Backoff;
    /// use std::sync::Arc;
    /// use std::sync::atomic::AtomicBool;
    /// use std::sync::atomic::Ordering::SeqCst;
    /// use std::thread;
    /// use std::time::Duration;
    ///
    /// fn blocking_wait(ready: &AtomicBool) {
    ///     let backoff = Backoff::new();
    ///
    ///     while !ready.load(SeqCst) {
    ///         if backoff.is_completed() {
    ///             thread::park();
    ///         } else {
    ///             backoff.snooze();
    ///         }
    ///     }
    /// }
    ///
    /// let ready = Arc::new(AtomicBool::new(false));
    /// let ready2 = ready.clone();
    /// let waiter = thread::current();
    ///
    /// # let t =
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_millis(100));
    ///
    ///     ready2.store(true, SeqCst);
    ///
    ///     waiter.unpark();
    /// });
    ///
    /// assert_eq!(ready.load(SeqCst), false);
    ///
    /// blocking_wait(&ready);
    ///
    /// assert_eq!(ready.load(SeqCst), true);
    /// # t.join().unwrap(); // join thread to avoid https://github.com/rust-lang/miri/issues/1371
    /// ```
    ///
    /// [`AtomicBool`]: std::sync::atomic::AtomicBool
    #[inline]
    pub fn is_completed(&self) -> bool {
        self.step.get() >= SPIN_LIMIT
    }
}

impl fmt::Debug for Backoff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Backoff")
            .field("step", &self.step)
            .field("is_completed", &self.is_completed())
            .finish()
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}
