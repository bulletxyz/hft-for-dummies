# Part 11 (bonus): Nonces, or how many things can you do at once?

Every Bullet transaction is signed, and a signed message could be captured and
sent again ("replayed"). Exchanges stop that with a **nonce**: a number in each
transaction that's only accepted once. How the exchange checks it decides
whether you can have more than one request in flight. Code:
[`part-11`](https://github.com/bulletxyz/hft-for-dummies/tree/part-11) ·
[diff](https://github.com/bulletxyz/hft-for-dummies/compare/part-10...part-11).

## Three schemes

| Scheme | Rule | In flight at once |
|---|---|---|
| Sequential (Ethereum, Bullet's `Nonce` mode) | must be exactly last + 1 | one: the next is rejected if it arrives before the last |
| Highest-N set ([Hyperliquid](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/nonces-and-api-wallets)) | any unused value above the smallest of your 100 highest, within a time window | many |
| Window (Bullet's [`Window` mode](https://tradingapi.bullet.xyz/docs/tx-fields.md)) | unique per key within a moving window | many |

With a sequential nonce, two transactions sent at once race: if the second
arrives first, it's rejected. So you wait for each one, which is what Parts 3-5
did, and Part 6's worker still sends one batch at a time. Bullet's SDK uses window mode by default with a
millisecond counter (`max(now, last + 1)`), so every transaction gets a fresh
value and order doesn't matter.

## Try it

```sh
cargo run --release --example in_flight
```

`examples/in_flight.rs` sends five harmless transactions ("cancel all my
SOL-USD orders" while there are none), first one after another, then all at
once. It's live: it cancels any SOL-USD orders you have resting, so don't
run it while a bot is quoting. From a laptop:

```text
5 transactions one after another:  540 ms
5 transactions all at once:        174 ms (5/5 accepted)
```

Five round trips versus about one and a half. That's what the nonce scheme
buys you.

## Using it: a lane for pulls

Since Part 6 the executor sends one batch at a time. That keeps each quote's
requests in order (a move of an order must arrive after the order), but it
means "cancel everything" can sit behind a request that's in flight for tens of
milliseconds (far longer in the tail), at exactly the moment you want out of the market.

The executor now has two lanes (`src/executor.rs`):

- the **order lane**: quotes, one batch at a time, as before;
- the **pull lane**: any batch that starts with "cancel everything", sent
  straight away, alongside whatever the order lane has in flight.

Two requests in flight need one new rule. If a pull happens while the order
lane is placing, the cancel-all can land *first*, and the new orders would be
left resting after we pulled. So the order lane notes a pull counter before
each request and, if it changed by the time the request returns, cancels the
orders it just placed (`undo_if_pulled`).

## What we measured

Live from a laptop, 4 minutes quoting 2 bps wide: 44 fills, every pull (`pull: decided → all
cancelled`) completed in one round trip (~115 ms) instead of waiting for
in-flight requests, and the bot's final position matched the exchange's. On
the Tokyo server a pull takes about 8.7 ms from decision to all cancelled
(p50).

## Scoreboard

The [standard benchmark](03-telemetry.md#the-scoreboard), live on the Tokyo
server over Bullet's direct endpoint, against Part 10:

| | end-to-end p50 / p99 | Binance → us p50 | tick-to-trade p50 | order-to-live p50 / p99 | queue p99 / max |
|---|---|---|---|---|---|
| Part 10 | 6.3 / 27 ms | 1.5 ms | 1 µs | 4.7 / 23 ms | 0.01 / 21 ms |
| **Part 11** | **5.4 / 47 ms** | 1.5 ms | 1 µs | 3.4 / 34 ms | 0.01 / 21 ms |

Unchanged: the pull lane matters when cancelling under pressure, not on the
normal requote path.

Both rows are medians of three 5-minute runs taken in rotation, so they
saw the same conditions at Bullet.

## Where we ended up

End-to-end, from a Binance update to our order open on Bullet, went from 211
ms p50 on a laptop (Part 3) to about 5.4 ms on the Tokyo server. Our own
reaction (tick-to-trade) is a microsecond or two, Binance's prices reach us
in 1.5 ms, and nothing we do blocks on the exchange. Most of what's left is
Bullet opening our order, about 3.5 ms over its direct endpoint; Part 10 shows
where that goes, and what the route costs without it.
