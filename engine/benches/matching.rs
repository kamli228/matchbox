//! Throughput benchmark: how many commands per second the engine processes.
//!
//! Run with: `cargo bench -p engine`

use std::hint::black_box;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use engine::book::OrderBook;
use engine::workload;

const N: usize = 100_000;

fn bench_matching(c: &mut Criterion) {
    let cmds = workload::generate(N, 42);

    let mut group = c.benchmark_group("engine");
    group.throughput(Throughput::Elements(N as u64));
    group.bench_function("process_100k_mixed", |b| {
        b.iter_batched(
            || (OrderBook::new(), cmds.clone()),
            |(mut book, cmds)| {
                for cmd in cmds {
                    black_box(book.process(cmd));
                }
                book
            },
            BatchSize::LargeInput,
        )
    });
    group.finish();
}

criterion_group!(benches, bench_matching);
criterion_main!(benches);
