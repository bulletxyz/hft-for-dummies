# Part 2: Hello World

A complete, if slow, market maker: it prices SOL off Binance and keeps one
bid and one ask resting on Bullet. Code: [`part-2`](https://github.com/bulletxyz/hft-for-dummies/tree/part-2).

## What's in it

The whole loop from the [diagram](../../README.md#what-were-building), in its
simplest form:

- **Feeds** (`src/binance.rs`, `src/bullet.rs`): Binance's best bid/ask and
  Bullet's order book arrive as events on one queue.
- **Fair price** (`src/fair_value.rs`): Binance mid plus the average gap
  between Bullet and Binance, learned over the first minute.
- **Strategy** (`src/strategy.rs`): quote `spread_bps` either side of fair,
  lean against inventory (`skew_bps`), never past `max_position`.
- **Executor** (`src/executor.rs`): cancel and replace when fair value moves
  more than `requote_threshold_bps`. It waits for each order to be accepted,
  which is the first thing later parts speed up.
- **Inventory** (`src/inventory.rs`): position and PnL, from fills only.

## Try it

`cargo run` (dry run). After a minute of warm-up you'll see `quoting` and two
paper orders. To trade for real, follow
[Going live](../../README.md#going-live).

Live, your orders show as ▸ markers in the app's order book. With the
default `spread_bps = "10"` they sit well behind the best prices (SOL-USD's
spread is around 2 bps), so fills are rare. Lower `spread_bps` to fill more
often, and watch what happens to your PnL when the price runs through you.
That's adverse selection, and it's why the rest of the series is about speed.

## What it doesn't do yet

- It's slow: every cancel and place is waited on before the next event is read.
- It doesn't measure anything. Part 3 timestamps every hop.
