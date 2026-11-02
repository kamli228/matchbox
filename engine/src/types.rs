/// Price in ticks (e.g. 1 tick = 1 cent). Never use floats for money.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Price(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Qty(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OrderId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn opposite(self) -> Side {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderType {
    Limit { price: Price },
    Market,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Order {
    pub id: OrderId,
    pub side: Side,
    pub price: Price,
    pub qty: Qty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trade {
    pub maker_id: OrderId,
    pub taker_id: OrderId,
    pub taker_side: Side,
    pub price: Price,
    pub qty: Qty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    ZeroQty,
    DuplicateId,
    UnknownOrder,
    NoLiquidity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    NewOrder {
        id: OrderId,
        side: Side,
        order_type: OrderType,
        qty: Qty,
    },

    Cancel {
        id: OrderId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Accepted { id: OrderId },

    Trade(Trade),

    Cancelled { id: OrderId, remaining: Qty },

    Rejected { id: OrderId, reason: RejectReason },

    BookUpdate { side: Side, price: Price, qty: Qty },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opposite_side() {
        assert_eq!(Side::Buy.opposite(), Side::Sell);
        assert_eq!(Side::Sell.opposite(), Side::Buy);
    }

    #[test]
    fn prices_are_ordered() {
        assert!(Price(100) < Price(101));
        assert_eq!(Price(100).max(Price(99)), Price(100));
    }

    #[test]
    fn command_can_be_matched() {
        let cmd = Command::NewOrder {
            id: OrderId(1),
            side: Side::Buy,
            order_type: OrderType::Limit { price: Price(100) },
            qty: Qty(10),
        };

        let price = match cmd {
            Command::NewOrder {
                order_type: OrderType::Limit { price },
                ..
            } => Some(price),
            _ => None,
        };

        assert_eq!(price, Some(Price(100)));
    }

    #[test]
    fn events_compare_by_value() {
        let t = Trade {
            maker_id: OrderId(1),
            taker_id: OrderId(2),
            taker_side: Side::Sell,
            price: Price(100),
            qty: Qty(5),
        };
        assert_eq!(Event::Trade(t), Event::Trade(t));
    }
}
