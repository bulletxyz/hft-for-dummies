# Part 4: Move next to the exchange

Part 3 showed our code takes microseconds and the network takes tens of
milliseconds. You can't optimise light, so move closer. Code:
[`part-4`](https://github.com/bulletxyz/hft-for-dummies/tree/part-4) ·
[diff](https://github.com/bulletxyz/hft-for-dummies/compare/part-3...part-4).

## Why Tokyo

Binance runs its matching engine in AWS's Tokyo region (`ap-northeast-1`).
Our bot's input is Binance's price, so every millisecond between Binance and
us is a millisecond our quotes on Bullet are stale. Measured from a laptop vs
a server in Tokyo:

| | Binance round trip (p50) |
|---|---|
| laptop | 81.5 ms |
| Tokyo server | 1.8 ms |

## Which zone: IDs, not names

A region has several availability zones (separate data centres), and a few
kilometres of fibre is measurable. AWS shuffles zone *names* per account
(your `ap-northeast-1a` may be my `ap-northeast-1c`), so always talk in zone
*IDs* (`apne1-az4`), which are the same building for everyone:

```sh
aws ec2 describe-availability-zones --region ap-northeast-1 \
  --query 'AvailabilityZones[].[ZoneName,ZoneId]' --output text
```

Don't guess which zone is closest: measure. `infra/probe/run.sh` starts one tiny
server per zone, has each time 100 TCP handshakes to each exchange (a handshake
is exactly one round trip), prints the table and deletes the servers. It costs
a few cents. (One trap: curl's `time_connect` includes the DNS lookup, about
1 ms here, which our first version of the probe forgot to subtract.)

```sh
./infra/probe/run.sh
```

```text
ZONE       HOST                   P50_MS  P99_MS
apne1-az1  fstream.binance.com    2.03    4.82
apne1-az2  fstream.binance.com    2.02    4.02
apne1-az4  fstream.binance.com    0.50    1.91
```

Half a millisecond: `apne1-az4` is in or next to the building Binance's
servers are in, four times closer than the other zones. That's the default
in `infra/bot/main.tf`. Run the probe yourself: results drift as exchanges
move hardware.

A caveat worth learning: a handshake only measures the distance to whoever
answers it. Bullet's API sits behind [Cloudflare](https://www.cloudflare.com/),
so a handshake reaches a Cloudflare edge, not Bullet itself. Which edge, and
how far, depends on Cloudflare's routing for your address at that moment: our
probes saw anywhere from 4 ms to 131 ms from the same zones on the same day,
while the bot's server reached the edge in under 2 ms. None of that is the
distance to Bullet. To see that, time something Bullet itself must answer: a
full request or, best, the bot's own telemetry (below).

## Which server

`c8g.large` ([pricing](https://aws.amazon.com/ec2/pricing/on-demand/), about
$0.10/hour):

- **`c` (compute) family, not `t` (burstable).** `t` instances run on CPU
  credits: when they run out the CPU is throttled, and your latency gets
  random spikes. Steady CPU matters more to us than raw speed.
- **Graviton (`g`, arm64).** Cheaper per unit of performance than x86, and Rust
  doesn't care.
- **`large` (2 cores, 4 GB).** Enough to compile the bot on the server. We
  aren't moving much data, so network bandwidth doesn't matter; distance does.

## Clocks

Part 3 couldn't measure one-way network latency on a laptop whose clock was
288 ms off. Amazon Linux syncs its clock with [chrony](https://chrony-project.org/)
to the [Amazon Time Sync Service](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/set-time.html),
a local source inside the data centre:

```sh
chronyc tracking   # System time : 0.000001207 seconds slow of NTP time
```

One microsecond. Now the network rows in the telemetry are real.

## Deploy

```sh
./infra/bot/deploy.sh           # create the server, copy the code, build, start
./infra/bot/deploy.sh logs      # follow the bot
./infra/bot/deploy.sh destroy   # stop the bot (it cancels its orders) and delete the server
```

[Terraform](https://developer.hashicorp.com/terraform) describes the server in
`infra/bot/main.tf`; `cloud-init.yaml` installs what the build needs on first
boot and registers the bot as a service. SSH is open to your IP only, and
`bot.key` is copied only when `dry_run = false`. A production setup would
skip SSH for [SSM Session Manager](https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager.html)
and keep keys in [Secrets Manager](https://aws.amazon.com/secrets-manager/);
for learning, SSH is easier to see through.

## What we measured

The same dry run, Tokyo server vs laptop:

| Hop (p50 / p99) | laptop | Tokyo |
|---|---|---|
| Binance → us | (clock off) | 1.5 / 35 ms |
| Bullet book → us | (clock off) | 102 / 112 ms |
| bot: received → handled | 10 µs / 300 µs | 3 / 6 µs |
| bot: decide | 1 / 20 µs | 1 / 3 µs |

Binance is now 1.5 ms away. Two surprises:

- **Bullet's feed was ~100 ms from Tokyo in this run** (4.6 ms in another,
  hours later). Placement can't fix what's on the exchange's side of the
  wire; Part 10 tracks down where those milliseconds go.
- **The server is far steadier than the laptop.** Queueing p99 dropped from
  300 µs to 6 µs: no browser, no laptop power management. That steadiness is
  what Part 8 tunes further.

## What's next

The bot still reconnects slowly when a socket goes quiet, and waits on every
order. Part 5 deals with the sockets.
