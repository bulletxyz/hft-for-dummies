# Part 8: Give the bot its own cores

Our code takes microseconds, but only if the CPU is free when an event
arrives. On a real server something else always wants the CPU: logging,
monitoring agents, a backup, a compile. This part shows what a noisy
neighbour does to the bot and how Linux lets you fence the bot off. Code:
[`part-8`](https://github.com/bulletxyz/hft-for-dummies/tree/part-8) ·
[diff](https://github.com/bulletxyz/hft-for-dummies/compare/part-7...part-8).

## What we're optimising: jitter

Not average speed: *consistency*. A trading bot is judged by its slowest
moments (p99 and max), and those come from the operating system making you
wait: another process on your core, the core waking from a power-saving sleep,
the network card interrupting you mid-calculation.

## Two threads, two cores

The bot now runs on exactly two threads: the Binance feed (Part 5) and
everything else (one `tokio` current-thread runtime). Two threads are easy to
reason about and easy to pin:

```toml
feed_core = 1   # in bot.toml
bot_core = 2
```

The server is now a 4-core `c8g.xlarge`: cores 1-2 for the bot, 0 and 3 for
everything else. (Everything else includes compiling: once cores 1-2 are
isolated, `cargo build` on the server only gets two cores and takes about
twice as long.)

## One knob at a time

`infra/bot/tune.sh` turns one knob per command, so you can measure each:

```sh
./infra/bot/deploy.sh tune status     # what's on now
./infra/bot/deploy.sh tune isolate    # keep the OS off cores 1-2 (reboots)
./infra/bot/deploy.sh tune tuned      # network-latency profile
./infra/bot/deploy.sh tune irqs       # network interrupts on core 0 only
./infra/bot/deploy.sh tune coalesce   # NIC interrupt batching off
./infra/bot/deploy.sh tune reset      # undo them all (reboots if isolated)
```

- **`isolate`** adds [`isolcpus=1,2 nohz_full=1,2 rcu_nocbs=1,2`](https://docs.kernel.org/admin-guide/kernel-parameters.html)
  to the kernel command line: the scheduler puts nothing on cores 1-2 unless
  asked to (our pinned threads are), stops the timer tick there, and moves
  kernel housekeeping elsewhere.
- **`tuned`** applies the [`network-latency`](https://github.com/redhat-performance/tuned)
  profile: CPUs stay at full speed and out of deep sleep states, and sockets
  are busy-polled.
- **`irqs`** sends the network card's interrupts to core 0 and stops
  `irqbalance` moving them back.
- **`coalesce`** stops the network card waiting (20 µs by default) to batch
  several packets into one interrupt.

## What we measured

Tokyo server, dry run, 4 minutes per step. To make jitter visible, steps 2-5
run [`stress-ng --cpu 4`](https://github.com/ColinIanKing/stress-ng) alongside:
a CPU hog on every core, our noisy neighbour.

| Step | received → handled p99 / max | decide max |
|---|---|---|
| 1. quiet server | 5 µs / 22 µs | 24 µs |
| 2. + noisy neighbour | 66 µs / **9.5 ms** | 175 µs |
| 3. + threads pinned | 13 µs / **9.7 ms** | 36 µs |
| 4. + cores isolated | 5 µs / 20 µs | 23 µs |
| 5. + tuned, IRQs, no batching | 5 µs / 25 µs | 23 µs |

- **A neighbour made the bot's worst case 400x worse** (22 µs to 9.5 ms).
- **Pinning alone isn't enough.** Our thread stays on its core, but the hog
  can still land there too: p99 improves, the worst case doesn't.
- **Isolation fixes it.** With cores 1-2 reserved, the loaded server behaves
  like an idle one.
- **The rest didn't show here.** `tuned`, IRQ placement and coalescing save
  microseconds in the kernel and network card. Our network numbers swing
  between ~17 and ~75 ms p99 from run to run (that's Binance and the
  internet), which drowns them. Even a direct test (Part 10) found round trips
  to Binance unchanged with them on or off. They matter once everything else
  is in order; don't expect them to rescue a slow design.

## Scoreboard

The [standard benchmark](03-telemetry.md#the-scoreboard), 10 minutes live on the Tokyo server, against Part 7 (milliseconds):

| | tick-to-trade p50 / p99 | Binance → us p50 | queue p99 / max | Bullet accepted p50 |
|---|---|---|---|---|
| Part 7 | 1.8 / 79 | 1.5 | 0.01 / 28 | 19 |
| **Part 8** | **2.2 / 27** | 1.5 | 0.01 / 3.0 | 15 |

The p50 stays at Binance's 1.5 ms, which tuning can't touch. The queue's
worst case fell from 28 ms to 3 ms, and this was the best p99 of any run. One
run doesn't prove a p99, though; the noisy-neighbour test above is the real
evidence.

## What's next

With the machine quiet, the remaining code-side time is ours. Part 9 profiles
it and removes waste.
