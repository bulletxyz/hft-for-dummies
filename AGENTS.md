# hft-for-dummies

Companion code for the *HFT for dummies* blog series: a SOL-USD market maker on
Bullet priced off Binance. Readers fork it and follow along part by part, many
of them new to Rust and to trading. See `README.md` for the series and setup.

## Commands

```sh
cargo run --release           # run (dry run by default, reads bot.toml)
cargo run -- keygen           # make bot.key for live trading
cargo test                    # after every change
cargo fmt && cargo clippy --all-targets -- -D warnings   # before every commit (CI runs both)
```

## Architecture (keep it)

Event-driven, the same shape as the bullet-bots harness, kept small:

- Feeds (`binance.rs`, `bullet.rs`) turn the outside world into `Event`s on one bus (`events.rs`).
- `MarketMaker` (`market_maker.rs`) is a pure state machine: `on_event` / `requote` return `Intent`s. No I/O, no awaits.
- An `Executor` (`executor.rs`) carries intents out and reports results back as events. Live and paper implement the same trait.
- `bot.rs` only wires and runs the loop.

Each series part should change one box and leave the others alone. When a part
needs a new capability, add an event, intent, or executor, not a special case
in the loop. Don't add generality no part needs yet (multiple venues or
strategies, actor registries).

## Rules

- One series part = one PR (several if the part is big), squash-merged and tagged `part-N`.
  PR titles are Conventional Commits (`feat(executor): ...`).
- Write for beginners: plain comments that explain *why*, short files, no clever tricks.
- `rust_decimal::Decimal` for every price, size and amount.
- Position changes only from fills (`Inventory::on_fill`) or an exchange read (`adopt`), never from sending an order.
- Money-safety code needs a test for every change: `market_maker.rs`, `strategy.rs`,
  `inventory.rs`, `executor.rs`, `book.rs`, `fair_value.rs`, and the config checks.
  Write the failing test first.
- `bot.toml` must ship with `dry_run = true` (a test enforces it). Never commit keys (`*.key` is ignored).
