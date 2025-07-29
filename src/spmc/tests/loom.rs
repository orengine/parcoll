use parcoll::loom_bindings::thread;
use parcoll::spmc::{
    new_cache_padded_const_bounded, new_const_bounded, CachePaddedSPMCConsumer, Consumer, Producer,
};

#[test]
fn loom_basic_steal() {
    const LOOP_COUNT: usize = 20;
    const ITEM_COUNT_PER_LOOP: usize = 10_000;

    loom::model(|| {
        let (mut producer, consumer) = new_const_bounded();

        let th = thread::spawn(move || {
            let (mut dest_producer, _) = new_const_bounded();
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

#[test]
fn loom_multi_stealer() {
    const ITEM_COUNT: usize = 15_000;

    fn steal_half(consumer: CachePaddedSPMCConsumer<usize>) -> usize {
        let (mut dest_worker, _) = new_cache_padded_const_bounded();

        let _ = consumer.steal_into(&mut dest_worker);

        let mut n = 0;
        while dest_worker.pop().is_some() {
            n += 1;
        }

        n
    }

    loom::model(|| {
        let (mut producer, consumer) = new_cache_padded_const_bounded();
        let consumer1 = consumer.clone();
        let consumer2 = consumer.clone();

        let th1 = thread::spawn(move || steal_half(consumer1));
        let th2 = thread::spawn(move || steal_half(consumer2));

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

#[test]
fn loom_chained_steal() {
    loom::model(|| {
        let (mut producer1, consumer1) = new_const_bounded();
        let (mut producer2, consumer2) = new_const_bounded();

        for _ in 0..40 {
            producer1.maybe_push(42).unwrap();
            producer2.maybe_push(42).unwrap();
        }

        let th = thread::spawn(move || {
            let (mut dest_producer, _) = new_const_bounded();
            let _ = consumer1.steal_into(&mut dest_producer);

            while dest_producer.pop().is_some() {}
        });

        while producer1.pop().is_some() {}

        let _ = consumer2.steal_into(&mut producer1);

        th.join().unwrap();

        while producer1.pop().is_some() {}
        while producer2.pop().is_some() {}
    });
}

#[test]
#[cfg(feature = "always_steal")]
fn loom_push_and_steal() {
    fn steal_half(consumer: CachePaddedSPMCConsumer<usize>) -> usize {
        let (mut dest_producer, _) = new_cache_padded_const_bounded();

        consumer.steal_into(&mut dest_producer)
    }

    loom::model(|| {
        let (mut producer, consumer) = new_cache_padded_const_bounded();
        let consumer1 = consumer.clone();
        let consumer2 = consumer.clone();

        let th1 = thread::spawn(move || steal_half(consumer1));
        let th2 = thread::spawn(move || steal_half(consumer2));

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
