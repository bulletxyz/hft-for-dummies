//! The two kinds of message the whole bot is built from.
//!
//! **Events** flow *in*: everything that happens (a Binance price, a Bullet
//! book update, a fill, a timer tick) becomes an [`Event`] on one queue, the
//! *bus*. Feeds only ever write to the bus; they never call the strategy.
//!
//! **Intents** flow *out*: the strategy answers events with [`Intent`]s
//! ("place this order", "cancel that one"). It never talks to an exchange
//! itself; an *executor* carries the intents out and reports what happened
//! back as more events.
//!
//! Keeping these apart is what lets each piece be swapped or sped up on its
//! own: dry run is just a different executor, and later posts move feeds and
//! the executor onto their own threads without touching the strategy.

use std::sync::Arc;
use std::time::Instant;

use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::book::Book;

/// Where events are sent. Every feed holds a clone.
pub type Bus = mpsc::UnboundedSender<Event>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug)]
pub enum Event {
    /// Binance's best bid and ask (our price reference).
    Reference {
        bid: Decimal,
        ask: Decimal,
        stamp: Stamp,
    },
    /// Bullet's order book changed.
    Book(Arc<Book>, Stamp),
    /// Our copy of the Bullet book is out of sync; don't trust it.
    BookLost,
    /// One of our orders (partly) filled.
    Fill(Fill),
    /// One of our orders left the book without filling (cancelled or rejected).
    OrderGone { client_id: u64 },
    /// The exchange confirmed one of our orders is resting on the book.
    OrderLive { client_id: u64, stamp: Stamp },
    /// The executor sent a request and the exchange answered (for telemetry).
    Sent {
        kind: SentKind,
        started: Instant,
        finished: Instant,
    },
    /// The executor could not place these orders.
    PlaceFailed { client_ids: Vec<u64> },
    /// The executor could not cancel an order, so it may still be resting.
    CancelFailed,
    /// Our position according to the exchange (answer to [`Intent::SyncPosition`]).
    Position(Decimal),
    /// The Bullet websocket dropped and is reconnecting. We may have missed things.
    Reconnecting,
    /// The Bullet websocket is gone for good.
    Disconnected(String),
    /// Once a second, so time-based checks (staleness) run even when the market is quiet.
    Tick,
    /// Ctrl-C.
    Shutdown,
}

/// When a market event happened and when we got it.
#[derive(Debug, Clone, Copy)]
pub struct Stamp {
    /// When we received it, on our monotonic clock (for measuring durations).
    pub received_at: Instant,
    /// The same moment on the wall clock, µs since 1970 (to compare with exchange clocks).
    pub received_unix_us: u64,
    /// When the exchange says it sent the message, µs since 1970.
    pub exchange_unix_us: Option<u64>,
    /// When the exchange says the underlying trade or book change happened.
    pub exchange_match_unix_us: Option<u64>,
}

impl Stamp {
    /// Stamp a message arriving now.
    pub fn now(exchange_unix_us: Option<u64>, exchange_match_unix_us: Option<u64>) -> Self {
        let received_unix_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_micros() as u64);
        Self {
            received_at: Instant::now(),
            received_unix_us,
            exchange_unix_us,
            exchange_match_unix_us,
        }
    }

    /// A stamp with only our receive time (tests).
    #[cfg(test)]
    pub fn at(received_at: Instant) -> Self {
        Self {
            received_at,
            received_unix_us: 0,
            exchange_unix_us: None,
            exchange_match_unix_us: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SentKind {
    Place,
    Replace,
    Cancel,
}

#[derive(Debug, Clone)]
pub struct Fill {
    /// The exchange's id for this fill (used to never count one twice).
    pub trade_id: u64,
    pub client_id: Option<u64>,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    pub fee: Decimal,
    /// The whole order is done and no longer rests on the book.
    pub order_finished: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    Place(Order),
    /// Swap a resting order for a new one in a single request.
    Replace {
        old: u64,
        new: Order,
    },
    Cancel {
        client_id: u64,
    },
    /// Cancel every order we have on the market, tracked or not.
    CancelAll,
    /// Ask the exchange for our position (comes back as [`Event::Position`]).
    SyncPosition,
    /// Ask the exchange for our fills since startup (they come back as
    /// [`Event::Fill`]s; ones we already have are ignored). The websocket can
    /// miss a fill; the exchange's own record can't.
    SyncFills,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Order {
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    /// Our own id for the order, so we can recognise its fills and cancel it.
    pub client_id: u64,
}
