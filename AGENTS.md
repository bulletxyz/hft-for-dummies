# hft-for-dummies

Companion code for the *HFT for dummies* blog series: a SOL-USD market maker on
Bullet priced off Binance. Readers fork it and follow along part by part, many
of them new to Rust and to trading. `README.md` is the user-facing guide; this
file is for agents.

Two jobs come up. Read the section for yours.

## Helping someone set up and run the bot

Follow the README: it is the setup guide, for people and agents alike (Run
it, Going live). Don't restate its steps to the user from
memory; read them from there. If a step there is wrong or unclear, fix the
README. What follows is only what an agent needs on top.

The user is probably new to Rust, trading, or both. Go one step at a time,
say what each command does before running it, and explain the log as it
appears.

- **Going live is the user's call.** Don't run with `--live`, set
  `dry_run = false`, or push them towards it unless they've asked to trade
  for real. Confirm the amount first. Go live with `cargo run -- --live`, not
  by editing `bot.toml`: the file stays on dry run and every part's checkout
  stays clean.
- **Keys.** Never ask for a private key in the chat, and never print, `cat`,
  copy, or commit `bot.key`. Prefer `keygen` and hand the user the link to
  confirm: the secret never leaves their disk. If they bring their own key,
  they paste it into `bot.key` themselves. Addresses are public and fine.
- **The Bullet app is the user's.** They create the account, deposit and
  approve delegates there. Ask for their account address so you can check
  things read-only (no key needed; `ADDR` the account, `KEY` the delegate's
  address):

  ```sh
  U=https://tradingapi.bullet.xyz
  curl -s "$U/api/v1/delegateOf?address=KEY"                     # {"parent": ADDR} once delegated
  curl -s "$U/fapi/v3/account?address=ADDR"                      # balance, positions
  curl -s "$U/fapi/v1/openOrders?address=ADDR&symbol=SOL-USD"    # resting orders
  ```

  Full API docs (REST, WebSocket, order types, delegates), written for agents:
  https://tradingapi.bullet.xyz/docs/llms.txt

  Run these before a live start (warn if there's already a SOL-USD position or
  orders) and after stopping (no orders should remain; any position is the
  user's to close or keep).
- **Stopping.** Ctrl-C or SIGINT, so the bot cancels its orders. Never
  SIGKILL a live bot.

## Working on the code

### Commands

```sh
cargo run                     # run (dry run by default, reads bot.toml)
cargo run -- keygen           # make bot.key for live trading
cargo test                    # after every change
cargo bench                   # hot-path microbenchmarks (benches/hot_path.rs)
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
  The PR adds `docs/parts/NN-name.md` (what it adds, how to try it, link to
  `compare/part-(N-1)...part-N`) and links it from the README's series table.
  The README itself only describes the current bot; part-specific notes go in the guide.
  PR titles are Conventional Commits (`feat(executor): ...`).
- Write for beginners: plain comments that explain *why*, short files, no clever tricks.
- `rust_decimal::Decimal` for every price, size and amount.
- Position changes only from fills (`Inventory::on_fill`) or an exchange read (`adopt`), never from sending an order.
- Money-safety code needs a test for every change: `market_maker.rs`, `strategy.rs`,
  `inventory.rs`, `executor.rs`, `book.rs`, `fair_value.rs`, `key.rs`, `rate_limit.rs`, and the config checks.
  Write the failing test first.
- `bot.toml` must ship with `dry_run = true` (a test enforces it). Never commit keys (`*.key` is ignored).
- Keep the README's setup steps and the section above in step with the code.
