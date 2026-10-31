#!/bin/bash
# Create (or update) the Tokyo server, copy this repo to it, build, and
# (re)start the bot with bot.toml. With --live it trades for real: bot.key goes
# to SSM Parameter Store (encrypted), not onto the server's disk.
#   ./infra/bot/deploy.sh          deploy and start (dry run)
#   ./infra/bot/deploy.sh --live   deploy and start trading live
#   ./infra/bot/deploy.sh logs     follow the bot's log
#   ./infra/bot/deploy.sh bench    10 minutes of bench-live.toml, then the scoreboard
#   ./infra/bot/deploy.sh stop     stop the bot (it cancels its orders)
#   ./infra/bot/deploy.sh chaos binance|bullet   cut one exchange off, time how fast the bot notices
#   ./infra/bot/deploy.sh destroy  stop and delete the server
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
cd "$here"

# A key pair just for this server.
[ -f .ssh/id_ed25519 ] || { mkdir -p .ssh && ssh-keygen -q -t ed25519 -N "" -f .ssh/id_ed25519; }

# Store bot.key as an encrypted SecureString the server's role can read.
upload_key() {
  aws ssm put-parameter --region ap-northeast-1 --name /hft-for-dummies/bot-key \
    --type SecureString --overwrite --value "file://$repo/bot.key" >/dev/null
  echo "bot.key stored in SSM Parameter Store"
}

tf() { terraform "$@" -input=false -var "my_ip=$(curl -s https://checkip.amazonaws.com)"; }
ssh_bot() { ssh -i .ssh/id_ed25519 -o StrictHostKeyChecking=accept-new "ec2-user@$(terraform output -raw ip)" "$@"; }

live=false
[ "${1:-}" = "--live" ] && { live=true; shift; }

case "${1:-deploy}" in
  logs) ssh_bot journalctl -u hft-bot -f -o cat; exit ;;
  bench)
    upload_key
    # Put back whatever was running before (dry or live) when the benchmark ends.
    before=$(ssh_bot 'cat ~/hft-config 2>/dev/null || echo CONFIG=bot.toml')
    ssh_bot 'echo CONFIG=bench-live.toml >~/hft-config && sudo systemctl restart hft-bot'
    echo "benchmarking live for 10 minutes..."
    sleep 600
    ssh_bot "echo $before >~/hft-config && sudo systemctl restart hft-bot && journalctl -u hft-bot --since '-11min' -o cat | grep SCOREBOARD | tail -1"
    exit ;;
  stop) ssh_bot sudo systemctl stop hft-bot; exit ;;
  chaos) ssh_bot "bash -s $2" <chaos.sh; exit ;;
  destroy) ssh_bot sudo systemctl stop hft-bot || true; tf destroy -auto-approve; exit ;;
esac

terraform init -input=false >/dev/null
tf apply -auto-approve >/dev/null
ip=$(terraform output -raw ip)
echo "server: $ip"

echo "waiting for first boot to finish..."
# (sudo: after a reboot cloud-init can't read its own state as a normal user.
# Any final status will do, including "degraded"; we only wait for it to end.)
until ssh_bot 'sudo cloud-init status --wait >/dev/null 2>&1 || true'; do sleep 5; done

echo "copying the code..."
# The key never travels with the code; live runs get it from SSM.
$live && upload_key
rsync -az --delete -e "ssh -i .ssh/id_ed25519" \
  --exclude target --exclude .git --exclude infra --exclude bot.key \
  "$repo/" "ec2-user@$ip:hft/"

echo "building (the first build takes a few minutes)..."
ssh_bot 'cd hft && ~/.cargo/bin/cargo build --release --quiet'
if $live; then
  # bot.toml stays as shipped; the server runs a copy with dry_run off.
  ssh_bot 'cd hft && sed "s/^dry_run = true/dry_run = false/" bot.toml > live.toml && echo CONFIG=live.toml >~/hft-config'
  echo "LIVE: trading with real money"
else
  ssh_bot 'echo CONFIG=bot.toml >~/hft-config'
fi
ssh_bot sudo systemctl restart hft-bot
echo "running. Follow it with: $0 logs"
