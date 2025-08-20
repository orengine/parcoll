use crate::generic_spmc_queue::{
    CrossbeamFifoWorker, CrossbeamLifoWorker, GenericStealer, GenericWorker,
};
use criterion::{criterion_group, criterion_main, Criterion};
use parcoll::{spsc, LightArc};
use std::sync::atomic::AtomicU64;
use std::time::Instant;
use parcoll::single_consumer::SingleConsumer;
use parcoll::single_producer::SingleProducer;

pub(crate) mod generic_spmc_queue;

// Single-threaded benchmark.
//
// `N` items are pushed and then popped from the queue.
pub fn push_pop<W: GenericWorker<usize>, const N: usize>(name: &str, c: &mut Criterion) {
    let worker = W::new();

    c.bench_function(&format!("push_pop-{}", name), |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();

            for _ in 0..iters {
                for i in 0..N {
                    let _ = worker.push(i);
                }

                for _ in 0..N {
                    let _ = worker.pop();
                }
            }

            start.elapsed() / N as _
        })
    });
}

pub fn push_pop_spsc<P, C, Creator, const N: usize>(name: &str, creator: Creator, c: &mut Criterion)
where
    P: SingleProducer<usize>,
    C: SingleConsumer<usize>,
    Creator: Fn() -> (P, C),
{
    let (producer, consumer) = creator();

    c.bench_function(&format!("push_pop-{}", name), |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();

            for _ in 0..iters {
                for i in 0..N {
                    let _ = producer.maybe_push(i);
                }

                for _ in 0..N {
                    let _ = consumer.pop();
                }
            }

            start.elapsed() / N as _
        })
    });
}

// region push_pop small

pub fn push_pop_small_st3_fifo(c: &mut Criterion) {
    push_pop::<st3::fifo::Worker<_>, 8>("small-st3_fifo", c);
}

pub fn push_pop_small_st3_lifo(c: &mut Criterion) {
    push_pop::<st3::lifo::Worker<_>, 8>("small-st3_lifo", c);
}

pub fn push_pop_small_crossbeam_fifo(c: &mut Criterion) {
    push_pop::<CrossbeamFifoWorker<_>, 8>("small-crossbeam_fifo", c);
}

pub fn push_pop_small_crossbeam_lifo(c: &mut Criterion) {
    push_pop::<CrossbeamLifoWorker<_>, 8>("small-crossbeam_lifo", c);
}

pub fn push_pop_small_crossbeam_array_queue(c: &mut Criterion) {
    push_pop::<LightArc<crossbeam_queue::ArrayQueue<_>>, 8>("small-crossbeam_array_queue", c);
}

pub fn push_pop_small_crossbeam_seg_queue(c: &mut Criterion) {
    push_pop::<LightArc<crossbeam_queue::SegQueue<_>>, 8>("small-crossbeam_seg_queue", c);
}

pub fn push_pop_small_parcoll_spmc_bounded(c: &mut Criterion) {
    push_pop::<parcoll::spmc::CachePaddedSPMCProducer<_, 256>, 8>("small-parcoll_spmc_bounded", c);
}

pub fn push_pop_small_parcoll_spmc_unbounded(c: &mut Criterion) {
    push_pop::<parcoll::spmc::CachePaddedSPMCUnboundedProducer<_>, 8>(
        "small-parcoll_spmc_unbounded",
        c,
    );
}

pub fn push_pop_small_parcoll_spsc_bounded(c: &mut Criterion) {
    push_pop_spsc::<_, _, _, 8>(
        "small-parcoll_spsc_bounded",
        || spsc::new_bounded::<_, 256>(),
        c,
    );
}

pub fn push_pop_small_parcoll_spsc_unbounded(c: &mut Criterion) {
    push_pop_spsc::<_, _, _, 8>(
        "small-parcoll_spsc_unbounded",
        || spsc::new_unbounded::<_>(),
        c,
    );
}

// endregion

// region push_pop large

pub fn push_pop_large_st3_fifo(c: &mut Criterion) {
    push_pop::<st3::fifo::Worker<_>, 256>("large-st3_fifo", c);
}

pub fn push_pop_large_st3_lifo(c: &mut Criterion) {
    push_pop::<st3::lifo::Worker<_>, 256>("large-st3_lifo", c);
}

pub fn push_pop_large_crossbeam_fifo(c: &mut Criterion) {
    push_pop::<CrossbeamFifoWorker<_>, 256>("large-crossbeam_fifo", c);
}

pub fn push_pop_large_crossbeam_lifo(c: &mut Criterion) {
    push_pop::<CrossbeamLifoWorker<_>, 256>("large-crossbeam_lifo", c);
}

pub fn push_pop_large_crossbeam_array_queue(c: &mut Criterion) {
    push_pop::<LightArc<crossbeam_queue::ArrayQueue<_>>, 256>("large-crossbeam_array_queue", c);
}

pub fn push_pop_large_crossbeam_seg_queue(c: &mut Criterion) {
    push_pop::<LightArc<crossbeam_queue::SegQueue<_>>, 256>("large-crossbeam_seg_queue", c);
}

pub fn push_pop_large_parcoll_spmc_bounded(c: &mut Criterion) {
    push_pop::<parcoll::spmc::CachePaddedSPMCProducer<_, 256>, 256>(
        "large-parcoll_spmc_bounded",
        c,
    );
}

pub fn push_pop_large_parcoll_spmc_unbounded(c: &mut Criterion) {
    push_pop::<parcoll::spmc::CachePaddedSPMCUnboundedProducer<_>, 256>(
        "large-parcoll_spmc_unbounded",
        c,
    );
}

pub fn push_pop_large_parcoll_spsc_bounded(c: &mut Criterion) {
    push_pop_spsc::<_, _, _, 256>(
        "large-parcoll_spsc_bounded",
        || spsc::new_bounded::<_, 256>(),
        c,
    );
}

pub fn push_pop_large_parcoll_spsc_unbounded(c: &mut Criterion) {
    push_pop_spsc::<_, _, _, 256>(
        "large-parcoll_spsc_unbounded",
        || spsc::new_unbounded::<_>(),
        c,
    );
}

// endregion

static NEXT_SEED: AtomicU64 = AtomicU64::new(0);

pub fn push_pop_steal<T, W: GenericWorker<u64> + 'static>(name: &str, c: &mut Criterion) {
    const NUMBER_OF_ITEMS: u64 = 256;

    let src_worker = W::new();
    let src_stealer = src_worker.stealer();
    let dst_worker = W::new();

    c.bench_function(name, |b| {
        b.iter(|| {
            for i in 0..NUMBER_OF_ITEMS {
                let _ = src_worker.push(i);
            }

            let _ = src_stealer.steal_batch(&dst_worker);

            while dst_worker.pop().is_some() {}
            while src_worker.pop().is_some() {}
        })
    });
}

pub fn push_pop_steal_st3_fifo(c: &mut Criterion) {
    push_pop_steal::<u64, st3::fifo::Worker<_>>("st3_fifo batch", c);
}

pub fn push_pop_steal_st3_lifo(c: &mut Criterion) {
    push_pop_steal::<u64, st3::lifo::Worker<_>>("st3_lifo batch", c);
}

pub fn push_pop_steal_crossbeam_fifo(c: &mut Criterion) {
    push_pop_steal::<u64, CrossbeamFifoWorker<_>>("crossbeam_fifo batch", c);
}

pub fn push_pop_steal_crossbeam_lifo(c: &mut Criterion) {
    push_pop_steal::<u64, CrossbeamLifoWorker<_>>("crossbeam_lifo batch", c);
}

pub fn push_pop_steal_crossbeam_array_queue(c: &mut Criterion) {
    push_pop_steal::<u64, LightArc<crossbeam_queue::ArrayQueue<_>>>(
        "crossbeam_array_queue batch",
        c,
    );
}

pub fn push_pop_steal_crossbeam_seg_queue(c: &mut Criterion) {
    push_pop_steal::<u64, LightArc<crossbeam_queue::SegQueue<_>>>("crossbeam_seg_queue batch", c);
}

pub fn push_pop_steal_parcoll_spmc_bounded(c: &mut Criterion) {
    push_pop_steal::<u64, parcoll::spmc::CachePaddedSPMCProducer<_, 256>>(
        "parcoll_spmc_bounded batch",
        c,
    );
}

pub fn push_pop_steal_parcoll_spmc_unbounded(c: &mut Criterion) {
    push_pop_steal::<u64, parcoll::spmc::CachePaddedSPMCUnboundedProducer<_>>(
        "parcoll_spmc_unbounded batch",
        c,
    );
}

criterion_group!(
    push_pop_benchmark,
    push_pop_small_st3_fifo,
    push_pop_small_st3_lifo,
    push_pop_small_crossbeam_fifo,
    push_pop_small_crossbeam_lifo,
    push_pop_small_crossbeam_array_queue,
    push_pop_small_crossbeam_seg_queue,
    push_pop_small_parcoll_spmc_bounded,
    push_pop_small_parcoll_spmc_unbounded,
    push_pop_small_parcoll_spsc_bounded,
    push_pop_small_parcoll_spsc_unbounded,
    push_pop_large_st3_fifo,
    push_pop_large_st3_lifo,
    push_pop_large_crossbeam_fifo,
    push_pop_large_crossbeam_lifo,
    push_pop_large_crossbeam_array_queue,
    push_pop_large_crossbeam_seg_queue,
    push_pop_large_parcoll_spmc_bounded,
    push_pop_large_parcoll_spmc_unbounded,
    push_pop_large_parcoll_spsc_bounded,
    push_pop_large_parcoll_spsc_unbounded
);

criterion_group!(
    push_pop_steal_benchmark,
    push_pop_steal_st3_fifo,
    push_pop_steal_st3_lifo,
    push_pop_steal_crossbeam_fifo,
    push_pop_steal_crossbeam_lifo,
    push_pop_steal_crossbeam_array_queue,
    push_pop_steal_crossbeam_seg_queue,
    push_pop_steal_parcoll_spmc_bounded,
    push_pop_steal_parcoll_spmc_unbounded
);

criterion_main!(push_pop_benchmark, push_pop_steal_benchmark);
