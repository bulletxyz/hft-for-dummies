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

## What live trading taught us

These all happened in the first afternoon of running this bot with real
money. Each is now handled in the code you're running.

> **Incident: the silent socket.** A wifi blip left the Binance connection
> half-open: no error, no data. The bot correctly stopped quoting on a stale
> price, and stayed stopped until restarted. *Fix:* if Binance sends nothing
> for 10 seconds, reconnect (`src/binance.rs`). *Lesson:* silence is not the
> same as "nothing happened". Part 5 comes back to this.

> **Incident: the fill that never arrived.** Bullet's websocket dropped one
> fill notification, with no error or reconnect. The bot's position was off
> by 0.05 SOL from then on, and since it thought it had room, it went past
> its own `max_position`. *Fix:* every 10 seconds, and after any reconnect,
> the bot reads its fills from the exchange's own record (`GET
> /fapi/v1/userTrades`) and applies any it missed; each fill has an ID, so
> none is ever counted twice. *Lesson:* a stream is a notification, not a
> record. Reconcile against the source of truth.

> **Incident: the fill during shutdown.** On Ctrl-C an order filled in the
> instant before the final cancel landed, and the bot's last summary
> disagreed with the exchange. *Fix:* on the way out the bot cancels, checks
> its fills one last time, and only then reports.

> **Incident: cancel versus fill.** The bot sometimes cancels an order that
> has just filled; Bullet answers "order not found" and rejects the whole
> request. Here the bot plays safe and cancels everything. Part 6 handles it
> precisely.

## What it doesn't do yet

- It's slow: every cancel and place is waited on before the next event is read.
- It doesn't measure anything. Part 3 timestamps every hop.
