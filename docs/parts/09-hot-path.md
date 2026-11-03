# Part 9: The hot path

The *hot path* is the code that runs on every market update: parse the
message, update the book, decide. It runs hundreds of times a second, so
small waste there adds up and, worse, adds jitter. This part measures it
properly, then removes the waste the measurements point to. Code:
[`part-9`](https://github.com/bulletxyz/hft-for-dummies/tree/part-9) ·
[diff](https://github.com/bulletxyz/hft-for-dummies/compare/part-8...part-9).

## Microbenchmarks

Live telemetry can't see a 200 ns change under milliseconds of network, so
we time each step on its own with [criterion](https://bheisler.github.io/criterion.rs/book/):

```sh
cargo bench
```

`benches/hot_path.rs` times parsing a Binance message, applying a Bullet book
update, and the market maker's decision. (To make that possible the crate is
now a library, `src/lib.rs`, with a thin `src/main.rs`.)

**Benchmark the before and after back to back.** Our first "before" run came
out 3-5x slower than a rerun minutes later, because the laptop was busy. The
"decide" benchmark, which we didn't touch, is the control: if it moves, the
machine moved, not your code.

## What we changed

**Share the book instead of copying it.** Every book update used to produce a
fresh copy of the whole book (20 levels a side) for the event, and the market maker
copied it again to keep it. Now the book is an
[`Arc`](https://doc.rust-lang.org/std/sync/struct.Arc.html) (a shared pointer):
the event and the market maker point at the same book, and an update changes
it in place unless someone still holds the old version, in which case it
copies once ([copy-on-write](https://doc.rust-lang.org/std/sync/struct.Arc.html#method.make_mut)).

**Parse without allocating.** Binance sends prices as strings. We used to copy
each into a new `String` before converting it; now we borrow the text straight
out of the message (`&str`).

**Log from a background thread.** Writing a log line can block on a slow
terminal or disk. [`tracing-appender`](https://docs.rs/tracing-appender)'s
non-blocking writer hands lines to a background thread instead.

**rustls instead of OpenSSL.** The Binance websocket now uses
[rustls](https://github.com/rustls/rustls), a TLS library written in Rust, like
the rest of the bot already did. One TLS stack instead of two, and no OpenSSL
to install on the server.

## What we measured

Back to back on the same laptop:

| Benchmark | Before | After | |
|---|---|---|---|
| apply a Bullet book update | 597 ns | 317 ns | **-47%** |
| parse a Binance bookTicker | 432 ns | 404 ns | -7% |
| decide (unchanged, the control) | 190 ns | 190 ns | 0% |

- **Copies were the real cost.** Sharing the book nearly halved the update.
- **Avoiding allocations in parsing barely mattered.** The time goes into
  scanning JSON and converting text to decimal numbers, not the allocations.
  That's why you measure before optimising: the "obvious" fix was the small one.
- **All of this is sub-microsecond.** It reduces CPU use and jitter, but the
  network is still thousands of times bigger. Optimise the hot path last.

## Profile the real thing

A benchmark times what you chose to time. A *profiler* samples the running
program and tells you where the time actually goes. On the Tokyo server,
with the bot running:

```sh
sudo dnf install -y --allowerasing perf
sudo perf record -F 999 -g -p $(systemctl show -p MainPID --value hft-bot) -- sleep 45
sudo perf report --no-children --sort dso,sym
```

(On a laptop, [samply](https://github.com/mstange/samply) does the same and
opens the result in your browser; on Linux, [cargo-flamegraph](https://github.com/flamegraph-rs/flamegraph)
draws it as a flame graph.) What we saw:

| Where | Share of samples |
|---|---|
| the Linux kernel | 84% |
| our bot | 8% |
| libc | 4% |

Most of the kernel's share is most likely the `tuned` profile from Part 8
*busy-polling*:
instead of sleeping until a packet arrives, the CPU spins checking the
network card, to pick packets up microseconds sooner. That's a deliberate
trade (a core spent for latency), which is why the bot needs cores of its own.

Within our 8%, nothing stands out: atomic reference counts, reading the
websocket, JSON parsing, the scheduler, each under 1%. A flat profile like
this means there's no single hotspot left worth chasing; the remaining wins
are elsewhere.

## Scoreboard

The [standard benchmark](03-telemetry.md#the-scoreboard), live on the Tokyo
server over Bullet's direct endpoint, against Part 8:

| | end-to-end p50 / p99 | Binance → us p50 | tick-to-trade p50 | order-to-live p50 / p99 | queue p99 / max |
|---|---|---|---|---|---|
| Part 8 | 5.8 / 63 ms | 1.5 ms | 1 µs | 3.9 / 59 ms | 0.01 / 19 ms |
| **Part 9** | **5.5 / 53 ms** | 1.4 ms | 1 µs | 3.7 / 47 ms | 0.01 / 16 ms |

Within noise, as predicted: the gains above are nanoseconds.

Both rows are medians of three 5-minute runs taken in rotation, so they
saw the same conditions at Bullet.

## What's next

Part 10 goes back to the network: what else AWS can do for the last mile.
