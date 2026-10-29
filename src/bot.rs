//! The harness: wires the pieces together and runs the loop.
//!
//! ```text
//!  Binance feed ──┐                                  ┌── LiveExecutor ──> Bullet
//!  Bullet feed ───┼──> bus ──> MarketMaker ──> intents
//!  timer, Ctrl-C ─┘     ^       (state machine)      └── PaperExecutor (dry run)
//!                       └──────── fills, acks, failures ──────┘
//! ```
//!
//! Every input becomes an event on one queue. The loop takes them one at a
//! time, lets the market maker update its view, and once it has caught up
//! with everything waiting, asks it what orders it wants.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::events::{Bus, Event};
use crate::executor::{Executor, LiveExecutor, PaperExecutor};
use crate::market_maker::MarketMaker;
use crate::{binance, bullet};

pub async fn run(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    if config.dry_run {
        tracing::info!(
            "DRY RUN: watching {} and logging the quotes we would place. No orders are sent.",
            config.symbol
        );
    } else {
        tracing::warn!("LIVE: trading {} with real money", config.symbol);
    }

    let (bus, events) = mpsc::unbounded_channel();
    binance::connect(&config.binance_symbol, bus.clone());
    let bullet = bullet::connect(&config, bus.clone()).await?;
    spawn_timer(bus.clone());
    spawn_ctrl_c(bus.clone());

    let dry_run = config.dry_run;
    let market_maker = MarketMaker::new(config, bullet.tick);
    if dry_run {
        event_loop(market_maker, PaperExecutor::new(bus), events).await
    } else {
        event_loop(market_maker, LiveExecutor::new(bullet, bus), events).await
    }
}

async fn event_loop(
    mut market_maker: MarketMaker,
    mut executor: impl Executor,
    mut events: mpsc::UnboundedReceiver<Event>,
) -> Result<()> {
    executor.execute(market_maker.start()).await;
    let result = loop {
        let Some(event) = events.recv().await else {
            break Ok(());
        };
        match &event {
            Event::Shutdown => break Ok(()),
            Event::Disconnected(why) => break Err(anyhow::anyhow!("Bullet disconnected: {why}")),
            _ => {}
        }
        let now = Instant::now();
        executor.on_event(&event);
        let intents = market_maker.on_event(&event, now);
        executor.execute(intents).await;

        // Only quote on the newest information: if more events are already
        // waiting, read them first. A quote based on a price we know is old
        // is a gift to whoever trades against it.
        if events.is_empty() {
            let intents = market_maker.requote(now);
            executor.execute(intents).await;
        }
    };
    tracing::info!("shutting down: cancelling our orders");
    executor.execute(market_maker.stop()).await;

    // An order can fill in the instant before our cancel lands. Keep listening
    // for a moment so the final numbers include those fills.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.recv()).await {
        if let Event::Fill(_) = event {
            market_maker.on_event(&event, Instant::now());
        }
    }
    market_maker.log_summary();
    result
}

fn spawn_timer(bus: Bus) {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        loop {
            timer.tick().await;
            if bus.send(Event::Tick).is_err() {
                return;
            }
        }
    });
}

fn spawn_ctrl_c(bus: Bus) {
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = bus.send(Event::Shutdown);
        }
    });
}
