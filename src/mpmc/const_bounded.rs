use crate::backoff::Backoff;
use crate::number_types::{LongAtomic, LongNumber, NotCachePaddedLongAtomic};
use crate::LockFreePopErr;
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::sync::atomic::Ordering;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};

// Implementation notes for an MPMC (multi-producer, multi-consumer) bounded queue.
//
// This design builds on simpler queue types. If you have not read the SPMC
// (single-producer, multi-consumer) implementation, start there because it is much
// simpler because it has only one producer. In fact, if you compare SPMC and
// SPSC (single-producer, single-consumer) queues, you’ll see that their producer
// code is identical, while the consumer side is only slightly more complex.
// The reason is that concurrent reading is safe, but concurrent writing is not.
//
// In an MPMC queue, multiple producers cannot safely update the tail pointer at
// the same time. We cannot advance `tail` before writing finishes (consumers might
// read uninitialized data), and we cannot advance it after writing finishes (two
// producers could overwrite each other’s slots). This is why a fully lock-free
// ring-buffer MPMC queue is tricky. However, we can make it *almost* lock-free
// and still very fast.
//
// The core idea is to give each slot its own atomic state (conceptually similar
// to semaphore). You could reduce this to one state per group of slots, but
// that hurts performance.
//
// First attempt:
// - Use an atomic boolean per slot for `empty` / `has value`.
// - Producers: update `tail`, write the value, then set the flag.
//   This avoids concurrent writing into the same slot.
// - Consumers: check the flag before reading.
//   Problem: the consumer may have to wait for a writing operation at the head,
//   so `pop` is no longer fully lock-free.
//   Advantage: `push` can be lock-free — producers can write into the next slot
//   while previous producers finish writing.
//
// Step-by-step model for the first attempt:
// Push: load the tail; load the head; compute a length; if full, return;
//       load a slot flag; if `has value`, retry;
//       CAS on tail; if CAS succeeds: write value, set the flag, return;
//       else retry.
// Pop:  load tail; load head; compute a length; if empty, return;
//       load a slot flag; if `empty`, retry;
//       CAS on head; if CAS succeeds: read value, set the flag to empty, return;
//       else retry.
//
// In the first approach, writers checked the slot's flag before writing, and
// readers checked it before reading. This means both sides communicate not
// only through the head and tail indexes but also through these per-slot flags.
// Writers rely on the head index to avoid overwriting unread data, and readers
// rely on the tail index to avoid reading unwritten data.
//
// However, head/tail alone do not tell us *when* a slot was last written or
// read, especially when the queue wraps around. To distinguish "still contains
// unread data from a previous pass" from "empty and ready to write," we need an
// additional concept: laps.
//
// A lap counter increments each time an index (head or tail) wraps around the
// buffer. By associating each slot with the lap number of its last writing, we
// can tell whether the data is from the current pass (safe to read) or from an
// earlier pass (stale for readers, full for writers). This allows us to stop
// checking head for writing safety and tail for reading safety, relying instead
// on per-slot state derived from lap information.
// In the `push` method load only the tail and the lap,
// in the `pop` method load only the head and the lap.
//
// Next optimization:
// Instead of a boolean, store an atomic *state* per slot:
//   - `empty`
//   - `value written this lap` (available for reading, not full for writers)
//   - `full` (value not yet read since a previous lap)
// The “full” state occurs when the slot still holds data from the previous lap
// of writing.
//
// We can encode these states using lap counters and slot indices:
//   - `lap + slot`             -> empty
//   - `current lap + slot + 1` -> value written this lap
//   - `old lap + slot + 1`     -> full (previous lap’s value)
// This "+1" encoding forbids queues of size 1 but allows inexpensive `wrapping_add`
// and simple comparisons (`== number + 1` or `< number`) without special cases.

struct State(LongAtomic);

impl State {
    #[inline(always)]
    fn new(slot_index: LongNumber) -> Self {
        Self(LongAtomic::new(slot_index))
    }

    #[inline(always)]
    fn load(&self, ordering: Ordering) -> LongNumber {
        self.0.load(ordering)
    }

    #[inline(always)]
    fn mark_as_occupied(&self, current_tail: LongNumber, ordering: Ordering) {
        self.0.store(current_tail.wrapping_add(1), ordering);
    }

    #[inline(always)]
    fn mark_as_free(&self, current_head: LongNumber, capacity: LongNumber, ordering: Ordering) {
        self.0.store(current_head.wrapping_add(capacity), ordering);
    }

    #[inline(always)]
    fn is_slot_free(current_tail: LongNumber, state_value: LongNumber) -> bool {
        current_tail == state_value
    }

    #[inline(always)]
    fn is_slot_occupied(current_head: LongNumber, state_value: LongNumber) -> bool {
        current_head.wrapping_add(1) == state_value
    }

    #[inline(always)]
    fn is_queue_empty(current_head: LongNumber, state_value: LongNumber) -> bool {
        current_head == state_value
    }

    #[inline(always)]
    fn is_queue_full(
        current_tail: LongNumber,
        state_value: LongNumber,
        capacity: LongNumber,
    ) -> bool {
        current_tail.wrapping_sub(capacity + 1) == state_value
    }
}

struct Slot<T> {
    state: State,
    value: UnsafeCell<MaybeUninit<T>>,
}

impl<T> Slot<T> {
    fn new_without_value(slot_index: LongNumber) -> Self {
        Self {
            state: State::new(slot_index),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    fn state(&self) -> &State {
        &self.state
    }

    unsafe fn read_value(&self) -> T {
        unsafe { self.value.get().cast::<T>().read() }
    }

    unsafe fn write_value(&self, value: T) {
        unsafe { self.value.get().cast::<T>().write(value) }
    }
}

#[repr(C)]
pub struct MPMCBoundedQueue<
    T,
    const CAPACITY: usize,
    AtomicWrapper: Deref<Target = LongAtomic> + Default = NotCachePaddedLongAtomic,
> {
    tail: AtomicWrapper,
    head: AtomicWrapper,
    // There were two decisions how to store the slots.
    // The first one is to store the slots in one array (it was chosen).
    // The second one is to store `states` and `values` in separate arrays.
    //
    // The first one is more efficient for single `push`/`pop` operations
    // because it provides better single slot cache distributing.
    //
    // The second one is more efficient for `push_many`/`pop_many` operations
    // because it allows loading more `states` at once.
    //
    // But based on observations, it is more likely to use MPMC queue for
    // single `push`/`pop` operations.
    slots: [Slot<T>; CAPACITY],
}

impl<T, const CAPACITY: usize, AtomicWrapper> MPMCBoundedQueue<T, CAPACITY, AtomicWrapper>
where
    AtomicWrapper: Deref<Target = LongAtomic> + Default,
{
    /// Creates a new [`MPMCBoundedQueue`].
    pub fn new() -> Self {
        debug_assert!(
            CAPACITY >= 2,
            "MPMCBoundedQueue should have at least 2 slots"
        );
        debug_assert!(size_of::<MaybeUninit<T>>() == size_of::<T>()); // Assume that we can just cast it

        let mut buffer: MaybeUninit<[Slot<T>; CAPACITY]> = MaybeUninit::uninit();
        let buffer_ptr = unsafe { buffer.assume_init_mut() }.as_mut_ptr();

        for i in 0..CAPACITY {
            unsafe {
                buffer_ptr
                    .add(i)
                    .write(Slot::new_without_value(i as LongNumber));
            };
        }

        Self {
            slots: unsafe { buffer.assume_init() },
            tail: AtomicWrapper::default(),
            head: AtomicWrapper::default(),
        }
    }

    /// Tries to push a value into the queue. Returns `Err(value)` if the queue is full.
    pub(crate) fn maybe_push(&self, value: T) -> Result<(), T> {
        let mut tail = self.tail.load(Relaxed);
        let mut slot: &Slot<T>; // TODO annotation

        loop {
            slot = &self.slots[tail as usize % CAPACITY];
            let state = slot.state().load(Acquire);

            if State::is_slot_free(tail, state) {
                let cas_result =
                    self.tail
                        .compare_exchange_weak(tail, tail.wrapping_add(1), Relaxed, Relaxed);

                match cas_result {
                    Ok(_) => {
                        unsafe { slot.write_value(value) };

                        slot.state().mark_as_occupied(tail, Release);

                        break Ok(());
                    }
                    Err(current_tail) => {
                        tail = current_tail;
                    }
                }
            } else if State::is_queue_full(tail, state, CAPACITY as LongNumber) {
                // Some value has occupied the slot since the previous lap.
                // It means that the queue is full.
                break Err(value);
            } else {
                // We lose the race. Try to push again.
                tail = self.tail.load(Relaxed);
                // Likely we were preempted, we should not call Backoff::snooze here.
            }
        }
    }
}

macro_rules! generate_pop_body {
    (
        $self:ident,
        $backoff_ident:ident,
        $backoff_init:expr,
        $on_writer_block:block,
        $success_value_ident:ident,
        $on_success_value:block,
        $on_empty:block
    ) => {{
        let mut head = $self.head.load(Relaxed);
        let mut slot: &Slot<T>;
        #[allow(unused, reason = "It improve readability")]
        let $backoff_ident = $backoff_init;

        loop {
            slot = &$self.slots[head as usize % CAPACITY];
            let state = slot.state().load(Acquire);

            if State::is_slot_occupied(head, state) {
                let cas_result =
                    $self
                        .head
                        .compare_exchange_weak(head, head.wrapping_add(1), Relaxed, Relaxed);

                match cas_result {
                    Ok(_) => {
                        let $success_value_ident = unsafe { slot.read_value() };

                        slot.state()
                            .mark_as_free(head, CAPACITY as LongNumber, Release);

                        $on_success_value
                    }
                    Err(current_head) => {
                        head = current_head;
                    }
                }
            } else if State::is_queue_empty(head, state) {
                $on_empty
            } else {
                let old_head = head;
                // We lose the race or the value is still not written. Try to pop again.
                head = $self.head.load(Relaxed);

                if head == old_head {
                    $on_writer_block // Maybe the writer has been preempted.
                }
            }
        }
    }};
}

// TODO BUG:
// head is moved -> read interrupt -> producer skip slot
// before it have been read -> slot is read -> slot is marked as free

impl<T, const CAPACITY: usize, AtomicWrapper> MPMCBoundedQueue<T, CAPACITY, AtomicWrapper>
where
    AtomicWrapper: Deref<Target = LongAtomic> + Default,
{
    /// Tries to pop a value from the queue. Returns `None` if the queue is empty.
    ///
    /// # Note
    ///
    /// This method is not lock-free.
    /// If you want to use a lock-free version, use [`lock_free_pop`](MPMCBoundedQueue::lock_free_pop).
    pub(crate) fn pop(&self) -> Option<T> {
        generate_pop_body!(
            self,
            backoff,
            Backoff::new(),
            { backoff.snooze() },
            value,
            {
                return Some(value);
            },
            {
                return None;
            }
        )
    }

    /// Pops values from the queue into the given buffer.
    /// Returns the number of popped values.
    ///
    /// # Note
    ///
    /// This method is not lock-free.
    /// If you want to use a lock-free version, use [`lock_free_pop_many`](MPMCBoundedQueue::lock_free_pop_many).
    pub(crate) fn pop_many(&self, dst: &mut [MaybeUninit<T>]) -> usize {
        let mut popped = 0;

        while popped < dst.len() {
            if let Some(value) = self.pop() {
                dst[popped].write(value);

                popped += 1;
            } else {
                break;
            }
        }

        popped
    }

    /// Tries to pop a value from the queue.
    /// On failure, returns `Err(`[`LockFreePopErr`]`)`.
    ///
    /// # Note
    ///
    /// This method is lock-free.
    /// If you want to use sync non-lock-free version, use [`pop`](MPMCBoundedQueue::pop).
    pub(crate) fn lock_free_pop(&self) -> Result<T, LockFreePopErr> {
        generate_pop_body!(
            self,
            _backoff,
            (),
            {
                return Err(LockFreePopErr::ShouldWait);
            },
            value,
            {
                return Ok(value);
            },
            {
                return Err(LockFreePopErr::Empty);
            }
        )
    }

    /// Tries to pop a value from the queue.
    /// Returns the number of popped values and whether the operation failed because it should wait.
    ///
    /// # Note
    ///
    /// This method is lock-free.
    /// If you want to use sync non-lock-free version, use [`lock_free_pop`](MPMCBoundedQueue::lock_free_pop).
    pub(crate) fn lock_free_pop_many(&self, dst: &mut [MaybeUninit<T>]) -> (usize, bool) {
        let mut popped = 0;

        while popped < dst.len() {
            match self.lock_free_pop() {
                Ok(value) => {
                    dst[popped].write(value);

                    popped += 1;
                }
                Err(LockFreePopErr::Empty) => {
                    return (popped, false);
                }
                Err(LockFreePopErr::ShouldWait) => {
                    return (popped, true);
                }
            }
        }

        (popped, false)
    }
}
