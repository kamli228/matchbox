//! order book price levels per side and the matching logic

use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::types::{Command, Event, Order, OrderId, Price, Qty, Side};

/// all order at one price - in arrival order FIFO.
#[derive(Debug, Default)]
pub struct Level {
    pub orders: VecDeque<Order>,

    /// sum of qty of all orders on this lvl
    pub total: Qty,
}

/// limit order book for single trading pair (e.g. btc/eur)
#[derive(Debug, Default)]
pub struct OrderBook {
    bids: BTreeMap<Price, Level>,
    asks: BTreeMap<Price, Level>,
    index: HashMap<OrderId, (Side, Price)>,
}

impl OrderBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// highest buy price (if any)
    pub fn best_bid(&self) -> Option<Price> {
        self.bids.last_key_value().map(|(price, _level)| *price)
    }

    /// lowest sell price if any
    pub fn best_ask(&self) -> Option<Price> {
        self.asks.first_key_value().map(|(price, _level)| *price)
    }

    /// number of orders which are currently in the book
    pub fn order_count(&self) -> usize {
        self.index.len()
    }

    /// put an order into the book at its price (no matching)
    #[allow(dead_code)]
    fn rest(&mut self, order: Order) {
        let book_side = match order.side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };

        let level = book_side.entry(order.price).or_default();

        level.total.0 += order.qty.0;
        level.orders.push_back(order);
        self.index.insert(order.id, (order.side, order.price));
    }

    /// apply one command and return resulting events
    pub fn process(&mut self, cmd: Command) -> Vec<Event> {
        match cmd {
            Command::NewOrder { .. } => todo!("step 3"),
            Command::Cancel { .. } => todo!("step 5"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(id: u64, side: Side, price: u64, qty: u64) -> Order {
        Order {
            id: OrderId(id),
            side,
            price: Price(price),
            qty: Qty(qty),
        }
    }

    #[test]
    fn empty_book_has_no_best_prices() {
        let book = OrderBook::new();
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.best_ask(), None);
        assert_eq!(book.order_count(), 0);
    }

    #[test]
    fn best_prices_come_from_the_right_end() {
        let mut book = OrderBook::new();
        for p in [99, 100] {
            book.bids
                .entry(Price(p))
                .or_default()
                .orders
                .push_back(order(p, Side::Buy, p, 1));
        }
        for p in [101, 102] {
            book.asks
                .entry(Price(p))
                .or_default()
                .orders
                .push_back(order(p, Side::Sell, p, 1));
        }
        assert_eq!(book.best_bid(), Some(Price(100)));
        assert_eq!(book.best_ask(), Some(Price(101)));
    }

    #[test]
    fn rest_puts_orders_on_the_right_side() {
        let mut book = OrderBook::new();
        book.rest(order(1, Side::Buy, 100, 5));
        book.rest(order(2, Side::Sell, 101, 3));

        assert_eq!(book.best_bid(), Some(Price(100)));
        assert_eq!(book.best_ask(), Some(Price(101)));
        assert_eq!(book.order_count(), 2);
    }

    #[test]
    fn rest_keeps_fifo_and_total_on_one_level() {
        let mut book = OrderBook::new();
        book.rest(order(1, Side::Buy, 100, 5));
        book.rest(order(2, Side::Buy, 100, 3));

        let level = &book.bids[&Price(100)];
        assert_eq!(level.total, Qty(8));
        let ids: Vec<OrderId> = level.orders.iter().map(|o| o.id).collect();
        assert_eq!(ids, vec![OrderId(1), OrderId(2)]);
    }
}
