# Part 6: Don't wait for the exchange

An order request to Bullet takes ~100-500 ms to come back. Until now the bot
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
  possible, but needs care with transaction nonces: that's Part 11.
- **Shutdown waits.** On Ctrl-C the bot queues "cancel everything" and then
  waits for the worker to finish (`finish()`), or it could exit with orders
  still resting.

## Amend, don't cancel and re-place

Moving a quote used to be two requests: cancel the old order, place a new
one. Between the two, we had no quote on the book at all. Bullet supports
**amending** (`AmendOrders`): cancel-and-replace in one transaction. One
request instead of two (every Bullet transaction also costs a little gas),
and the book never sees us disappear.

## The race: an order fills while you cancel it

You decide to move your bid. In the ~100 ms your amend takes to arrive,
someone sells into the old bid. When the amend lands, the old order no
longer exists, and Bullet rejects the request with
`client_order_id_not_found`. Because a transaction is all or nothing, the
*other* orders in it weren't touched either.

The old response was blunt: cancel everything, start again. Now the worker
reads the error (`bullet::order_not_found`), removes the order that's gone
from the request, sends the rest again, and places the replacement as a fresh
order. The fill itself arrives as an event like any other fill.

## What we measured

Live from a laptop, 5 minutes each, quoting 10 bps wide:

| | Part 5 | Part 6 |
|---|---|---|
| bot: received → handled, p99 | 2,802 ms | 14 ms |
| bot: received → handled, max | 5,014 ms | 271 ms |
| requests per quote move | 2 | 1 |

## What's next

Without waiting, nothing stops the bot from sending requests as fast as the
market moves. Every exchange has limits; Part 7 keeps us inside them.
