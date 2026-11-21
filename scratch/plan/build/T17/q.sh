#!/bin/sh
# quick: build head example(s) and count probe against kept base self file.
# usage: q.sh TAG probe
U=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T17
TAG=$1; P=$2
case $P in
 scan_at) BIN=scan_at_probe; TOG=scan_at_probe::scans; ARGS="100000 1000 100"; OPS=1000;;
 land) BIN=land_probe; TOG=land_probe::rounds; ARGS="50000 8 4"; OPS=100000;;
esac
mkdir -p $U/q/$TAG
(cd /home/user/supdb/bench && cargo build --release --example $BIN) > $U/q/$TAG/build.log 2>&1 || { echo BUILD FAIL; exit 2; }
mkdir -p $U/q/$TAG/tmp
TMPDIR=$U/q/$TAG/tmp valgrind --tool=callgrind --collect-atstart=no --toggle-collect=$TOG --callgrind-out-file=$U/q/$TAG/$P.out /home/user/supdb/bench/target/release/examples/$BIN $ARGS > $U/q/$TAG/$P.stdout 2>$U/q/$TAG/$P.vg || { echo RUN FAIL; exit 2; }
rm -rf $U/q/$TAG/tmp
callgrind_annotate --auto=no --inclusive=no --threshold=100 $U/q/$TAG/$P.out > $U/q/$TAG/$P.self
callgrind_annotate --auto=no --inclusive=no --threshold=100 $U/before-out/base.$P.out > $U/q/base.$P.self
SEEN=/dev/null python3 /home/user/supdb/scripts/cost-diff.py $U/q/base.$P.self $U/q/$TAG/$P.self $OPS /home/user/supdb/scripts/cost-hot.txt $P toggle "supdb::,supdb_bench::,$BIN::"
echo "EXIT $?"
