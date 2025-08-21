use crate::backoff::Backoff;
use crate::loom_bindings::thread::yield_now;
use crate::single_producer::SingleProducer;
use crate::spsc::{
    new_bounded, new_cache_padded_bounded, new_cache_padded_unbounded, new_unbounded,
};
use crate::test_lock::TEST_LOCK;
use crate::{Consumer as ConsumerExt, Producer as ProducerExt};
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::spawn;

static RAND: AtomicUsize = AtomicUsize::new(4);

#[allow(clippy::uninlined_format_args, reason = "Else cargo shows a warning")]
fn test_spsc_multi_threaded_steal<Producer, Consumer>(creator: fn() -> (Producer, Consumer))
where
    Producer: ProducerExt<usize> + SingleProducer<usize> + Send + 'static,
    Consumer: ConsumerExt<usize> + Send + 'static,
{
    const N: usize = 1_000_000;
    const CHECK_TO: usize = if cfg!(feature = "always_steal") {
        N
    } else {
        N - 10
    };

    let (producer, consumer) = creator();

    // Producer thread.
    //
    // Push all numbers from 0 to N, popping one from time to time.
    let t0 = spawn(move || {
        let mut i = 0;

        'outer: loop {
            for _ in 0..RAND.fetch_add(1, Ordering::Relaxed) % 1000 {
                while producer.maybe_push(i).is_err() {}

                i += 1;

                if i == N {
                    break 'outer;
                }

                yield_now();
            }
        }
    });

    // Stealer threads.
    //
    // Repeatedly steal a random number of items.
    let steal_periodically = move |consumer: Consumer| -> (Vec<usize>, Consumer) {
        let counter = AtomicUsize::new(0);
        let mut stats = vec![0; N];
        let (dst_producer, dst_consumer) = creator();

        loop {
            consumer.steal_into(&dst_producer);

            while let Some(i) = dst_consumer.pop() {
                stats[i] += 1;
                counter.fetch_add(1, Ordering::Relaxed);
            }

            let count = counter.load(Ordering::Relaxed);

            if count == N || !cfg!(feature = "always_steal") && count > CHECK_TO {
                break;
            }

            assert!(count < N);
        }

        (stats, consumer)
    };

    let t1 = spawn(move || steal_periodically(consumer));
    let (mut stats, consumer) = t1.join().unwrap();

    t0.join().unwrap();

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
                stats[unsafe { slice[i].assume_init() }] += 1;
                delta -= 1;
            }
        }
    }

    for (i, item) in stats.iter().enumerate().take(check_to) {
        assert_eq!(*item, 1, "stats[{}] = {item}", i);
    }
}

fn test_spsc_multi_threaded_pop_many<Producer, Consumer>(creator: fn() -> (Producer, Consumer))
where
    Producer: SingleProducer<usize> + Send + 'static,
    Consumer: ConsumerExt<usize> + Send + 'static,
{
    const N: usize = 1_000_000;
    const BATCH_SIZE: usize = 5;
    const RES: usize = (N - 1) * N / 2;

    let consumer_pop_many = |consumer: &Consumer, count: &AtomicUsize| {
        let mut slice = [const { MaybeUninit::uninit() }; BATCH_SIZE];
        let backoff = Backoff::new();

        for _ in 0..N {
            let popped = consumer.pop_many(&mut slice);

            for item in slice.iter().take(popped) {
                let res = count.fetch_add(unsafe { item.assume_init() }, Ordering::Relaxed);

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

    let (producer, consumer) = creator();
    let count = Arc::new(AtomicUsize::new(0));
    let count1 = count.clone();

    let t0 = spawn(move || consumer_pop_many(&consumer, &count1));

    let mut slice = [0; BATCH_SIZE];
    for i in 0..N / BATCH_SIZE {
        for (j, item) in slice.iter_mut().enumerate().take(BATCH_SIZE) {
            *item = i * BATCH_SIZE + j;
        }

        let backoff = Backoff::new();

        while unsafe { producer.maybe_push_many(&slice[..BATCH_SIZE]).is_err() } {
            backoff.snooze();
        }
    }

    t0.join().unwrap();

    assert_eq!(count.load(Ordering::Relaxed), RES);
}

#[test]
fn test_bounded_spsc_multi_threaded_steal() {
    let test_guard = TEST_LOCK.lock();

    test_spsc_multi_threaded_steal(new_bounded::<usize, 256>);

    println!("Non cache padded done, start cache padded");

    test_spsc_multi_threaded_steal(new_cache_padded_bounded::<usize, 256>);

    drop(test_guard);
}

#[test]
fn test_unbounded_spsc_multi_threaded_steal() {
    let test_guard = TEST_LOCK.lock();

    test_spsc_multi_threaded_steal(|| {
        let queue = new_unbounded();

        queue.0.reserve(128);

        queue
    });

    println!("Non cache padded done, start cache padded");

    test_spsc_multi_threaded_steal(|| {
        let queue = new_cache_padded_unbounded();

        queue.0.reserve(128);

        queue
    });

    drop(test_guard);
}

#[test]
fn test_bounded_spsc_multi_threaded_pop_many() {
    let test_guard = TEST_LOCK.lock();

    test_spsc_multi_threaded_pop_many(new_bounded::<usize, 256>);

    println!("Non cache padded done, start cache padded");

    test_spsc_multi_threaded_pop_many(new_cache_padded_bounded::<usize, 256>);

    drop(test_guard);
}

#[test]
fn test_unbounded_spsc_multi_threaded_pop_many() {
    let test_guard = TEST_LOCK.lock();

    test_spsc_multi_threaded_pop_many(new_unbounded);

    println!("Non cache padded done, start cache padded");

    test_spsc_multi_threaded_pop_many(new_cache_padded_unbounded);

    drop(test_guard);
}
