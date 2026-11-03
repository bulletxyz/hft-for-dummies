//! Everything you can tune lives in `bot.toml`. This file reads it and checks
//! the numbers make sense before any money is involved.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rust_decimal::Decimal;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// "mainnet" or "testnet".
    pub network: String,
    /// Talk to Bullet at this address instead of the network's public one,
    /// e.g. a direct endpoint for whitelisted market makers (Part 10).
    #[serde(default)]
    pub api_url: Option<String>,
    /// true: watch the market and log the quotes we *would* place. No key, no money.
    pub dry_run: bool,
    /// Key file made by `cargo run -- keygen`. Only needed when `dry_run = false`.
    pub key_file: PathBuf,

    /// The Bullet market we quote, e.g. "SOL-USD".
    pub symbol: String,
    /// The Binance USD-M perp we borrow the price from, e.g. "SOLUSDT".
    pub binance_symbol: String,

    /// Distance of each quote from fair value, in basis points (1 bp = 0.01%).
    pub spread_bps: Decimal,
    /// Size of each order, in the base asset (SOL for SOL-USD).
    pub order_size: Decimal,
    /// Never hold more than this, long or short.
    pub max_position: Decimal,
    /// How far (bps) both quotes lean against our position when it is at the max.
    pub skew_bps: Decimal,
    /// Only replace a resting quote once fair value has moved this far from it (bps).
    pub requote_threshold_bps: Decimal,
    /// And never more often than this.
    pub min_requote_ms: u64,
    /// Every order we place expires after this long, so if the bot dies its
    /// quotes stop being fillable. The bot refreshes them at half this age.
    pub quote_ttl_secs: u64,
    /// Our own cap on order transactions per second (Bullet allows far more).
    pub max_requests_per_sec: f64,

    /// Stop quoting for the rest of the run once we are down this much (USD, fees included).
    pub max_loss_usd: Decimal,
    /// Pull quotes if Binance goes quiet for this long.
    pub reference_stale_secs: u64,
    /// Pull quotes if the Bullet book goes quiet for this long.
    pub book_stale_secs: u64,
    /// How slowly the Binance→Bullet price gap (the "basis") is averaged.
    pub basis_halflife_secs: u64,

    /// Serve latency metrics to Prometheus on this local port, for Grafana
    /// (`infra/grafana`). Off when unset.
    #[serde(default)]
    pub metrics_port: Option<u16>,
}

impl Config {
    /// `--live` on the command line: trade for real without editing the file,
    /// so `bot.toml` never changes and every part's checkout stays clean.
    pub fn live(mut self, live: bool) -> Self {
        if live {
            self.dry_run = false;
        }
        self
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let config: Self =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if !matches!(self.network.as_str(), "mainnet" | "testnet") {
            bail!(
                "network must be \"mainnet\" or \"testnet\", got {:?}",
                self.network
            );
        }
        for (name, value) in [
            ("spread_bps", self.spread_bps),
            ("order_size", self.order_size),
            ("max_position", self.max_position),
            ("requote_threshold_bps", self.requote_threshold_bps),
            ("max_loss_usd", self.max_loss_usd),
        ] {
            if value <= Decimal::ZERO {
                bail!("{name} must be greater than 0, got {value}");
            }
        }
        if self.max_requests_per_sec < 1.0 {
            bail!(
                "max_requests_per_sec must be at least 1, got {}",
                self.max_requests_per_sec
            );
        }
        if self.skew_bps < Decimal::ZERO {
            bail!("skew_bps can't be negative, got {}", self.skew_bps);
        }
        if self.order_size > self.max_position {
            bail!(
                "order_size ({}) is bigger than max_position ({}), so no order could ever be placed",
                self.order_size,
                self.max_position
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_bot_toml_is_valid_and_safe() {
        let config = Config::load(Path::new("bot.toml")).unwrap();
        assert!(config.dry_run, "a fresh clone must never trade real money");
    }

    #[test]
    fn live_from_the_command_line_only_turns_dry_run_off() {
        let config = Config::load(Path::new("bot.toml")).unwrap();
        assert!(config.live(false).dry_run, "no flag: the file decides");
        let config = Config::load(Path::new("bot.toml")).unwrap();
        assert!(!config.live(true).dry_run, "--live trades for real");
    }

    #[test]
    fn the_benchmark_config_is_valid() {
        assert!(!Config::load(Path::new("bench-live.toml")).unwrap().dry_run);
    }

    #[test]
    fn metrics_are_on_in_the_shipped_configs() {
        for path in ["bot.toml", "bench-live.toml"] {
            let config = Config::load(Path::new(path)).unwrap();
            assert_eq!(config.metrics_port, Some(9000), "{path}");
        }
    }

    #[test]
    fn rejects_an_order_bigger_than_the_position_limit() {
        let mut config = Config::load(Path::new("bot.toml")).unwrap();
        config.order_size = config.max_position + Decimal::ONE;
        assert!(config.validate().is_err());
    }
}
