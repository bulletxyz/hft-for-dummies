//! Telemetry: how long each hop of the loop takes, from a Binance trade to
//! our order resting on Bullet.
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

/// The headline: from the Binance trade (or book change) that moved our price
/// until Bullet opens our new order. How long our quote lags the market.
pub const END_TO_END: &str = "END-TO-END: Binance trade → our order open";

// The steps of that trip, in order. They roughly add up to it.
pub const BINANCE_MATCH_TO_SEND: &str = "inside Binance (match → sent)";
pub const BINANCE_NETWORK: &str = "binance→us (network)";
pub const QUEUE: &str = "queue (received → handled)";
/// Our reaction: from receiving the tick to handing the order to the executor.
pub const TICK_TO_TRADE: &str = "tick-to-trade (our code)";
/// From our request leaving until Bullet's matching engine opens the order
/// (its own timestamp): the route there and Bullet's processing.
pub const ORDER_TO_LIVE: &str = "order-to-live (route + Bullet)";
const TRIP: [&str; 5] = [
    BINANCE_MATCH_TO_SEND,
    BINANCE_NETWORK,
    QUEUE,
    TICK_TO_TRADE,
    ORDER_TO_LIVE,
];

// Also measured, overlapping the trip or off it.
pub const DECIDE: &str = "decide (part of tick-to-trade)";
pub const SUBMIT_PLACE: &str = "Bullet's reply to a place (round trip)";
pub const SUBMIT_CANCEL: &str = "Bullet's reply to a cancel (round trip)";
pub const BULLET_NETWORK: &str = "Bullet book → us (network)";

pub struct Telemetry {
    /// Microseconds, per hop.
    hops: BTreeMap<&'static str, Histogram<u64>>,
    /// When each order we placed was sent (µs since 1970, our clock), until
    /// Bullet reports it open.
    placed_at: HashMap<u64, u64>,
    /// For each order we decided to send: when the Binance event that moved
    /// our price happened (µs since 1970, Binance's clock).
    triggered_by: HashMap<u64, u64>,
    /// When Bullet opened orders we hadn't yet heard our own request finish
    /// for (µs since 1970, Bullet's clock): its websocket can beat its reply.
    live_at: HashMap<u64, u64>,
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
            triggered_by: HashMap::new(),
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
                        let sent_us = wall_clock(*started);
                        for id in client_ids {
                            match self.live_at.remove(id) {
                                Some(opened) => self.order_to_live(sent_us, opened),
                                None => {
                                    self.placed_at.insert(*id, sent_us);
                                }
                            }
                        }
                        SUBMIT_PLACE
                    }
                    crate::events::SentKind::Cancel => SUBMIT_CANCEL,
                };
                self.record(hop, finished.saturating_duration_since(*started));
            }
            Event::OrderLive { client_id, stamp } => {
                // Bullet's matching-engine timestamp for the order opening.
                let Some(opened) = stamp.exchange_match_unix_us.or(stamp.exchange_unix_us) else {
                    return;
                };
                if let Some(matched) = self.triggered_by.remove(client_id) {
                    self.between(END_TO_END, matched, opened);
                }
                match self.placed_at.remove(client_id) {
                    Some(sent_us) => self.order_to_live(sent_us, opened),
                    None => {
                        self.live_at.insert(*client_id, opened);
                    }
                }
            }
            _ => {}
        }
    }

    /// The bot decided to send these orders because of the Binance event
    /// stamped `tick`; remember it until we know when they actually left.
    pub fn on_decision(&mut self, intents: &[crate::events::Intent], tick: Option<&Stamp>) {
        let Some(tick) = tick else { return };
        let Some(matched) = tick.exchange_match_unix_us.or(tick.exchange_unix_us) else {
            return;
        };
        for intent in intents {
            if let crate::events::Intent::Place(order) = intent {
                self.triggered_by.insert(order.client_id, matched);
            }
        }
    }

    fn order_to_live(&mut self, sent_us: u64, opened: u64) {
        self.between(ORDER_TO_LIVE, sent_us, opened);
    }

    /// Record `to - from` (µs since 1970, possibly on two clocks). A negative
    /// gap means the clocks disagree, so it's flagged rather than recorded.
    fn between(&mut self, hop: &'static str, from: u64, to: u64) {
        if to < from {
            self.clock_behind += 1;
            self.clock_behind_max_us = self.clock_behind_max_us.max(from - to);
        } else {
            self.record_us(hop, to - from);
        }
    }

    /// One line to compare runs and parts: the series' scoreboard. Per-order
    /// numbers use p95: a run has a few hundred orders, so its p99 is just
    /// the one or two slowest.
    pub fn scoreboard(&self) -> String {
        let ms = |hop: &str, q: f64| {
            self.hops.get(hop).map_or("-".to_string(), |h| {
                format!("{:.2}", h.value_at_quantile(q) as f64 / 1000.0)
            })
        };
        let us = |hop: &str, q: f64| {
            self.hops
                .get(hop)
                .map_or("-".to_string(), |h| h.value_at_quantile(q).to_string())
        };
        format!(
            "SCOREBOARD end-to-end p50 {} ms p95 {} ms | binance→us p50 {} ms | \
             tick-to-trade p50 {} µs p99 {} µs | order-to-live p50 {} ms p95 {} ms | \
             queue p99 {} ms max {} ms | orders {}",
            ms(END_TO_END, 0.5),
            ms(END_TO_END, 0.95),
            ms(BINANCE_NETWORK, 0.5),
            us(TICK_TO_TRADE, 0.5),
            us(TICK_TO_TRADE, 0.99),
            ms(ORDER_TO_LIVE, 0.5),
            ms(ORDER_TO_LIVE, 0.95),
            ms(QUEUE, 0.99),
            ms(QUEUE, 1.0),
            self.hops.get(END_TO_END).map_or(0, Histogram::len),
        )
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
            // Orders that never opened (rejected, or a dry run) would pile up.
            let cutoff = unix_us().saturating_sub(REPORT_EVERY.as_micros() as u64);
            self.placed_at.retain(|_, sent| *sent > cutoff);
            self.live_at.retain(|_, opened| *opened > cutoff);
            if self.triggered_by.len() > 10_000 {
                self.triggered_by.clear();
            }
        }
    }

    fn row(&self, out: &mut String, label: &str, hop: &str) {
        if let Some(h) = self.hops.get(hop) {
            let ms = |us: u64| us as f64 / 1000.0;
            let _ = writeln!(
                out,
                "  {:<46} {:>7} {:>9.3} {:>9.3} {:>9.3}",
                label,
                h.len(),
                ms(h.value_at_quantile(0.5)),
                ms(h.value_at_quantile(0.99)),
                ms(h.max())
            );
        }
    }

    /// p50 / p99 / max per hop, in milliseconds.
    pub fn table(&self) -> String {
        let mut out = format!(
            "  {:<46} {:>7} {:>9} {:>9} {:>9}\n",
            "", "count", "p50 ms", "p99 ms", "max ms"
        );
        self.row(&mut out, END_TO_END, END_TO_END);
        for (i, hop) in TRIP.iter().enumerate() {
            self.row(&mut out, &format!("  {} {hop}", i + 1), hop);
        }
        let mut others = self
            .hops
            .keys()
            .filter(|hop| **hop != END_TO_END && !TRIP.contains(hop))
            .peekable();
        if others.peek().is_some() {
            let others: Vec<&str> = others.copied().collect();
            let _ = writeln!(out, "  also measured:");
            for hop in others {
                self.row(&mut out, &format!("  {hop}"), hop);
            }
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

/// `at` on the wall clock, µs since 1970 (to compare with an exchange's clock).
fn wall_clock(at: Instant) -> u64 {
    unix_us().saturating_sub(u64::try_from(at.elapsed().as_micros()).unwrap_or(0))
}

fn unix_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
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

    fn decided(t: &mut Telemetry, client_id: u64, matched: u64) {
        use crate::events::{Intent, Order, Side};
        let tick = Stamp {
            exchange_match_unix_us: Some(matched),
            ..Stamp::at(Instant::now())
        };
        let order = Order {
            side: Side::Buy,
            price: Decimal::ONE,
            size: Decimal::ONE,
            client_id,
        };
        t.on_decision(&[Intent::Place(order)], Some(&tick));
    }

    /// Bullet's websocket reports our order open, stamped `opened` on its clock.
    fn opened(t: &mut Telemetry, client_id: u64, opened: u64) {
        let stamp = Stamp {
            exchange_match_unix_us: Some(opened),
            ..Stamp::at(Instant::now())
        };
        t.on_event(&Event::OrderLive { client_id, stamp }, Instant::now());
    }

    fn sent(t: &mut Telemetry, client_id: u64, started: Instant) {
        t.on_event(
            &Event::Sent {
                kind: SentKind::Place,
                client_ids: vec![client_id],
                started,
                finished: Instant::now(),
            },
            Instant::now(),
        );
    }

    #[test]
    fn end_to_end_runs_from_the_binance_trade_to_bullet_opening_our_order() {
        let mut t = Telemetry::new();
        // Binance's trade at 1.000000 s, Bullet opens our order at 1.015000 s.
        decided(&mut t, 4, 1_000_000);
        opened(&mut t, 4, 1_015_000);
        assert_near(p50_us(&t, END_TO_END), 15_000);
        assert!(
            t.scoreboard().starts_with("SCOREBOARD end-to-end p50 15.0"),
            "{}",
            t.scoreboard()
        );
    }

    #[test]
    fn order_to_live_runs_from_our_request_to_bullet_opening_the_order() {
        let mut t = Telemetry::new();
        let started = Instant::now();
        sent(&mut t, 7, started);
        opened(&mut t, 7, unix_us() + 8_000);
        let p50 = p50_us(&t, ORDER_TO_LIVE);
        // Rebuilding a wall-clock time from an `Instant` can be a microsecond off.
        assert!((7_990..9_000).contains(&p50), "{p50}");
    }

    #[test]
    fn the_echo_can_arrive_before_the_http_response() {
        let mut t = Telemetry::new();
        let started = Instant::now();
        opened(&mut t, 9, unix_us() + 8_000);
        sent(&mut t, 9, started);
        let p50 = p50_us(&t, ORDER_TO_LIVE);
        // Rebuilding a wall-clock time from an `Instant` can be a microsecond off.
        assert!((7_990..9_000).contains(&p50), "{p50}");
    }

    #[test]
    fn the_table_follows_one_trip_in_order() {
        let mut t = Telemetry::new();
        for hop in [
            SUBMIT_PLACE,
            DECIDE,
            ORDER_TO_LIVE,
            TICK_TO_TRADE,
            QUEUE,
            BINANCE_NETWORK,
            BINANCE_MATCH_TO_SEND,
            END_TO_END,
        ] {
            t.record_us(hop, 1_000);
        }
        let table = t.table();
        let at = |label: &str| {
            table
                .find(label)
                .unwrap_or_else(|| panic!("{label}:\n{table}"))
        };
        let order = [
            at(END_TO_END),
            at(&format!("1 {BINANCE_MATCH_TO_SEND}")),
            at(&format!("2 {BINANCE_NETWORK}")),
            at(&format!("3 {QUEUE}")),
            at(&format!("4 {TICK_TO_TRADE}")),
            at(&format!("5 {ORDER_TO_LIVE}")),
            at("also measured"),
            at(SUBMIT_PLACE),
        ];
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{table}");
    }
}
