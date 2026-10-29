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
        received_at: Instant,
    },
    /// Bullet's order book changed.
    Book(Book),
    /// Our copy of the Bullet book is out of sync; don't trust it.
    BookLost,
    /// One of our orders (partly) filled.
    Fill(Fill),
    /// One of our orders left the book without filling (cancelled or rejected).
    OrderGone { client_id: u64 },
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
    Cancel {
        client_id: u64,
    },
    /// Cancel every order we have on the market, tracked or not.
    CancelAll,
    /// Ask the exchange for our position (comes back as [`Event::Position`]).
    SyncPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Order {
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    /// Our own id for the order, so we can recognise its fills and cancel it.
    pub client_id: u64,
}
