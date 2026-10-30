# Part 6: Don't wait for the exchange

An order request to Bullet takes tens of milliseconds to come back from
Tokyo, over 100 from a laptop, and far longer in the tail. Until now the bot
sat and waited for each one, deaf to Binance the whole time. This part stops
the waiting, halves the number of requests, and handles the race every market
maker hits sooner or later. Code:
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

## Amend, don't cancel and re-place

Moving a quote used to be two requests: cancel the old order, place a new
one. Between the two, we had no quote on the book at all. Bullet supports
**amending** (`AmendOrders`): cancel-and-replace in one transaction. One
request instead of two (every Bullet transaction also costs a small fee),
and the book never sees us disappear.

## The race: an order fills while you cancel it

You decide to move your bid. In the milliseconds your amend takes to arrive,
someone sells into the old bid. When the amend lands, the old order no
longer exists, and Bullet rejects the request with
`client_order_id_not_found`. Because a transaction is all or nothing, the
*other* orders in it weren't touched either.

The old response was blunt: cancel everything, start again. Now the worker
reads the error (`bullet::order_not_found`; Part 7 generalises it), removes the order that's gone
from the request, sends the rest again, and places the replacement as a fresh
order. The fill itself arrives as an event like any other fill.

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

The [standard benchmark](03-telemetry.md#the-scoreboard), 10 minutes live
on the Tokyo server, against Part 5 (milliseconds):

| | tick-to-trade p50 / p99 | Binance → us p50 | queue p99 / max | Bullet accepted p50 |
|---|---|---|---|---|
| Part 5 | 37.2 / 204 | 1.5 | 199 / 449 | 27 |
| **Part 6** | **1.7 / 51** | 1.5 | 0.01 / 0.03 | 17 |

The big one: 37 ms to 1.7 ms. With nothing waiting, the queue is empty and
tick-to-trade is almost all Binance's own 1.5 ms. That beats the series goal
of 20 ms, and later parts improve what's left: the tail.

## What's next

Without waiting, nothing stops the bot from sending requests as fast as the
market moves. Every exchange has limits; Part 7 keeps us inside them.
