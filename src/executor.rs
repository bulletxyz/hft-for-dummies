//! Executors carry out the strategy's intents and report back as events.
//!
//! - [`LiveExecutor`] sends real orders to Bullet.
//! - [`PaperExecutor`] pretends to (dry run): it remembers the orders and
//!   fills them when the real Bullet book trades through their price.
//!
//! The strategy can't tell the two apart, so what you see in dry run is the
//! same code path that trades live.
//!
//! Handing intents to an executor never waits: the live one passes them to a
//! worker task and returns at once, so the bot keeps reading prices while a
//! request to Bullet is in flight. Results come back as events.

use std::time::Instant;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::bullet::{self, Bullet, Reject};
use crate::events::{Bus, Event, Fill, Intent, Order, SentKind, Side};
use crate::rate_limit::TokenBucket;

pub trait Executor {
    /// Queue `intents`. Returns immediately.
    fn execute(&mut self, intents: Vec<Intent>);

    /// See every event too (the paper executor uses the book to fill orders).
    fn on_event(&mut self, _event: &Event) {}

    /// Wait for everything queued to be done (on shutdown).
    async fn finish(self);
}

/// Sends intents to a worker task that talks to Bullet, one batch at a time.
pub struct LiveExecutor {
    queue: mpsc::UnboundedSender<Vec<Intent>>,
    worker: JoinHandle<()>,
}

impl LiveExecutor {
    pub fn new(bullet: Bullet, bus: Bus, max_requests_per_sec: f64) -> Self {
        let (queue, mut batches) = mpsc::unbounded_channel::<Vec<Intent>>();
        // A one-second burst, a quarter of it kept for cancels.
        let mut worker = Worker {
            bullet,
            bus,
            budget: TokenBucket::new(max_requests_per_sec, max_requests_per_sec),
            reserve: (max_requests_per_sec / 4.0).max(1.0),
            fills_since_us: None,
        };
        let worker = tokio::spawn(async move {
            // One batch at a time, in order: a cancel always lands before the
            // order that follows it.
            while let Some(batch) = batches.recv().await {
                worker.run(batch).await;
            }
        });
        Self { queue, worker }
    }
}

impl Executor for LiveExecutor {
    fn execute(&mut self, intents: Vec<Intent>) {
        if !intents.is_empty() {
            let _ = self.queue.send(intents);
        }
    }

    async fn finish(self) {
        drop(self.queue); // no more work: the worker stops once the queue is empty
        let _ = self.worker.await;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Priority {
    Cancel,
    NewOrder,
}

struct Worker {
    bullet: Bullet,
    bus: Bus,
    /// Our own cap on transactions per second (see `max_requests_per_sec`).
    budget: TokenBucket,
    /// Tokens new orders must leave for cancels.
    reserve: f64,
    /// Fills from this time on (µs since 1970) are ours to count. Set when we
    /// read the starting position: everything before is already in it.
    fills_since_us: Option<u64>,
}

impl Worker {
    async fn run(&mut self, intents: Vec<Intent>) {
        let mut cancels = Vec::new();
        let mut replaces = Vec::new();
        let mut places = Vec::new();
        for intent in intents {
            match intent {
                Intent::Place(order) => places.push(order),
                Intent::Replace { old, new } => replaces.push((old, new)),
                Intent::Cancel { client_id } => cancels.push(client_id),
                Intent::CancelAll => {
                    self.spend(Priority::Cancel).await;
                    if let Err(e) = self.bullet.cancel_all().await {
                        tracing::error!(
                            "cancel all failed, check the Bullet app for resting orders: {e}"
                        );
                    }
                }
                Intent::SyncPosition => match self.bullet.position().await {
                    Ok(position) => {
                        // Fills before this read are already in `position`. Our
                        // clock may be off from the exchange's, so start counting
                        // 10s later: nothing of ours can fill that early anyway,
                        // since we cancelled everything and the bot spends its
                        // first minute warming up before it quotes.
                        self.fills_since_us
                            .get_or_insert_with(|| unix_us() + 10_000_000);
                        self.send(Event::Position(position));
                    }
                    Err(e) => tracing::error!("reading our position failed: {e}"),
                },
                Intent::SyncFills => {
                    let Some(since) = self.fills_since_us else {
                        continue;
                    };
                    match self.bullet.fills_since(since).await {
                        Ok(fills) => fills.into_iter().for_each(|f| self.send(Event::Fill(f))),
                        Err(e) => tracing::warn!("checking our fills failed (will retry): {e}"),
                    }
                }
            }
        }

        // Everything goes in one request: cancels (including the old side of
        // each moved quote) and new orders. A cancel for an order that filled
        // just before it arrived is skipped by Bullet, so that race needs no
        // special handling.
        let moved = !replaces.is_empty();
        cancels.extend(replaces.iter().map(|(old, _)| *old));
        places.extend(replaces.into_iter().map(|(_, new)| new));
        if places.is_empty() {
            return self.cancel_only(cancels).await;
        }
        let new_ids: Vec<u64> = places.iter().map(|o| o.client_id).collect();
        if !self.spend(Priority::NewOrder).await {
            return self
                .abandon(cancels, new_ids, "rate budget spent, holding back")
                .await;
        }
        let kind = if moved {
            SentKind::Replace
        } else {
            SentKind::Place
        };
        let Err(e) = self
            .timed(kind, self.bullet.cancel_and_place(&cancels, &places))
            .await
        else {
            return;
        };
        // A rejected request did nothing: the old orders still rest.
        match bullet::classify(&e) {
            // The price moved between pricing and arriving: normal in a fast market.
            Reject::WouldCross => {
                self.abandon(cancels, new_ids, "post-only would cross")
                    .await
            }
            Reject::RateLimited => {
                self.budget.empty();
                self.abandon(cancels, new_ids, "rate limited").await;
            }
            Reject::Other => self.give_up(&e, new_ids),
        }
    }

    /// Cancels take risk off, so they may use the tokens we keep in reserve,
    /// and wait for one if the budget is empty.
    async fn cancel_only(&mut self, cancels: Vec<u64>) {
        if cancels.is_empty() {
            return;
        }
        loop {
            self.spend(Priority::Cancel).await;
            let request = self.bullet.cancel_and_place(&cancels, &[]);
            match self.timed(SentKind::Cancel, request).await {
                Ok(()) => return,
                Err(e) if bullet::classify(&e) == Reject::RateLimited => self.budget.empty(), // wait, then try again
                Err(e) => return self.give_up(&e, Vec::new()),
            }
        }
    }

    /// Spend one request from the budget. Cancels wait for a token; new
    /// orders don't (a quote that waits is stale anyway) and leave the reserve.
    async fn spend(&mut self, priority: Priority) -> bool {
        let keep = match priority {
            Priority::Cancel => 0.0,
            Priority::NewOrder => self.reserve,
        };
        loop {
            let now = Instant::now();
            if self.budget.try_take(now, keep) {
                return true;
            }
            if priority == Priority::NewOrder {
                return false;
            }
            tokio::time::sleep(self.budget.wait_time(now)).await;
        }
    }

    /// Don't place these new orders: cancel the old ones (they may still be
    /// resting) and tell the strategy the new ones never went out.
    async fn abandon(&mut self, cancels: Vec<u64>, new_ids: Vec<u64>, why: &str) {
        tracing::info!("{why}; pulling the quotes we meant to move");
        self.cancel_only(cancels).await;
        self.send(Event::PlaceFailed {
            client_ids: new_ids,
        });
    }

    /// Something failed that we can't untangle: tell the strategy, which
    /// cancels everything and starts its quotes afresh.
    fn give_up(&self, e: &anyhow::Error, new_ids: Vec<u64>) {
        tracing::warn!("sending orders failed (often: not enough margin): {e}");
        self.send(Event::CancelFailed);
        self.send(Event::PlaceFailed {
            client_ids: new_ids,
        });
    }

    fn send(&self, event: Event) {
        let _ = self.bus.send(event);
    }

    /// Run one request to the exchange and report how long it took.
    async fn timed<T>(
        &self,
        kind: SentKind,
        request: impl Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        let started = Instant::now();
        let result = request.await;
        if result.is_ok() {
            self.send(Event::Sent {
                kind,
                started,
                finished: Instant::now(),
            });
        }
        result
    }
}

fn unix_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}

/// Dry run: orders live only in memory.
pub struct PaperExecutor {
    resting: Vec<Order>,
    bus: Bus,
    next_trade_id: u64,
}

impl PaperExecutor {
    pub fn new(bus: Bus) -> Self {
        Self {
            resting: Vec::new(),
            bus,
            next_trade_id: 0,
        }
    }
}

impl Executor for PaperExecutor {
    async fn finish(self) {}

    fn execute(&mut self, intents: Vec<Intent>) {
        for intent in intents {
            match intent {
                Intent::Place(order) => {
                    tracing::info!(side = ?order.side, price = %order.price, size = %order.size, "paper: place");
                    self.resting.push(order);
                }
                Intent::Replace { old, new } => {
                    tracing::info!(side = ?new.side, price = %new.price, size = %new.size, "paper: replace");
                    self.resting.retain(|o| o.client_id != old);
                    self.resting.push(new);
                }
                Intent::Cancel { client_id } => self.resting.retain(|o| o.client_id != client_id),
                Intent::CancelAll => self.resting.clear(),
                // Paper trading starts flat and only changes by paper fills.
                Intent::SyncPosition | Intent::SyncFills => {}
            }
        }
    }

    /// Fill any order the book has traded through, at the order's price.
    /// Optimistic: a real order waits in a queue behind others at its price
    /// and might never fill. Treat dry-run profit as a best case.
    fn on_event(&mut self, event: &Event) {
        let Event::Book(book, _) = event else { return };
        let (best_bid, best_ask) = (book.best_bid(), book.best_ask());
        let crossed = |o: &Order| match o.side {
            Side::Buy => best_ask.is_some_and(|ask| ask <= o.price),
            Side::Sell => best_bid.is_some_and(|bid| bid >= o.price),
        };
        let (filled, resting): (Vec<Order>, Vec<Order>) = self.resting.drain(..).partition(crossed);
        self.resting = resting;
        for order in filled {
            self.next_trade_id += 1;
            let _ = self.bus.send(Event::Fill(Fill {
                trade_id: self.next_trade_id,
                client_id: Some(order.client_id),
                side: order.side,
                price: order.price,
                size: order.size,
                fee: rust_decimal::Decimal::ZERO,
                order_finished: true,
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;
    use tokio::sync::mpsc;

    use super::*;
    use crate::book::Book;
    use crate::events::Stamp;

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn book_event(book: Book) -> Event {
        Event::Book(book, Stamp::at(Instant::now()))
    }

    #[tokio::test]
    async fn paper_fills_when_the_book_trades_through_our_price() {
        let (bus, mut events) = mpsc::unbounded_channel();
        let mut paper = PaperExecutor::new(bus);
        let bid = Order {
            side: Side::Buy,
            price: d("100"),
            size: d("1"),
            client_id: 7,
        };
        paper.execute(vec![Intent::Place(bid)]);

        // Best ask 100.5: above our bid, nothing happens.
        paper.on_event(&book_event(Book::from_levels(
            &[(d("99"), d("1"))],
            &[(d("100.5"), d("1"))],
        )));
        assert!(events.try_recv().is_err());

        // Best ask drops to 100: someone would have sold to us.
        paper.on_event(&book_event(Book::from_levels(
            &[(d("99"), d("1"))],
            &[(d("100"), d("1"))],
        )));
        let Ok(Event::Fill(fill)) = events.try_recv() else {
            panic!("expected a fill")
        };
        assert_eq!(
            (fill.client_id, fill.price, fill.side),
            (Some(7), d("100"), Side::Buy)
        );

        // Filled orders are gone.
        paper.on_event(&book_event(Book::from_levels(
            &[(d("99"), d("1"))],
            &[(d("99.5"), d("1"))],
        )));
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn paper_replace_moves_the_order() {
        let (bus, mut events) = mpsc::unbounded_channel();
        let mut paper = PaperExecutor::new(bus);
        let bid = Order {
            side: Side::Buy,
            price: d("100"),
            size: d("1"),
            client_id: 1,
        };
        let moved = Order {
            price: d("98"),
            client_id: 2,
            ..bid
        };
        paper.execute(vec![
            Intent::Place(bid),
            Intent::Replace { old: 1, new: moved },
        ]);
        // Asks down to 99: would have filled the old bid at 100, not the moved one at 98.
        paper.on_event(&book_event(Book::from_levels(
            &[(d("97"), d("1"))],
            &[(d("99"), d("1"))],
        )));
        assert!(events.try_recv().is_err());
        paper.on_event(&book_event(Book::from_levels(
            &[(d("97"), d("1"))],
            &[(d("98"), d("1"))],
        )));
        let Ok(Event::Fill(fill)) = events.try_recv() else {
            panic!("expected a fill")
        };
        assert_eq!(fill.client_id, Some(2));
    }

    #[tokio::test]
    async fn paper_cancels_remove_orders() {
        let (bus, mut events) = mpsc::unbounded_channel();
        let mut paper = PaperExecutor::new(bus);
        let ask = Order {
            side: Side::Sell,
            price: d("101"),
            size: d("1"),
            client_id: 8,
        };
        paper.execute(vec![Intent::Place(ask), Intent::Cancel { client_id: 8 }]);
        paper.on_event(&book_event(Book::from_levels(
            &[(d("102"), d("1"))],
            &[(d("103"), d("1"))],
        )));
        assert!(events.try_recv().is_err());
    }
}
