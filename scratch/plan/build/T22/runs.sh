#!/bin/bash
# usage: runs.sh TAG N BIN [extra qemu args] -- [test args]
D=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T22
tag=$1; n=$2; bin=$3; shift 3
qa=(); while [ $# -gt 0 ] && [ "$1" != "--" ]; do qa+=("$1"); shift; done; [ "$1" = "--" ] && shift
mkdir -p $D/runs/$tag
sum=$D/runs/$tag/summary.log; : > $sum
echo "start $(date -u +%FT%TZ) bin=$(sha256sum $bin|cut -c1-12) qa=${qa[*]} args=$*" >> $sum
for i in $(seq 1 $n); do
  t=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T22/$tag-$i; rm -rf "$t"; mkdir -p "$t"
  TMPDIR=$t timeout 1500 qemu-aarch64-static -L /usr/aarch64-linux-gnu "${qa[@]//@I@/$i}" $bin "$@" > $D/runs/$tag/run$i.log 2>&1
  rc=$?
  res=$(grep '^test result' $D/runs/$tag/run$i.log | tail -1)
  seg=$(grep -c 'QEMU internal SIGSEGV' $D/runs/$tag/run$i.log)
  echo "run $i exit $rc segv $seg $res" >> $sum
  rm -rf "$t"
done
echo "EXIT done $(date -u +%FT%TZ)" >> $sum
