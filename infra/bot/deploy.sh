#!/bin/bash
# Create (or update) the Tokyo server, copy this repo to it, build, and
# (re)start the bot. Run from anywhere; uses bot.toml (and bot.key if live).
#   ./infra/bot/deploy.sh          deploy and start
#   ./infra/bot/deploy.sh logs     follow the bot's log
#   ./infra/bot/deploy.sh stop     stop the bot (it cancels its orders)
#   ./infra/bot/deploy.sh destroy  stop and delete the server
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
cd "$here"

# A key pair just for this server.
[ -f .ssh/id_ed25519 ] || { mkdir -p .ssh && ssh-keygen -q -t ed25519 -N "" -f .ssh/id_ed25519; }

tf() { terraform "$@" -input=false -var "my_ip=$(curl -s https://checkip.amazonaws.com)"; }
ssh_bot() { ssh -i .ssh/id_ed25519 -o StrictHostKeyChecking=accept-new "ec2-user@$(terraform output -raw ip)" "$@"; }

case "${1:-deploy}" in
  logs) ssh_bot journalctl -u hft-bot -f -o cat; exit ;;
  stop) ssh_bot sudo systemctl stop hft-bot; exit ;;
  destroy) ssh_bot sudo systemctl stop hft-bot || true; tf destroy -auto-approve; exit ;;
esac

terraform init -input=false >/dev/null
tf apply -auto-approve >/dev/null
ip=$(terraform output -raw ip)
echo "server: $ip"

echo "waiting for first boot to finish..."
until ssh_bot cloud-init status --wait >/dev/null 2>&1; do sleep 5; done

echo "copying the code..."
# The trading key only goes to the server when you trade live.
grep -q '^dry_run = false' "$repo/bot.toml" && key=() || key=(--exclude bot.key)
rsync -az --delete -e "ssh -i .ssh/id_ed25519" \
  --exclude target --exclude .git --exclude infra "${key[@]}" \
  "$repo/" "ec2-user@$ip:hft/"

echo "building (the first build takes a few minutes)..."
ssh_bot 'cd hft && ~/.cargo/bin/cargo build --release --quiet'
ssh_bot sudo systemctl restart hft-bot
echo "running. Follow it with: $0 logs"
