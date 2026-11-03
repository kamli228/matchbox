//! Load bot: connects to the server and streams random orders around a drifting mid price.
//!
//! Run (with the server already running): `cargo run -p bots --release -- 200`
//! The number is orders per second (default 100).

use std::collections::VecDeque;
use std::time::Duration;

use engine::workload::Rng;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const URL: &str = "ws://localhost:3000/ws";

#[tokio::main]
async fn main() {
    let rate: u64 = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(100)
        .max(1);

    let (ws, _) = connect_async(URL)
        .await
        .expect("server is running on localhost:3000");
    let (mut write, mut read) = ws.split();
    println!("bot connected, sending {rate} orders/s (Ctrl+C to stop)");

    let mut rng = Rng::new(rate);
    let mut mid: i64 = 10_000;
    let mut my_orders: VecDeque<u64> = VecDeque::new();
    let mut sent = 0u64;
    let mut trades = 0u64;

    let mut tick = tokio::time::interval(Duration::from_micros(1_000_000 / rate));
    let mut report = tokio::time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            // time to send the next order
            _ = tick.tick() => {
                // slow random walk so the price chart moves
                if rng.below(20) == 0 {
                    mid += rng.below(3) as i64 - 1;
                }
                let msg = next_order(&mut rng, mid, &mut my_orders);
                if write.send(Message::Text(msg.to_string().into())).await.is_err() {
                    println!("connection closed");
                    break;
                }
                sent += 1;
            }
            // something from the server
            incoming = read.next() => {
                let Some(Ok(Message::Text(text))) = incoming else {
                    if incoming.is_none() { println!("server closed the connection"); break; }
                    continue;
                };
                let Ok(msg) = serde_json::from_str::<Value>(&text) else { continue };
                match msg["type"].as_str() {
                    Some("Assigned") => {
                        if let Some(id) = msg["id"].as_u64() {
                            my_orders.push_back(id);
                            if my_orders.len() > 500 {
                                my_orders.pop_front();
                            }
                        }
                    }
                    Some("Trade") => trades += 1,
                    _ => {}
                }
            }
            // once a second: print stats
            _ = report.tick() => {
                println!("sent {sent:>8} orders | seen {trades:>8} trades | mid {mid}");
            }
        }
    }
}

/// Pick the next message: mostly limit orders near the mid, some market orders and cancels.
fn next_order(rng: &mut Rng, mid: i64, my_orders: &mut VecDeque<u64>) -> Value {
    let roll = rng.below(100);
    let side = if rng.below(2) == 0 { "Buy" } else { "Sell" };
    let qty = 1 + rng.below(10);

    if roll < 15 && !my_orders.is_empty() {
        // cancel one of our recent orders (it may already be filled)
        let index = rng.below(my_orders.len() as u64) as usize;
        let id = my_orders.remove(index).expect("index in range");
        json!({ "type": "Cancel", "id": id })
    } else if roll < 25 {
        json!({ "type": "Market", "side": side, "qty": qty })
    } else {
        // buys slightly below mid, sells slightly above; some overlap crosses
        let offset = rng.below(10) as i64;
        let price = match side {
            "Buy" => mid - 7 + offset,
            _ => mid - 2 + offset,
        };
        json!({ "type": "Limit", "side": side, "price": price.max(1), "qty": qty })
    }
}
