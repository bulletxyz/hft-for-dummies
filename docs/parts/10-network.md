# Part 10: The last mile

We're in the right zone, on quiet cores, with a flat profile. What's left is
the network between us and the exchanges. This part is mostly about how to
*look* at that network, so you know which problems are yours to fix. Code:
[`part-10`](https://github.com/bulletxyz/hft-for-dummies/tree/part-10) ·
[diff](https://github.com/bulletxyz/hft-for-dummies/compare/part-9...part-10).

```sh
./infra/bot/deploy.sh netcheck
```

## Who's on the path

[`mtr`](https://github.com/traviscross/mtr) combines traceroute and ping:
every hop to a destination, its latency, and (`-z`) the *AS*, the
organisation that runs that network. From our Tokyo server:

```text
-- fstream.binance.com
  1. AS16509  ec2-54-150-128-123.ap-northeast-1.compute.amazonaws.com
-- tradingapi.bullet.xyz
  ...
  3. AS16509  99.83.116.59      (Amazon)
  4. AS13335  103.22.201.97     (Cloudflare)
  5. AS13335  104.26.9.211      1.6 ms
```

Binance is one hop away, inside Amazon's network (AS16509): we're in the same
data centre. Bullet's address belongs to Cloudflare (AS13335), a proxy in
front of Bullet's servers. That proxy turned out to matter more than
anything else on the path.

## Incident: the Dallas detour

**The symptom.** On some runs from Part 4 on, our Tokyo server saw Bullet's
order book arrive ~100 ms after Bullet stamped it, and HTTPS requests take
~220 ms. Binance,
from the same server: 1.5 ms. It looked as if Bullet simply ran far from Tokyo.

**The wrong leads.** `mtr` (above) said Cloudflare was 1.6 ms away, so it
didn't look like the network. TCP handshakes from new servers in every Tokyo
zone varied wildly between runs: 4 ms, 102 ms, 131 ms.

**The clue.** Cloudflare stamps every response with a `cf-ray` header naming
the site that handled it:

```sh
curl -sI https://tradingapi.bullet.xyz/fapi/v1/time | grep -i cf-ray
# from a laptop in Asia:     cf-ray: ...-SIN   (Singapore)
# from our Tokyo server:     cf-ray: ...-DFW   (Dallas)
```

Dallas. A few minutes later, the same request from the same server came back
from Tokyo (`NRT`), and we reran the bot:

| Cloudflare site serving us | TCP handshake | Bullet book → us (p50) |
|---|---|---|
| Dallas (`DFW`) | ~131 ms | ~102 ms |
| Tokyo (`NRT`) | 2-7 ms | **4.6 ms** |

Bullet's engine is near Tokyo after all. The extra 100 ms was a trip across
the Pacific and back.

**Why.** Cloudflare's addresses are
[anycast](https://www.cloudflare.com/learning/cdn/glossary/anycast-network/):
every Cloudflare site announces the same address, so from Tokyo your packets
enter Cloudflare in Tokyo (hence `mtr`'s 1.6 ms). But Cloudflare also moves
traffic between its sites when one is busy or expensive
([Traffic Manager](https://blog.cloudflare.com/meet-traffic-manager/)), and
moves lower-tier plans first. That evening, our requests were being handled
in Dallas. You can check where you're being served right now:

```sh
curl -s https://tradingapi.bullet.xyz/cdn-cgi/trace | grep colo
```

**What venues do about it.** No setting on our side changes Cloudflare's
routing. Latency-sensitive venues keep a CDN in front of the public API (it
absorbs attacks and abuse) and give vetted market makers a direct route: a
regional host, an AWS [PrivateLink](https://aws.amazon.com/privatelink/)
endpoint, a cross-connect. Bullet has one: a direct endpoint for whitelisted
market makers. Point the bot at it with `api_url` in `bot.toml`. The server's
Elastic IP (`terraform output ip`) is what gets whitelisted.

We got whitelisted and ran the benchmark four times, 10 minutes each,
alternating the two routes so both saw the same market. Cloudflare served us
from Tokyo both times, its best case:

| Route (two runs each) | Bullet book → us p50 | order-to-live p50 / p99 | end-to-end p50 / p99 |
|---|---|---|---|
| direct | 1.5 ms | 4.0-4.3 / 81-83 ms | 5.9-6.7 / 92-93 ms |
| Cloudflare, served in Tokyo | 3.5-4.5 ms | 8.2-9.9 / 27-59 ms | 10.6-12.0 / 50-61 ms |

The direct route halves the typical trip, even against Cloudflare's best day.
Its p99 was worse in these runs: the slow tail is Bullet processing the
order, and ten minutes of one route against ten of the other can't tell the
routes apart there. And on a bad day the gap is far bigger: an earlier
evening, halfway through the same experiment, Cloudflare moved us to
Singapore and the order took about 190 ms to go live. That's what the direct
route really buys: the bad days go away.

If you're not a whitelisted market maker, this is the honest caveat for
everything after Part 6: your numbers to Bullet depend on where Cloudflare
serves you that day.

**Lessons.**

- **Look at the whole path, not one number.** `mtr` said 1.6 ms; the real
  route was sometimes 130 ms. `cf-ray`, `/cdn-cgi/trace`, `via` and `server`
  headers tell you who you're actually talking to.
- **Variance is the enemy.** A feed that's 5 ms most of the time and 100 ms
  some of the time is harder to trade on than a steady 20 ms: you can't tell
  which you're getting until your quotes are stale.
- **Split the trip at your own door.** The scoreboard (Part 3) separates our
  part (tick-to-trade, microseconds) from the venue's (order-to-live), so a
  slow evening at Bullet can't be mistaken for a slow bot, or the other way
  round.

## A hostname is a pool

`fstream.binance.com` resolves to many addresses, a different set each time
you ask. They aren't all equally close, so `netcheck` times each one
(`curl --resolve` connects to a chosen address):

```text
57.183.56.49    0.34 ms
52.193.15.42    0.35 ms
...
52.194.127.137  0.51 ms
```

Today they're all within 0.2 ms of each other, so it doesn't matter which we
get. Bullet's direct endpoint is different: three addresses, one load
balancer per zone, and a simple request takes about 3 ms on the nearest and
4-6 ms on the farthest. A bot could look the name up itself and stick to the
fastest. We don't: a load balancer's addresses change over time, and a
pinned address that goes away takes the bot down with it. A millisecond isn't
worth an outage.

## Orders over the websocket?

Bullet also accepts orders over its websocket (`order.place`,
`order.cancel`), on a connection that's already open. That sounds faster
than an HTTPS request, so we measured before building it: the same harmless
transaction, alternating between the two, 40 times each, from the Tokyo
server over the direct endpoint.

| | p50 | p90 |
|---|---|---|
| HTTPS | 10 ms | 13 ms |
| websocket | 11 ms | 17-20 ms |

No faster, and the websocket's reply only says the transaction was
*submitted*, not whether it succeeded, which the executor needs (Part 7).
The time is in Bullet processing the transaction, not in how it gets there.
So the bot stays on HTTPS: measure first, and don't build what the numbers
don't ask for.

## Drops you'd never see

AWS caps each instance's network traffic (bandwidth, packets per second,
tracked connections, and traffic to link-local services like DNS and the time
server). Go over and the network card silently **drops packets**: on a TCP
connection that means a retransmit, and a latency spike from nowhere. The
[ENA driver counts these](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/monitoring-network-performance-ena.html):

```text
bw_in_allowance_exceeded: 0
pps_allowance_exceeded: 0
conntrack_allowance_exceeded: 0
linklocal_allowance_exceeded: 0
```

Ours are zero: a market maker sends very little. A bot taking full market
data for hundreds of symbols would watch these closely, and might move to a
network-optimised instance (the `n` in `c8gn`) for higher allowances.

## Other levers, and when they help

- **Cluster placement groups** pack *your* instances onto the same rack. Great
  between your own boxes (say, a feed handler and a strategy box), useless for
  reaching an exchange, which isn't in your group.
- **Hops inside AWS cost.** A Transit Gateway or NAT gateway on the hot path
  adds latency; a VPC peering or a public IP in the same zone doesn't.
- **Bare metal** (`.metal` instances) removes the hypervisor, and with it a
  little jitter. Worth trying once everything above is done.
- **Better clocks.** Some instances expose a [PTP hardware clock](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/configure-ec2-ntp.html)
  backed by Amazon's time service; ours doesn't, and doesn't need to: chrony
  over the network keeps it within a microsecond (`chronyc tracking`), plenty
  for millisecond-scale measurements.

## Scoreboard

The [standard benchmark](03-telemetry.md#the-scoreboard), live on the Tokyo
server over Bullet's direct endpoint, against Part 9:

| | end-to-end p50 / p99 | Binance → us p50 | tick-to-trade p50 | order-to-live p50 / p99 | queue p99 / max |
|---|---|---|---|---|---|
| Part 9 | 5.5 / 53 ms | 1.4 ms | 1 µs | 3.7 / 47 ms | 0.01 / 16 ms |
| **Part 10** | **6.3 / 27 ms** | 1.5 ms | 1 µs | 4.7 / 23 ms | 0.01 / 21 ms |

Unchanged here, because every part's benchmark already runs over Bullet's
direct endpoint. This part's gain is in the comparison above: about 6 ms
direct against 11 ms through Cloudflare on its best day.

Both rows are medians of three 5-minute runs taken in rotation, so they
saw the same conditions at Bullet.

## Where the time goes now

| Step | p50 |
|---|---|
| Binance → us | 1.5 ms |
| queue + tick-to-trade (our code) | microseconds |
| order-to-live (send, route, Bullet opening it) | 4 ms direct; 8-10 ms via Cloudflare Tokyo; ~190 ms on a detour |
| **end-to-end** | **6 ms direct; 11 ms via Cloudflare Tokyo** |

Our side of the loop is microseconds, and Binance is next door. What's left
is Bullet's own processing (most of those 4 ms) and, unless you have a direct
route, which Cloudflare site serves you.
