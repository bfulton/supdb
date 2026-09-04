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

# Only the regions this account can actually query. Without the filter,
# describe-regions also returns regions that are not opted into, and
# describe-instances against those fails -- which is why this once discarded
# stderr and forced success. That discarded every other failure with it: a
# throttle, an expired credential or a missing permission left the id list
# empty, and an empty list is indistinguishable from "nothing running". For a
# tool whose only job is making sure nothing is still billing, a false all
# clear is the expensive way to be wrong.
REGIONS=$(aws ec2 describe-regions \
  --filters "Name=opt-in-status,Values=opt-in-not-required,opted-in" \
  --query 'Regions[].RegionName' --output text)

FOUND=0
UNQUERIED=""
for R in $REGIONS; do
  if ! IDS=$(aws ec2 describe-instances --region "$R" \
    --filters "Name=tag:supdb-bench,Values=true" \
              "Name=instance-state-name,Values=pending,running,stopping,stopped" \
    --query 'Reservations[].Instances[].[InstanceId,InstanceType,LaunchTime]' \
    --output text 2>&1); then
    echo "  $R: could not query: $IDS" >&2
    UNQUERIED="$UNQUERIED $R"
    continue
  fi
  [ -z "$IDS" ] && continue
  echo "$R:"; echo "$IDS" | sed 's/^/  /'
  FOUND=1
  if [ "$LIST_ONLY" != "--list" ]; then
    aws ec2 terminate-instances --region "$R" \
      --instance-ids $(echo "$IDS" | awk '{print $1}') >/dev/null
    echo "  terminated"
  fi
done

# Say nothing reassuring about a region that was never reached.
if [ -n "$UNQUERIED" ]; then
  echo "could not query:$UNQUERIED" >&2
  echo "this is NOT a clean bill of health -- instances may be running there" >&2
  exit 1
fi
# An `if` rather than `[ ... ] && echo`, whose exit status is the failed test
# when something *was* found -- so a successful reap used to report failure.
if [ "$FOUND" = 0 ]; then
  echo "no supdb-bench instances anywhere"
fi
