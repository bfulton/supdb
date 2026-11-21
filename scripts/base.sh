#!/bin/sh
# The commit a change is made on, printed as a full sha: the one definition
# every check that compares the tree with something before it reads (the
# lists' check, scripts/ratchet.sh; the cost group, scripts/cost.sh).
#
#   sh scripts/base.sh        # in the repository whose base is wanted
#
# A candidate is taken from the first of these that is set and not empty:
#   SUPDB_BASE          an explicit override, any commit-ish (the cost group
#                       passes COST_BASE here)
#   SUPDB_PR_BASE       a pull request's base (CI: the event's base sha)
#   SUPDB_PUSH_BEFORE   a push's before (CI: the event's before); all zeros
#                       is a push that made its branch or tag, which has no
#                       before, and its candidate is HEAD~1, the commit
#                       before the pushed head, as the cost job took it
#                       before this script decided it
#   otherwise           origin/main, the remote-tracking branch
# and the base printed is the merge-base of HEAD and the candidate, which
# is the candidate itself wherever it is an ancestor of HEAD (a push's
# before, HEAD~1, and a pull request's base under CI, whose HEAD is the
# merge of the request into that base). CI passes the event's values in the
# environment and nothing else; the deciding is here.
#
# Each of these fails, exit 2, with what to do, and nothing is printed on
# stdout: a shallow clone (a merge-base in one can be wrong at the graft);
# a candidate that is no commit of this clone (a sha not fetched, a root
# commit's HEAD~1, no origin/main); a candidate with no merge-base with
# HEAD. There is no fallback: a check against a base it guessed is a
# verdict it has not earned. How the base was decided goes to stderr.
set -eu

fail() {
  echo "base: $1" >&2
  exit 2
}

git rev-parse --git-dir >/dev/null 2>&1 || fail "not in a git repository"
if [ "$(git rev-parse --is-shallow-repository)" = true ]; then
  fail "this clone is shallow, so a merge-base in it may be wrong: fetch the whole history (git fetch --unshallow; in CI, actions/checkout with fetch-depth: 0)"
fi

zeros=0000000000000000000000000000000000000000
if [ -n "${SUPDB_BASE:-}" ]; then
  cand=$SUPDB_BASE how="SUPDB_BASE (an explicit override)"
elif [ -n "${SUPDB_PR_BASE:-}" ]; then
  cand=$SUPDB_PR_BASE how="SUPDB_PR_BASE (a pull request's base)"
elif [ -n "${SUPDB_PUSH_BEFORE:-}" ] && [ "$SUPDB_PUSH_BEFORE" != "$zeros" ]; then
  cand=$SUPDB_PUSH_BEFORE how="SUPDB_PUSH_BEFORE (a push's before)"
elif [ -n "${SUPDB_PUSH_BEFORE:-}" ]; then
  cand=HEAD~1 how="HEAD~1 (a push that made its branch or tag has no before)"
else
  cand=refs/remotes/origin/main how="origin/main (no override and no event)"
  git rev-parse --verify --quiet "$cand^{commit}" >/dev/null ||
    fail "this clone has no origin/main: git fetch origin main, or name the base in SUPDB_BASE"
fi

sha=$(git rev-parse --verify --quiet "$cand^{commit}") ||
  fail "$how names '$cand', which is no commit of this clone: fetch it, or name another in SUPDB_BASE"
mb=$(git merge-base HEAD "$sha") ||
  fail "$how names $sha, which has no merge-base with HEAD"
echo "base: $mb, the merge-base of HEAD and $how" >&2
echo "$mb"
