# Part 3: Measure first

Before making anything faster, find out where the time goes. This part times
every hop of the tick-to-trade loop and prints a table. Code:
[`part-3`](https://github.com/bulletxyz/hft-for-dummies/tree/part-3) ·
[diff](https://github.com/bulletxyz/hft-for-dummies/compare/part-2...part-3).

## How it works

Every market event now carries a **stamp** (`src/events.rs`): when the exchange
says it sent the message, and when we received it. The live executor reports
when it sent each order and when Bullet accepted it, and Bullet's websocket
echo tells us when the order is actually on the book. `src/telemetry.rs` turns
those into one [HDR histogram](https://hdrhistogram.github.io/HdrHistogram/)
per hop and logs the table every 30 seconds and on exit:

| Hop | What it is | Whose clock |
|---|---|---|
| binance: match → sent | Binance's own processing (`E` − `T` in its message) | Binance's |
| binance / bullet book: sent → received | the network | theirs vs ours |
| bot: received → handled | waiting in our queue | ours |
| bot: decide | updating state and working out quotes | ours |
| bot: tick received → orders sent | our whole reaction time | ours |
| bullet: place/cancel sent → accepted | request round trip to Bullet | ours |
| bullet: place sent → live on book | until Bullet's websocket confirms the order rests | ours |

**Read p99, not p50.** p50 is the typical case. p99 is the slowest 1%, and
that's when you get picked off: the moment the price jumps is exactly when
everything is busiest. HDR histograms keep both cheaply and accurately.

## Try it

```sh
cargo run --release
```

Use `--release` whenever you measure. A debug build skips optimisations so it
compiles fast, and that distorts the code-side numbers.

## What we measured

A 4-minute dry run on a laptop, debug vs release, side by side:

| Hop | debug p50 / p99 | release p50 / p99 |
|---|---|---|
| bot: decide | 5 µs / 50 µs | 1 µs / 20 µs |
| bot: tick received → orders sent | 24 µs / 230 µs | 7 µs / 134 µs |
| bot: received → handled | 10 µs / 200 µs | 10 µs / 300 µs |
| binance: match → sent | ~0 / 31 ms | ~0 / 31 ms |

And live, from the same laptop (5 minutes, quoting 10 bps wide):

| Hop | p50 | p99 |
|---|---|---|
| bullet: place sent → accepted | 161 ms | 548 ms |
| bullet: place sent → live on book | 319 ms | 1,972 ms |
| bot: received → handled | 0.06 ms | **2,802 ms** |

Four lessons:

1. **Our code is not the problem.** Deciding takes microseconds. Release
   makes it 3-5x faster, and it still doesn't matter next to the network,
   which is milliseconds. That's why Part 4 moves the bot instead of
   optimising code.
2. **Binance itself has a tail.** Usually it sends a book change the same
   millisecond it happens, but 1% of the time it's 30+ ms late. No amount of
   tuning on our side fixes that; we can only notice it.
3. **Waiting is the real cost.** The bot waits for each order request to
   finish before reading the next event, so every Binance tick that arrives
   meanwhile sits in the queue: p99 of almost 3 seconds. Part 6 fixes that.
4. **You can't measure one-way network latency with an unsynced clock.** The
   laptop's clock was 288 ms behind real time, so every message "arrived
   before it was sent". The bot now spots this and tells you to sync
   (`sudo sntp -sS time.apple.com` on macOS). Servers keep their clocks well within a
   millisecond with [chrony](https://chrony-project.org/), which Part 4 sets
   up.

## What's next

The numbers say the network is the problem, so Part 4 moves the bot next to
the exchanges.
