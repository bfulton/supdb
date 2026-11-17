#!/bin/bash
# One pairing, both sittings: BIN=... [EXTRA=...] sit.sh NAME A B SIZE PERIOD OPTS...
# Sitting 1 runs A,B and sitting 2 B,A; sitting 2 runs even if 1 failed.
S=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/page
BIN=${BIN:?}
NAME=$1; A=$2; B=$3; SIZE=$4; PERIOD=$5; shift 5; OPTS="$*"
PARENT=/sys/fs/cgroup/memory$(awk -F: '$2=="memory"{print $3}' /proc/self/cgroup)
UNSET="-u GLIBC_TUNABLES -u MALLOC_ARENA_MAX $(env | sed -n 's/^\(SUPDB_[A-Za-z0-9_]*\)=.*/-u \1/p')"
SETS="SUPDB_LAG_MARK=1 SUPDB_LOAD_PHASES=1 SUPDB_ZPAGES_TRACE=1 SUPDB_PAGER_TRACE=1 $EXTRA"
cd $S/pairs
for k in 1 2; do
  if [ $k = 1 ]; then ARMS=$A,$B; else ARMS=$B,$A; fi
  busy=$(ps -eo pid,args | grep -E 'bench-once|bench-page|bench-p4080|target/release/bench|cargo|rustc|perf ' | grep -v grep | grep -v sit.sh)
  if [ -n "$busy" ]; then echo "BUSY before $NAME-$k: $busy"; exit 3; fi
  if [ -n "$(ls -A $S/tmp)" ]; then echo "tmp not empty before $NAME-$k"; ls -A $S/tmp; exit 3; fi
  df -h / | tail -1 > $NAME-$SIZE-$k.df
  python3 -I $S/sampler.py "$PARENT" $S/samples/$NAME-$k.txt $PERIOD & SPID=$!
  python3 -I $S/storesize.py $S/tmp $S/samples/$NAME-$k.size & ZPID=$!
  T0=$(date +%s); DL=$(dmesg 2>/dev/null | wc -l)
  env $UNSET TMPDIR=$S/tmp $SETS $BIN ab --arms $ARMS --size $SIZE --reps 1 $OPTS \
    > $NAME-$SIZE-$k.out 2> $NAME-$SIZE-$k.err
  echo $? > $NAME-$SIZE-$k.rc
  T1=$(date +%s)
  kill $SPID $ZPID
  echo "$NAME-$k rc=$(cat $NAME-$SIZE-$k.rc) secs=$((T1-T0))"
  grep -m2 pagerlayout $NAME-$SIZE-$k.err
  dmesg 2>/dev/null | tail -n +$((DL+1)) | grep -iE "out of memory|oom|drop_caches" | tail -20 > $NAME-$SIZE-$k.dmesg
  [ -s $NAME-$SIZE-$k.dmesg ] && { echo "dmesg:"; cat $NAME-$SIZE-$k.dmesg; }
  for d in $(ls -A $S/tmp); do echo "removing $S/tmp/$d"; rm -rf "$S/tmp/$d"; done
  ls $PARENT | grep supdb-bench && echo "LEFTOVER GROUP"
done
{ git -C /home/user/supdb rev-parse --short HEAD; git -C /home/user/supdb status --porcelain; echo "set:$SETS bin:$BIN opts:$OPTS"; } > $NAME-$SIZE-head.txt
