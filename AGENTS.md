# hft-for-dummies

Companion code for the *HFT for dummies* blog series: a SOL-USD market maker on
Bullet priced off Binance. Readers fork it and follow along part by part, many
of them new to Rust and to trading. `README.md` is the user-facing guide; this
file is for agents.

Two jobs come up. Read the section for yours.

## Helping someone set up and run the bot

The user is probably new to Rust, trading, or both. Go one step at a time,
say what each command does before running it, and explain the output.

### Dry run (no money, no key)

1. `cargo --version`. If it's missing, Rust isn't installed: give the user the
   rustup command from the README to run themselves (it changes their shell),
   then ask them to open a new terminal.
2. `cargo run --release`. The first build takes a few minutes. Then explain the
   log as it appears: both venues connect, about a minute of "learning the
   Binance-to-Bullet price gap", then `quoting` and `paper: place ...` lines and
   a summary every 10 seconds. `FILL` lines mean the real market traded through
   a paper quote.
3. Stop it with Ctrl-C (or SIGINT). Never SIGKILL a live bot: it would skip
   cancelling its orders.

Let them change `bot.toml` and rerun; that's the point of the exercise.

### Going live: only when the user says so

Going live risks real money. Do not set `dry_run = false`, start a live run,
or suggest it is the next step unless the user has asked to trade for real.
Confirm the amount first.

- **Keys.** Never ask the user to paste a private key into the chat, and never
  print, `cat`, copy, or commit `bot.key`. Either the user pastes the key from
  the Bullet app's Delegate accounts page into `bot.key` themselves, or you
  run `cargo run --release -- keygen` (the key stays on their disk) and give
  them the printed link to approve. An address (base58, ~44 chars) is public
  and fine to see; the private key is not.
- **Account and money.** The user needs a Bullet account with about $20 USDC
  (1x; or $10 with 2-3x account leverage). If they don't have one, walk them
  through the README's Going live step 1: a Solana wallet with USDC, sign up
  with the series' referral link https://app.bullet.xyz/trade?r=HFT4DUMMIES
  (code `HFT4DUMMIES`), then bridge the USDC in. They do this in the app
  themselves; ask for their account address afterwards so you can check the
  balance read-only.
- **Before starting,** check the account read-only (no key needed; `ADDR` is
  the account's address, `KEY` the delegate's address):

  ```sh
  U=https://tradingapi.bullet.xyz
  curl -s "$U/api/v1/delegateOf?address=KEY"                     # {"parent": ADDR} once delegated
  curl -s "$U/fapi/v3/account?address=ADDR"                      # balances, positions
  curl -s "$U/fapi/v1/openOrders?address=ADDR&symbol=SOL-USD"    # resting orders
  ```

  Warn the user if the account holds a SOL-USD position or orders: the bot
  cancels every SOL-USD order on start and refuses to quote if the position is
  bigger than `max_position`.
- **Run it** with `dry_run = false`. Prefer a copy of the config
  (`cargo run --release -- run my-live.toml`) so `bot.toml` stays on dry run.
- **While it runs,** watch for `quoting`, then two orders about `spread_bps`
  either side of the mid (the user can see them in the Bullet app). Warnings
  worth explaining are in the README's Troubleshooting section.
- **After stopping,** re-run the read-only checks: no open orders should remain.
  Any position stays open; tell the user and let them decide.

## Working on the code

### Commands

```sh
cargo run --release           # run (dry run by default, reads bot.toml)
cargo run -- keygen           # make bot.key for live trading
cargo test                    # after every change
cargo fmt && cargo clippy --all-targets -- -D warnings   # before every commit (CI runs both)
```

### Architecture (keep it)

Event-driven, the same shape as the bullet-bots harness, kept small:

- Feeds (`binance.rs`, `bullet.rs`) turn the outside world into `Event`s on one bus (`events.rs`).
- `MarketMaker` (`market_maker.rs`) is a pure state machine: `on_event` / `requote` return `Intent`s. No I/O, no awaits.
- An `Executor` (`executor.rs`) carries intents out and reports results back as events. Live and paper implement the same trait.
- `bot.rs` only wires and runs the loop.

Each series part should change one box and leave the others alone. When a part
needs a new capability, add an event, intent, or executor, not a special case
in the loop. Don't add generality no part needs yet (multiple venues or
strategies, actor registries).

### Rules

- One series part = one PR (several if the part is big), squash-merged and tagged `part-N`.
  PR titles are Conventional Commits (`feat(executor): ...`).
- Write for beginners: plain comments that explain *why*, short files, no clever tricks.
- `rust_decimal::Decimal` for every price, size and amount.
- Position changes only from fills (`Inventory::on_fill`) or an exchange read (`adopt`), never from sending an order.
- Money-safety code needs a test for every change: `market_maker.rs`, `strategy.rs`,
  `inventory.rs`, `executor.rs`, `book.rs`, `fair_value.rs`, `key.rs`, and the config checks.
  Write the failing test first.
- `bot.toml` must ship with `dry_run = true` (a test enforces it). Never commit keys (`*.key` is ignored).
- Keep the README's setup steps and the section above in step with the code.
