//! Telemetry: how long each hop of the tick-to-trade loop takes.
//!
//! You can't speed up what you haven't measured. Every market event carries a
//! [`Stamp`] (when the exchange says it happened, when we received it), and the
//! executor reports when it sent each order and when the exchange answered.
//! From those we keep a histogram per hop and print the percentiles:
//!
//! - **p50**: the typical case.
//! - **p99**: the slow 1%. This is the one that matters: a market maker is
//!   picked off in its slowest moments, not its average ones.
//!
//! Hops that compare our clock with an exchange's ("network") are only as
//! accurate as the two clocks agree, often within a few milliseconds on a
//! laptop. Hops measured on our own clock are exact.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;
use std::time::{Duration, Instant};

use hdrhistogram::Histogram;

use crate::events::{Event, Stamp};

/// How often to print the table.
const REPORT_EVERY: Duration = Duration::from_secs(30);

/// The hops we time, in loop order.
pub const BINANCE_MATCH_TO_SEND: &str = "binance: match → sent (inside Binance)";
pub const BINANCE_NETWORK: &str = "binance: sent → received (network)";
pub const BULLET_NETWORK: &str = "bullet book: sent → received (network)";
pub const QUEUE: &str = "bot: received → handled (queue)";
pub const DECIDE: &str = "bot: decide (update + requote)";
pub const TICK_TO_ORDER: &str = "bot: tick received → orders sent";
pub const SUBMIT_PLACE: &str = "bullet: place sent → accepted";
pub const SUBMIT_CANCEL: &str = "bullet: cancel sent → accepted";
pub const PLACE_TO_LIVE: &str = "bullet: place sent → live on book (ws)";

pub struct Telemetry {
    /// Microseconds, per hop.
    hops: BTreeMap<&'static str, Histogram<u64>>,
    /// When each order we placed was sent, until the exchange echoes it.
    placed_at: HashMap<u64, Instant>,
    /// Echoes that arrived before we heard our own request had finished: the
    /// websocket can beat the HTTP response back to us.
    live_at: HashMap<u64, Instant>,
    /// Messages that arrived "before" the exchange sent them, and by how much
    /// at worst: our clock is behind theirs, so network numbers are wrong.
    clock_behind: u64,
    clock_behind_max_us: u64,
    last_report: Instant,
}

impl Telemetry {
    pub fn new() -> Self {
        Self {
            hops: BTreeMap::new(),
            placed_at: HashMap::new(),
            live_at: HashMap::new(),
            clock_behind: 0,
            clock_behind_max_us: 0,
            last_report: Instant::now(),
        }
    }

    pub fn record(&mut self, hop: &'static str, duration: Duration) {
        let micros = u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
        self.record_us(hop, micros);
    }

    fn record_us(&mut self, hop: &'static str, micros: u64) {
        let hist = self.hops.entry(hop).or_insert_with(|| {
            // Up to 1 minute at 3 significant figures. `new_with_max` only fails on bad bounds.
            Histogram::new_with_max(60_000_000, 3).expect("valid histogram bounds")
        });
        hist.saturating_record(micros.max(1));
    }

    /// Record what an event tells us about latency. `now` is when the loop picked it up.
    pub fn on_event(&mut self, event: &Event, now: Instant) {
        match event {
            Event::Reference { stamp, .. } => {
                if let (Some(sent), Some(matched)) =
                    (stamp.exchange_unix_us, stamp.exchange_match_unix_us)
                {
                    self.record_us(BINANCE_MATCH_TO_SEND, sent.saturating_sub(matched));
                }
                self.network(BINANCE_NETWORK, stamp);
                self.record(QUEUE, now.saturating_duration_since(stamp.received_at));
            }
            Event::Book(_, stamp) => {
                self.network(BULLET_NETWORK, stamp);
                self.record(QUEUE, now.saturating_duration_since(stamp.received_at));
            }
            Event::Sent {
                kind,
                client_ids,
                started,
                finished,
                ..
            } => {
                let hop = match kind {
                    crate::events::SentKind::Place => {
                        for id in client_ids {
                            match self.live_at.remove(id) {
                                Some(live) => self.record(
                                    PLACE_TO_LIVE,
                                    live.saturating_duration_since(*started),
                                ),
                                None => {
                                    self.placed_at.insert(*id, *started);
                                }
                            }
                        }
                        SUBMIT_PLACE
                    }
                    crate::events::SentKind::Cancel => SUBMIT_CANCEL,
                };
                self.record(hop, finished.saturating_duration_since(*started));
            }
            Event::OrderLive { client_id, stamp } => match self.placed_at.remove(client_id) {
                Some(sent) => self.record(
                    PLACE_TO_LIVE,
                    stamp.received_at.saturating_duration_since(sent),
                ),
                None => {
                    self.live_at.insert(*client_id, stamp.received_at);
                }
            },
            _ => {}
        }
    }

    /// Wall-clock gap between the exchange sending and us receiving.
    fn network(&mut self, hop: &'static str, stamp: &Stamp) {
        let Some(sent) = stamp.exchange_unix_us else {
            return;
        };
        if stamp.received_unix_us < sent {
            // Received before it was sent: our clock is behind theirs.
            self.clock_behind += 1;
            self.clock_behind_max_us = self.clock_behind_max_us.max(sent - stamp.received_unix_us);
            return;
        }
        self.record_us(hop, stamp.received_unix_us - sent);
    }

    /// Print the table every [`REPORT_EVERY`].
    pub fn maybe_report(&mut self, now: Instant) {
        if now.duration_since(self.last_report) >= REPORT_EVERY {
            self.last_report = now;
            tracing::info!("latency so far:\n{}", self.table());
            // Placed orders the exchange never echoed (rejected, or a dry run) would pile up.
            self.placed_at
                .retain(|_, sent| now.duration_since(*sent) < REPORT_EVERY);
            self.live_at
                .retain(|_, live| now.duration_since(*live) < REPORT_EVERY);
        }
    }

    /// p50 / p99 / max per hop, in milliseconds.
    pub fn table(&self) -> String {
        let mut out = format!(
            "  {:<42} {:>7} {:>9} {:>9} {:>9}\n",
            "hop", "count", "p50 ms", "p99 ms", "max ms"
        );
        for (hop, h) in &self.hops {
            let ms = |us: u64| us as f64 / 1000.0;
            let _ = writeln!(
                out,
                "  {:<42} {:>7} {:>9.3} {:>9.3} {:>9.3}",
                hop,
                h.len(),
                ms(h.value_at_quantile(0.5)),
                ms(h.value_at_quantile(0.99)),
                ms(h.max())
            );
        }
        if self.clock_behind > 0 {
            let _ = writeln!(
                out,
                "  ! {} messages arrived before the exchange sent them: your clock is behind by at least {:.0} ms,\n    \
                 so the network rows are wrong. Sync it (macOS: sudo sntp -sS time.apple.com; Linux: chrony).",
                self.clock_behind,
                self.clock_behind_max_us as f64 / 1000.0
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;
    use crate::events::SentKind;

    fn stamp(received_at: Instant, received_unix_us: u64, sent: u64, matched: u64) -> Stamp {
        Stamp {
            received_at,
            received_unix_us,
            exchange_unix_us: Some(sent),
            exchange_match_unix_us: Some(matched),
        }
    }

    fn p50_us(t: &Telemetry, hop: &str) -> u64 {
        t.hops[hop].value_at_quantile(0.5)
    }

    /// Histograms keep 3 significant figures: equal to within 0.1%.
    fn assert_near(actual: u64, expected: u64) {
        assert!(
            actual.abs_diff(expected) * 1000 <= expected,
            "{actual} vs {expected}"
        );
    }

    #[test]
    fn splits_a_binance_tick_into_its_hops() {
        let mut t = Telemetry::new();
        let received = Instant::now();
        // Matched at 1_000_000µs, sent 2ms later, received 30ms after that, handled 0.5ms later.
        let event = Event::Reference {
            bid: Decimal::ONE,
            ask: Decimal::TWO,
            stamp: stamp(received, 1_032_000, 1_002_000, 1_000_000),
        };
        t.on_event(&event, received + Duration::from_micros(500));
        assert_near(p50_us(&t, BINANCE_MATCH_TO_SEND), 2_000);
        assert_near(p50_us(&t, BINANCE_NETWORK), 32_000 - 2_000);
        assert_near(p50_us(&t, QUEUE), 500);
    }

    #[test]
    fn a_clock_behind_the_exchange_is_flagged_not_recorded() {
        let mut t = Telemetry::new();
        let now = Instant::now();
        let event = Event::Reference {
            bid: Decimal::ONE,
            ask: Decimal::TWO,
            stamp: stamp(now, 1_000_000, 1_005_000, 1_004_000),
        };
        t.on_event(&event, now);
        assert!(
            !t.hops.contains_key(BINANCE_NETWORK),
            "a negative gap is not a latency"
        );
        assert!(
            t.table().contains("your clock is behind by at least 5 ms"),
            "{}",
            t.table()
        );
    }

    #[test]
    fn the_echo_can_arrive_before_the_http_response() {
        let mut t = Telemetry::new();
        let sent = Instant::now();
        let live = Stamp {
            received_at: sent + Duration::from_millis(90),
            ..Stamp::at(sent)
        };
        t.on_event(
            &Event::OrderLive {
                client_id: 9,
                stamp: live,
            },
            sent,
        );
        t.on_event(
            &Event::Sent {
                kind: SentKind::Place,
                client_ids: vec![9],
                started: sent,
                finished: sent + Duration::from_millis(130),
            },
            sent,
        );
        assert_near(p50_us(&t, PLACE_TO_LIVE), 90_000);
    }

    #[test]
    fn times_an_order_from_sent_to_live_on_the_book() {
        let mut t = Telemetry::new();
        let sent = Instant::now();
        t.on_event(
            &Event::Sent {
                kind: SentKind::Place,
                client_ids: vec![7],
                started: sent,
                finished: sent + Duration::from_millis(80),
            },
            sent,
        );
        let live = Stamp {
            received_at: sent + Duration::from_millis(120),
            received_unix_us: 0,
            exchange_unix_us: None,
            exchange_match_unix_us: None,
        };
        t.on_event(
            &Event::OrderLive {
                client_id: 7,
                stamp: live,
            },
            sent,
        );
        assert_near(p50_us(&t, SUBMIT_PLACE), 80_000);
        assert_near(p50_us(&t, PLACE_TO_LIVE), 120_000);
        assert!(t.table().contains(PLACE_TO_LIVE));
    }
}
