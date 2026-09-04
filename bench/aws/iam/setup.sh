#!/bin/bash
# Create a least-privilege benchmarking identity, verify it, then drop admin.
#
# Run this yourself with admin credentials. The order matters: the scoped policy
# is created and *verified with the policy simulator* before anything is
# detached, so a mistake in the policy is caught while you still have the
# privileges to fix it.
#
#   bench/aws/iam/setup.sh supdb-bench-user
#
# If it goes wrong after the detach, reattach admin from the console:
#   aws iam attach-user-policy --user-name <user> \
#     --policy-arn arn:aws:iam::aws:policy/AdministratorAccess
set -euo pipefail
USER_NAME="${1:?usage: setup.sh <iam-user-name>}"
POLICY_NAME="${POLICY_NAME:-SupdbBenchLeastPrivilege}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ACCT=$(aws sts get-caller-identity --query Account --output text)
echo "account $ACCT, user $USER_NAME"

ARN="arn:aws:iam::${ACCT}:policy/${POLICY_NAME}"
if aws iam get-policy --policy-arn "$ARN" >/dev/null 2>&1; then
  echo "policy exists; adding a new default version"
  aws iam create-policy-version --policy-arn "$ARN" \
    --policy-document "file://$HERE/bench-policy.json" --set-as-default >/dev/null
else
  aws iam create-policy --policy-name "$POLICY_NAME" \
    --policy-document "file://$HERE/bench-policy.json" >/dev/null
fi
aws iam attach-user-policy --user-name "$USER_NAME" --policy-arn "$ARN"
echo "attached $ARN"

# --- verify before removing anything -----------------------------------------
# The simulator answers what the policy set would decide, without needing the
# reduced credentials to exist yet.
PRINCIPAL="arn:aws:iam::${ACCT}:user/${USER_NAME}"
check() { # action, expected(allowed|implicitDeny|explicitDeny), context...
  local action="$1" want="$2"; shift 2
  local got
  got=$(aws iam simulate-principal-policy --policy-source-arn "$PRINCIPAL" \
        --action-names "$action" --resource-arns '*' "$@" \
        --query 'EvaluationResults[0].EvalDecision' --output text)
  if [ "$got" = "$want" ]; then echo "  ok   $action -> $got"
  else echo "  FAIL $action -> $got (wanted $want)"; FAILED=1; fi
}
FAILED=0
# Only the denials are meaningful here, and they are meaningful *because*
# admin is still attached: an explicit Deny beating an administrator Allow is
# exactly the property being proven. A positive check is worthless in this
# position -- admin allows it whatever the new policy says -- so the allow is
# asserted after the detach instead, where only the new policy can grant it.
echo "simulating with admin still attached (so an explicit deny must still win):"
check iam:CreateUser explicitDeny            # must be denied even under admin
check iam:AttachUserPolicy explicitDeny      # no path back to admin
check s3:CreateBucket explicitDeny           # nothing outside EC2
[ "$FAILED" = 0 ] || { echo "policy did not behave as intended; admin left attached"; exit 1; }

# --- drop admin --------------------------------------------------------------
for P in $(aws iam list-attached-user-policies --user-name "$USER_NAME" \
           --query 'AttachedPolicies[?PolicyName!=`'"$POLICY_NAME"'`].PolicyArn' --output text); do
  echo "detaching $P"
  aws iam detach-user-policy --user-name "$USER_NAME" --policy-arn "$P"
done

echo "remaining policies:"
aws iam list-attached-user-policies --user-name "$USER_NAME" \
  --query 'AttachedPolicies[].PolicyName' --output text

# Now that admin is gone, an allow can only have come from the bench policy.
echo
echo "simulating with only the bench policy attached:"
check ec2:DescribeInstances allowed
check ec2:RunInstances allowed
[ "$FAILED" = 0 ] || { echo "the bench policy does not grant what the runs need"; exit 1; }
echo
echo "Service Quotas are still worth setting -- they are the only hard stop."
echo "A policy limits what can be launched; a quota limits how much."
