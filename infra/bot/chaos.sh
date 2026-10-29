#!/bin/bash
# Runs on the server. Silently drops all traffic to one exchange (a "blackhole"
# route: packets vanish, nothing errors, just like a half-dead connection) and
# times how long the bot takes to notice. Then restores the route.
# Usage: chaos.sh binance|bullet
set -euo pipefail
pid=$(systemctl show -p MainPID --value hft-bot)
peers=$(sudo ss -tnp | grep "pid=$pid," | awk '{print $5}' | cut -d: -f1 | sort -u)
bullet=$(getent ahostsv4 tradingapi.bullet.xyz | awk '{print $1}' | sort -u)
case $1 in
  bullet) ips=$bullet; noticed="Bullet reconnecting" ;;
  binance) ips=$(comm -23 <(echo "$peers") <(echo "$bullet")); noticed="binance: nothing heard" ;;
esac

since=$(date '+%Y-%m-%d %H:%M:%S'); start=$(date +%s%N)
for ip in $ips; do sudo ip route add blackhole "$ip"; done
trap 'for ip in $ips; do sudo ip route del blackhole "$ip"; done' EXIT
until journalctl -u hft-bot --since "$since" -o cat | grep -q "$noticed"; do sleep 0.1; done
echo "$1: cut off, bot noticed after $(( ($(date +%s%N) - start) / 1000000 )) ms"
