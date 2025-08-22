#![cfg(all(parcoll_loom, test))]
use orengine_utils::backoff::Backoff;
use parcoll::multi_consumer::MultiConsumer;
use parcoll::multi_producer::MultiProducer;
use parcoll::single_producer::SingleProducer;
use parcoll::spmc::{
    new_bounded, new_cache_padded_bounded, new_cache_padded_unbounded, new_unbounded,
};
use parcoll::spmc_producer::SPMCProducer;
use parcoll::{mpmc, spsc, Consumer};
use std::thread::spawn;
use std::time::{Duration, Instant};

fn loom_spmc_basic_steal<P, C, Cr>(creator: Cr)
where
    P: SPMCProducer<usize> + 'static,
    C: Consumer<usize> + 'static,
    Cr: 'static + Sync + Send + Clone + Copy + Fn() -> (P, C),
{
    const LOOP_COUNT: usize = 20;
    const ITEM_COUNT_PER_LOOP: usize = 10_000;

    loom::model(move || {
        let (producer, consumer) = creator();

        let th = loom::thread::spawn(move || {
            let (mut dest_producer, _) = creator();
            let mut n = 0;

            for _ in 0..3 {
                let _ = consumer.steal_into(&mut dest_producer);

                while dest_producer.pop().is_some() {
                    n += 1;
                }
            }

            n
        });

        let mut n = 0;

        for _ in 0..LOOP_COUNT {
            for _ in 0..(ITEM_COUNT_PER_LOOP - 1) {
                if producer.maybe_push(42).is_err() {
                    n += 1;
                }
            }

            if producer.pop().is_some() {
                n += 1;
            }

            // Push another task
            if producer.maybe_push(42).is_err() {
                n += 1;
            }

            while producer.pop().is_some() {
                n += 1;
            }
        }

        n += th.join().unwrap();

        assert_eq!(ITEM_COUNT_PER_LOOP * LOOP_COUNT, n);
    });
}

fn loom_spmc_multi_stealer<P, C, Cr>(creator: Cr)
where
    P: SPMCProducer<usize> + 'static,
    C: MultiConsumer<usize> + 'static,
    Cr: 'static + Sync + Send + Clone + Copy + Fn() -> (P, C),
{
    const ITEM_COUNT: usize = 15_000;

    let steal_half = move |consumer: C| -> usize {
        let (mut dest_worker, _) = creator();

        let _ = consumer.steal_into(&mut dest_worker);

        let mut n = 0;
        while dest_worker.pop().is_some() {
            n += 1;
        }

        n
    };

    loom::model(move || {
        let (producer, consumer) = creator();
        let consumer1 = consumer.clone();
        let consumer2 = consumer.clone();

        let th1 = loom::thread::spawn(move || steal_half(consumer1));
        let th2 = loom::thread::spawn(move || steal_half(consumer2));

        let mut n = 0;
        for _ in 0..ITEM_COUNT {
            if producer.maybe_push(42).is_err() {
                n += 1;
            }
        }

        while producer.pop().is_some() {
            n += 1;
        }

        n += th1.join().unwrap();
        n += th2.join().unwrap();

        assert_eq!(ITEM_COUNT, n);
    });
}

fn loom_spmc_chained_steal<P, C, Cr>(creator: Cr)
where
    P: SPMCProducer<usize> + 'static,
    C: Consumer<usize> + 'static,
    Cr: 'static + Sync + Send + Clone + Copy + Fn() -> (P, C),
{
    loom::model(move || {
        let (producer1, consumer1) = creator();
        let (producer2, consumer2) = creator();

        for _ in 0..40 {
            producer1.maybe_push(42).unwrap();
            producer2.maybe_push(42).unwrap();
        }

        let th = loom::thread::spawn(move || {
            let (mut dest_producer, _) = creator();
            let _ = consumer1.steal_into(&mut dest_producer);

            while dest_producer.pop().is_some() {}
        });

        while producer1.pop().is_some() {}

        let _ = consumer2.steal_into(&producer1);

        th.join().unwrap();

        while producer1.pop().is_some() {}
        while producer2.pop().is_some() {}
    });
}

fn loom_spmc_push_and_steal<P, C, Cr>(creator: Cr)
where
    P: SPMCProducer<usize> + 'static,
    C: MultiConsumer<usize> + 'static,
    Cr: 'static + Sync + Send + Clone + Copy + Fn() -> (P, C),
{
    let steal_half = move |consumer: C| -> usize {
        let (mut dest_producer, _) = creator();

        consumer.steal_into(&mut dest_producer)
    };

    loom::model(move || {
        let (producer, consumer) = creator();
        let consumer1 = consumer.clone();
        let consumer2 = consumer.clone();

        let th1 = loom::thread::spawn(move || steal_half(consumer1));
        let th2 = loom::thread::spawn(move || steal_half(consumer2));

        producer.maybe_push(42).unwrap();
        producer.maybe_push(42).unwrap();

        let mut n = 0;
        while producer.pop().is_some() {
            n += 1;
        }

        n += th1.join().unwrap();
        n += th2.join().unwrap();

        assert_eq!(n, 2);
    });
}

fn loom_spsc_basic_steal<P, C, Cr>(creator: Cr)
where
    P: SingleProducer<usize> + 'static,
    C: Consumer<usize> + 'static,
    Cr: 'static + Sync + Send + Clone + Copy + Fn() -> (P, C),
{
    const LOOP_COUNT: usize = 20;
    const ITEM_COUNT_PER_LOOP: usize = 10_000;

    loom::model(move || {
        let (producer, consumer) = creator();
        let (producer2, consumer2) = creator();

        let mut n = 0;

        for _ in 0..LOOP_COUNT {
            for _ in 0..(ITEM_COUNT_PER_LOOP - 1) {
                if producer.maybe_push(42).is_err() {
                    n += 1;
                }
            }

            if consumer.pop().is_some() {
                n += 1;
            }

            // Push another task
            if producer.maybe_push(42).is_err() {
                n += 1;
            }

            consumer.steal_into(&producer2);

            while consumer.pop().is_some() {
                n += 1;
            }

            while consumer2.pop().is_some() {
                n += 1;
            }
        }

        assert_eq!(ITEM_COUNT_PER_LOOP * LOOP_COUNT, n);
    });
}

fn loom_mpmc_basic<P, C, Cr>(creator: Cr)
where
    P: MultiProducer<usize> + 'static + Send,
    C: MultiConsumer<usize> + 'static + Send,
    Cr: 'static + Sync + Send + Clone + Copy + Fn() -> (P, C),
{
    const ITEMS: usize = 10_000;
    const PAR_MULTIPLIER: usize = 3;

    loom::model(move || {
        let (producer, consumer) = creator();
        let mut producer_handles = Vec::new();
        let mut consumer_handles = Vec::new();

        for _ in 0..PAR_MULTIPLIER {
            let producer = producer.clone();

            producer_handles.push(spawn(move || {
                for _ in 0..ITEMS {
                    let start = Instant::now();
                    let backoff = Backoff::new();

                    while producer.maybe_push(42).is_err() {
                        if start.elapsed() >= Duration::from_secs(3) {
                            panic!("push is impossible")
                        }

                        backoff.snooze();
                    }
                }
            }));

            let consumer = consumer.clone();

            consumer_handles.push(spawn(move || {
                for _ in 0..ITEMS {
                    let start = Instant::now();
                    let backoff = Backoff::new();

                    loop {
                        if let Some(_) = consumer.pop() {
                            break;
                        }

                        if start.elapsed() >= Duration::from_secs(3) {
                            panic!("pop is impossible")
                        }

                        backoff.snooze();
                    }
                }
            }));
        }

        for handle in producer_handles {
            handle.join().unwrap();
        }

        for handle in consumer_handles {
            handle.join().unwrap();
        }
    });
}

#[test]
fn loom_spmc_bounded_multi_stealer() {
    loom_spmc_multi_stealer(new_bounded::<usize, 256>);

    println!("Non cache padded done, start cache padded");

    loom_spmc_multi_stealer(new_cache_padded_bounded::<usize, 256>);
}

#[test]
fn loom_spmc_bounded_chained_steal_chained_steal() {
    loom_spmc_chained_steal(new_bounded::<usize, 256>);

    println!("Non cache padded done, start cache padded");

    loom_spmc_chained_steal(new_cache_padded_bounded::<usize, 256>);
}

#[test]
fn loom_spmc_bounded_push_and_steal() {
    loom_spmc_push_and_steal(new_bounded::<usize, 256>);

    println!("Non cache padded done, start cache padded");

    loom_spmc_push_and_steal(new_cache_padded_bounded::<usize, 256>);
}

#[test]
fn loom_spmc_unbounded_basic_steal() {
    loom_spmc_basic_steal(new_unbounded);

    println!("Non cache padded done, start cache padded");

    loom_spmc_basic_steal(new_cache_padded_unbounded);
}

#[test]
fn loom_spmc_unbounded_multi_stealer() {
    loom_spmc_multi_stealer(new_unbounded);

    println!("Non cache padded done, start cache padded");

    loom_spmc_multi_stealer(new_cache_padded_unbounded);
}

#[test]
fn loom_spmc_unbounded_chained_steal_chained_steal() {
    loom_spmc_chained_steal(new_unbounded);

    println!("Non cache padded done, start cache padded");

    loom_spmc_chained_steal(new_cache_padded_unbounded);
}

#[test]
fn loom_spmc_unbounded_push_and_steal() {
    loom_spmc_push_and_steal(new_unbounded);

    println!("Non cache padded done, start cache padded");

    loom_spmc_push_and_steal(new_cache_padded_unbounded);
}

#[test]
fn loom_spsc_bounded_basic_steal() {
    loom_spsc_basic_steal(spsc::new_bounded::<usize, 256>);

    println!("Non cache padded done, start cache padded");

    loom_spsc_basic_steal(spsc::new_cache_padded_bounded::<usize, 256>);
}

#[test]
fn loom_spsc_unbounded_basic_steal() {
    loom_spsc_basic_steal(spsc::new_unbounded::<usize>);

    println!("Non cache padded done, start cache padded");

    loom_spsc_basic_steal(spsc::new_cache_padded_unbounded::<usize>);
}

#[test]
fn loom_mpmc_bounded_basic() {
    loom_mpmc_basic(mpmc::new_bounded::<usize, 256>);

    println!("Non cache padded done, start cache padded");

    loom_mpmc_basic(mpmc::new_cache_padded_bounded::<usize, 256>);
}
