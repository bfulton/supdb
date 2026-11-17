#!/bin/bash
# usage: run_counts.sh <label>   (head|base); runs each probe under callgrind one at a time
L=$1
S=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/fix/count
B=/home/user/supdb/bench/target/release/examples
export TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/tmp/count
mkdir -p $TMPDIR
run() { name=$1; shift
  valgrind --tool=callgrind --callgrind-out-file=$S/$L.$name.out "$@" > $S/$L.$name.stdout 2> $S/$L.$name.vg
  echo "$name exit $?" >> $S/$L.status
  callgrind_annotate --threshold=100 --inclusive=no $S/$L.$name.out > $S/$L.$name.self.txt 2>&1
  callgrind_annotate --threshold=100 --inclusive=yes $S/$L.$name.out > $S/$L.$name.incl.txt 2>&1
  rm -rf $TMPDIR/*
}
: > $S/$L.status
run load_durable $B/load_only supdb 100000 1
run load_buffered $B/load_only supdb-ingest 100000 1
run work $B/work_probe 100000 whole
run read --collect-atstart=no '--toggle-collect=<supdb_bench::engines::Supdb as supdb_bench::engines::Engine>::get' $B/read_only supdb 100000 1 n-none
run merge $B/merge_walk 100000 4 1
echo done >> $S/$L.status
