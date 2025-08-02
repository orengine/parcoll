use crate::backoff::Backoff;
use crate::spmc::{new_bounded, new_cache_padded_bounded, new_cache_padded_unbounded, new_unbounded, Consumer as ConsumerExt, Producer as ProducerExt};
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::spawn;
use crate::test_lock::TEST_LOCK;
// Note: test values are boxed in Miri tests so that destructors called on freed
// values and forgotten destructors can be detected.

#[cfg(miri)]
type TestValue<T> = Box<T>;

#[cfg(not(miri))]
#[derive(Debug, Default, PartialEq, Copy, Clone)]
struct TestValue<T>(T);

#[cfg(not(miri))]
impl<T> TestValue<T> {
    fn new(val: T) -> Self {
        Self(val)
    }
}

#[cfg(not(miri))]
impl<T> std::ops::Deref for TestValue<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

static RAND: AtomicUsize = AtomicUsize::new(4);

fn test_spmc_multi_threaded_steal<Producer, Consumer>(creator: fn() -> (Producer, Consumer))
    where
        Producer: ProducerExt<TestValue<usize>> + 'static,
        Consumer: ConsumerExt<TestValue<usize>, AssociatedProducer = Producer> + 'static
{
    const N: usize = if cfg!(miri) { 200 } else { 10_000_000 };
    const CHECK_TO: usize = if cfg!(feature = "always_steal") {
        N
    } else {
        N - 10
    };

    let counter = Arc::new(AtomicUsize::new(0));
    let (mut producer, mut consumer) = creator();

    let counter0 = counter.clone();
    let mut consumer1 = consumer.clone();
    let counter1 = counter.clone();
    let mut consumer2 = consumer.clone();
    let counter2 = counter;

    // Producer thread.
    //
    // Push all numbers from 0 to N, popping one from time to time.
    let t0 = spawn(move || {
        let mut i = 0;
        let mut stats = vec![0; N];

        'outer: loop {
            for _ in 0..RAND.fetch_add(1, Ordering::Relaxed) % 10 {
                while let Err(_) = producer.maybe_push(TestValue::new(i)) {}

                i += 1;

                if i == N {
                    break 'outer;
                }
            }

            if let Some(j) = producer.pop() {
                stats[*j] += 1;

                counter0.fetch_add(1, Ordering::Relaxed);
            }
        }

        stats
    });

    // Stealer threads.
    //
    // Repeatedly steal a random number of items.
    let steal_periodically = move |
        consumer: &mut Consumer,
        counter: Arc<AtomicUsize>,
    | -> Vec<usize> {

        let mut stats = vec![0; N];
        let (mut dest_producer, _) = creator();

        loop {
            consumer.steal_into(&mut dest_producer);

            while let Some(i) = dest_producer.pop() {
                stats[*i] += 1;
                counter.fetch_add(1, Ordering::Relaxed);
            }

            let count = counter.load(Ordering::Relaxed);

            if count == N || !cfg!(feature = "always_steal") && count > CHECK_TO {
                break;
            }

            assert!(count < N);
        }

        stats
    };

    let t1 = spawn(move || steal_periodically(&mut consumer1, counter1));
    let t2 = spawn(move || steal_periodically(&mut consumer2, counter2));
    let mut stats = Vec::new();

    stats.push(t0.join().unwrap());
    stats.push(t1.join().unwrap());
    stats.push(t2.join().unwrap());

    let check_to = if cfg!(feature = "always_steal") {
        N
    } else {
        CHECK_TO
    };

    if consumer.len() + check_to > N {
        let mut delta = consumer.len() + check_to - N;
        assert!(delta < 100);

        let mut slice = [const { MaybeUninit::uninit() }; 100];

        while delta > 0 {
            let popped = consumer.pop_many(slice.as_mut_slice());
            for i in 0..popped {
                stats[0][unsafe { *slice[i].assume_init() }] += 1;
                delta -= 1;
            }
        }
    }

    for i in 0..check_to {
        let mut count = 0;

        for j in 0..stats.len() {
            count += stats[j][i];
        }

        assert_eq!(count, 1, "stats[{i}] = {}", count);
    }
}

fn test_spmc_multi_threaded_pop_many<Producer, Consumer>(creator: fn() -> (Producer, Consumer))
    where
        Producer: ProducerExt<TestValue<usize>> + 'static,
        Consumer: ConsumerExt<TestValue<usize>, AssociatedProducer = Producer> + 'static
{
    const N: usize = if cfg!(miri) { 200 } else { 10_000_000 };
    const BATCH_SIZE: usize = 5;
    const RES: usize = (N - 1) * N / 2;

    let consumer_pop_many = |consumer: &mut Consumer, count: &AtomicUsize| {
        let mut slice = [MaybeUninit::uninit(); BATCH_SIZE];
        let backoff = Backoff::new();

        for _ in 0..N {
            let popped = consumer.pop_many(&mut slice);

            for i in 0..popped {
                let res = count.fetch_add(unsafe { *slice[i].assume_init() }, Ordering::Relaxed);

                if res == RES {
                    break;
                }
            }

            if popped < BATCH_SIZE {
                backoff.snooze();
            } else {
                backoff.reset();
            }
        }
    };

    let (mut producer, consumer) = creator();
    let mut consumer1 = consumer.clone();
    let mut consumer2 = consumer.clone();
    let count = Arc::new(AtomicUsize::new(0));
    let count1 = count.clone();
    let count2 = count.clone();

    let t0 = spawn(move || consumer_pop_many(&mut consumer1, &count1));
    let t1 = spawn(move || consumer_pop_many(&mut consumer2, &count2));

    let mut slice = [TestValue(0); BATCH_SIZE];
    for i in 0..N / BATCH_SIZE {
        for j in 0..BATCH_SIZE {
            slice[j] = TestValue(i * BATCH_SIZE + j);
        }

        let backoff = Backoff::new();

        while producer.maybe_push_many(&slice[..BATCH_SIZE]).is_err() {
            backoff.snooze();
        }
    }

    t0.join().unwrap();
    t1.join().unwrap();

    assert_eq!(count.load(Ordering::Relaxed), RES);
}

#[test]
fn test_bounded_spmc_multi_threaded_steal() {
    let test_guard = TEST_LOCK.lock();

    test_spmc_multi_threaded_steal(new_bounded::<TestValue<usize>, 256>);

    println!("Non cache padded done, start cache padded");

    test_spmc_multi_threaded_steal(new_cache_padded_bounded::<TestValue<usize>, 256>);

    drop(test_guard);
}

#[test]
fn test_unbounded_spmc_multi_threaded_steal() {
    let test_guard = TEST_LOCK.lock();

    test_spmc_multi_threaded_steal(|| {
        let mut queue = new_unbounded();

        queue.0.reserve(128);

        queue
    });

    println!("Non cache padded done, start cache padded");

    test_spmc_multi_threaded_steal(|| {
        let mut queue = new_cache_padded_unbounded();

        queue.0.reserve(128);

        queue
    });

    drop(test_guard);
}

#[test]
fn test_bounded_spmc_multi_threaded_pop_many() {
    let test_guard = TEST_LOCK.lock();

    test_spmc_multi_threaded_pop_many(new_bounded::<TestValue<usize>, 256>);

    println!("Non cache padded done, start cache padded");

    test_spmc_multi_threaded_pop_many(new_cache_padded_bounded::<TestValue<usize>, 256>);

    drop(test_guard);
}

#[test]
fn test_unbounded_spmc_multi_threaded_pop_many() {
    let test_guard = TEST_LOCK.lock();

    test_spmc_multi_threaded_pop_many(new_unbounded);

    println!("Non cache padded done, start cache padded");

    test_spmc_multi_threaded_pop_many(new_cache_padded_unbounded);

    drop(test_guard);
}