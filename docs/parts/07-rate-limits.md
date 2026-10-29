# Part 7: Stay inside the limits

Since Part 6 nothing stops the bot from sending requests as fast as the
market moves. Every exchange caps that, and the time you hit the cap is the
worst possible time: the market is moving fast, and you need to cancel. Code:
[`part-7`](https://github.com/bulletxyz/hft-for-dummies/tree/part-7) ·
[diff](https://github.com/bulletxyz/hft-for-dummies/compare/part-6...part-7).

## Know your limits

Bullet [documents its limits](https://tradingapi.bullet.xyz/docs/connectivity.md#rate-limits):

| What | Limit | Over it |
|---|---|---|
| HTTP requests, per IP | 50/s, bursts of 150 | `429` from Cloudflare |
| Transactions (orders, cancels, amends), per signing key | 240/s | `503 The sender was rate-limited` |

Our bot sends a couple of transactions a second, nowhere near 240. But a
bug, a fast market or an aggressive setting can change that in an instant,
so we budget ourselves well below the exchange's cap and find out on our own
terms.

## A token bucket

The standard tool (`src/rate_limit.rs`): a bucket holds up to N tokens and
refills at N per second. Each transaction spends one; when the bucket is
empty, you wait. Short bursts are fine, the average can't exceed the rate.
`max_requests_per_sec` in `bot.toml` sets ours (default 5).

## Cancels first

Not all requests are equal. A new order *adds* risk; a cancel *removes* it.
So in the executor:

- New orders and amends must leave a quarter of the bucket in reserve. If
  they can't, they're held back: the quote they'd have placed would be stale
  by the time a token freed up anyway.
- Cancels can use the reserve, and if the bucket is truly empty they wait
  for the next token rather than give up.
- When an amend is held back, the old order is cancelled instead (it would
  otherwise rest at a price we no longer want).

## Not every reject is an error

Part 6 handled one reject ("order not found"). The executor now sorts them
(`bullet::classify`):

| Reject | Meaning | What the bot does |
|---|---|---|
| order not found | it filled or was cancelled first | drop it, retry the rest |
| post-only would cross | the book moved while our order was in flight | nothing rests; requote |
| rate limited | over a limit | empty our bucket (back off), requote later |
| anything else | unknown | cancel everything and start fresh |

Treating every reject the same, either all retried or all fatal, is how bots
end up hammering an exchange or stopping for no reason.

## What we measured

Live, 3 minutes each, with the bot set to requote on every 0.5 bp move and
no minimum gap between requotes:

| | budget 50/s | budget 2/s |
|---|---|---|
| order requests per second | 0.32 | 0.44 |
| amends / plain places | 57 / 1 | 49 / 31 |
| held back by our own budget | 0 | 55 |
| rejected by Bullet for rate | 0 | 0 |

Two lessons:

1. **A market maker sends far fewer requests than you'd think.** Even
   requoting on every half-tick move, a calm market gave us one request every
   two to three seconds, a hundredth of a percent of Bullet's limit. Rate
   limits bite in bursts (a fast market, a reconnect, a bug), which is why
   the budget is there.
2. **Too tight a budget backfires.** At 2/s, 55 amends were held back. Each
   became a cancel now and a fresh order later: *more* requests in total
   (0.44/s vs 0.32/s), and more time with no quote on the book. Pick a
   budget comfortably above your normal peak and well below the exchange's
   cap. The default of 5/s is both.

## What's next

The bot is now correct and polite. The next parts make it steady: Part 8
tunes the Linux server it runs on.
