use rust_decimal::Decimal;

/// How the market maker turns a reference price into quotes.
pub struct Config {
    /// Distance from the center of our quotes to each side.
    pub half_spread: Decimal,
    /// How far to shift both quotes per unit of inventory.
    pub skew_per_unit: Decimal,
    /// Stop quoting the side that would push inventory past this.
    pub max_position: Decimal,
}

/// The orders we want resting on the book. `None` means "don't quote this side".
#[derive(Debug, PartialEq)]
pub struct Quotes {
    pub bid: Option<Decimal>,
    pub ask: Option<Decimal>,
}

/// Quote around the reference price, leaning against our inventory.
pub fn quote(reference: Decimal, position: Decimal, cfg: &Config) -> Quotes {
    // Long: shift both quotes down so we sell more easily and buy less.
    let center = reference - position * cfg.skew_per_unit;
    Quotes {
        bid: (position < cfg.max_position).then(|| center - cfg.half_spread),
        ask: (position > -cfg.max_position).then(|| center + cfg.half_spread),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(n: i64) -> Decimal {
        Decimal::new(n, 2)
    }

    fn cfg() -> Config {
        Config {
            half_spread: d(2),
            skew_per_unit: d(1),
            max_position: Decimal::from(5),
        }
    }

    #[test]
    fn flat_quotes_are_symmetric_around_reference() {
        let q = quote(d(10000), Decimal::ZERO, &cfg());
        assert_eq!(
            q,
            Quotes {
                bid: Some(d(9998)),
                ask: Some(d(10002))
            }
        );
    }

    #[test]
    fn long_inventory_shifts_quotes_down() {
        let q = quote(d(10000), Decimal::from(2), &cfg());
        assert_eq!(
            q,
            Quotes {
                bid: Some(d(9996)),
                ask: Some(d(10000))
            }
        );
    }

    #[test]
    fn at_max_long_stops_bidding() {
        let q = quote(d(10000), Decimal::from(5), &cfg());
        assert_eq!(q.bid, None);
        assert!(q.ask.is_some());
    }

    #[test]
    fn at_max_short_stops_offering() {
        let q = quote(d(10000), Decimal::from(-5), &cfg());
        assert_eq!(q.ask, None);
        assert!(q.bid.is_some());
    }
}
