#!/bin/bash
# Runs once at boot on each probe instance. A TCP handshake takes exactly one
# round trip, so curl's time_connect is the network distance to each venue.
# Results go to the console log as "PROBE host p50_ms p99_ms".
for host in fstream.binance.com tradingapi.bullet.xyz; do
  for _ in $(seq 100); do
    curl -o /dev/null -s --max-time 5 -w '%{time_connect}\n' "https://$host/"
  done | sort -n | awk -v host="$host" '
    { ms[NR] = $1 * 1000 }
    END { printf "PROBE %s %.2f %.2f\n", host, ms[int(NR * 0.5)], ms[int(NR * 0.99)] }'
done > /dev/console
