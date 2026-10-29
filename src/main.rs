//! HFT for dummies: a market maker on Bullet that borrows its price from Binance.
//!
//! Start reading at `events.rs` (the vocabulary), then `bot.rs` (the loop),
//! `market_maker.rs` (the decisions) and `strategy.rs` (the prices).

mod binance;
mod book;
mod bot;
mod bullet;
mod config;
mod events;
mod executor;
mod fair_value;
mod inventory;
mod key;
mod market_maker;
mod strategy;

use std::path::PathBuf;

use anyhow::{Result, bail};
use bullet_rust_sdk::Keypair;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "A simple market maker for the HFT for dummies series")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the bot (the default).
    Run {
        #[arg(default_value = "bot.toml")]
        config: PathBuf,
    },
    /// Make a new trading key for the bot, to delegate to in the Bullet app.
    Keygen {
        #[arg(default_value = "bot.key")]
        path: PathBuf,
        #[arg(long, default_value = "mainnet")]
        network: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    match Cli::parse().command.unwrap_or(Command::Run {
        config: "bot.toml".into(),
    }) {
        Command::Run { config } => bot::run(&config).await,
        Command::Keygen { path, network } => keygen(&path, &network),
    }
}

fn keygen(path: &PathBuf, network: &str) -> Result<()> {
    if path.exists() {
        bail!("{} already exists; not overwriting a key", path.display());
    }
    let keypair = Keypair::generate();
    keypair.write_to_file(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Only you can read it.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let address = keypair.address();
    println!(
        "Wrote a new key to {}. Keep it secret; never commit it.\n",
        path.display()
    );
    println!("Its address: {address}\n");
    println!("Next: open this link to add it as a delegate on your Bullet account:");
    println!("  {}", bullet::delegate_link(network, &address));
    Ok(())
}
