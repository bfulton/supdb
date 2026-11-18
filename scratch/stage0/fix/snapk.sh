#!/bin/sh
# usage: snapk.sh [-n] <keep dir> <message file>
# A snapshot of a unit's kept copy, not of the working tree, which a mutant runner
# may be writing: HEAD's tree with the kept copies of the unit's files in place of
# HEAD's, committed on top of the remote branch's head and pushed there by URL. The
# local branch, the index, the working tree and the remote-tracking ref are not
# touched. Refuses when the tree holds a change the keep directory has no copy of.
S=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad
B=claude/inspiring-brahmagupta-d7y8n7
DRY=
if [ "$1" = "-n" ]; then DRY=-n; shift; fi
K=$1; M=$2
cd /home/user/supdb || exit 1
[ -d "$K" ] && [ -f "$M" ] || { echo "usage: snapk.sh [-n] <keep dir> <message file>"; exit 1; }
head -1 "$M" | grep -q '^In progress: ' || { echo "the message's subject does not start 'In progress: '"; exit 1; }
URL=$(git remote get-url origin) || exit 1
RH=$(git ls-remote "$URL" "refs/heads/$B" | cut -f1)
[ -n "$RH" ] || { echo "no remote head"; exit 1; }
git cat-file -e "$RH^{commit}" 2>/dev/null || { echo "remote head $RH is not here"; exit 1; }
# A kept copy's name: the path with its slashes as underscores (src_db.rs), or, for
# the two files the first units kept, the bare name they used (db.rs is src/db.rs).
kept() {
  u=$(echo "$1" | tr / _)
  if [ -f "$K/$u" ]; then echo "$u"; return; fi
  case $1 in
    src/db.rs) echo db.rs;;
    tests/durable.rs) echo durable.rs;;
    *) echo "";;
  esac
}
UNT=$(git status --porcelain | grep '^??' | head -3)
[ -z "$UNT" ] || { echo "untracked files the snapshot would leave out:"; echo "$UNT"; exit 1; }
I=$S/stage0/fix/snapk.index
rm -f "$I"
GIT_INDEX_FILE=$I git read-tree HEAD || exit 1
for p in $(git diff --name-only HEAD); do
  k=$(kept "$p")
  [ -n "$k" ] && [ -f "$K/$k" ] || { echo "the tree changes $p and $K has no copy of it"; rm -f "$I"; exit 1; }
  blob=$(git hash-object -w "$K/$k") || exit 1
  mode=$(git ls-files -s -- "$p" | cut -d' ' -f1)
  GIT_INDEX_FILE=$I git update-index --cacheinfo "$mode,$blob,$p" || exit 1
  same=differs; cmp -s "$K/$k" "$p" && same=same
  echo "$p <- $K/$k ($same as the tree's now)"
done
T=$(GIT_INDEX_FILE=$I git write-tree) || exit 1
rm -f "$I"
if [ "$T" = "$(git rev-parse "$RH^{tree}")" ]; then echo "nothing new: the remote head has this tree"; exit 0; fi
echo "tree $T on $(echo $RH | cut -c1-8)"
[ "$DRY" = "-n" ] && exit 0
C=$(git commit-tree "$T" -p "$RH" -F "$M") || exit 1
git push -q "$URL" "$C:refs/heads/$B" || { echo "push failed"; exit 1; }
echo "$C" > $S/stage0/fix/remote-head
echo "pushed $C on $RH"
