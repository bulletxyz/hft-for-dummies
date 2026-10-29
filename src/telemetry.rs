//! Telemetry: how long each hop of the loop takes, from a Binance update to
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
//! Hops that compare our clock with an exchange's are only as accurate as the
//! two clocks agree: within microseconds on a synced server, but often hundreds
//! of milliseconds off on a laptop. Hops measured on our own clock are exact.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use hdrhistogram::Histogram;
use metrics_exporter_prometheus::PrometheusBuilder;

use crate::events::{Event, Stamp};

/// How often to print the table.
const REPORT_EVERY: Duration = Duration::from_secs(30);

/// The headline: from the Binance update that moved our price until Bullet
/// opens our new order. How long our quote lags the market.
pub const END_TO_END: &str = "END-TO-END: Binance update → our order open";

// The steps of that trip, in order. They roughly add up to it.
pub const BINANCE_MATCH_TO_SEND: &str = "inside Binance (match → sent)";
pub const BINANCE_NETWORK: &str = "binance→us (network)";
pub const QUEUE: &str = "queue (received → handled)";
/// Our reaction: from picking up the Binance update to deciding on orders.
pub const TICK_TO_TRADE: &str = "tick-to-trade (our code)";
/// From that decision until Bullet's matching engine opens the order (its own
/// timestamp): sending it, the route there and Bullet's processing.
pub const ORDER_TO_LIVE: &str = "order-to-live (send + route + Bullet)";
const TRIP: [&str; 5] = [
    BINANCE_MATCH_TO_SEND,
    BINANCE_NETWORK,
    QUEUE,
    TICK_TO_TRADE,
    ORDER_TO_LIVE,
];

// Also measured, overlapping the trip or off it.
pub const DECIDE: &str = "decide (every requote)";
pub const SUBMIT_PLACE: &str = "Bullet's reply to a place (round trip)";
pub const SUBMIT_CANCEL: &str = "Bullet's reply to a cancel (round trip)";
pub const SUBMIT_REPLACE: &str = "Bullet's reply to a replace (round trip)";
pub const BULLET_NETWORK: &str = "Bullet book → us (network)";

pub struct Telemetry {
    /// Microseconds, per hop.
    hops: BTreeMap<&'static str, Histogram<u64>>,
    /// For each order a Binance update made us place, until Bullet opens it:
    /// when we decided (µs since 1970, our clock) and when that update
    /// happened (Binance's clock).
    decided: HashMap<u64, Decision>,
    /// Times that can't be true unless a clock is off, and by how much at worst.
    our_clock_behind: Skew,
    our_clock_ahead: Skew,
    exchanges_disagree: Skew,
    /// Order transactions sent to Bullet, to compare with the rate limit.
    requests: u64,
    started: Instant,
    last_report: Instant,
}

impl Telemetry {
    pub fn new() -> Self {
        Self {
            hops: BTreeMap::new(),
            decided: HashMap::new(),
            our_clock_behind: Skew::default(),
            our_clock_ahead: Skew::default(),
            exchanges_disagree: Skew::default(),
            requests: 0,
            started: Instant::now(),
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
        let micros = micros.max(1); // a log-scale chart can't show zero
        hist.saturating_record(micros);
        // Also to Prometheus, if `metrics_port` is set (a no-op otherwise).
        metrics::histogram!("hft_hop_seconds", "hop" => hop).record(micros as f64 / 1e6);
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
                started,
                finished,
                ..
            } => {
                self.requests += 1;
                let hop = match kind {
                    crate::events::SentKind::Place => SUBMIT_PLACE,
                    crate::events::SentKind::Replace => SUBMIT_REPLACE,
                    crate::events::SentKind::Cancel => SUBMIT_CANCEL,
                };
                self.record(hop, finished.saturating_duration_since(*started));
            }
            Event::OrderLive { client_id, stamp } => {
                // Bullet's matching-engine timestamp for the order opening.
                let Some(opened) = stamp.exchange_match_unix_us.or(stamp.exchange_unix_us) else {
                    return;
                };
                if let Some(decision) = self.decided.remove(client_id) {
                    self.order_opened(decision, opened, stamp.received_unix_us);
                }
            }
            _ => {}
        }
    }

    /// The bot decided, at `decided`, to send these orders. `tick` is the
    /// Binance update that moved our price, if one did. Only those orders are
    /// timed, so end-to-end and its steps always describe the same orders.
    pub fn on_decision(
        &mut self,
        intents: &[crate::events::Intent],
        tick: Option<&Stamp>,
        picked_up: Instant,
        decided: Instant,
    ) {
        let Some(tick) = tick else { return };
        let Some(matched_us) = tick.exchange_match_unix_us.or(tick.exchange_unix_us) else {
            return;
        };
        let decision = Decision {
            matched_us,
            sent_us: tick.exchange_unix_us.unwrap_or(matched_us),
            received_us: tick.received_unix_us,
            handled_us: wall_clock(picked_up),
            decided_us: wall_clock(decided),
        };
        for intent in intents {
            if let crate::events::Intent::Place(order)
            | crate::events::Intent::Replace { new: order, .. } = intent
            {
                self.decided.insert(order.client_id, decision);
            }
        }
    }

    /// Bullet opened an order at `opened` (its clock), and we heard about it at
    /// `heard` (our clock).
    fn order_opened(&mut self, decision: Decision, opened: u64, heard: u64) {
        // Binance's clock against Bullet's: ours plays no part.
        if opened < decision.matched_us {
            self.exchanges_disagree.saw(decision.matched_us - opened);
        } else {
            self.record_us(END_TO_END, opened - decision.matched_us);
        }
        // Bullet must open the order after we decided and before we hear it did.
        // Outside that window, our clock disagrees with Bullet's.
        if opened > heard {
            self.our_clock_behind.saw(opened - heard);
        } else if opened < decision.decided_us {
            self.our_clock_ahead.saw(decision.decided_us - opened);
        } else {
            self.record_us(ORDER_TO_LIVE, opened - decision.decided_us);
            // This order's own trip, for Grafana: its steps add up to its
            // end-to-end exactly, where averages over all messages don't.
            for (step, us) in TRIP.iter().zip(decision.steps(opened)) {
                metrics::histogram!("hft_trip_seconds", "step" => *step).record(us as f64 / 1e6);
            }
            metrics::histogram!("hft_trip_seconds", "step" => END_TO_END)
                .record(opened.saturating_sub(decision.matched_us) as f64 / 1e6);
        }
    }

    /// One line to compare runs and parts: the series' scoreboard.
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
            "SCOREBOARD end-to-end p50 {} ms p99 {} ms | binance→us p50 {} ms | \
             tick-to-trade p50 {} µs p99 {} µs | order-to-live p50 {} ms p99 {} ms | \
             queue p99 {} ms max {} ms | orders {}",
            ms(END_TO_END, 0.5),
            ms(END_TO_END, 0.99),
            ms(BINANCE_NETWORK, 0.5),
            us(TICK_TO_TRADE, 0.5),
            us(TICK_TO_TRADE, 0.99),
            ms(ORDER_TO_LIVE, 0.5),
            ms(ORDER_TO_LIVE, 0.99),
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
            self.our_clock_behind.saw(sent - stamp.received_unix_us);
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
            self.decided.retain(|_, d| d.decided_us > cutoff);
        }
    }

    /// One row; a hop with no samples yet shows dashes, so the trip's shape
    /// is always visible.
    fn row(&self, out: &mut String, label: &str, hop: &str) {
        let _ = match self.hops.get(hop) {
            Some(h) => {
                let ms = |us: u64| us as f64 / 1000.0;
                writeln!(
                    out,
                    "  {:<46} {:>7} {:>9.3} {:>9.3} {:>9.3}",
                    label,
                    h.len(),
                    ms(h.value_at_quantile(0.5)),
                    ms(h.value_at_quantile(0.99)),
                    ms(h.max())
                )
            }
            None => writeln!(
                out,
                "  {label:<46} {:>7} {:>9} {:>9} {:>9}",
                0, "-", "-", "-"
            ),
        };
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
        if !self.hops.contains_key(END_TO_END) {
            let _ = writeln!(
                out,
                "  (no orders have reached Bullet yet: end-to-end and order-to-live start with \
                 the first live order)"
            );
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
        if self.requests > 0 {
            let secs = self.started.elapsed().as_secs_f64().max(1.0);
            let _ = writeln!(
                out,
                "  order requests sent: {} ({:.2} per second)",
                self.requests,
                self.requests as f64 / secs
            );
        }
        let sync = "Fix the clock (macOS: turn automatic time off and on; Linux: chrony), or trust end-to-end, which doesn't use it.";
        for (skew, what) in [
            (&self.our_clock_behind, "your clock is behind an exchange's"),
            (&self.our_clock_ahead, "your clock is ahead of Bullet's"),
        ] {
            if skew.count > 0 {
                let _ = writeln!(
                    out,
                    "  ! {} times {what} by at least {:.0} ms, so the rows that compare\n    \
                     clocks are wrong. {sync}",
                    skew.count,
                    skew.max_us as f64 / 1000.0
                );
            }
        }
        if self.exchanges_disagree.count > 0 {
            let _ = writeln!(
                out,
                "  ! {} times Binance's and Bullet's clocks disagree by at least {:.0} ms \
                 (not yours): end-to-end skipped those.",
                self.exchanges_disagree.count,
                self.exchanges_disagree.max_us as f64 / 1000.0
            );
        }
        out
    }
}

/// One order's trip, each moment in µs since 1970: Binance's match and send
/// (its clock), our receiving, handling and deciding (ours).
#[derive(Clone, Copy)]
struct Decision {
    matched_us: u64,
    sent_us: u64,
    received_us: u64,
    handled_us: u64,
    decided_us: u64,
}

impl Decision {
    /// The five steps of this order's trip, ending at `opened` (Bullet's clock).
    /// Each starts where the last ended, so they sum to `opened - matched_us`.
    fn steps(&self, opened: u64) -> [u64; 5] {
        let moments = [
            self.matched_us,
            self.sent_us,
            self.received_us,
            self.handled_us,
            self.decided_us,
            opened,
        ];
        std::array::from_fn(|i| moments[i + 1].saturating_sub(moments[i]))
    }
}

/// How often two clocks disagreed, and by how much at worst.
#[derive(Default)]
struct Skew {
    count: u64,
    max_us: u64,
}

impl Skew {
    fn saw(&mut self, by_us: u64) {
        self.count += 1;
        self.max_us = self.max_us.max(by_us);
    }
}

/// Serve every hop to Prometheus at `http://127.0.0.1:{port}/metrics`, as
/// p50 and p99 over the last five minutes (so a tail has enough samples), plus
/// a running sum and count for averages, for Grafana to draw (`infra/grafana`).
pub fn serve_metrics(port: u16) -> anyhow::Result<()> {
    PrometheusBuilder::new()
        .with_http_listener(([127, 0, 0, 1], port))
        .set_quantiles(&[0.5, 0.99])?
        .set_bucket_duration(Duration::from_secs(60))?
        .set_bucket_count(NonZeroU32::new(5).expect("5 is not zero"))
        .install()?;
    tracing::info!("metrics for Grafana at http://127.0.0.1:{port}/metrics");
    Ok(())
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
            t.table()
                .contains("your clock is behind an exchange's by at least 5 ms"),
            "{}",
            t.table()
        );
    }

    fn order(client_id: u64) -> crate::events::Intent {
        use crate::events::{Intent, Order, Side};
        Intent::Place(Order {
            side: Side::Buy,
            price: Decimal::ONE,
            size: Decimal::ONE,
            client_id,
        })
    }

    /// We decided, at `decided` (our clock), to place `client_id` because of
    /// the Binance update stamped `matched` (Binance's clock).
    fn decided(t: &mut Telemetry, client_id: u64, matched: u64, decided: Instant) {
        let tick = Stamp {
            exchange_match_unix_us: Some(matched),
            ..Stamp::at(decided)
        };
        t.on_decision(&[order(client_id)], Some(&tick), decided, decided);
    }

    /// Bullet opened the order at `opened` (its clock); its confirmation
    /// reached us at `received` (our clock).
    fn opened(t: &mut Telemetry, client_id: u64, opened: u64, received: u64) {
        let stamp = Stamp {
            received_unix_us: received,
            exchange_match_unix_us: Some(opened),
            ..Stamp::at(Instant::now())
        };
        t.on_event(&Event::OrderLive { client_id, stamp }, Instant::now());
    }

    #[test]
    fn end_to_end_runs_from_the_binance_update_to_bullet_opening_our_order() {
        let mut t = Telemetry::new();
        let now = unix_us();
        // Binance's update 15 ms before Bullet opened our order.
        decided(&mut t, 4, now - 5_000, Instant::now());
        opened(&mut t, 4, now + 10_000, now + 12_000);
        assert_near(p50_us(&t, END_TO_END), 15_000);
        assert!(
            t.scoreboard().starts_with("SCOREBOARD end-to-end p50 15.0"),
            "{}",
            t.scoreboard()
        );
    }

    #[test]
    fn order_to_live_runs_from_our_decision_to_bullet_opening_the_order() {
        let mut t = Telemetry::new();
        let now = unix_us();
        decided(&mut t, 7, now - 1_000, Instant::now());
        // Opened 8 ms after we decided; its confirmation reached us 2 ms later.
        opened(&mut t, 7, now + 8_000, now + 10_000);
        let p50 = p50_us(&t, ORDER_TO_LIVE);
        // Rebuilding a wall-clock time from an `Instant` can be a microsecond off.
        assert!((7_990..9_000).contains(&p50), "{p50}");
    }

    #[test]
    fn orders_not_caused_by_a_binance_update_are_not_timed() {
        // A requote after a fill or a Bullet book change: no Binance update to
        // time from, so neither end-to-end nor order-to-live counts it, and the
        // two always describe the same orders.
        let mut t = Telemetry::new();
        let now = unix_us();
        t.on_decision(&[order(11)], None, Instant::now(), Instant::now());
        opened(&mut t, 11, now + 8_000, now + 10_000);
        assert!(!t.hops.contains_key(ORDER_TO_LIVE));
        assert!(!t.hops.contains_key(END_TO_END));
    }

    #[test]
    fn one_orders_steps_add_up_to_its_end_to_end_exactly() {
        let d = Decision {
            matched_us: 1_000_000,
            sent_us: 1_000_400,
            received_us: 1_039_000,
            handled_us: 1_039_600,
            decided_us: 1_039_608,
        };
        let steps = d.steps(1_250_000);
        assert_eq!(steps, [400, 38_600, 600, 8, 210_392]);
        assert_eq!(steps.iter().sum::<u64>(), 1_250_000 - 1_000_000);
    }

    #[test]
    fn an_order_opened_after_we_heard_about_it_means_our_clock_is_behind() {
        let mut t = Telemetry::new();
        let now = unix_us();
        decided(&mut t, 8, now - 1_000, Instant::now());
        // Bullet says it opened the order 500 ms after we heard it had: impossible
        // unless our clock is 500 ms behind Bullet's.
        opened(&mut t, 8, now + 510_000, now + 10_000);
        assert!(!t.hops.contains_key(ORDER_TO_LIVE), "not a latency");
        assert!(t.table().contains("your clock is behind"), "{}", t.table());
    }

    #[test]
    fn an_order_opened_before_we_decided_means_our_clock_is_ahead() {
        let mut t = Telemetry::new();
        let now = unix_us();
        decided(&mut t, 9, now - 600_000, Instant::now());
        opened(&mut t, 9, now - 300_000, now - 299_000);
        assert!(!t.hops.contains_key(ORDER_TO_LIVE), "not a latency");
        assert!(t.table().contains("your clock is ahead"), "{}", t.table());
    }

    #[test]
    fn exchanges_disagreeing_is_not_blamed_on_our_clock() {
        let mut t = Telemetry::new();
        let now = unix_us();
        // Bullet's stamp is earlier than Binance's: their clocks disagree.
        decided(&mut t, 10, now + 50_000, Instant::now());
        opened(&mut t, 10, now + 5_000, now + 6_000);
        assert!(!t.hops.contains_key(END_TO_END));
        let table = t.table();
        assert!(
            table.contains("Binance's and Bullet's clocks disagree"),
            "{table}"
        );
        assert!(!table.contains("your clock is behind"), "{table}");
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

    #[test]
    fn the_whole_trip_shows_before_any_order_reaches_bullet() {
        let mut t = Telemetry::new();
        t.record_us(BINANCE_NETWORK, 40_000);
        let table = t.table();
        for hop in [END_TO_END, TICK_TO_TRADE, ORDER_TO_LIVE] {
            assert!(table.contains(hop), "{hop} missing:\n{table}");
        }
        assert!(
            table.contains("no orders have reached Bullet yet"),
            "{table}"
        );
    }
}
