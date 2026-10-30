#!/bin/sh
# One arm against itself, the load pass alone (the ordered load, then the
# shuffled load into a fresh store), five million keys under 640 MiB, the
# process's anonymous bytes sampled each second and the phases stamped.
S=/tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad
BIN=/home/user/supdb/bench/target/release/bench
arm=$1; out=$S/oom-$arm
T=$S/tmp-oom; mkdir -p $T; rm -rf $T/*
cd /home/user/supdb/bench || exit 2
env TMPDIR=$T SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 SUPDB_LOAD_PHASES=1 "$BIN" ab --arms "$arm,$arm" --size 5000000 --bound 671088640 --ops-div 10 --reps 1 --passes load > $out.out 2> $out.err &
P=$!
( while kill -0 $P 2>/dev/null; do b=$(pgrep -P $P -x bench | head -1); [ -z "$b" ] && b=$P; a=$(awk '/RssAnon/{print $2}' /proc/$b/status 2>/dev/null); c=$(awk '/RssAnon/{print $2}' /proc/$P/status 2>/dev/null); echo "$(cut -d' ' -f1 /proc/uptime) $c"; sleep 1; done ) > $out.mem
wait $P; rc=$?
echo "$arm rc $rc; peak anon $(sort -k2 -n $out.mem | tail -1 | awk '{printf "%d MiB", $2/1024}'); $(grep -c 'load start' $out.err) load starts; $(grep -E 'Killed|killed' $out.err | head -1)"
rm -rf $T/*
