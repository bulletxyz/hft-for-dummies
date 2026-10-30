# Part 6: Don't wait for the exchange

An order request to Bullet takes tens of milliseconds to come back from
Tokyo, over 100 from a laptop, and far longer in the tail. Until now the bot
sat and waited for each one, deaf to Binance the whole time. This part stops
the waiting, halves the number of requests, and sidesteps the race every
market maker hits sooner or later. Code:
[`part-6`](https://github.com/bulletxyz/hft-for-dummies/tree/part-6) ·
[diff](https://github.com/bulletxyz/hft-for-dummies/compare/part-5...part-6).

## The cost of waiting

Part 3's telemetry, live: `bot: received → handled` had a p99 of **2.8
seconds**. That's how long Binance ticks sat in our queue while the loop was
stuck on an order request. If the price moved in that window (and when the
price moves is exactly when you're requoting), we didn't see it.

## Hand off, don't wait

The live executor (`src/executor.rs`) now passes each batch of intents to a
**worker task** over a channel and returns immediately. The worker talks to
Bullet; the loop goes straight back to reading prices.

This is where Part 2's event-driven layout pays off. The market maker never
waited for answers anyway: it updates its own view the moment it asks for an
order ("I now have a bid at 100.12"), and the executor reports problems back
as events (`PlaceFailed`, `CancelFailed`). So moving the executor into its own
task changed nothing else in the bot.

Two details:

- **Order is kept.** The worker runs one batch at a time, so a cancel always
  lands before the order that replaces it. Sending several requests at once is
  possible (Part 11), but then they can land out of order.
- **Shutdown waits.** On Ctrl-C the bot queues "cancel everything" and then
  waits for the worker to finish (`finish()`), or it could exit with orders
  still resting.

## One request per move

Moving a quote used to be two requests: cancel the old order, place a new
one. Between the two, we had no quote on the book at all. Now each batch is
one `CancelAndPlaceOrders` transaction (`bullet::cancel_and_place`), the call
Part 5 introduced for expiry, with the old order in its cancel list. One
request instead of two (every Bullet transaction also costs a small fee),
and the book never sees us disappear.

## The race: an order fills while you cancel it

You decide to move your bid. In the milliseconds your request takes to
arrive, someone sells into the old bid. When it lands, the old order no
longer exists. Bullet's other way to move an order, `AmendOrders`, rejects
the whole request in that case (`client_order_id_not_found`), and the bot has
to work out which order went and try again. `CancelAndPlaceOrders` skips a
cancel for an order that's already gone and places the new one anyway, so
the race needs no special handling. The fill itself arrives as an event like
any other fill.

If the request fails for another reason (not enough margin, say), nothing in
it happened. The worker reports that, and the strategy cancels everything and
starts its quotes afresh.

> **Incident: the confirmation that beat its request.** After this change,
> the telemetry's "placed → live on book" row went empty. Bullet's websocket
> was announcing our new orders *before* the HTTP reply to our own request
> arrived, and the telemetry only expected them the other way round. *Fix:*
> match the two in whichever order they come. *Lesson:* once nothing waits,
> events arrive in orders you didn't plan for.

## What we measured

Live from a laptop, 5 minutes each, quoting 10 bps wide:

| | waiting (Parts 3-5) | Part 6 |
|---|---|---|
| bot: received → handled, p99 | 2,802 ms | 14 ms |
| bot: received → handled, max | 5,014 ms | 271 ms |
| requests per quote move | 2 | 1 |

## Scoreboard

The [standard benchmark](03-telemetry.md#the-scoreboard), live on the Tokyo
server over Bullet's direct endpoint, against Part 5:

| | end-to-end p50 / p99 | Binance → us p50 | tick-to-trade p50 | order-to-live p50 / p99 | queue p99 / max |
|---|---|---|---|---|---|
| Part 5 | 25.5 / 121 ms | 1.6 ms | 1 µs | 23.2 / 119 ms | 100 / 540 ms |
| **Part 6** | **5.6 / 71 ms** | 1.5 ms | 2 µs | 3.9 / 70 ms | 0.01 / 20 ms |

The big one: 25 ms to under 6. One request per quote move and no waiting:
order-to-live drops from 23 ms to about 4, and the queue empties. That beats
the series goal of 20 ms, and what's left is mostly Bullet opening our order.

Part 6 is the median of three 5-minute runs (see the README's scoreboard);
Part 5 is a single 10-minute run.

## What's next

Without waiting, nothing stops the bot from sending requests as fast as the
market moves. Every exchange has limits; Part 7 keeps us inside them.
