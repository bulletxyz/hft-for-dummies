//! How many transactions can be in flight at once? (Part 11)
//!
//!     cargo run --release --example in_flight
//!
//! Sends five harmless transactions ("cancel all my SOL-USD orders" while we
//! have none) one after another, then all five at once, and times both. Uses
//! `bot.key` and the account it's delegated on, like the live bot.

use std::time::Instant;

use futures_util::future::join_all;
use hft_for_dummies::bullet;
use hft_for_dummies::config::Config;

const N: usize = 5;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut config = Config::load("bot.toml".as_ref())?;
    config.dry_run = false; // we need the key to sign
    let (bus, _events) = tokio::sync::mpsc::unbounded_channel();
    let bullet = bullet::connect(&config, bus).await?;

    let start = Instant::now();
    for _ in 0..N {
        bullet.cancel_all().await?;
    }
    let one_by_one = start.elapsed();

    let start = Instant::now();
    let results = join_all((0..N).map(|_| bullet.cancel_all())).await;
    let all_at_once = start.elapsed();
    let ok = results.iter().filter(|r| r.is_ok()).count();

    println!(
        "{N} transactions one after another: {:>4} ms",
        one_by_one.as_millis()
    );
    println!(
        "{N} transactions all at once:       {:>4} ms ({ok}/{N} accepted)",
        all_at_once.as_millis()
    );
    for e in results.iter().filter_map(|r| r.as_ref().err()) {
        println!("  rejected: {e}");
    }
    Ok(())
}
