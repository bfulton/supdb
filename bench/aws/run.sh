#!/bin/bash
# Launch a bare-metal EC2 instance, run every suite on it, pull the results
# back, terminate.
#
#   bench/aws/run.sh c7g.metal            # Graviton3, ARM64
#   bench/aws/run.sh c6i.metal            # Ice Lake, x86-64
#   KEEP=1 bench/aws/run.sh m7i.metal-24xl  # leave it running to poke at
#
# Why .metal: virtualised Nitro instances do not expose the PMU, so `perf`
# reports every hardware event as <not supported>. That is the same wall this
# project hit on Firecracker. A .metal size is the reliable way to get counters
# on AWS, and it is the only reason to pay for one.
set -euo pipefail

TYPE="${1:?usage: run.sh <instance-type> [git-ref]}"
REF="${2:-main}"
REGION="${AWS_REGION:-us-east-1}"
KEY="${KEY_NAME:?set KEY_NAME to an EC2 key pair name}"
SG="${SECURITY_GROUP:?set SECURITY_GROUP to a group id allowing inbound 22}"
SUBNET="${SUBNET_ID:-}"
REPO="${REPO_URL:-https://github.com/bfulton/supdb}"
KEYFILE="${KEY_FILE:-$HOME/.ssh/$KEY.pem}"

case "$TYPE" in
  *g.metal|*g.metal-*|*gd.metal*) ARCH=arm64 ;;
  *) ARCH=x86_64 ;;
esac

AMI=$(aws ec2 describe-images --region "$REGION" --owners 099720109477 \
  --filters "Name=name,Values=ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-${ARCH}-server-*" \
            "Name=state,Values=available" \
  --query 'sort_by(Images,&CreationDate)[-1].ImageId' --output text)
echo "AMI $AMI ($ARCH)"

# Spot is roughly 70% cheaper and a benchmark run is interruptible: if it dies,
# rerun it. Set ON_DEMAND=1 for a long interactive session instead.
# MaxPrice is set explicitly so a spot price spike cannot quietly cost
# on-demand rates.
MARKET=()
if [ -z "${ON_DEMAND:-}" ]; then
  MARKET=(--instance-market-options \
    "MarketType=spot,SpotOptions={MaxPrice=${MAX_PRICE:-1.00},SpotInstanceType=one-time}")
fi

# The bootstrap runs from user-data rather than being pushed over ssh, so the
# watchdog is armed at boot even if ssh never connects. Combined with
# instance-initiated-shutdown-behavior=terminate, the instance ends itself:
# nothing about the teardown depends on this script, this shell, or this
# machine still being alive.
USERDATA=$(mktemp)
{
  echo '#!/bin/bash'
  echo "exec > /var/log/supdb-bootstrap.log 2>&1"
  cat "$(dirname "$0")/bootstrap.sh"
} > "$USERDATA"

MAX_MINUTES="${MAX_MINUTES:-240}"
ID=$(aws ec2 run-instances --region "$REGION" --image-id "$AMI" \
  --instance-type "$TYPE" --key-name "$KEY" --security-group-ids "$SG" \
  ${SUBNET:+--subnet-id "$SUBNET"} "${MARKET[@]}" \
  --instance-initiated-shutdown-behavior terminate \
  --user-data "file://$USERDATA" \
  --block-device-mappings 'DeviceName=/dev/sda1,Ebs={VolumeSize=200,VolumeType=gp3,Iops=6000,DeleteOnTermination=true}' \
  --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=supdb-bench},{Key=supdb-bench,Value=true},{Key=MaxMinutes,Value=$MAX_MINUTES}]" \
  --query 'Instances[0].InstanceId' --output text)
rm -f "$USERDATA"
echo "instance $ID"
cleanup() {
  if [ -z "${KEEP:-}" ]; then
    echo "terminating $ID"
    aws ec2 terminate-instances --region "$REGION" --instance-ids "$ID" >/dev/null
  else
    echo "KEEP set; $ID left running -- terminate it yourself"
  fi
}
trap cleanup EXIT

aws ec2 wait instance-running --region "$REGION" --instance-ids "$ID"
IP=$(aws ec2 describe-instances --region "$REGION" --instance-ids "$ID" \
  --query 'Reservations[0].Instances[0].PublicIpAddress' --output text)
echo "ip $IP"

SSH="ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -i $KEYFILE ubuntu@$IP"
for _ in $(seq 60); do $SSH true 2>/dev/null && break; sleep 5; done

echo "bootstrap is running from user-data; watchdog armed for ${MAX_MINUTES} min"
echo "if this script dies, the instance still terminates on its own"
echo "orphan check any time:  bench/aws/reap.sh --list"

echo "running; polling for completion"
while ! $SSH "test -f ~/DONE" 2>/dev/null; do
  sleep 60
  $SSH "sudo tail -1 /var/log/supdb-bootstrap.log" 2>/dev/null || true
done

OUT="results/aws-$TYPE-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$OUT"
scp -o StrictHostKeyChecking=no -i "$KEYFILE" "ubuntu@$IP:~/results.tgz" "$OUT/"
tar xzf "$OUT/results.tgz" -C "$OUT" --strip-components=1 && rm "$OUT/results.tgz"
$SSH "cat ~/PMU-OK ~/PMU-MISSING" 2>/dev/null || true
echo "results in $OUT"
