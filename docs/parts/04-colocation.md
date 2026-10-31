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
| Tokyo server (best zone, below) | 0.5 ms |

To follow along you need an AWS account, the
[AWS CLI](https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html)
logged in, and [Terraform](https://developer.hashicorp.com/terraform/install).
The server costs about $0.10/hour; `./infra/bot/deploy.sh destroy` when
you're done.

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
a few cents.

> **Incident: the probe that measured DNS.** Our first probe reported 1.5 ms
> to Binance. A later check from the bot's server said 0.4 ms. The
> difference: curl's `time_connect` *includes* looking up the hostname
> (~1 ms inside AWS). The probe now subtracts `time_namelookup`. *Lesson:*
> know exactly what your tool measures; when two numbers disagree, find out
> why before trusting either.

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
./infra/bot/deploy.sh --live    # create the server, copy the code, build, start trading
./infra/bot/deploy.sh logs      # follow the bot
./infra/bot/deploy.sh bench     # the Part 3 benchmark on the server (real money)
./infra/bot/deploy.sh destroy   # stop the bot (it cancels its orders) and delete the server
```

Without `--live` it deploys in dry run. Either way `bot.toml` stays as
shipped: for a live run the server makes a copy with `dry_run` off.

[Terraform](https://developer.hashicorp.com/terraform) describes the server in
`infra/bot/main.tf`; `cloud-init.yaml` installs what the build needs on first
boot and registers the bot as a service. SSH is open to your IP only.

**The key never touches the server's disk.** For a live bot, `deploy.sh`
stores `bot.key` in [SSM Parameter Store](https://docs.aws.amazon.com/systems-manager/latest/userguide/systems-manager-parameter-store.html)
as an encrypted `SecureString`. The server's IAM role may read that one
parameter and nothing else, and each time the bot starts it fetches the key
into `/run` (memory) where only the bot's user can read it. Compared with
copying a key file around: it isn't in your code, your server image or your
backups, access is controlled and logged by AWS, and replacing the key is one
command. ([Secrets Manager](https://aws.amazon.com/secrets-manager/) does the
same with automatic rotation, at $0.40 a month per secret; for one key that
rarely changes, Parameter Store is free and enough.)

## What we measured

The same dry run, Tokyo server vs laptop:

| Hop (p50 / p99) | laptop | Tokyo |
|---|---|---|
| Binance → us | (clock off) | 1.5 / 35 ms |
| Bullet book → us | (clock off) | 102 / 112 ms |
| queue | 10 µs / 300 µs | 3 / 6 µs |
| decide | 1 / 20 µs | 1 / 3 µs |

Binance's prices now reach us 1.5 ms after Binance stamps them. The wire is
only a quarter of a millisecond of that (half the 0.5 ms round trip); the
rest is inside Binance, between stamping an event and sending it. Two
surprises:

- **Bullet's feed was ~100 ms from Tokyo in this run** (4.6 ms in another,
  hours later). Placement can't fix what's on the exchange's side of the
  wire; Part 10 finds out why.
- **The server is far steadier than the laptop.** Queueing p99 dropped from
  300 µs to 6 µs: no browser, no laptop power management. That steadiness is
  what Part 8 tunes further.

## Scoreboard

The [standard benchmark](03-telemetry.md#the-scoreboard), live on the Tokyo
server over Bullet's direct endpoint, against Part 3's laptop baseline (a different machine, shown for the starting point only):

| | end-to-end p50 / p99 | Binance → us p50 | tick-to-trade p50 | order-to-live p50 / p99 | queue p99 / max |
|---|---|---|---|---|---|
| Part 3 (laptop baseline) | 211 / 369 ms | (clock off) | 6 µs | (clock off) | 337 / 657 ms |
| **Part 4** | **26.8 / 110 ms** | 1.6 ms | 1 µs | 24.6 / 89 ms | 93 / 350 ms |

211 ms to 27 ms: Binance's prices now arrive in 1.6 ms instead of about
40, and each trip to Bullet takes about 15 ms instead of over a hundred.
Order-to-live is still most of what's left, because the bot still cancels,
waits for the reply, then places. Part 6 stops that.

The benchmark ran over Bullet's direct endpoint (`api_url` in
`bench-live.toml`, Part 10). Through the public endpoint, Cloudflare adds a
few milliseconds and the occasional detour.

## What's next

The bot still reconnects slowly when a socket goes quiet, and waits on every
order. Part 5 deals with the sockets.
