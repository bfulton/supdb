#!/bin/bash
# usage: mutants3.sh RESULTS NAME FILE N 'old' 'new' [NAME FILE N old new ...]
# each mutant: exact-string replace (one match asserted) in FILE, group.sh mut3-NAME N (stop at
# first red), FILE put back from keep/ and checked by sha256 after each.
D=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T22
res=$1; shift
while [ $# -ge 5 ]; do
  name=$1; F=$2; n=$3; old=$4; new=$5; shift 5
  b=$(basename $F); want=$(grep " $b\$" $D/keep/sums.txt | cut -d' ' -f1)
  python3 - "$F" "$old" "$new" <<'PY'
import sys; p,o,n=sys.argv[1:]; s=open(p).read()
for a,b in zip(o.split('\x01'),n.split('\x01')):
    assert s.count(a)==1, a; s=s.replace(a,b,1)
open(p,'w').write(s)
PY
  if [ $? -ne 0 ]; then echo "$name: could not apply" >> $res; continue; fi
  $D/group.sh mut3-$name $n 1
  echo "$name: $(grep -c 'exit [1-9]' $D/runs/mut3-$name/summary.log) red of $(grep -c '^run' $D/runs/mut3-$name/summary.log) runs; last: $(grep '^run' $D/runs/mut3-$name/summary.log | tail -1)" >> $res
  cp $D/keep/$b $F
  got=$(sha256sum $F | cut -d' ' -f1); if [ "$got" = "$want" ]; then echo "$name: restored $b $got" >> $res; else echo "$name: RESTORE FAILED" >> $res; exit 1; fi
done
echo "EXIT done $(date -u +%FT%TZ)" >> $res
