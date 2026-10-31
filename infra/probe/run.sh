#!/bin/bash
# Find the Tokyo zone closest to Binance and Bullet. Costs a few cents.
set -euo pipefail
cd "$(dirname "$0")"

terraform init -input=false >/dev/null
trap 'echo "destroying probes..."; terraform destroy -auto-approve -input=false >/dev/null' EXIT
terraform apply -auto-approve -input=false >/dev/null

echo "probes booting; measuring (about 3 minutes)..."
{
echo "ZONE HOST P50_MS P99_MS"
for zone in $(terraform output -json instances | jq -r 'keys[]'); do
  id=$(terraform output -json instances | jq -r --arg z "$zone" '.[$z]')
  until out=$(aws ec2 get-console-output --region ap-northeast-1 --instance-id "$id" --latest --output text) \
    && [ "$(grep -c 'PROBE ' <<<"$out")" -ge 2 ]; do
    sleep 10
  done
  grep -o 'PROBE .*' <<<"$out" | sed "s/^PROBE/$zone/"
done
} | column -t
