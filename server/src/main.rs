//! Matchbox server: runs the engine in one task and exposes it over WebSocket.
//!
//! Run from the workspace root: `cargo run -p server`, then open http://localhost:3000

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use engine::book::OrderBook;
use engine::journal::{self, Journal};
use engine::types::{Command, Event, OrderId, OrderType, Price, Qty, Side};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, watch};
use tower_http::services::ServeDir;

/// How many price levels per side are sent to clients.
const DEPTH: usize = 20;
/// File where every accepted command is appended.
const JOURNAL_PATH: &str = "matchbox.journal";

/// What a client may send us (JSON). The server assigns order ids itself.
#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum ClientMsg {
    Limit { side: Side, price: u64, qty: u64 },
    Market { side: Side, qty: u64 },
    Cancel { id: u64 },
}

/// Top of the book, sent to clients whenever it changes.
#[derive(Serialize, Clone, Default, Debug)]
struct BookSnapshot {
    bids: Vec<(Price, Qty)>,
    asks: Vec<(Price, Qty)>,
}

/// Shared handles every client connection gets a copy of.
#[derive(Clone)]
struct AppState {
    /// Commands from all clients → the single engine task.
    cmd_tx: mpsc::Sender<Command>,
    /// Engine events → every client.
    events_tx: broadcast::Sender<Event>,
    /// Latest book snapshot (only the newest value is kept).
    book_rx: watch::Receiver<BookSnapshot>,
    /// Source of unique order ids.
    next_id: Arc<AtomicU64>,
}

#[tokio::main]
async fn main() {
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(10_000);
    let (events_tx, _) = broadcast::channel::<Event>(10_000);
    let (book_tx, book_rx) = watch::channel(BookSnapshot::default());

    // restore the book from the journal before accepting new commands
    let history = journal::replay(JOURNAL_PATH).expect("journal is readable");
    let next_id = max_order_id(&history) + 1;
    println!("replayed {} commands from {JOURNAL_PATH}", history.len());
    let journal = Journal::open(JOURNAL_PATH).expect("journal can be opened");

    tokio::spawn(run_engine(
        history,
        journal,
        cmd_rx,
        events_tx.clone(),
        book_tx,
    ));

    let state = AppState {
        cmd_tx,
        events_tx,
        book_rx,
        next_id: Arc::new(AtomicU64::new(next_id)),
    };

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .fallback_service(ServeDir::new("web"))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("port 3000 is free");
    println!("Matchbox listening on http://localhost:3000");
    axum::serve(listener, app).await.expect("server runs");
}

/// The only place that touches the order book: one command at a time.
async fn run_engine(
    history: Vec<Command>,
    mut journal: Journal,
    mut cmd_rx: mpsc::Receiver<Command>,
    events_tx: broadcast::Sender<Event>,
    book_tx: watch::Sender<BookSnapshot>,
) {
    let mut book = OrderBook::new();
    for cmd in history {
        book.process(cmd);
    }
    book_tx.send_replace(BookSnapshot {
        bids: book.depth(Side::Buy, DEPTH),
        asks: book.depth(Side::Sell, DEPTH),
    });

    while let Some(cmd) = cmd_rx.recv().await {
        // write first, then apply: a crash can never lose a command that changed the book
        if let Err(err) = journal.append(&cmd) {
            eprintln!("journal write failed: {err}");
        }
        for event in book.process(cmd) {
            // an error only means nobody is connected right now
            let _ = events_tx.send(event);
        }
        book_tx.send_replace(BookSnapshot {
            bids: book.depth(Side::Buy, DEPTH),
            asks: book.depth(Side::Sell, DEPTH),
        });
    }
}

/// Highest order id in the journal, so new ids continue after it.
fn max_order_id(history: &[Command]) -> u64 {
    history
        .iter()
        .filter_map(|cmd| match cmd {
            Command::NewOrder { id, .. } => Some(id.0),
            Command::Cancel { .. } => None,
        })
        .max()
        .unwrap_or(0)
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_client(socket, state))
}

/// One task per connected client: forwards its orders to the engine
/// and pushes events and book snapshots back to it.
async fn handle_client(mut socket: WebSocket, state: AppState) {
    let mut events_rx = state.events_tx.subscribe();
    let mut book_rx = state.book_rx.clone();

    // send the current book right away
    let snapshot = book_rx.borrow_and_update().clone();
    if send_json(&mut socket, &tagged("Snapshot", &snapshot))
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            // 1. a message from this client
            incoming = socket.recv() => {
                let Some(Ok(msg)) = incoming else { break };
                let Message::Text(text) = msg else { continue };
                match serde_json::from_str::<ClientMsg>(&text) {
                    Ok(client_msg) => {
                        let (cmd, assigned) = to_command(client_msg, &state.next_id);
                        if let Some(id) = assigned {
                            let reply = serde_json::json!({ "type": "Assigned", "id": id });
                            if send_json(&mut socket, &reply).await.is_err() { break; }
                        }
                        if state.cmd_tx.send(cmd).await.is_err() { break; }
                    }
                    Err(err) => {
                        let reply = serde_json::json!({ "type": "Error", "message": err.to_string() });
                        if send_json(&mut socket, &reply).await.is_err() { break; }
                    }
                }
            }
            // 2. an event from the engine (book levels come via snapshots instead)
            event = events_rx.recv() => {
                match event {
                    Ok(Event::BookUpdate { .. }) => {}
                    Ok(event) => {
                        if send_json(&mut socket, &event).await.is_err() { break; }
                    }
                    // we were too slow and missed some events; snapshots resync the book
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            // 3. the book changed
            changed = book_rx.changed() => {
                if changed.is_err() { break; }
                let snapshot = book_rx.borrow_and_update().clone();
                if send_json(&mut socket, &tagged("Snapshot", &snapshot)).await.is_err() { break; }
            }
        }
    }
}

/// Turn a client message into an engine command, assigning a fresh id to new orders.
fn to_command(msg: ClientMsg, next_id: &AtomicU64) -> (Command, Option<u64>) {
    let new_id = || next_id.fetch_add(1, Ordering::Relaxed);
    match msg {
        ClientMsg::Limit { side, price, qty } => {
            let id = new_id();
            let cmd = Command::NewOrder {
                id: OrderId(id),
                side,
                order_type: OrderType::Limit {
                    price: Price(price),
                },
                qty: Qty(qty),
            };
            (cmd, Some(id))
        }
        ClientMsg::Market { side, qty } => {
            let id = new_id();
            let cmd = Command::NewOrder {
                id: OrderId(id),
                side,
                order_type: OrderType::Market,
                qty: Qty(qty),
            };
            (cmd, Some(id))
        }
        ClientMsg::Cancel { id } => (Command::Cancel { id: OrderId(id) }, None),
    }
}

/// Wrap a value into `{"type": <kind>, ...fields}`.
fn tagged<T: Serialize>(kind: &str, value: &T) -> serde_json::Value {
    let mut json = serde_json::to_value(value).expect("serializable");
    if let serde_json::Value::Object(map) = &mut json {
        map.insert("type".into(), kind.into());
    }
    json
}

async fn send_json<T: Serialize>(socket: &mut WebSocket, value: &T) -> Result<(), axum::Error> {
    let text = serde_json::to_string(value).expect("serializable");
    socket.send(Message::Text(text.into())).await
}
