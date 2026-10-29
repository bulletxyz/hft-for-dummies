//! Executors carry out the strategy's intents and report back as events.
//!
//! - [`LiveExecutor`] sends real orders to Bullet.
//! - [`PaperExecutor`] pretends to (dry run): it remembers the orders and
//!   fills them when the real Bullet book trades through their price.
//!
//! The strategy can't tell the two apart, so what you see in dry run is the
//! same code path that trades live.
//!
//! For now an executor finishes each batch before the bot reads the next
//! event, so a slow exchange slows everything down. Part 6 fixes that.

use crate::bullet::Bullet;
use crate::events::{Bus, Event, Fill, Intent, Order, Side};

pub trait Executor {
    async fn execute(&mut self, intents: Vec<Intent>);

    /// See every event too (the paper executor uses the book to fill orders).
    fn on_event(&mut self, _event: &Event) {}
}

pub struct LiveExecutor {
    bullet: Bullet,
    bus: Bus,
    /// Fills from this time on (µs since 1970) are ours to count. Set when we
    /// read the starting position: everything before is already in it.
    fills_since_us: Option<u64>,
}

impl LiveExecutor {
    pub fn new(bullet: Bullet, bus: Bus) -> Self {
        Self {
            bullet,
            bus,
            fills_since_us: None,
        }
    }
}

impl Executor for LiveExecutor {
    async fn execute(&mut self, intents: Vec<Intent>) {
        let mut cancels = Vec::new();
        let mut places = Vec::new();
        for intent in intents {
            match intent {
                Intent::Place(order) => places.push(order),
                Intent::Cancel { client_id } => cancels.push(client_id),
                Intent::CancelAll => {
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

        if !cancels.is_empty()
            && let Err(e) = self.bullet.cancel(&cancels).await
        {
            tracing::warn!("cancel failed: {e}");
            self.send(Event::CancelFailed);
            // Don't add new orders on top of ones that may still be resting.
            self.send(Event::PlaceFailed {
                client_ids: places.iter().map(|o| o.client_id).collect(),
            });
            return;
        }
        if !places.is_empty()
            && let Err(e) = self.bullet.place(&places).await
        {
            tracing::warn!("placing orders failed (often: not enough margin): {e}");
            // They may have landed anyway (we just never heard back). Make sure they're gone.
            let ids: Vec<u64> = places.iter().map(|o| o.client_id).collect();
            let _ = self.bullet.cancel(&ids).await;
            self.send(Event::PlaceFailed { client_ids: ids });
        }
    }
}

impl LiveExecutor {
    fn send(&self, event: Event) {
        let _ = self.bus.send(event);
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
    async fn execute(&mut self, intents: Vec<Intent>) {
        for intent in intents {
            match intent {
                Intent::Place(order) => {
                    tracing::info!(side = ?order.side, price = %order.price, size = %order.size, "paper: place");
                    self.resting.push(order);
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
        let Event::Book(book) = event else { return };
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

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
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
        paper.execute(vec![Intent::Place(bid)]).await;

        // Best ask 100.5: above our bid, nothing happens.
        paper.on_event(&Event::Book(Book::from_levels(
            &[(d("99"), d("1"))],
            &[(d("100.5"), d("1"))],
        )));
        assert!(events.try_recv().is_err());

        // Best ask drops to 100: someone would have sold to us.
        paper.on_event(&Event::Book(Book::from_levels(
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
        paper.on_event(&Event::Book(Book::from_levels(
            &[(d("99"), d("1"))],
            &[(d("99.5"), d("1"))],
        )));
        assert!(events.try_recv().is_err());
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
        paper
            .execute(vec![Intent::Place(ask), Intent::Cancel { client_id: 8 }])
            .await;
        paper.on_event(&Event::Book(Book::from_levels(
            &[(d("102"), d("1"))],
            &[(d("103"), d("1"))],
        )));
        assert!(events.try_recv().is_err());
    }
}
