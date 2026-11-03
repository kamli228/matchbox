//! Per-command latency percentiles (p50 / p99 / p99.9).
//!
//! Run with: `cargo run -p engine --release --example latency`

use std::hint::black_box;
use std::time::Instant;

use engine::book::OrderBook;
use engine::workload;

const WARMUP: usize = 100_000;
const N: usize = 1_000_000;

fn main() {
    let cmds = workload::generate(WARMUP + N, 7);
    let mut book = OrderBook::new();
    let mut nanos = Vec::with_capacity(N);

    for (i, cmd) in cmds.into_iter().enumerate() {
        let start = Instant::now();
        black_box(book.process(cmd));
        let elapsed = start.elapsed().as_nanos() as u64;
        if i >= WARMUP {
            nanos.push(elapsed);
        }
    }

    nanos.sort_unstable();
    let pct = |p: f64| nanos[((nanos.len() as f64 * p) as usize).min(nanos.len() - 1)];
    let total: u64 = nanos.iter().sum();

    println!("commands measured: {N}");
    println!("orders in book at end: {}", book.order_count());
    println!("mean:  {:>6} ns", total / N as u64);
    println!("p50:   {:>6} ns", pct(0.50));
    println!("p99:   {:>6} ns", pct(0.99));
    println!("p99.9: {:>6} ns", pct(0.999));
    println!("max:   {:>6} ns", nanos[N - 1]);
    println!("(includes ~20-40 ns timer overhead per measurement)");
}
