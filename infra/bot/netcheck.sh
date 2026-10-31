#!/bin/bash
# Runs on the server: the network checks from docs/parts/10-network.md, in one go.
nic=$(ip route show default | awk '{print $5}')
which mtr >/dev/null 2>&1 || sudo dnf install -y -q mtr >/dev/null

echo "== Route to each exchange (20 probes per hop; AS = who owns the network)"
for host in fstream.binance.com tradingapi.bullet.xyz; do
  echo "-- $host"
  mtr -rwz -c 20 "$host" | tail -n +3
done

echo
echo "== Each Binance address: TCP round trip, p50 of 20 connections"
for ip in $(getent ahostsv4 fstream.binance.com | awk '{print $1}' | sort -u); do
  ms=$(for _ in $(seq 20); do
         curl -o /dev/null -s --max-time 2 -w '%{time_connect}\n' \
           --resolve "fstream.binance.com:443:$ip" https://fstream.binance.com/
       done | sort -n | awk '{a[NR] = $1} END {printf "%.2f", a[int(NR / 2)] * 1000}')
  echo "$ip  $ms ms"
done

echo
echo "== Network allowance drops (anything non-zero: the instance dropped packets)"
ethtool -S "$nic" | grep allowance_exceeded

echo
echo "== Clock"
ls /dev/ptp* 2>/dev/null || echo "no PTP hardware clock exposed on this instance"
chronyc tracking | grep -E "Reference ID|System time"
