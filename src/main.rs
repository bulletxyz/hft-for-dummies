//! HFT for dummies: a market maker on Bullet that borrows its price from Binance.
//!
//! Start reading at `events.rs` (the vocabulary), then `bot.rs` (the loop),
//! `market_maker.rs` (the decisions) and `strategy.rs` (the prices).

mod binance;
mod book;
mod bot;
mod bullet;
mod config;
mod cpu;
mod events;
mod executor;
mod fair_value;
mod inventory;
mod key;
mod market_maker;
mod rate_limit;
mod strategy;
mod telemetry;

use std::path::PathBuf;

use anyhow::{Result, bail};
use bullet_rust_sdk::Keypair;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "A simple market maker for the HFT for dummies series")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Trade with real money, whatever `dry_run` says in the config.
    #[arg(long, global = true)]
    live: bool,
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

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        // Colours only in a terminal, not in a server's log files.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();

    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Run {
        config: "bot.toml".into(),
    }) {
        Command::Run { config } => {
            let config = config::Config::load(&config)?.live(cli.live);
            // The whole bot (apart from the Binance feed) runs on this one
            // thread: simple to reason about, and one thing to pin.
            cpu::pin_current_thread(config.bot_core, "bot");
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(bot::run(config))
        }
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
    println!("Next: open this link (logged in to Bullet) and click Confirm Delegation:");
    println!("  {}", bullet::delegate_link(network, &address));
    Ok(())
}
