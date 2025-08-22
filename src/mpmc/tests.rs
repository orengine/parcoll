use orengine_utils::backoff::Backoff;
use crate::mpmc::{new_bounded, new_cache_padded_bounded};
use crate::multi_consumer::MultiConsumer;
use crate::multi_producer::MultiProducer;
use crate::test_lock::TEST_LOCK;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::spawn;

fn test_mpmc_pop_push<Producer, Consumer>(creator: fn() -> (Producer, Consumer))
where
    Producer: MultiProducer<usize> + Send + 'static,
    Consumer: MultiConsumer<usize> + Send + 'static,
{
    const N: usize = 500_000;
    const PAR_MULTIPLIER: usize = 3;

    let (producer, consumer) = creator();
    let received = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();

    for _ in 0..PAR_MULTIPLIER {
        let producer = producer.clone();

        handles.push(spawn(move || {
            for i in 1..=N {
                let backoff = Backoff::new();

                while producer.maybe_push(i).is_err() {
                    backoff.snooze();
                }
            }
        }));

        let consumer = consumer.clone();
        let received = received.clone();

        handles.push(spawn(move || {
            for _ in 0..N {
                let backoff = Backoff::new();

                loop {
                    if let Some(value) = consumer.pop() {
                        received.fetch_add(value, Ordering::Relaxed);

                        break;
                    }

                    backoff.snooze();
                }
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    assert_eq!(
        N * PAR_MULTIPLIER * (1 + N) / 2,
        received.load(Ordering::Relaxed)
    );
}

#[test]
fn test_bounded_mpmc_multi_threaded_pop_many() {
    let test_guard = TEST_LOCK.lock();

    test_mpmc_pop_push(new_bounded::<_, 2048>);

    println!("Non cache padded done, start cache padded");

    test_mpmc_pop_push(new_cache_padded_bounded::<_, 2048>);

    drop(test_guard);
}
