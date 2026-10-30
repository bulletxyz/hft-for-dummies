# Part 5: Sockets that fail loudly

Every input to the bot arrives over a websocket: Binance prices, Bullet's
book, our own fills. Part 4 made those connections short. This part makes
sure that when one breaks, we know within seconds. Code:
[`part-5`](https://github.com/bulletxyz/hft-for-dummies/tree/part-5) ·
[diff](https://github.com/bulletxyz/hft-for-dummies/compare/part-4...part-5).

## Why streaming, and why it's not enough

Polling (asking "what's the price?" over REST every N ms) costs a round trip
per question and is always up to N ms stale. A websocket pushes each change
the moment it happens, which is why the bot has used websockets for all
market data since Part 2. REST is only for setup and checks.

The catch: a websocket can die **silently**. If a router drops your packets,
nothing sends an error. TCP will keep "waiting" for minutes, and your bot
keeps quoting off the last price it saw. We met this in Part 2, when a wifi
blip left the Binance socket half-open until a restart. The general rule:
**silence is not the same as "nothing happened"**.

## Heartbeats

The fix is to make silence impossible on a healthy connection. Every second
the bot sends a websocket **ping**; the server must answer with a **pong**.
Prices and pongs both count as "heard from". If we hear nothing for 3
seconds, the connection is dead: drop it and reconnect.

- Binance (`src/binance.rs`): our own ping loop.
- Bullet (`src/bullet.rs`): the SDK's managed socket reconnects after an
  `idle_timeout` (default 60 s; we set 3 s) and we ping it every second.

Why 3 seconds? Long enough that one slow pong isn't a false alarm on a
datacentre network (the Tokyo server had none), short enough that we don't
quote on a dead feed for long.

> **Incident: a false alarm.** Testing from a laptop, one connection hit the
> 3-second limit with nothing wrong: wifi delivered a pong late. The bot
> pulled its quotes, reconnected and carried on, which is the right outcome
> for a false alarm. The Tokyo server never had one. *Lesson:* every timeout
> trades false alarms against reaction time; pick it for where the bot
> actually runs. Remember the
stale-price guard pulls our quotes even sooner (after `reference_stale_secs`),
so we're never quoting blind while we wait.

## One connection per job

Bullet's order book (many updates a second) and our own orders and fills
(rare, but crucial) now come over **separate** connections. A burst of book
updates can't queue in front of a fill, and if one connection drops, the
other keeps going. After any reconnect the bot still checks its fills
against the exchange's record (Part 2): a connection that's back up isn't
necessarily caught up.

The Binance feed also gets its own OS thread. Honestly, that changes little
today, because `tokio` already spreads tasks over several threads. It
separates the hot input from everything else, which starts to matter as the
main loop gets busier in Part 6.

## Test it yourself

Pull the plug on purpose. On the Tokyo server, `chaos.sh` adds a
*blackhole route* to the exchange (packets silently vanish, the way a dead
link behaves) and times how long the bot takes to notice:

```sh
./infra/bot/deploy.sh chaos binance
./infra/bot/deploy.sh chaos bullet
```

| Connection cut | Part 4 noticed after | Part 5 |
|---|---|---|
| Binance | 10.1 s (silence timeout) | 3.1 s |
| Bullet | 30.7 s (SDK default) | 2.9 s |

## Scoreboard

The [standard benchmark](03-telemetry.md#the-scoreboard), 10 minutes live on the Tokyo server, against Part 4 (milliseconds):

| | tick-to-trade p50 / p99 | Binance → us p50 | queue p99 / max | Bullet accepted p50 |
|---|---|---|---|---|
| Part 4 | 35.7 / 306 | 1.6 | 340 / 565 | 27 |
| **Part 5** | **37.2 / 204** | 1.5 | 199 / 449 | 27 |

No change, as expected. This part is about noticing a dead connection in
3 seconds instead of 30, and a healthy run never exercises that. (The p99s
differ, but p99 moves this much between identical runs.)

## What's next

The bot still waits for every order request to finish before reading the
next event. While it waits ~100 ms for Bullet, Binance can move, and we're
not looking. Part 6 fixes that.
