#!/bin/bash
# Runs on the server. One Linux tuning knob per command, so you can measure
# each on its own (docs/parts/08-linux.md). Cores 1 and 2 are for the bot
# (feed_core = 1, bot_core = 2 in bot.toml); 0 and 3 are for everything else.
set -euo pipefail
nic=$(ip route show default | awk '{print $5}')

case ${1:-status} in
  status)
    echo "tuned profile: $(tuned-adm active 2>/dev/null || echo none)"
    echo "irqbalance:    $(systemctl is-active irqbalance || true)"
    echo "NIC IRQs on:   $(grep "$nic" /proc/interrupts | awk -F: '{print $1}' | while read -r irq; do cat /proc/irq/$irq/smp_affinity_list; done | sort -u | tr '\n' ' ')"
    echo "NIC batching:  $(ethtool -c "$nic" | awk '/^rx-usecs:/ {print "rx-usecs " $2}')"
    echo "kernel args:   $(grep -o 'isolcpus=[^ ]*' /proc/cmdline || echo 'no isolcpus')"
    ;;
  tuned)
    # A ready-made profile: CPUs stay at full speed and out of deep sleep
    # states (waking up takes tens of µs), and the kernel busy-polls sockets.
    sudo dnf install -y -q tuned
    sudo systemctl enable --now tuned
    sudo tuned-adm profile network-latency
    ;;
  irqs)
    # The NIC interrupts a CPU on every packet. Send those interrupts to core 0
    # only, so they never land on the bot's cores. irqbalance would move them
    # back, so it goes.
    sudo systemctl disable --now irqbalance || true
    for irq in $(grep "$nic" /proc/interrupts | awk -F: '{print $1}'); do
      echo 0 | sudo tee "/proc/irq/$irq/smp_affinity_list" >/dev/null
    done
    ;;
  coalesce)
    # By default the NIC waits a little to batch several packets into one
    # interrupt: less CPU, more latency. Turn the batching off.
    sudo ethtool -C "$nic" adaptive-rx off rx-usecs 0 tx-usecs 0
    ;;
  isolate)
    # Tell the kernel to keep everything else off cores 1-2 (isolcpus), stop
    # its timer tick there (nohz_full) and move its housekeeping off them
    # (rcu_nocbs). Needs a reboot.
    sudo grubby --update-kernel=ALL --args="isolcpus=1,2 nohz_full=1,2 rcu_nocbs=1,2"
    echo "rebooting..."
    sudo systemctl reboot
    ;;
  *) echo "usage: tune.sh status|tuned|irqs|coalesce|isolate"; exit 1 ;;
esac
