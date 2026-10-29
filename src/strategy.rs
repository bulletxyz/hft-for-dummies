//! The strategy: where to put our bid and our ask.
//!
//! A market maker earns the spread: buy a little below fair value, sell a
//! little above it, repeat. Three rules on top of that:
//!
//! 1. **Spread.** Quote `spread_bps` either side of fair value.
//! 2. **Skew.** If we're long, lean both quotes down so we're more likely to
//!    sell (and less likely to buy more). Short: lean up. This keeps
//!    inventory near zero without ever crossing the spread to dump it.
//! 3. **Limits.** Don't quote the side that would take us past `max_position`,
//!    and never price a quote so it would cross the book (it would take,
//!    not make).

use rust_decimal::{Decimal, RoundingStrategy};

use crate::book::Book;
use crate::config::Config;

const BPS: Decimal = Decimal::from_parts(10_000, 0, 0, false, 0);

/// The prices we want resting. `None` = don't quote that side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Quotes {
    pub bid: Option<Decimal>,
    pub ask: Option<Decimal>,
}

pub fn quote(
    fair: Decimal,
    position: Decimal,
    config: &Config,
    book: &Book,
    tick: Decimal,
) -> Quotes {
    // How full our inventory is, from -1 (max short) to +1 (max long).
    let fullness = (position / config.max_position).clamp(Decimal::NEGATIVE_ONE, Decimal::ONE);
    let lean_bps = config.skew_bps * fullness;

    let bid = fair * (Decimal::ONE - (config.spread_bps + lean_bps) / BPS);
    let ask = fair * (Decimal::ONE + (config.spread_bps - lean_bps) / BPS);

    let room_to_buy = position + config.order_size <= config.max_position;
    let room_to_sell = position - config.order_size >= -config.max_position;

    Quotes {
        bid: room_to_buy.then(|| {
            // Stay at least one tick below the best ask, rounded down to a valid price.
            let bid = match book.best_ask() {
                Some(best_ask) => bid.min(best_ask - tick),
                None => bid,
            };
            round_to_tick(bid, tick, RoundingStrategy::ToNegativeInfinity)
        }),
        ask: room_to_sell.then(|| {
            let ask = match book.best_bid() {
                Some(best_bid) => ask.max(best_bid + tick),
                None => ask,
            };
            round_to_tick(ask, tick, RoundingStrategy::ToPositiveInfinity)
        }),
    }
}

fn round_to_tick(price: Decimal, tick: Decimal, rounding: RoundingStrategy) -> Decimal {
    if tick.is_zero() {
        return price;
    }
    (price / tick).round_dp_with_strategy(0, rounding) * tick
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    /// 10 bps spread, 10 bps skew, size 0.1, max position 1, tick 0.01.
    fn config() -> Config {
        let mut c = Config::load(Path::new("bot.toml")).unwrap();
        c.spread_bps = d("10");
        c.skew_bps = d("10");
        c.order_size = d("0.1");
        c.max_position = d("1");
        c
    }

    /// A wide book, so it never gets in the way unless a test wants it to.
    fn wide_book() -> Book {
        Book::from_levels(&[(d("90"), d("1"))], &[(d("110"), d("1"))])
    }

    const TICK: Decimal = Decimal::from_parts(1, 0, 0, false, 2);

    #[test]
    fn flat_quotes_spread_either_side() {
        let q = quote(d("100"), Decimal::ZERO, &config(), &wide_book(), TICK);
        assert_eq!(
            q,
            Quotes {
                bid: Some(d("99.90")),
                ask: Some(d("100.10"))
            }
        );
    }

    #[test]
    fn long_leans_both_quotes_down() {
        // Half full: lean 5 bps.
        let q = quote(d("100"), d("0.5"), &config(), &wide_book(), TICK);
        assert_eq!(
            q,
            Quotes {
                bid: Some(d("99.85")),
                ask: Some(d("100.05"))
            }
        );
    }

    #[test]
    fn short_leans_both_quotes_up() {
        let q = quote(d("100"), d("-0.5"), &config(), &wide_book(), TICK);
        assert_eq!(
            q,
            Quotes {
                bid: Some(d("99.95")),
                ask: Some(d("100.15"))
            }
        );
    }

    #[test]
    fn stops_buying_at_max_long() {
        let q = quote(d("100"), d("1"), &config(), &wide_book(), TICK);
        assert_eq!(q.bid, None);
        assert!(q.ask.is_some());
    }

    #[test]
    fn stops_selling_at_max_short() {
        let q = quote(d("100"), d("-1"), &config(), &wide_book(), TICK);
        assert_eq!(q.ask, None);
        assert!(q.bid.is_some());
    }

    #[test]
    fn stops_buying_when_one_more_order_would_pass_the_max() {
        let q = quote(d("100"), d("0.95"), &config(), &wide_book(), TICK);
        assert_eq!(q.bid, None);
    }

    #[test]
    fn never_crosses_the_book() {
        // Fair value says bid 99.90, but the best ask is 99.50.
        let book = Book::from_levels(&[(d("99.40"), d("1"))], &[(d("99.50"), d("1"))]);
        let q = quote(d("100"), Decimal::ZERO, &config(), &book, TICK);
        assert_eq!(q.bid, Some(d("99.49")), "one tick under the best ask");
        assert_eq!(q.ask, Some(d("100.10")));
    }

    #[test]
    fn rounds_bids_down_and_asks_up_to_the_tick() {
        let q = quote(d("100.003"), Decimal::ZERO, &config(), &wide_book(), TICK);
        assert_eq!(
            q,
            Quotes {
                bid: Some(d("99.90")),
                ask: Some(d("100.11"))
            }
        );
    }
}
