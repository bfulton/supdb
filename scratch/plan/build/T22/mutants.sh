#!/bin/bash
D=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T22
F=/home/user/supdb/scripts/check.sh
res=$D/mutants-results.log; : > $res
want=$(grep check.sh $D/keep/sums.txt | cut -d' ' -f1)
m() { # name, python replacement old -> new
  name=$1; old=$2; new=$3
  python3 - "$F" "$old" "$new" <<'PY'
import sys; p,o,n=sys.argv[1:]; s=open(p).read(); assert s.count(o)==1, o; open(p,'w').write(s.replace(o,n))
PY
  [ $? -eq 0 ] || { echo "$name: could not apply" >> $res; return; }
  $D/group.sh mut2-$name 16 1
  echo "$name: $(grep -c 'exit [1-9]' $D/runs/mut2-$name/summary.log) red of $(grep -c '^run' $D/runs/mut2-$name/summary.log) runs; $(grep '^run' $D/runs/mut2-$name/summary.log | tail -1)" >> $res
  cp $D/keep/check.sh $F
  got=$(sha256sum $F | cut -d' ' -f1); [ "$got" = "$want" ] && echo "$name: restored $got" >> $res || { echo "$name: RESTORE FAILED" >> $res; exit 1; }
}
m wrong-var 'RUST_BACKTRACE=0 \' 'RUST_LIB_BACKTRACE=0 \'
echo "EXIT done" >> $res
