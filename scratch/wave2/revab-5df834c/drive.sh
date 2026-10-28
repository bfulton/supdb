#!/bin/sh
# Two engine revisions alternated a process at a time, ABBA, in one sitting.
S=/tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/revab
SIT=$1; ROUNDS=${2:-4}
export TMPDIR=/tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/qtmp
mkdir -p $S/$SIT
i=0
for r in $(seq 1 $ROUNDS); do
  if [ $((r % 2)) -eq 1 ]; then order="5df834c 44389dc"; else order="44389dc 5df834c"; fi
  for rev in $order; do
    i=$((i+1))
    rm -rf /tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/qtmp/supdb-*
    echo "$(date -u +%T) round $r proc $i $rev" >> $S/$SIT/driver.log
    $S/bin/bench-$rev run --scale quick --arms supdb,supdb-ingest,lmdb,lmdb-nosync --reps 3 --out $S/$SIT/p$i-$rev > $S/$SIT/p$i-$rev.log 2>&1
    echo "$(date -u +%T) exit $?" >> $S/$SIT/driver.log
  done
done
echo "$(date -u +%T) done" >> $S/$SIT/driver.log
