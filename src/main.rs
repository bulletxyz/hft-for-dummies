mod quoter;
mod sim;

use rust_decimal::Decimal;

use quoter::{Config, quote};
use sim::Market;

const TICKS: u32 = 10_000;
const REPORT_EVERY: u32 = 1_000;

fn main() {
    let cfg = Config {
        half_spread: Decimal::new(2, 2),   // 0.02
        skew_per_unit: Decimal::new(1, 2), // 0.01
        max_position: Decimal::from(5),
    };
    let mut market = Market::new(Decimal::from(100), 42);
    let mut position = Decimal::ZERO;
    let mut cash = Decimal::ZERO;

    for tick in 1..=TICKS {
        let t = market.step();
        let quotes = quote(t.fair, position, &cfg);

        // A trade at or through our quote fills one unit.
        if let Some(bid) = quotes.bid
            && t.trade <= bid
        {
            position += Decimal::ONE;
            cash -= bid;
        }
        if let Some(ask) = quotes.ask
            && t.trade >= ask
        {
            position -= Decimal::ONE;
            cash += ask;
        }

        if tick % REPORT_EVERY == 0 {
            let pnl = cash + position * t.fair;
            println!(
                "tick {tick:>6}  fair {:>8}  position {position:>3}  pnl {pnl:>8}",
                t.fair
            );
        }
    }
}
