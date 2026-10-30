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
| Sequential (Ethereum, Bullet's `Nonce` mode) | must be exactly last + 1 | one: the next can't be signed validly until you know the last landed |
| Highest-N set ([Hyperliquid](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/nonces-and-api-wallets)) | any unused value above the smallest of your 100 highest, within a time window | many |
| Window (Bullet's [`Window` mode](https://tradingapi.bullet.xyz/docs/tx-fields.md)) | unique per key within a moving window | many |

With a sequential nonce, two transactions sent at once race: if the second
arrives first, it's rejected. So you wait for each one, which is exactly the
problem Part 6 worked around. Bullet's SDK uses window mode by default with a
millisecond counter (`max(now, last + 1)`), so every transaction gets a fresh
value and order doesn't matter.

## Try it

```sh
cargo run --release --example in_flight
```

`examples/in_flight.rs` sends five harmless transactions ("cancel all my
SOL-USD orders" while there are none), first one after another, then all at
once:

```text
5 transactions one after another:  540 ms
5 transactions all at once:        174 ms (5/5 accepted)
```

Five round trips versus about one and a half. That's what the nonce scheme
buys you.

## Using it: a lane for pulls

Since Part 6 the executor sends one batch at a time. That keeps each quote's
requests in order (an amend of an order must arrive after the order), but it
means "cancel everything" can sit behind an amend that's in flight for 100+
ms, at exactly the moment you want out of the market.

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

Live, 4 minutes quoting 2 bps wide: 44 fills, every pull (`pull: decided → all
cancelled`) completed in one round trip (~115 ms) instead of waiting for
in-flight amends, and the bot's final position matched the exchange's.

## Scoreboard

The [standard benchmark](03-telemetry.md#the-scoreboard), 10 minutes live on the Tokyo server, against Part 10 (milliseconds):

| | tick-to-trade p50 / p99 | Binance → us p50 | queue p99 / max | Bullet accepted p50 |
|---|---|---|---|---|
| Part 10 | 1.9 / 58 | 1.5 | 0.01 / 2.9 | 21 |
| **Part 11** | **1.9 / 61** | 1.6 | 0.01 / 3.0 | 21 |

Unchanged: the pull lane matters when cancelling under pressure, not on the
normal requote path.

## Where we ended up

From Part 2 to here, our own reaction (tick in, orders out) went to about 10
µs, Binance's prices reach us in 1.5 ms, and nothing we do blocks on the
exchange. What's left is the trip to Bullet's matching engine; Part 10 shows
where that goes.
