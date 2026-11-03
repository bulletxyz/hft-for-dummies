//! A token bucket: the standard way to stay under a rate limit.
//!
//! The bucket holds up to `burst` tokens and refills at `per_sec` tokens per
//! second. Each request to the exchange spends one. Empty bucket: wait (or
//! don't send). That allows short bursts while capping the average rate.
//!
//! We keep a few tokens back for cancels: when the budget is tight, the
//! orders we most need to get through are the ones taking risk *off*.

use std::time::{Duration, Instant};

pub struct TokenBucket {
    per_sec: f64,
    burst: f64,
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    pub fn new(per_sec: f64, burst: f64) -> Self {
        Self {
            per_sec,
            burst,
            tokens: burst,
            last: Instant::now(),
        }
    }

    /// Spend a token if that leaves at least `keep` in the bucket.
    pub fn try_take(&mut self, now: Instant, keep: f64) -> bool {
        self.refill(now);
        if self.tokens - 1.0 >= keep {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// How long until a token is available.
    pub fn wait_time(&mut self, now: Instant) -> Duration {
        self.refill(now);
        Duration::from_secs_f64(((1.0 - self.tokens) / self.per_sec).max(0.0))
    }

    /// The exchange says we're over its limit: stop and let the bucket refill.
    pub fn empty(&mut self) {
        self.tokens = 0.0;
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.per_sec).min(self.burst);
        self.last = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_a_burst_then_the_steady_rate() {
        let start = Instant::now();
        let mut b = TokenBucket::new(2.0, 4.0);
        assert_eq!(
            (0..10).filter(|_| b.try_take(start, 0.0)).count(),
            4,
            "the burst"
        );
        assert!(!b.try_take(start, 0.0));
        assert!(
            b.try_take(start + Duration::from_millis(500), 0.0),
            "one more after 1/2 s at 2/s"
        );
        assert!(!b.try_take(start + Duration::from_millis(500), 0.0));
    }

    #[test]
    fn keeps_tokens_back_for_cancels() {
        let now = Instant::now();
        let mut b = TokenBucket::new(1.0, 4.0);
        let keep = 2.0;
        assert_eq!(
            (0..10).filter(|_| b.try_take(now, keep)).count(),
            2,
            "new orders stop at the reserve"
        );
        assert!(
            b.try_take(now, 0.0) && b.try_take(now, 0.0),
            "cancels can use the reserve"
        );
        assert!(!b.try_take(now, 0.0));
    }

    #[test]
    fn says_how_long_to_wait() {
        let now = Instant::now();
        let mut b = TokenBucket::new(4.0, 1.0);
        assert!(b.try_take(now, 0.0));
        assert_eq!(b.wait_time(now), Duration::from_millis(250));
        b.empty();
        assert_eq!(b.wait_time(now), Duration::from_millis(250));
    }
}
