use rand::{Rng, SeedableRng, rngs::StdRng};
use rust_decimal::Decimal;

/// A fake market: a fair value that random-walks, and a trade printed around it each tick.
pub struct Market {
    fair: Decimal,
    rng: StdRng,
}

pub struct Tick {
    /// The market's true fair value. Our reference price in this first version.
    pub fair: Decimal,
    /// Where the market actually traded this tick.
    pub trade: Decimal,
}

impl Market {
    pub fn new(start: Decimal, seed: u64) -> Self {
        Self {
            fair: start,
            rng: StdRng::seed_from_u64(seed),
        }
    }

    pub fn step(&mut self) -> Tick {
        // Prices move in whole cents: fair value by up to 1, trades land up to 3 away.
        self.fair += Decimal::new(self.rng.random_range(-1..=1), 2);
        let trade = self.fair + Decimal::new(self.rng.random_range(-3..=3), 2);
        Tick {
            fair: self.fair,
            trade,
        }
    }
}
