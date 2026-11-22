#!/bin/bash
# usage: srcmut.sh RESULTS NAME FILE 'old' 'new' ...: each mutant applied (one match asserted),
# `cargo test --profile checked --test source -- maps` run, its exit and failing tests kept, the
# file put back from keep/ and checked by sha256.
D=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T22
T=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T22/src
res=$1; shift; mkdir -p $D/runs/srcmut $T
cd /home/user/supdb
while [ $# -ge 4 ]; do
  name=$1; F=$2; old=$3; new=$4; shift 4
  b=$(basename $F); want=$(grep " $b\$" $D/keep/sums.txt | cut -d' ' -f1)
  python3 - "$F" "$old" "$new" <<'PY'
import sys; p,o,n=sys.argv[1:]; s=open(p).read(); assert s.count(o)==1, o; open(p,'w').write(s.replace(o,n,1))
PY
  if [ $? -ne 0 ]; then echo "$name: could not apply" >> $res; continue; fi
  TMPDIR=$T cargo test --profile checked --test source -- maps > $D/runs/srcmut/$name.log 2>&1
  rc=$?
  echo "$name: exit $rc; failed: $(grep -E '^test .* FAILED$' $D/runs/srcmut/$name.log | tr '\n' ' ') $(grep '^test result' $D/runs/srcmut/$name.log)" >> $res
  cp $D/keep/$b $F
  got=$(sha256sum $F | cut -d' ' -f1); if [ "$got" = "$want" ]; then echo "$name: restored $b" >> $res; else echo "$name: RESTORE FAILED" >> $res; exit 1; fi
done
echo "EXIT done $(date -u +%FT%TZ)" >> $res
