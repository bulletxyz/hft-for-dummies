//! HFT for dummies: a market maker on Bullet that borrows its price from Binance.
//!
//! Start reading at `events.rs` (the vocabulary), then `bot.rs` (the loop),
//! `market_maker.rs` (the decisions) and `strategy.rs` (the prices).
//!
//! The code is a library so `benches/` can time pieces of it on their own;
//! `main.rs` is just the command line.

pub mod binance;
pub mod book;
pub mod bot;
pub mod bullet;
pub mod config;
pub mod cpu;
pub mod events;
pub mod executor;
pub mod fair_value;
pub mod inventory;
pub mod key;
pub mod market_maker;
pub mod rate_limit;
pub mod strategy;
pub mod telemetry;
