# Part 3: Measure first

Before making anything faster, find out where the time goes. This part times
every hop of the loop, from a Binance update to our order resting on Bullet,
and shows it as a table and in Grafana. Code:
[`part-3`](https://github.com/bulletxyz/hft-for-dummies/tree/part-3) ·
[diff](https://github.com/bulletxyz/hft-for-dummies/compare/part-2...part-3).

## How it works

Every market event now carries a **stamp** (`src/events.rs`): when the
exchange says it happened and sent it, and when we received it. Bullet's
order updates carry its matching engine's timestamp for our order opening.
`src/telemetry.rs` turns the gaps between those moments into one
[HDR histogram](https://hdrhistogram.github.io/HdrHistogram/) per hop and logs
a table every 30 seconds and on exit. It follows one trip around the loop, in
order:

| Step | What it is | Clocks |
|---|---|---|
| **end-to-end** | Binance update → our order open on Bullet: the total | Binance's, Bullet's |
| 1 inside Binance | Binance's book changing → Binance sending it to us | Binance's |
| 2 binance→us | the network from Binance | Binance's, ours |
| 3 queue | waiting in our queue before we look at it | ours |
| 4 tick-to-trade | our code: picking the update up → deciding on orders | ours |
| 5 order-to-live | sending the order, the route to Bullet, and Bullet opening it | ours, Bullet's |

Each step starts where the one before it ends. They still only roughly add up
to end-to-end: steps 1-3 count every market message, while steps 4-5 and
end-to-end count only the orders the bot placed right after a Binance update
(a requote caused by a fill or by Bullet's book has no Binance update to time
from). And percentiles of parts never quite sum to the percentile of the
whole. Binance stamps whole milliseconds, so step 1 usually reads zero.

Under "also measured" the table adds rows that overlap the trip or sit off it:
Bullet's reply to each request (a round trip, so it runs past step 5), how
long deciding takes (on every requote, not just the timed ones), and how fast
Bullet's own book reaches us.

**p99 is what costs you money.** p50 is the typical case. p99 is the slowest
1%, and that's when you get picked off: the moment the price jumps is exactly
when everything is busiest. HDR histograms keep both cheaply and accurately.

## Try it

You went live at the end of Part 2, so run it live (dry run works too, but no
order reaches Bullet, so end-to-end and order-to-live stay empty):

```sh
cargo run --release -- --live
```

`--live` overrides `dry_run` for this run, so `bot.toml` stays as shipped and
`git checkout part-4` later won't trip over your edits. (If you set
`dry_run = false` in the file in Part 2, put it back: `git checkout bot.toml`.)
Use `--release` whenever you measure. A debug build skips optimisations so it
compiles fast, and that distorts the code-side numbers.

Then watch it. Production bots export their histograms to
[Prometheus](https://prometheus.io/), a database for numbers over time, and
draw them in [Grafana](https://grafana.com/). This bot does too, through the
[`metrics`](https://docs.rs/metrics) crate (`metrics_port = 9000` in
`bot.toml`). With [Docker](https://docs.docker.com/get-started/get-docker/)
running, in a second terminal:

```sh
cd infra/grafana && docker compose up
```

and open <http://localhost:3000>. The dashboard
(`infra/grafana/dashboards/latency.json`) shows:

- **the average trip over the last minute**, with a "sum of steps" bar that
  matches end-to-end (averages add up; percentiles don't);
- **end-to-end over time**, p50 and p99;
- **every step on a log scale**, so our code's microseconds and the
  network's milliseconds fit on one chart;
- **the network steps**, binance→us and order-to-live, p50 and p99;
- **the queue**, whose p99 shows events waiting behind a slow request.

It fills in within a minute.
(Tested with Docker on a Mac. On Linux, `host.docker.internal` can't reach a
port bound to `127.0.0.1`: run Prometheus with `network_mode: host` and scrape
`localhost:9000` instead.)

## The scoreboard

Every part from here on ends with the same benchmark, so you can see what each
change bought. At exit the bot prints one line:

```text
SCOREBOARD end-to-end p50 … ms p99 … ms | binance→us p50 … ms | tick-to-trade p50 … µs p99 … µs | order-to-live p50 … ms p99 … ms | queue p99 … ms max … ms | orders …
```

It splits one trip around the loop into its steps:

```text
 Binance update ──► reaches us ──► waits in our queue ──► we decide ──► order to Bullet ──► our order is open
 (Binance's clock)   binance→us        queue             tick-to-trade    order-to-live       (Bullet's clock)
 └─────────────────────────────────────── end-to-end ───────────────────────────────────────┘
```

- **End-to-end** is the headline: from the Binance update that moved our
  price to Bullet's matching engine opening our new order, each stamped by
  its own exchange. That's how long our quote lags the market, which decides
  whether we get picked off.
- **Binance → us** is the network from Binance.
- **Tick-to-trade** is our reaction, from picking the update up to deciding
  on orders: the usual meaning of the term in HFT, measured in microseconds.
- **Order-to-live** runs from that decision until Bullet's engine stamps the
  order open: sending it, the route, and Bullet's processing. It ends there,
  not when Bullet's reply gets back to us: the order rests before the reply
  arrives.
- **Queue p99 and max** show jitter: how long events sometimes wait before we
  even look at them.

A requote the bot held back on purpose (`min_requote_ms`) isn't counted: the
clock starts at the update it finally reacts to. That's a strategy choice,
not a delay.

The benchmark is `bench-live.toml`: the same settings in every part, quoting
live, requoting often to get plenty of samples. Run it for ten minutes, then
Ctrl-C (the bot cancels its orders and prints the scoreboard):

```sh
cargo run --release -- run bench-live.toml
```

This trades real money (`bench-live.toml` is live), with the key from
[Going live](../../README.md#going-live).

A run places several hundred orders, so a per-order p99 rests on its slowest
handful: it's real, but it moves tens of milliseconds between two identical
runs. Read p50 as the trend, and trust a p99 change only when it's large.

The goal for the series: end-to-end under 20 ms p50.

## Clocks

Binance → us and order-to-live compare an exchange's clock with ours, so ours
must be right. (End-to-end uses Binance's and Bullet's clocks only, and
tick-to-trade only ours.) Your computer keeps time with a quartz crystal,
which ticks at a slightly wrong rate that changes with temperature: typically
tens of parts per million, a few seconds a day. Operating systems correct it
by asking time servers ([NTP](https://en.wikipedia.org/wiki/Network_Time_Protocol))
every so often, and in between the clock wanders. Laptops are worst: they
sleep, change networks and correct lazily. Ours was 288 ms behind real time
one afternoon and 407 ms behind a few hours later.

> **Incident: messages from the future.** The first telemetry run reported
> every network hop as zero. The laptop's clock was 288 ms behind, so every
> Binance and Bullet message "arrived before it was sent", and the code was
> quietly rounding the negative times up to zero. *Fix:* the bot checks
> instead of trusting. A message can't arrive before it was sent, and Bullet
> can't open our order before we decided to send it or after we heard it
> did. When a clock breaks those rules, the table says which one and by how
> much, instead of printing nonsense. *Lesson:* a measurement you can't
> sanity-check is a guess.

The checks catch a clock that's badly off, not one that's a little off: a
laptop clock 5 ms behind still passes, and quietly makes binance→us 5 ms too
long. On a laptop, sync before you measure (and check with
`sntp time.apple.com`):

```sh
sudo sntp -sS time.apple.com    # macOS
sudo chronyc makestep           # Linux with chrony
```

Servers run [chrony](https://chrony-project.org/), which corrects
continuously; on AWS it syncs to a source inside the data centre and holds
the clock within microseconds (Part 4).

## What we measured

A 4-minute dry run on a laptop, debug vs release, side by side:

| Hop | debug p50 / p99 | release p50 / p99 |
|---|---|---|
| decide | 5 µs / 50 µs | 1 µs / 20 µs |
| tick-to-trade | 24 µs / 230 µs | 7 µs / 134 µs |
| queue | 10 µs / 200 µs | 10 µs / 300 µs |
| inside Binance | ~0 / 31 ms | ~0 / 31 ms |

Three lessons, with the scoreboard below:

1. **Our code is not the problem.** Deciding takes microseconds. Release
   makes it 3-5x faster at p50, and it still doesn't matter next to the
   network, which is milliseconds.
2. **Binance itself has a tail.** Usually it sends a book change the same
   millisecond it happens, but 1% of the time it's 30+ ms late. No amount of
   tuning on our side fixes that; we can only notice it.
3. **Waiting on the network is the real cost.** The bot waits for each
   request to Bullet to finish before reading the next event, and from a
   laptop each one is a long round trip. Every Binance update that arrives
   meanwhile sits in the queue. Part 4 makes each round trip shorter; Part 6
   stops waiting for them.

## Scoreboard

This part's benchmark, 10 minutes live from a laptop:

| | end-to-end p50 / p99 | Binance → us p50 | tick-to-trade p50 | order-to-live p50 / p99 | queue p99 / max |
|---|---|---|---|---|---|
| Part 3 (laptop) | 224 / 383 ms | (clock off) | 11 µs | (clock off) | 464 / 828 ms |

The baseline: 224 ms from a Binance update to our order resting on Bullet,
and our code is 11 µs of it. Most of the rest is waiting: the bot waits for
Bullet's reply to each request (about 120 ms from a laptop) before reading the
next event, so updates pile up in the queue.

The laptop's clock slipped half a second behind *during* the run, so the two
numbers that use our clock are blank. End-to-end doesn't use it (only
Binance's and Bullet's), which is why it's the headline.

## What's next

Part 4 moves the bot next to the exchanges, so every round trip it waits on
gets shorter.
