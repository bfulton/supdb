#!/bin/bash
# Find and terminate benchmark instances, across every region.
#
# The instances terminate themselves -- a watchdog at boot plus
# instance-initiated-shutdown-behavior -- so this should never find anything.
# It exists because "should never" is not a billing guarantee, and because
# checking costs nothing.
#
#   bench/aws/reap.sh --list     # show what is running, terminate nothing
#   bench/aws/reap.sh            # terminate everything tagged supdb-bench
set -euo pipefail
LIST_ONLY=${1:-}
REGIONS=$(aws ec2 describe-regions --query 'Regions[].RegionName' --output text)
FOUND=0
for R in $REGIONS; do
  IDS=$(aws ec2 describe-instances --region "$R" \
    --filters "Name=tag:supdb-bench,Values=true" \
              "Name=instance-state-name,Values=pending,running,stopping,stopped" \
    --query 'Reservations[].Instances[].[InstanceId,InstanceType,LaunchTime]' \
    --output text 2>/dev/null || true)
  [ -z "$IDS" ] && continue
  echo "$R:"; echo "$IDS" | sed 's/^/  /'
  FOUND=1
  if [ "$LIST_ONLY" != "--list" ]; then
    aws ec2 terminate-instances --region "$R" \
      --instance-ids $(echo "$IDS" | awk '{print $1}') >/dev/null
    echo "  terminated"
  fi
done
[ "$FOUND" = 0 ] && echo "no supdb-bench instances anywhere"
