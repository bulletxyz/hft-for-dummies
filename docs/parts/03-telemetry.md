# Part 3: Measure first

Before making anything faster, find out where the time goes. This part times
every hop of the tick-to-trade loop and prints a table. Code:
[`part-3`](https://github.com/bulletxyz/hft-for-dummies/tree/part-3) ·
[diff](https://github.com/bulletxyz/hft-for-dummies/compare/part-2...part-3).

## How it works

Every market event now carries a **stamp** (`src/events.rs`): when the
exchange says it happened and sent it, and when we received it. The live
executor reports when each request went out and when Bullet replied, and
Bullet's order updates carry its matching engine's timestamp for our order
opening. `src/telemetry.rs` turns the gaps between those moments into one
[HDR histogram](https://hdrhistogram.github.io/HdrHistogram/) per hop and logs
a table every 30 seconds and on exit. It follows one trip around the loop, in
order:

| Step | What it is | Clocks |
|---|---|---|
| **end-to-end** | Binance trade → our order open on Bullet: the total | Binance's, Bullet's |
| 1 inside Binance | Binance matching a trade → sending it to us | Binance's |
| 2 binance→us | the network from Binance | Binance's, ours |
| 3 queue | waiting in our queue before we look at it | ours |
| 4 tick-to-trade | our code: tick in → order out | ours |
| 5 order-to-live | the route to Bullet, and Bullet opening the order | ours, Bullet's |

The steps roughly add up to end-to-end, not exactly: steps 1-3 count every
Binance message, the rest only the ones that led to an order, and percentiles
of parts never quite sum to the percentile of the whole. Under "also
measured", the table adds rows that overlap the trip or sit off it: Bullet's
reply to each request (a round trip, so it runs past step 5), the part of
tick-to-trade spent deciding, and how fast Bullet's own book reaches us.

**p99 is what costs you money.** p50 is the typical case. p99 is the slowest
1%, and that's when you get picked off: the moment the price jumps is exactly
when everything is busiest. HDR histograms keep both cheaply and accurately.

## Seeing it in Grafana

The table is enough to compare runs, but a picture shows how latency moves
over time. Production bots export their histograms to
[Prometheus](https://prometheus.io/), a database for numbers over time, and
draw them in [Grafana](https://grafana.com/). This bot can too, through the
[`metrics`](https://docs.rs/metrics) crate: set `metrics_port = 9898` in
`bot.toml`, run the bot, then (with Docker installed):

```sh
cd infra/grafana && docker compose up
```

and open <http://localhost:3000>. The dashboard (`infra/grafana/dashboards/latency.json`)
shows one trip right now, end-to-end over time, and each step on a log scale,
so microseconds and milliseconds fit on one chart.

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
| decide | 5 µs / 50 µs | 1 µs / 20 µs |
| tick-to-trade | 24 µs / 230 µs | 7 µs / 134 µs |
| queue | 10 µs / 200 µs | 10 µs / 300 µs |
| inside Binance | ~0 / 31 ms | ~0 / 31 ms |

And live, from the same laptop (5 minutes, quoting 10 bps wide):

| Hop | p50 | p99 |
|---|---|---|
| Bullet's reply to a place | 161 ms | 548 ms |
| place sent → Bullet's websocket shows it | 319 ms | 1,972 ms |
| queue | 0.06 ms | **2,802 ms** |

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

This part's benchmark, 10 minutes live from a laptop:

| | end-to-end p50 / p95 | Binance → us p50 | tick-to-trade p50 | order-to-live p50 / p95 | queue p99 / max |
|---|---|---|---|---|---|
| Part 3 (laptop) | 224 / 383 ms | (clock off) | 11 µs | (clock off) | 464 / 828 ms |

The baseline: 224 ms from a Binance trade to our order resting on Bullet,
and our code is 11 µs of it. Most of the rest is waiting: the bot waits for
Bullet's reply to each order (about 120 ms from a laptop) before reading the
next event, so ticks pile up in the queue. Part 4 attacks the distance, Part 6
the waiting.

The laptop's clock slipped half a second behind *during* the run, so the two
numbers that use our clock are blank. End-to-end doesn't use it (only
Binance's and Bullet's), which is why it's the headline.

## What's next

The numbers say the network is the problem, so Part 4 moves the bot next to
the exchanges.
