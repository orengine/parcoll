use crate::backoff::Backoff;
use crate::multi_consumer::MultiConsumer;
use crate::single_producer::SingleProducer;
use crate::spmc::{
    new_bounded, new_cache_padded_bounded, new_cache_padded_unbounded, new_unbounded,
};
use crate::spmc_producer::SPMCProducer;
use crate::test_lock::TEST_LOCK;
use crate::{Consumer as ConsumerExt, Producer as ProducerExt};
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::spawn;

static RAND: AtomicUsize = AtomicUsize::new(4);

fn test_spmc_multi_threaded_steal<Producer, Consumer>(creator: fn() -> (Producer, Consumer))
where
    Producer: ProducerExt<usize> + Send + 'static + SPMCProducer<usize>,
    Consumer: ConsumerExt<usize> + MultiConsumer<usize> + Send + 'static,
{
    const N: usize = 1_000_000;
    const CHECK_TO: usize = N - 10;

    let counter = Arc::new(AtomicUsize::new(0));
    let (producer, consumer) = creator();

    let counter0 = counter.clone();
    let consumer1 = consumer.clone();
    let counter1 = counter.clone();
    let consumer2 = consumer.clone();
    let counter2 = counter;

    // Producer thread.
    //
    // Push all numbers from 0 to N, popping one from time to time.
    let t0 = spawn(move || {
        let mut i = 0;
        let mut stats = vec![0; N];

        'outer: loop {
            for _ in 0..RAND.fetch_add(1, Ordering::Relaxed) % 10 {
                let backoff = Backoff::new();

                while producer.maybe_push(i).is_err() {
                    backoff.snooze();
                }

                i += 1;

                if i == N {
                    break 'outer;
                }
            }

            if let Some(j) = producer.pop() {
                stats[j] += 1;

                counter0.fetch_add(1, Ordering::Relaxed);
            }
        }

        stats
    });

    // Stealer threads.
    //
    // Repeatedly steal a random number of items.
    let steal_periodically = move |consumer: &Consumer, counter: Arc<AtomicUsize>| -> Vec<usize> {
        let mut stats = vec![0; N];
        let (dest_producer, _) = creator();
        let backoff = Backoff::new();
        let mut backoff_steps = 0;

        loop {
            consumer.steal_into(&dest_producer);

            while let Some(i) = dest_producer.pop() {
                stats[i] += 1;
                counter.fetch_add(1, Ordering::Relaxed);
            }

            backoff.snooze();

            if backoff_steps == 7 {
                backoff.reset();

                backoff_steps = 0;
            } else {
                backoff_steps += 1;
            }

            let count = counter.load(Ordering::Relaxed);

            if count > CHECK_TO {
                break;
            }

            assert!(count < N);
        }

        stats
    };

    let t1 = spawn(move || steal_periodically(&consumer1, counter1));
    let t2 = spawn(move || steal_periodically(&consumer2, counter2));
    let mut stats = vec![t0.join().unwrap(), t1.join().unwrap(), t2.join().unwrap()];

    if consumer.len() + CHECK_TO > N {
        let mut delta = consumer.len() + CHECK_TO - N;
        assert!(delta < 100);

        let mut slice = [const { MaybeUninit::uninit() }; 100];

        while delta > 0 {
            let popped = consumer.pop_many(slice.as_mut_slice());
            for item in slice.iter().take(popped) {
                stats[0][unsafe { item.assume_init() }] += 1;
                delta -= 1;
            }
        }
    }

    for i in 0..CHECK_TO {
        let mut count = 0;

        for item in &stats {
            count += item[i];
        }

        assert_eq!(count, 1, "stats[{i}] = {count}");
    }
}

fn test_spmc_multi_threaded_pop_many<Producer, Consumer>(creator: fn() -> (Producer, Consumer))
where
    Producer: SingleProducer<usize> + Send + 'static,
    Consumer: ConsumerExt<usize> + MultiConsumer<usize> + Send + 'static,
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
    let consumer1 = consumer.clone();
    let consumer2 = consumer;
    let count = Arc::new(AtomicUsize::new(0));
    let count1 = count.clone();
    let count2 = count.clone();

    let t0 = spawn(move || consumer_pop_many(&consumer1, &count1));
    let t1 = spawn(move || consumer_pop_many(&consumer2, &count2));

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
    t1.join().unwrap();

    assert_eq!(count.load(Ordering::Relaxed), RES);
}

#[test]
fn test_bounded_spmc_multi_threaded_steal() {
    let test_guard = TEST_LOCK.lock();

    test_spmc_multi_threaded_steal(new_bounded::<usize, 256>);

    println!("Non cache padded done, start cache padded");

    test_spmc_multi_threaded_steal(new_cache_padded_bounded::<usize, 256>);

    drop(test_guard);
}

#[test]
fn test_unbounded_spmc_multi_threaded_steal() {
    let test_guard = TEST_LOCK.lock();

    test_spmc_multi_threaded_steal(|| {
        let queue = new_unbounded();

        queue.0.reserve(128);

        queue
    });

    println!("Non cache padded done, start cache padded");

    test_spmc_multi_threaded_steal(|| {
        let queue = new_cache_padded_unbounded();

        queue.0.reserve(128);

        queue
    });

    drop(test_guard);
}

#[test]
fn test_bounded_spmc_multi_threaded_pop_many() {
    let test_guard = TEST_LOCK.lock();

    test_spmc_multi_threaded_pop_many(new_bounded::<usize, 256>);

    println!("Non cache padded done, start cache padded");

    test_spmc_multi_threaded_pop_many(new_cache_padded_bounded::<usize, 256>);

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
