//! Interactive demo: type orders, watch the book and the events.
//!
//! Run with: `cargo run -p engine --example cli`

use std::io::{self, BufRead, Write};

use engine::book::OrderBook;
use engine::types::{Command, Event, OrderId, OrderType, Price, Qty, Side};

const HELP: &str = "\
commands:
  buy <price> <qty>     limit buy
  sell <price> <qty>    limit sell
  mbuy <qty>            market buy
  msell <qty>           market sell
  cancel <id>           cancel a resting order
  book                  show the order book
  help                  show this help
  quit                  exit";

fn main() {
    let mut book = OrderBook::new();
    let mut next_id = 1;

    println!("Matchbox CLI. Type `help` for commands.");
    prompt();

    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let words: Vec<&str> = line.split_whitespace().collect();

        match words.as_slice() {
            [] => {}
            ["quit" | "exit"] => break,
            ["help"] => println!("{HELP}"),
            ["book"] => print_book(&book),
            ["cancel", id] => match id.parse() {
                Ok(id) => run(&mut book, Command::Cancel { id: OrderId(id) }),
                Err(_) => println!("bad id: {id}"),
            },
            [cmd @ ("buy" | "sell"), price, qty] => match (price.parse(), qty.parse()) {
                (Ok(price), Ok(qty)) => {
                    let cmd = Command::NewOrder {
                        id: OrderId(next_id),
                        side: parse_side(cmd),
                        order_type: OrderType::Limit {
                            price: Price(price),
                        },
                        qty: Qty(qty),
                    };
                    next_id += 1;
                    run(&mut book, cmd);
                }
                _ => println!("price and qty must be whole numbers"),
            },
            [cmd @ ("mbuy" | "msell"), qty] => match qty.parse() {
                Ok(qty) => {
                    let cmd = Command::NewOrder {
                        id: OrderId(next_id),
                        side: parse_side(&cmd[1..]),
                        order_type: OrderType::Market,
                        qty: Qty(qty),
                    };
                    next_id += 1;
                    run(&mut book, cmd);
                }
                Err(_) => println!("qty must be a whole number"),
            },
            _ => println!("unknown command, type `help`"),
        }
        prompt();
    }
}

fn parse_side(word: &str) -> Side {
    if word == "buy" { Side::Buy } else { Side::Sell }
}

fn run(book: &mut OrderBook, cmd: Command) {
    for event in book.process(cmd) {
        print_event(&event);
    }
    print_book(book);
}

fn print_event(event: &Event) {
    match event {
        Event::Accepted { id } => println!("  accepted   #{}", id.0),
        Event::Trade(t) => println!(
            "  TRADE      {} @ {}  (maker #{}, taker #{})",
            t.qty.0, t.price.0, t.maker_id.0, t.taker_id.0
        ),
        Event::Cancelled { id, remaining } => {
            println!("  cancelled  #{} ({} left unfilled)", id.0, remaining.0)
        }
        Event::Rejected { id, reason } => println!("  rejected   #{} ({:?})", id.0, reason),
        Event::BookUpdate { .. } => {}
    }
}

fn print_book(book: &OrderBook) {
    const LEVELS: usize = 5;
    println!();
    println!("        price      qty");
    for (price, qty) in book.depth(Side::Sell, LEVELS).iter().rev() {
        println!("  ASK {:>8} {:>8}", price.0, qty.0);
    }
    println!("  ---------------------");
    for (price, qty) in book.depth(Side::Buy, LEVELS) {
        println!("  BID {:>8} {:>8}", price.0, qty.0);
    }
    println!();
}

fn prompt() {
    print!("> ");
    let _ = io::stdout().flush();
}
