//! Fair value: what we think SOL is worth right now.
//!
//! Binance moves first, so we start from the Binance mid. But Binance quotes
//! in USDT and Bullet in USD, and the two venues rarely trade at exactly the
//! same price. That steady gap is the *basis*. We average it slowly, then:
//!
//! ```text
//! fair value = Binance mid + average basis
//! ```
//!
//! This follows Binance tick by tick without drifting away from where Bullet
//! actually trades. If we don't have a trustworthy fair value (Binance went
//! quiet, or we haven't watched the basis long enough) we say so, and the bot
//! stops quoting. Quoting on a bad price is how market makers lose money.

use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;

/// Watch the basis this long before trusting it.
pub const WARMUP: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
pub struct FairValue {
    /// Latest Binance mid and when it arrived.
    binance_mid: Option<(Decimal, Instant)>,
    basis: Option<Basis>,
}

#[derive(Debug, Clone, Copy)]
struct Basis {
    /// Running average of (Bullet mid − Binance mid).
    average: Decimal,
    last_update: Instant,
    started: Instant,
}

/// Why there is no fair value right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotReady {
    NoBinancePrice,
    BinanceStale,
    WarmingUp,
}

impl FairValue {
    /// A new best bid/ask from Binance.
    pub fn on_binance(&mut self, bid: Decimal, ask: Decimal, at: Instant) {
        if bid > Decimal::ZERO && ask >= bid {
            self.binance_mid = Some(((bid + ask) / Decimal::TWO, at));
        }
    }

    /// A new Bullet mid: fold today's gap into the running average.
    pub fn on_bullet_mid(
        &mut self,
        bullet_mid: Decimal,
        now: Instant,
        stale: Duration,
        halflife: Duration,
    ) {
        let Ok(binance_mid) = self.fresh_binance_mid(now, stale) else {
            // Without a live Binance price the gap means nothing; start over later.
            self.basis = None;
            return;
        };
        let sample = bullet_mid - binance_mid;
        self.basis = Some(match self.basis {
            None => Basis {
                average: sample,
                last_update: now,
                started: now,
            },
            Some(b) => {
                let weight = ema_weight(now - b.last_update, now - b.started, halflife);
                Basis {
                    average: (b.average + weight * (sample - b.average)).round_dp(8),
                    last_update: now,
                    started: b.started,
                }
            }
        });
    }

    /// The price to quote around, or why there isn't one.
    pub fn get(&self, now: Instant, stale: Duration) -> Result<Decimal, NotReady> {
        let binance_mid = self.fresh_binance_mid(now, stale)?;
        match self.basis {
            Some(b) if now.saturating_duration_since(b.started) >= WARMUP => {
                Ok(binance_mid + b.average)
            }
            _ => Err(NotReady::WarmingUp),
        }
    }

    fn fresh_binance_mid(&self, now: Instant, stale: Duration) -> Result<Decimal, NotReady> {
        match self.binance_mid {
            None => Err(NotReady::NoBinancePrice),
            Some((_, at)) if now.saturating_duration_since(at) > stale => {
                Err(NotReady::BinanceStale)
            }
            Some((mid, _)) => Ok(mid),
        }
    }
}

/// How much a new sample moves the average. A sample `halflife` after the last
/// one moves it halfway. Early on (when we have little history) we take the
/// plain time-weighted mean instead, so the very first sample doesn't dominate.
fn ema_weight(since_last: Duration, since_start: Duration, halflife: Duration) -> Decimal {
    if since_last.is_zero() {
        return Decimal::ZERO;
    }
    let ema = 1.0 - 0.5_f64.powf(since_last.as_secs_f64() / halflife.as_secs_f64().max(1.0));
    let mean = since_last.as_secs_f64() / since_start.as_secs_f64();
    Decimal::from_f64(ema.max(mean).clamp(0.0, 1.0)).unwrap_or(Decimal::ONE)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STALE: Duration = Duration::from_secs(5);
    const HALFLIFE: Duration = Duration::from_secs(300);

    fn d(n: i64) -> Decimal {
        Decimal::from(n)
    }

    /// Binance mid 100, Bullet mid 101, once a second for `secs` seconds.
    fn watched_for(secs: u64) -> (FairValue, Instant) {
        let start = Instant::now();
        let mut fv = FairValue::default();
        for s in 0..=secs {
            let t = start + Duration::from_secs(s);
            fv.on_binance(d(99), d(101), t);
            fv.on_bullet_mid(d(101), t, STALE, HALFLIFE);
        }
        (fv, start + Duration::from_secs(secs))
    }

    #[test]
    fn nothing_until_binance_speaks() {
        let fv = FairValue::default();
        assert_eq!(fv.get(Instant::now(), STALE), Err(NotReady::NoBinancePrice));
    }

    #[test]
    fn waits_for_the_basis_to_warm_up() {
        let (fv, now) = watched_for(30);
        assert_eq!(fv.get(now, STALE), Err(NotReady::WarmingUp));
    }

    #[test]
    fn binance_mid_plus_basis_once_warm() {
        let (mut fv, now) = watched_for(60);
        assert_eq!(fv.get(now, STALE), Ok(d(101)));
        // Binance jumps 2: fair value follows straight away; the basis stays 1.
        fv.on_binance(d(101), d(103), now);
        assert_eq!(fv.get(now, STALE), Ok(d(103)));
    }

    #[test]
    fn stops_when_binance_goes_quiet() {
        let (fv, now) = watched_for(60);
        let later = now + STALE + Duration::from_secs(1);
        assert_eq!(fv.get(later, STALE), Err(NotReady::BinanceStale));
    }
}
