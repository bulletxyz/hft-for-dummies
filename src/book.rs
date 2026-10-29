//! Our copy of Bullet's order book.
//!
//! Bullet sends a full **snapshot** of the top 20 levels every few seconds,
//! and in between only the **updates**: levels that changed (size `0` means
//! the level is gone). We apply updates to our copy to keep it current.
//!
//! Every message is numbered: `u` is its id and `pu` the id of the one before.
//! If `pu` doesn't match the last `u` we saw, we missed a message and our copy
//! is wrong. Then we throw it away and wait for the next snapshot. A wrong
//! book means wrong prices, and that is worse than no book.

use std::collections::BTreeMap;
use std::str::FromStr;

use bullet_rust_sdk::types::{DepthUpdate, MessageType, PriceLevel};
use rust_decimal::Decimal;

/// Price → size, for each side.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Book {
    pub bids: BTreeMap<Decimal, Decimal>,
    pub asks: BTreeMap<Decimal, Decimal>,
}

impl Book {
    pub fn best_bid(&self) -> Option<Decimal> {
        self.bids.keys().next_back().copied()
    }

    pub fn best_ask(&self) -> Option<Decimal> {
        self.asks.keys().next().copied()
    }

    pub fn mid(&self) -> Option<Decimal> {
        Some((self.best_bid()? + self.best_ask()?) / Decimal::TWO)
    }

    #[cfg(test)]
    pub fn from_levels(bids: &[(Decimal, Decimal)], asks: &[(Decimal, Decimal)]) -> Self {
        Self {
            bids: bids.iter().copied().collect(),
            asks: asks.iter().copied().collect(),
        }
    }
}

/// What happened to the book after one message.
#[derive(Debug)]
pub enum BookEvent {
    /// The book changed; here it is.
    Updated(Book),
    /// We missed a message. Don't trust any book until the next snapshot.
    Lost,
    /// Nothing to report.
    None,
}

/// Keeps our copy of the book in sync, message by message.
#[derive(Debug, Default)]
pub struct BookKeeper {
    /// `None` until the first snapshot, and again after we lose sync.
    book: Option<Book>,
    last_id: u64,
}

impl BookKeeper {
    pub fn on_message(&mut self, msg: &DepthUpdate) -> BookEvent {
        let (Some(bids), Some(asks)) = (parse_levels(&msg.bids), parse_levels(&msg.asks)) else {
            return self.lose_sync("unreadable price level");
        };

        if matches!(msg.msg_type, MessageType::Snapshot) {
            // A snapshot replaces everything (and fixes a lost sync).
            let book = Book {
                bids: bids.into_iter().collect(),
                asks: asks.into_iter().collect(),
            };
            self.book = Some(book.clone());
            self.last_id = msg.last_update_id;
            return BookEvent::Updated(book);
        }

        // An update needs a book to apply to. Before the first snapshot there isn't one.
        let Some(book) = self.book.as_mut() else {
            return BookEvent::None;
        };
        if msg.prev_update_id != self.last_id {
            return self.lose_sync("missed a message");
        }
        for (side, changes) in [(&mut book.bids, bids), (&mut book.asks, asks)] {
            for (price, size) in changes {
                if size.is_zero() {
                    side.remove(&price);
                } else {
                    side.insert(price, size);
                }
            }
        }
        self.last_id = msg.last_update_id;
        BookEvent::Updated(book.clone())
    }

    /// The connection dropped: forget everything, a fresh snapshot is coming.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn lose_sync(&mut self, why: &str) -> BookEvent {
        if self.book.take().is_some() {
            tracing::warn!("bullet: order book out of sync ({why}), waiting for the next snapshot");
            BookEvent::Lost
        } else {
            BookEvent::None
        }
    }
}

/// `None` if any level fails to parse: half a book is a wrong book.
fn parse_levels(levels: &[PriceLevel]) -> Option<Vec<(Decimal, Decimal)>> {
    levels
        .iter()
        .map(|PriceLevel(price, size)| {
            Some((
                Decimal::from_str(price).ok()?,
                Decimal::from_str(size).ok()?,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn msg(
        kind: &str,
        id: u64,
        prev: u64,
        bids: &[(&str, &str)],
        asks: &[(&str, &str)],
    ) -> DepthUpdate {
        let levels = |v: &[(&str, &str)]| {
            v.iter()
                .map(|(p, q)| [p.to_string(), q.to_string()])
                .collect::<Vec<_>>()
        };
        serde_json::from_value(serde_json::json!({
            "e": "depthUpdate", "E": 1, "T": 1, "s": "SOL-USD",
            "U": id, "u": id, "pu": prev, "b": levels(bids), "a": levels(asks), "mt": kind,
        }))
        .unwrap()
    }

    fn snapshot(id: u64) -> DepthUpdate {
        msg(
            "s",
            id,
            0,
            &[("100", "1"), ("99", "2")],
            &[("101", "1"), ("102", "2")],
        )
    }

    fn updated(event: BookEvent) -> Book {
        match event {
            BookEvent::Updated(book) => book,
            other => panic!("expected an updated book, got {other:?}"),
        }
    }

    #[test]
    fn snapshot_gives_a_book() {
        let book = updated(BookKeeper::default().on_message(&snapshot(1)));
        assert_eq!(
            (book.best_bid(), book.best_ask()),
            (Some(d("100")), Some(d("101")))
        );
        assert_eq!(book.mid(), Some(d("100.5")));
    }

    #[test]
    fn updates_change_and_remove_levels() {
        let mut keeper = BookKeeper::default();
        keeper.on_message(&snapshot(1));
        // Best bid leaves (size 0), a new bid arrives, the best ask grows.
        let book = updated(keeper.on_message(&msg(
            "u",
            2,
            1,
            &[("100", "0"), ("98", "5")],
            &[("101", "7")],
        )));
        assert_eq!(book.best_bid(), Some(d("99")));
        assert_eq!(book.bids.get(&d("98")), Some(&d("5")));
        assert_eq!(book.asks.get(&d("101")), Some(&d("7")));
    }

    #[test]
    fn a_missed_message_drops_the_book_until_the_next_snapshot() {
        let mut keeper = BookKeeper::default();
        keeper.on_message(&snapshot(1));
        // Message 2 never arrived; message 3 says its predecessor was 2.
        assert!(matches!(
            keeper.on_message(&msg("u", 3, 2, &[], &[])),
            BookEvent::Lost
        ));
        assert!(matches!(
            keeper.on_message(&msg("u", 4, 3, &[], &[])),
            BookEvent::None
        ));
        updated(keeper.on_message(&snapshot(10)));
        updated(keeper.on_message(&msg("u", 11, 10, &[], &[])));
    }

    #[test]
    fn updates_before_the_first_snapshot_are_ignored() {
        let mut keeper = BookKeeper::default();
        assert!(matches!(
            keeper.on_message(&msg("u", 5, 4, &[("100", "1")], &[])),
            BookEvent::None
        ));
    }

    #[test]
    fn an_unreadable_level_drops_the_book() {
        let mut keeper = BookKeeper::default();
        keeper.on_message(&snapshot(1));
        assert!(matches!(
            keeper.on_message(&msg("u", 2, 1, &[("oops", "1")], &[])),
            BookEvent::Lost
        ));
    }
}
