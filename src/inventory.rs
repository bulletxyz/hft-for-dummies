//! What we hold and how much we've made.
//!
//! One rule: **position only changes from fills** (or when we ask the
//! exchange and it disagrees). Never from "I sent an order", because sent
//! orders don't always fill.

use std::collections::HashSet;

use rust_decimal::Decimal;

use crate::events::{Fill, Side};

#[derive(Debug, Default)]
pub struct Inventory {
    /// Base asset held: positive = long, negative = short.
    pub position: Decimal,
    /// Fees paid so far.
    pub fees: Decimal,
    /// USD in and out from fills.
    cash: Decimal,
    /// Position we adopted from the exchange but haven't put a price on yet.
    unvalued: Decimal,
    seen_trades: HashSet<u64>,
}

impl Inventory {
    /// Apply a fill. Returns false if we had already counted it.
    pub fn on_fill(&mut self, fill: &Fill) -> bool {
        if !self.seen_trades.insert(fill.trade_id) {
            return false;
        }
        let signed = match fill.side {
            Side::Buy => fill.size,
            Side::Sell => -fill.size,
        };
        self.position += signed;
        self.cash -= signed * fill.price;
        self.fees += fill.fee;
        true
    }

    /// The exchange says our position is `venue`. Believe it. The difference
    /// (a position we started with, or fills we missed) is valued at the next
    /// price we see, so it doesn't count as profit or loss.
    pub fn adopt(&mut self, venue: Decimal) {
        self.unvalued += venue - self.position;
        self.position = venue;
    }

    /// A new market price: put a value on any adopted position.
    pub fn mark(&mut self, mid: Decimal) {
        self.cash -= self.unvalued * mid;
        self.unvalued = Decimal::ZERO;
    }

    /// Profit since start in USD, valuing what we hold at `mid`.
    pub fn pnl(&self, mid: Decimal) -> Decimal {
        self.cash + (self.position - self.unvalued) * mid - self.fees
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn fill(id: u64, side: Side, price: &str, size: &str) -> Fill {
        Fill {
            trade_id: id,
            client_id: None,
            side,
            price: d(price),
            size: d(size),
            fee: d("0.01"),
            order_finished: true,
        }
    }

    #[test]
    fn buy_low_sell_high() {
        let mut inv = Inventory::default();
        inv.on_fill(&fill(1, Side::Buy, "100", "1"));
        inv.on_fill(&fill(2, Side::Sell, "101", "1"));
        assert_eq!(inv.position, Decimal::ZERO);
        assert_eq!(
            inv.pnl(d("500")),
            d("0.98"),
            "1 profit minus two 0.01 fees, whatever the price now"
        );
    }

    #[test]
    fn an_open_position_is_valued_at_the_current_price() {
        let mut inv = Inventory::default();
        inv.on_fill(&fill(1, Side::Buy, "100", "2"));
        assert_eq!(inv.pnl(d("99")), d("-2.01"));
    }

    #[test]
    fn the_same_fill_twice_counts_once() {
        let mut inv = Inventory::default();
        assert!(inv.on_fill(&fill(1, Side::Buy, "100", "1")));
        assert!(!inv.on_fill(&fill(1, Side::Buy, "100", "1")));
        assert_eq!(inv.position, d("1"));
    }

    #[test]
    fn an_adopted_position_starts_at_zero_profit() {
        let mut inv = Inventory::default();
        inv.adopt(d("3"));
        assert_eq!(
            inv.pnl(d("100")),
            Decimal::ZERO,
            "not valued yet, not counted"
        );
        inv.mark(d("100"));
        assert_eq!(inv.pnl(d("100")), Decimal::ZERO);
        assert_eq!(
            inv.pnl(d("101")),
            d("3"),
            "after that it moves with the price"
        );
    }
}
