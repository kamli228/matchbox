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

    /// Top `levels` price levels of one side, best price first: (price, total qty).
    pub fn depth(&self, side: Side, levels: usize) -> Vec<(Price, Qty)> {
        let map = match side {
            Side::Buy => &self.bids,
            Side::Sell => &self.asks,
        };
        let to_pair = |(price, level): (&Price, &Level)| (*price, level.total);
        match side {
            Side::Buy => map.iter().rev().take(levels).map(to_pair).collect(),
            Side::Sell => map.iter().take(levels).map(to_pair).collect(),
        }
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
                OrderType::Market => self.new_market(id, side, qty),
            },
            Command::Cancel { id } => self.cancel(id),
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

        let remaining = self.match_order(id, side, Some(price), qty, &mut events);

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

    /// Handle a market order: match at any price, cancel the remainder.
    fn new_market(&mut self, id: OrderId, side: Side, qty: Qty) -> Vec<Event> {
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

        let opposite_empty = match side {
            Side::Buy => self.asks.is_empty(),
            Side::Sell => self.bids.is_empty(),
        };

        if opposite_empty {
            return vec![Event::Rejected {
                id,
                reason: RejectReason::NoLiquidity,
            }];
        }

        events.push(Event::Accepted { id });

        let remaining = self.match_order(id, side, None, qty, &mut events);

        if remaining.0 > 0 {
            events.push(Event::Cancelled { id, remaining });
        }

        events
    }

    fn cancel(&mut self, id: OrderId) -> Vec<Event> {
        let Some((side, price)) = self.index.remove(&id) else {
            return vec![Event::Rejected {
                id,
                reason: RejectReason::UnknownOrder,
            }];
        };

        let book_side = match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        let level = book_side
            .get_mut(&price)
            .expect("indexed order has a level");

        let pos = level
            .orders
            .iter()
            .position(|o| o.id == id)
            .expect("indexed order is on its level");
        let order = level.orders.remove(pos).expect("position is valid");

        level.total.0 -= order.qty.0;
        let level_total = level.total;

        if level.orders.is_empty() {
            book_side.remove(&price);
        }

        vec![
            Event::Cancelled {
                id,
                remaining: order.qty,
            },
            Event::BookUpdate {
                side,
                price,
                qty: level_total,
            },
        ]
    }

    fn match_order(
        &mut self,
        id: OrderId,
        side: Side,
        limit: Option<Price>,
        qty: Qty,
        events: &mut Vec<Event>,
    ) -> Qty {
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

            let crosses = match (side, limit) {
                (_, None) => true,
                (Side::Buy, Some(price)) => price >= best,
                (Side::Sell, Some(price)) => price <= best,
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

        remaining
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

    fn market(id: u64, side: Side, qty: u64) -> Command {
        Command::NewOrder {
            id: OrderId(id),
            side,
            order_type: OrderType::Market,
            qty: Qty(qty),
        }
    }

    /// Extract (maker_id, price, qty) of every trade, for short assertions.
    fn trades(events: &[Event]) -> Vec<(u64, u64, u64)> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Trade(t) => Some((t.maker_id.0, t.price.0, t.qty.0)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn limit_sweeps_multiple_levels() {
        let mut book = OrderBook::new();
        book.process(limit(2, Side::Sell, 101, 3));
        book.process(limit(5, Side::Sell, 102, 5));
        let events = book.process(limit(9, Side::Buy, 102, 6));
        assert_eq!(trades(&events), vec![(2, 101, 3), (5, 102, 3)]);
        assert_eq!(book.best_ask(), Some(Price(102)));
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.order_count(), 1);
    }

    #[test]
    fn fifo_within_level() {
        let mut book = OrderBook::new();
        book.process(limit(1, Side::Sell, 101, 2));
        book.process(limit(2, Side::Sell, 101, 2));
        let events = book.process(limit(9, Side::Buy, 101, 3));
        assert_eq!(trades(&events), vec![(1, 101, 2), (2, 101, 1)]);
        assert_eq!(book.order_count(), 1);
    }

    #[test]
    fn trade_happens_at_maker_price() {
        let mut book = OrderBook::new();
        book.process(limit(1, Side::Buy, 100, 5));
        let events = book.process(limit(9, Side::Sell, 95, 5));
        assert_eq!(trades(&events), vec![(1, 100, 5)]);
    }

    #[test]
    fn market_order_on_empty_book_is_rejected() {
        let mut book = OrderBook::new();
        let events = book.process(market(1, Side::Buy, 5));
        assert_eq!(
            events,
            vec![Event::Rejected {
                id: OrderId(1),
                reason: RejectReason::NoLiquidity
            }]
        );
    }

    #[test]
    fn market_order_sweeps_and_cancels_remainder() {
        let mut book = OrderBook::new();
        book.process(limit(1, Side::Sell, 101, 2));
        book.process(limit(2, Side::Sell, 105, 1));
        let events = book.process(market(9, Side::Buy, 5));
        assert_eq!(trades(&events), vec![(1, 101, 2), (2, 105, 1)]);
        assert_eq!(
            events.last(),
            Some(&Event::Cancelled {
                id: OrderId(9),
                remaining: Qty(2)
            })
        );
        assert_eq!(book.order_count(), 0);
        assert_eq!(book.best_bid(), None);
    }

    #[test]
    fn market_order_never_rests() {
        let mut book = OrderBook::new();
        book.process(limit(1, Side::Buy, 100, 10));
        let events = book.process(market(9, Side::Sell, 4));
        assert_eq!(trades(&events), vec![(1, 100, 4)]);
        assert!(!events.iter().any(|e| matches!(e, Event::Cancelled { .. })));
        assert_eq!(book.best_bid(), Some(Price(100)));
        assert_eq!(book.best_ask(), None);
    }
    #[test]
    fn cancel_removes_order() {
        let mut book = OrderBook::new();
        book.process(limit(1, Side::Buy, 100, 5));
        book.process(limit(2, Side::Buy, 100, 3));
        let events = book.process(Command::Cancel { id: OrderId(1) });
        assert_eq!(
            events,
            vec![
                Event::Cancelled {
                    id: OrderId(1),
                    remaining: Qty(5)
                },
                Event::BookUpdate {
                    side: Side::Buy,
                    price: Price(100),
                    qty: Qty(3)
                },
            ]
        );
        assert_eq!(book.order_count(), 1);
        // the cancelled order must not trade anymore
        let ev = book.process(limit(9, Side::Sell, 100, 3));
        assert_eq!(trades(&ev), vec![(2, 100, 3)]);
    }

    #[test]
    fn cancel_last_order_removes_level() {
        let mut book = OrderBook::new();
        book.process(limit(1, Side::Sell, 101, 5));
        book.process(Command::Cancel { id: OrderId(1) });
        assert_eq!(book.best_ask(), None);
        assert_eq!(book.order_count(), 0);
    }

    #[test]
    fn cancel_unknown_is_rejected() {
        let mut book = OrderBook::new();
        let ev = book.process(Command::Cancel { id: OrderId(7) });
        assert_eq!(
            ev,
            vec![Event::Rejected {
                id: OrderId(7),
                reason: RejectReason::UnknownOrder
            }]
        );
    }

    #[test]
    fn cancel_filled_order_is_rejected() {
        let mut book = OrderBook::new();
        book.process(limit(1, Side::Sell, 101, 2));
        book.process(limit(2, Side::Buy, 101, 2));
        let ev = book.process(Command::Cancel { id: OrderId(1) });
        assert_eq!(
            ev,
            vec![Event::Rejected {
                id: OrderId(1),
                reason: RejectReason::UnknownOrder
            }]
        );
    }

    #[test]
    fn cancel_partially_filled_returns_remaining() {
        let mut book = OrderBook::new();
        book.process(limit(1, Side::Sell, 101, 5));
        book.process(limit(2, Side::Buy, 101, 2));
        let ev = book.process(Command::Cancel { id: OrderId(1) });
        assert_eq!(
            ev[0],
            Event::Cancelled {
                id: OrderId(1),
                remaining: Qty(3)
            }
        );
        assert_eq!(book.best_ask(), None);
    }

    #[test]
    fn sell_limit_sweeps_bids_from_highest() {
        let mut book = OrderBook::new();
        book.process(limit(1, Side::Buy, 100, 2));
        book.process(limit(2, Side::Buy, 99, 2));
        let events = book.process(limit(9, Side::Sell, 99, 3));
        assert_eq!(trades(&events), vec![(1, 100, 2), (2, 99, 1)]);
        assert_eq!(book.best_bid(), Some(Price(99)));
        assert_eq!(book.order_count(), 1);
    }

    #[test]
    fn market_zero_qty_is_rejected() {
        let mut book = OrderBook::new();
        book.process(limit(1, Side::Sell, 101, 5));
        let events = book.process(market(2, Side::Buy, 0));
        assert_eq!(
            events,
            vec![Event::Rejected {
                id: OrderId(2),
                reason: RejectReason::ZeroQty
            }]
        );
        assert_eq!(book.order_count(), 1);
    }

    #[test]
    fn book_update_reports_level_total() {
        let mut book = OrderBook::new();
        book.process(limit(1, Side::Buy, 100, 5));
        let events = book.process(limit(2, Side::Buy, 100, 3));
        assert_eq!(
            events,
            vec![
                Event::Accepted { id: OrderId(2) },
                Event::BookUpdate {
                    side: Side::Buy,
                    price: Price(100),
                    qty: Qty(8)
                },
            ]
        );
    }
}
