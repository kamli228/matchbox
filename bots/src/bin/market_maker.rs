//! Market maker bot: keeps two-sided quotes on several levels around the mid price.
//!
//! Every refresh it cancels its old quotes and places new ones, so the book always
//! has liquidity near the current price.
//!
//! Run (with the server already running): `cargo run -p bots --release --bin market_maker`
//! Set MATCHBOX_URL to connect elsewhere.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// Quote levels per side.
const LEVELS: u64 = 5;
/// Distance in ticks from the mid to the first quote on each side.
const HALF_SPREAD: u64 = 2;
/// How often quotes are refreshed.
const REFRESH: Duration = Duration::from_millis(700);

#[tokio::main]
async fn main() {
    let url = std::env::var("MATCHBOX_URL").unwrap_or_else(|_| "ws://localhost:3000/ws".into());
    let (ws, _) = connect_async(url.as_str())
        .await
        .expect("server is running");
    let (mut write, mut read) = ws.split();
    println!("market maker connected, quoting {LEVELS} levels per side");

    let mut mid: u64 = 10_000;
    let mut live: Vec<u64> = Vec::new(); // ids of our resting quotes
    let mut refresh = tokio::time::interval(REFRESH);
    let mut quotes_sent = 0u64;
    let mut fills = 0u64;

    loop {
        tokio::select! {
            _ = refresh.tick() => {
                // 1. pull the old quotes
                for id in live.drain(..) {
                    let msg = json!({ "type": "Cancel", "id": id });
                    if write.send(Message::Text(msg.to_string().into())).await.is_err() { return; }
                }
                // 2. quote fresh levels around the current mid
                for i in 0..LEVELS {
                    let qty = 10 + 5 * i;
                    let bid = mid.saturating_sub(HALF_SPREAD + i).max(1);
                    let ask = mid + HALF_SPREAD + i;
                    for (side, price) in [("Buy", bid), ("Sell", ask)] {
                        let msg = json!({ "type": "Limit", "side": side, "price": price, "qty": qty });
                        if write.send(Message::Text(msg.to_string().into())).await.is_err() { return; }
                        quotes_sent += 1;
                    }
                }
                if quotes_sent.is_multiple_of(LEVELS * 2 * 10) {
                    println!("quotes sent {quotes_sent:>7} | fills {fills:>6} | mid {mid}");
                }
            }
            incoming = read.next() => {
                let Some(Ok(Message::Text(text))) = incoming else {
                    if incoming.is_none() { println!("server closed the connection"); return; }
                    continue;
                };
                let Ok(msg) = serde_json::from_str::<Value>(&text) else { continue };
                match msg["type"].as_str() {
                    // remember the id of every quote we placed
                    Some("Assigned") => {
                        if let Some(id) = msg["id"].as_u64() {
                            live.push(id);
                        }
                    }
                    // follow the market: mid = halfway between best bid and best ask
                    Some("Snapshot") => {
                        let best = |side: &str| msg[side][0][0].as_u64();
                        if let (Some(bid), Some(ask)) = (best("bids"), best("asks")) {
                            mid = (bid + ask) / 2;
                        }
                    }
                    Some("Trade")
                        if msg["maker_id"].as_u64().is_some_and(|id| live.contains(&id)) =>
                    {
                        fills += 1;
                    }
                    _ => {}
                }
            }
        }
    }
}
