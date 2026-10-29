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
front of Bullet's servers. The `cf-ray` header names the Cloudflare site
that handled your request:

```sh
curl -sI https://tradingapi.bullet.xyz/fapi/v1/time | grep -i cf-ray
# from a laptop in Asia:     cf-ray: ...-SIN   (Singapore)
# from our Tokyo server:     cf-ray: ...-DFW   (Dallas)
```

Dallas, at that moment. Cloudflare's address answers pings from Tokyo in
1.6 ms (it's [anycast](https://www.cloudflare.com/learning/cdn/glossary/anycast-network/):
every Cloudflare site announces the same address), so traffic enters
Cloudflare in Tokyo. But Cloudflare also moves traffic between its sites
when one is busy or costly ([Traffic Manager](https://blog.cloudflare.com/meet-traffic-manager/)),
lower-tier plans first, and that evening our requests were being handled
across the Pacific. Half an hour later the same server got Tokyo again:

| Cloudflare site serving us | TCP handshake | Bullet book → us (p50) |
|---|---|---|
| Dallas (`DFW`) | ~131 ms | ~102 ms |
| Tokyo (`NRT`) | 2-7 ms | **4.6 ms** |

So Bullet's engine is near Tokyo after all; the extra 100 ms was the detour.
`curl https://tradingapi.bullet.xyz/cdn-cgi/trace` prints `colo=`, the site
serving you right now.

Lessons:

- **Look at the whole path, not one number.** `mtr` said 1.6 ms; the real
  route was sometimes 130 ms. Headers and endpoints like `cf-ray` and
  `/cdn-cgi/trace` tell you who you're actually talking to.
- **Variance is the enemy.** A feed that's 5 ms most of the time and 100 ms
  some of the time is worse to trade on than a steady 20 ms: you can't tell
  which you're getting until your quotes are stale.
- **Some of this is the venue's call.** No setting on our server changes
  Cloudflare's routing. Latency-sensitive venues keep trading endpoints off a
  CDN's shared tier, or offer a direct route (a regional host, an AWS
  [PrivateLink](https://aws.amazon.com/privatelink/) endpoint, a
  cross-connect). Ask.

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

## Where the time goes now

| Hop | p50 |
|---|---|
| Binance → us | 1.5 ms |
| our reaction (tick → order sent) | ~0.01 ms |
| Bullet → us (book) | 4.6 ms via Tokyo, ~100 ms via Dallas |
| us → Bullet → order accepted | ~100-200 ms (measured via Dallas and from a laptop) |

Our side of the loop is now microseconds, and Binance is next door. What's
left depends on which Cloudflare site Bullet's traffic is handled in, which
only the venue can pin down.
