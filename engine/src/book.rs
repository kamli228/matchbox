//! order book price levels per side and the matching logic

use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::types::{
    Command, Event, Order, OrderId, OrderType, Price, Qty, RejectReason, Side, Trade,
};

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
    fn rest(&mut self, order: Order) -> Qty {
        let book_side = match order.side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };

        let level = book_side.entry(order.price).or_default();

        level.total.0 += order.qty.0;
        level.orders.push_back(order);

        let tot = level.total;

        self.index.insert(order.id, (order.side, order.price));

        tot
    }

    /// apply one command and return resulting events
    pub fn process(&mut self, cmd: Command) -> Vec<Event> {
        match cmd {
            Command::NewOrder {
                id,
                side,
                order_type,
                qty,
            } => match order_type {
                OrderType::Limit { price } => self.new_limit(id, side, price, qty),
                OrderType::Market => todo!("step 4"),
            },
            Command::Cancel { .. } => todo!("step 5"),
        }
    }

    /// handle a new limit order: validate, match, rest the remainder
    fn new_limit(&mut self, id: OrderId, side: Side, price: Price, qty: Qty) -> Vec<Event> {
        let mut events = Vec::new();

        if qty.0 == 0 {
            return vec![Event::Rejected {
                id,
                reason: RejectReason::ZeroQty,
            }];
        }

        if self.index.contains_key(&id) {
            return vec![Event::Rejected {
                id,
                reason: RejectReason::DuplicateId,
            }];
        }

        events.push(Event::Accepted { id });

        let mut remaining = qty;
        let opposite = side.opposite();

        while remaining.0 > 0 {
            let best = match side {
                Side::Buy => self.best_ask(),
                Side::Sell => self.best_bid(),
            };
            let Some(best) = best else {
                break;
            };

            let crosses = match side {
                Side::Buy => price >= best,
                Side::Sell => price <= best,
            };
            if !crosses {
                break;
            }

            let book_side = match opposite {
                Side::Buy => &mut self.bids,
                Side::Sell => &mut self.asks,
            };
            let level = book_side.get_mut(&best).expect("best level exists");
            let maker = level.orders.front_mut().expect("level is never empty");

            let fill = remaining.min(maker.qty);

            maker.qty.0 -= fill.0;
            level.total.0 -= fill.0;
            remaining.0 -= fill.0;

            let maker_id = maker.id;
            let maker_done = maker.qty.0 == 0;

            events.push(Event::Trade(Trade {
                maker_id,
                taker_id: id,
                taker_side: side,
                price: best,
                qty: fill,
            }));

            if maker_done {
                level.orders.pop_front();
                self.index.remove(&maker_id);
            }

            let level_total = level.total;

            if level.orders.is_empty() {
                book_side.remove(&best);
            }

            events.push(Event::BookUpdate {
                side: opposite,
                price: best,
                qty: level_total,
            });
        }

        if remaining.0 > 0 {
            let total = self.rest(Order {
                id,
                side,
                price,
                qty: remaining,
            });
            events.push(Event::BookUpdate {
                side,
                price,
                qty: total,
            });
        }

        events
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

    fn limit(id: u64, side: Side, price: u64, qty: u64) -> Command {
        Command::NewOrder {
            id: OrderId(id),
            side,
            order_type: OrderType::Limit {
                price: Price(price),
            },
            qty: Qty(qty),
        }
    }

    #[test]
    fn zero_qty_is_rejected() {
        let mut book = OrderBook::new();
        let events = book.process(limit(1, Side::Buy, 100, 0));
        assert_eq!(
            events,
            vec![Event::Rejected {
                id: OrderId(1),
                reason: RejectReason::ZeroQty
            }]
        );
        assert_eq!(book.order_count(), 0);
    }

    #[test]
    fn duplicate_id_is_rejected() {
        let mut book = OrderBook::new();
        book.process(limit(1, Side::Buy, 100, 5));
        let events = book.process(limit(1, Side::Buy, 99, 5));
        assert_eq!(
            events,
            vec![Event::Rejected {
                id: OrderId(1),
                reason: RejectReason::DuplicateId
            }]
        );
        assert_eq!(book.order_count(), 1);
    }

    #[test]
    fn non_crossing_limit_rests_in_book() {
        let mut book = OrderBook::new();
        let events = book.process(limit(1, Side::Buy, 100, 5));
        assert_eq!(
            events,
            vec![
                Event::Accepted { id: OrderId(1) },
                Event::BookUpdate {
                    side: Side::Buy,
                    price: Price(100),
                    qty: Qty(5)
                },
            ]
        );
        assert_eq!(book.best_bid(), Some(Price(100)));
    }

    #[test]
    fn partial_fill_rests_remainder() {
        let mut book = OrderBook::new();
        book.process(limit(2, Side::Sell, 101, 3));
        let events = book.process(limit(9, Side::Buy, 101, 4));
        assert_eq!(
            events,
            vec![
                Event::Accepted { id: OrderId(9) },
                Event::Trade(Trade {
                    maker_id: OrderId(2),
                    taker_id: OrderId(9),
                    taker_side: Side::Buy,
                    price: Price(101),
                    qty: Qty(3),
                }),
                Event::BookUpdate {
                    side: Side::Sell,
                    price: Price(101),
                    qty: Qty(0)
                },
                Event::BookUpdate {
                    side: Side::Buy,
                    price: Price(101),
                    qty: Qty(1)
                },
            ]
        );
        assert_eq!(book.best_ask(), None);
        assert_eq!(book.best_bid(), Some(Price(101)));
        assert_eq!(book.order_count(), 1);
    }
}
