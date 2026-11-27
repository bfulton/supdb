#!/bin/sh
# Every commit a change adds carries a signature: the main line takes only
# commits with verified signatures, every commit of a pull request and not
# its head alone (CLAUDE.md, the main line takes only signed commits).
#
#   sh scripts/signed.sh [TIP]
#
# The commits are those after the base (scripts/base.sh) up to TIP, HEAD by
# default; under CI's pull request, whose HEAD is the merge the checkout
# makes of the request into its base, and unsigned, the request's own head.
# A commit carries a signature where its header has one; whether GitHub
# verifies it is GitHub's to say, and this check is of the case seen: a
# pull request opened with fifty-four commits that had none, made where the
# environment did not sign and by `git commit-tree`, which signs only with
# -S. The red input is `--red`: an unsigned commit made on top of TIP, which
# the check has to refuse. Exit 0 green, 1 red, 2 for a base not decided.
set -eu
if [ "${1:-}" = --red ]; then
  tip=$(git rev-parse "${2:-HEAD}")
  red=$(GIT_AUTHOR_NAME=red GIT_AUTHOR_EMAIL=red@invalid GIT_COMMITTER_NAME=red \
    GIT_COMMITTER_EMAIL=red@invalid git commit-tree "$tip^{tree}" -p "$tip" -m 'signed.sh red')
  st=0
  sh "$0" "$red" >/dev/null 2>&1 || st=$?
  [ "$st" = 1 ] || { echo "signed: the check exited $st over an unsigned commit" >&2; exit 1; }
  echo "signed red: ok"
  exit 0
fi
base=$(sh scripts/base.sh) || exit 2
tip=${1:-HEAD}
if [ -z "${1:-}" ] && [ -n "${SUPDB_PR_BASE:-}" ] &&
  [ "$(git rev-list --parents -n 1 HEAD | wc -w)" = 3 ] &&
  [ "$(git rev-parse HEAD^1)" = "$(git rev-parse "$SUPDB_PR_BASE^{commit}")" ]; then
  tip=HEAD^2
fi
n=0
bad=0
for c in $(git rev-list "$base..$tip"); do
  n=$((n + 1))
  if ! git cat-file commit "$c" | sed '/^$/q' | grep -q '^gpgsig '; then
    echo "signed: $(git log -1 --format='%h %ad %s' --date=short "$c") carries no signature" >&2
    bad=$((bad + 1))
  fi
done
if [ "$bad" != 0 ]; then
  echo "signed: $bad of the $n commits after the base carry no signature" >&2
  exit 1
fi
echo "signed: the $n commits after the base each carry a signature"
