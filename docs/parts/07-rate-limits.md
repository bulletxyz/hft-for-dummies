# Part 7: Stay inside the limits

Every exchange caps how fast you may send requests, and the time you hit
the cap is the worst possible time: the market is moving fast, and you need to cancel. Code:
[`part-7`](https://github.com/bulletxyz/hft-for-dummies/tree/part-7) ·
[diff](https://github.com/bulletxyz/hft-for-dummies/compare/part-6...part-7).

## Know your limits

Bullet [documents its limits](https://tradingapi.bullet.xyz/docs/connectivity.md#rate-limits):

| What | Limit | Over it |
|---|---|---|
| HTTP requests, per IP | 50/s, bursts of 150 | `429` from Cloudflare |
| Transactions (orders and cancels), per signing key | 240/s | `503 The sender was rate-limited` |

Our bot sends well under one transaction a second, nowhere near 240. But a
bug, a fast market or an aggressive setting can change that in an instant,
so we budget ourselves well below the exchange's cap and find out on our own
terms.

## A token bucket

The standard tool (`src/rate_limit.rs`): a bucket holds up to N tokens and
refills at N per second. Each transaction spends one; when the bucket is
empty, you wait. Short bursts are fine, the average can't exceed the rate.
`max_requests_per_sec` in `bot.toml` sets ours (shipped as 5; required).

## Cancels first

Not all requests are equal. A new order *adds* risk; a cancel *removes* it.
So in the executor:

- Requests with new orders must leave a quarter of the bucket in reserve. If
  they can't, they're held back: the quote they'd have placed would be stale
  by the time a token freed up anyway.
- Cancels can use the reserve, and if the bucket is truly empty they wait
  for the next token rather than give up.
- When a quote move is held back, the old order is cancelled instead (it would
  otherwise rest at a price we no longer want).

## Not every reject is an error

In Part 6 any rejected request made the bot cancel everything and start
again. (A reminder: our quotes are *post-only*: they may only rest on the
book, and Bullet rejects one that would trade immediately.) A rejected
request did nothing, so the old quote it meant to move still rests. The
executor now sorts rejects (`bullet::classify`):

| Reject | Meaning | What the bot does |
|---|---|---|
| post-only would cross | the book moved while our order was in flight | cancel the old quote; requote |
| rate limited | over a limit | cancel the old quote, empty our bucket (back off), requote later |
| anything else | unknown | cancel everything and start fresh |

> **Incident: the misleading error.** Before this part, a post-only order
> that would have crossed was logged as "placing orders failed (often: not
> enough margin)". The account had plenty of margin; the price had simply
> moved while the order was in flight. A reader chasing that log line would
> have deposited money for nothing. *Lesson:* an error message is a claim;
> make it a true one.

Treating every reject the same, either all retried or all fatal, is how bots
end up hammering an exchange or stopping for no reason.

## What we measured

Live, 3 minutes each, with the bot set to requote on every 0.5 bp move and
no minimum gap between requotes:

| | budget 50/s | budget 2/s |
|---|---|---|
| order requests per second | 0.32 | 0.44 |
| quote moves / plain places | 57 / 1 | 49 / 31 |
| held back by our own budget | 0 | 55 |
| rejected by Bullet for rate | 0 | 0 |

Two lessons:

1. **A market maker sends far fewer requests than you'd think.** Even
   requoting on every half-basis-point move, a calm market gave us one request every
   two to three seconds: about 0.1% of Bullet's limit. Rate
   limits bite in bursts (a fast market, a reconnect, a bug), which is why
   the budget is there.
2. **Too tight a budget backfires.** At 2/s, 55 quote moves were held back. Each
   became a cancel now and a fresh order later: *more* requests in total
   (0.44/s vs 0.32/s), and more time with no quote on the book. Pick a
   budget comfortably above your normal peak and well below the exchange's
   cap. The default of 5/s is both.

## Scoreboard

The [standard benchmark](03-telemetry.md#the-scoreboard), live on the Tokyo
server over Bullet's direct endpoint, against Part 6:

| | end-to-end p50 / p99 | Binance → us p50 | tick-to-trade p50 | order-to-live p50 / p99 | queue p99 / max |
|---|---|---|---|---|---|
| Part 6 | 5.6 / 71 ms | 1.5 ms | 2 µs | 3.9 / 70 ms | 0.01 / 20 ms |
| **Part 7** | **6.0 / 162 ms** | 1.5 ms | 2 µs | 3.9 / 161 ms | 0.01 / 13 ms |

Same as Part 6, within noise: the budget only bites in bursts. The p99
swings a lot between runs (that's Bullet's tail), so don't read the change
there as this part's doing.

Both rows are medians of three 5-minute runs taken in rotation, so they
saw the same conditions at Bullet.

## What's next

The bot is now correct and polite. The next parts make it steady: Part 8
tunes the Linux server it runs on.
