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

**p99 is what costs you money.** p50 is the typical case. p99 is the slowest
1%, and that's when you get picked off: the moment the price jumps is exactly
when everything is busiest. HDR histograms keep both cheaply and accurately.

## The scoreboard

Every part from here on ends with the same benchmark, so you can see what each
change bought. At exit the bot prints one line:

```text
SCOREBOARD end-to-end p50 … ms p95 … ms | binance→us p50 … ms | tick-to-trade p50 … µs p99 … µs | order-to-live p50 … ms p95 … ms | queue p99 … ms max … ms | orders …
```

It splits one trip around the loop into the parts that add up to it:

```text
 Binance trade ──► reaches us ──► waits in our queue ──► we decide ──► request to Bullet ──► our order is open
 (Binance's clock)  binance→us        queue              tick-to-trade    order-to-live        (Bullet's clock)
 └─────────────────────────────────────── end-to-end ───────────────────────────────────────┘
```

- **End-to-end** is the headline: from the Binance trade or book change that
  moved our price to Bullet's matching engine opening our new order, each
  stamped by its own exchange. That's how long our quote lags the market,
  which decides whether we get picked off.
- **Binance → us** is the network from Binance.
- **Tick-to-trade** is our reaction, from the tick arriving to the order going
  out: the usual meaning of the term in HFT, measured in microseconds.
- **Order-to-live** is the route to Bullet and Bullet's processing, until its
  engine stamps the order open. It ends there, not when Bullet's reply gets
  back to us: the order is resting before the reply arrives.
- **Queue p99 and max** show jitter: how long events sometimes wait before we
  even look at them.

The clock starts at the tick that made the bot requote. A requote the bot
held back on purpose (`min_requote_ms`) isn't counted: that's a strategy
choice, not a delay.

The benchmark is `bench-live.toml`: the same settings in every part, quoting
live, requoting often to get plenty of samples. Ten minutes of it:

```sh
cargo run --release -- run bench-live.toml
```

This trades real money: it needs the funded, delegated key from
[Going live](../../README.md#going-live). On dry run, read our numbers instead.

A run has a few hundred orders, so per-order numbers use p95: a p99 would be
just the one or two slowest, and moves tens of milliseconds between two
identical runs. p99 is still what costs you money; it needs more samples than
one run gives to read.

The goal for the series: end-to-end under 20 ms p50.

## Clocks

Binance → us and order-to-live compare an exchange's clock with ours, so ours
must be right. (End-to-end uses Binance's and Bullet's clocks only, and
tick-to-trade only ours.) Your computer keeps time with a quartz crystal,
which ticks at a slightly wrong rate that changes with temperature: typically
tens of parts per million, a few seconds a day. Operating systems correct it by asking time servers
([NTP](https://en.wikipedia.org/wiki/Network_Time_Protocol)) every so often,
and in between the clock wanders. Laptops are worst: they sleep, change
networks and correct lazily. Ours was 288 ms behind real time one afternoon
and 407 ms behind a few hours later.

> **Incident: messages from the future.** The first telemetry run reported
> every network hop as zero. The laptop's clock was 288 ms behind, so every
> Binance and Bullet message "arrived before it was sent", and the code was
> quietly rounding the negative times up to zero. *Fix:* the bot now counts
> such messages and tells you to sync your clock instead of printing
> nonsense. *Lesson:* a measurement you can't sanity-check is a guess.

On a laptop, sync before you measure (and check with `sntp time.apple.com`):

```sh
sudo sntp -sS time.apple.com    # macOS
sudo chronyc makestep           # Linux with chrony
```

Servers run [chrony](https://chrony-project.org/), which corrects continuously
against a nearby source and keeps the clock within microseconds (Part 4).

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
4. **Sync your clock before measuring one-way latency** (see Clocks).

## Scoreboard

This part's benchmark, 10 minutes live from a laptop (milliseconds):

| | tick-to-trade p50 / p99 | Binance → us p50 | queue p99 / max | Bullet accepted p50 |
|---|---|---|---|---|
| Part 3 (laptop) | 180 / 586 | 55 | 829 / 2,167 | 119 |

The baseline, from a laptop: 180 ms from Binance's trade to our order
leaving. Binance's prices take 55 ms to reach us; most of the rest is ticks
waiting in the queue while the bot waits ~120 ms for Bullet to accept its
last order. Part 4 attacks the distance, Part 6 the waiting.

## What's next

The numbers say the network is the problem, so Part 4 moves the bot next to
the exchanges.
