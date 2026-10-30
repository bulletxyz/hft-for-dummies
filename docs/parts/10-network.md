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

**Lessons.**

- **Look at the whole path, not one number.** `mtr` said 1.6 ms; the real
  route was sometimes 130 ms. `cf-ray`, `/cdn-cgi/trace`, `via` and `server`
  headers tell you who you're actually talking to.
- **Variance is the enemy.** A feed that's 5 ms most of the time and 100 ms
  some of the time is harder to trade on than a steady 20 ms: you can't tell
  which you're getting until your quotes are stale.
- **Measure up to your own door.** That's why the scoreboard (Part 3) stops
  when our order leaves: everything after it is the venue's, and can change
  under you.

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
get. When it does, a bot can resolve the name itself and connect to the
fastest address.

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

The [standard benchmark](03-telemetry.md#the-scoreboard), 10 minutes live
on the Tokyo server, against Part 9 (milliseconds):

| | tick-to-trade p50 / p99 | Binance → us p50 | queue p99 / max | Bullet accepted p50 |
|---|---|---|---|---|
| Part 9 | 2.1 / 145 | 1.5 | 0.01 / 45 | 17 |
| **Part 10** | **1.9 / 58** | 1.5 | 0.01 / 2.9 | 21 |

Unchanged on our side, as it should be: tick-to-trade stops at our door,
and this part is about what lies beyond it. On this run Bullet accepted
orders in 21 ms through Cloudflare.

## Where the time goes now

| Hop | p50 |
|---|---|
| Binance → us | 1.5 ms |
| our reaction (tick → order sent) | ~0.01 ms |
| Bullet → us (book) | 4.6 ms via Tokyo, ~100 ms via Dallas |
| us → Bullet → order accepted | ~20 ms via Tokyo, ~220 ms via Dallas |

Our side of the loop is now microseconds, and Binance is next door. What's
left depends on which Cloudflare site Bullet's traffic is handled in, which
only the venue can pin down.
