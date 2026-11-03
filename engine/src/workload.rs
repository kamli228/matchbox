//! Deterministic synthetic order flow for benchmarks, demos and bots.

use crate::types::{Command, OrderId, OrderType, Price, Qty, Side};

/// Tiny deterministic pseudo-random generator (xorshift64).
/// Same seed → same sequence, so benchmark runs are comparable.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform-ish number in `0..n`.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

/// Generate `n` commands around a mid price: ~60% limit orders
/// (some of them crossing), ~15% market orders, ~25% cancels.
pub fn generate(n: usize, seed: u64) -> Vec<Command> {
    const MID: u64 = 10_000;
    let mut rng = Rng::new(seed);
    let mut cmds = Vec::with_capacity(n);
    let mut next_id = 1;

    for _ in 0..n {
        let roll = rng.below(100);
        let side = if rng.below(2) == 0 {
            Side::Buy
        } else {
            Side::Sell
        };
        let qty = Qty(1 + rng.below(10));

        let cmd = if roll < 60 {
            // buys mostly below mid, sells mostly above; overlap makes some cross
            let offset = rng.below(21);
            let price = match side {
                Side::Buy => MID - 15 + offset,
                Side::Sell => MID - 5 + offset,
            };
            Command::NewOrder {
                id: OrderId(next_id),
                side,
                order_type: OrderType::Limit {
                    price: Price(price),
                },
                qty,
            }
        } else if roll < 75 {
            Command::NewOrder {
                id: OrderId(next_id),
                side,
                order_type: OrderType::Market,
                qty,
            }
        } else {
            // cancel one of the recent orders (may already be filled → rejected)
            let back = 1 + rng.below(next_id.min(1_000));
            Command::Cancel {
                id: OrderId(next_id.saturating_sub(back).max(1)),
            }
        };

        if matches!(cmd, Command::NewOrder { .. }) {
            next_id += 1;
        }
        cmds.push(cmd);
    }
    cmds
}
