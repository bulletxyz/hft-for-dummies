//! Microbenchmarks for the code on the tick-to-trade path: `cargo bench`.
//!
//! Each one times a single step thousands of times and reports a typical
//! duration, so a change to that step shows up even when it's far too small
//! to see in the live telemetry.

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use hft_for_dummies::binance;
use hft_for_dummies::book::{BookEvent, BookKeeper};
use hft_for_dummies::config::Config;
use hft_for_dummies::events::{Event, Stamp};
use hft_for_dummies::market_maker::MarketMaker;
use rust_decimal::Decimal;

/// A real Binance bookTicker message.
const TICKER: &str = r#"{"e":"bookTicker","u":9127380911467,"s":"SOLUSDT","b":"100.6300","B":"431.75","a":"100.6400","A":"88.26","T":1793276669648,"E":1793276669650}"#;

/// A 20-level snapshot, then a delta that moves a few levels, like Bullet sends.
fn depth(kind: &str, id: u64, prev: u64, levels: usize) -> bullet_rust_sdk::types::DepthUpdate {
    let side = |start: f64, step: f64| {
        (0..levels)
            .map(|i| [format!("{:.3}", start + step * i as f64), "12.5".into()])
            .collect::<Vec<_>>()
    };
    serde_json::from_value(serde_json::json!({
        "e": "depthUpdate", "E": 1, "T": 1, "s": "SOL-USD", "U": id, "u": id, "pu": prev,
        "b": side(100.600, -0.001), "a": side(100.610, 0.001), "mt": kind,
    }))
    .unwrap()
}

fn stamp(at: Instant) -> Stamp {
    Stamp {
        received_at: at,
        received_unix_us: 0,
        exchange_unix_us: None,
        exchange_match_unix_us: None,
    }
}

fn parse_ticker(c: &mut Criterion) {
    c.bench_function("parse a Binance bookTicker", |b| {
        b.iter(|| binance::parse(black_box(TICKER)).unwrap())
    });
}

fn apply_book_update(c: &mut Criterion) {
    let mut keeper = BookKeeper::default();
    keeper.on_message(&depth("s", 1, 0, 20));
    let mut delta = depth("u", 2, 1, 3);
    c.bench_function("apply a Bullet book update", |b| {
        b.iter(|| {
            let event = keeper.on_message(black_box(&delta));
            delta.prev_update_id = delta.last_update_id;
            delta.last_update_id += 1;
            event
        })
    });
}

fn decide(c: &mut Criterion) {
    // A market maker that has watched the market long enough to quote.
    let config = Config::load("bot.toml".as_ref()).unwrap();
    let mut mm = MarketMaker::new(config, Decimal::new(1, 3));
    let mut keeper = BookKeeper::default();
    let BookEvent::Updated(book) = keeper.on_message(&depth("s", 1, 0, 20)) else {
        panic!()
    };
    let start = Instant::now();
    for s in 0..=61 {
        let t = start + Duration::from_secs(s);
        mm.on_event(
            &Event::Reference {
                bid: Decimal::new(100_600, 3),
                ask: Decimal::new(100_610, 3),
                stamp: stamp(t),
            },
            t,
        );
        mm.on_event(&Event::Book(book.clone(), stamp(t)), t);
    }
    let now = start + Duration::from_secs(61);
    let tick = Event::Reference {
        bid: Decimal::new(100_600, 3),
        ask: Decimal::new(100_610, 3),
        stamp: stamp(now),
    };
    c.bench_function("decide: handle a tick and requote", |b| {
        b.iter(|| {
            mm.on_event(black_box(&tick), now);
            mm.requote(now)
        })
    });
}

criterion_group!(benches, parse_ticker, apply_book_update, decide);
criterion_main!(benches);
