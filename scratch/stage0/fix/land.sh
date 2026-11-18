#!/bin/sh
# usage: land.sh [-n]
# The local commits the remote branch lacks, pushed. Where the remote head holds
# snapshots made by snap.sh ("In progress: ..."), each local commit is made again
# with its own tree, message, author and dates on top of the remote head, the chain
# is pushed, and the local branch is moved onto it: every commit keeps its tree, so
# the last one's tree is HEAD's and the index and the working tree are not touched.
# -n prints what it would do.
S=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad
B=claude/inspiring-brahmagupta-d7y8n7
DRY=$1
cd /home/user/supdb || exit 1
URL=$(git remote get-url origin) || exit 1
RH=$(git ls-remote "$URL" "refs/heads/$B" | cut -f1)
[ -n "$RH" ] || { echo "no remote head"; exit 1; }
git cat-file -e "$RH^{commit}" 2>/dev/null || { echo "remote head $RH is not here"; exit 1; }
H=$(git rev-parse HEAD) || exit 1
if [ "$RH" = "$H" ]; then echo "nothing to land: the remote head is HEAD"; exit 0; fi
if git merge-base --is-ancestor "$RH" "$H"; then
  echo "fast-forward: $(git rev-list --count "$RH..$H") commit(s) on $RH"
  [ "$DRY" = "-n" ] && exit 0
  git push -q "$URL" "$H:refs/heads/$B" || { echo "push failed"; exit 1; }
  git update-ref "refs/remotes/origin/$B" "$H"
  echo "$H" > $S/stage0/fix/remote-head
  echo "pushed $H"; exit 0
fi
MB=$(git merge-base "$RH" "$H") || exit 1
# What the remote has past the common commit must be snapshots and nothing else.
BAD=$(git log --format=%s "$MB..$RH" | grep -v '^In progress: ' | head -1)
[ -z "$BAD" ] || { echo "the remote holds a commit that is no snapshot: $BAD"; exit 1; }
N=$(git rev-list --count "$MB..$H")
if [ "$N" = 0 ]; then echo "nothing to land: no local commit past $MB (the remote is $(git rev-list --count "$MB..$RH") snapshot(s) ahead)"; exit 0; fi
[ -z "$(git rev-list --merges "$MB..$H")" ] || { echo "a merge among the local commits"; exit 1; }
P=$RH
# The snapshots are taken back first: a commit whose tree is the common commit's,
# so that each commit made again after it shows its whole change against its parent
# and not what it added to a snapshot.
if [ "$(git rev-parse "$RH^{tree}")" != "$(git rev-parse "$MB^{tree}")" ]; then
  echo "taken back on $(echo $P | cut -c1-8): the tree of $(git log -1 --format='%h %s' $MB)"
  if [ "$DRY" != "-n" ]; then
    P=$(git commit-tree "$MB^{tree}" -p "$P" -F $S/stage0/fix/takeback-msg.txt) || exit 1
  fi
fi
for c in $(git rev-list --reverse "$MB..$H"); do
  echo "re-made on $(echo $P | cut -c1-8): $(git log -1 --format='%h %s' $c)"
  [ "$DRY" = "-n" ] && continue
  P=$(GIT_AUTHOR_NAME=$(git log -1 --format=%an $c) GIT_AUTHOR_EMAIL=$(git log -1 --format=%ae $c) \
      GIT_AUTHOR_DATE=$(git log -1 --format=%aI $c) GIT_COMMITTER_NAME=$(git log -1 --format=%cn $c) \
      GIT_COMMITTER_EMAIL=$(git log -1 --format=%ce $c) GIT_COMMITTER_DATE=$(git log -1 --format=%cI $c) \
      sh -c 'git cat-file commit "$0" | sed "1,/^\$/d" | git commit-tree "$0^{tree}" -p "$1"' "$c" "$P") || exit 1
done
[ "$DRY" = "-n" ] && exit 0
[ "$(git rev-parse "$P^{tree}")" = "$(git rev-parse "$H^{tree}")" ] || { echo "the chain's tree is not HEAD's"; exit 1; }
git push -q "$URL" "$P:refs/heads/$B" || { echo "push failed"; exit 1; }
echo "$P" > $S/stage0/fix/remote-head
# The local branch moves only if nothing was committed meanwhile.
[ "$(git rev-parse HEAD)" = "$H" ] || { echo "pushed $P, but HEAD moved meanwhile: run again"; exit 1; }
git reset -q --soft "$P" && git update-ref "refs/remotes/origin/$B" "$P" && echo "pushed $P; the local branch is on it"
