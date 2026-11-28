#!/bin/sh
# usage: stalldump.sh OUT SECS -- cmd...: runs cmd; if still running after SECS, dumps threads
OUT=$1; SECS=$2; shift 3
"$@" > $OUT.log 2>&1 &
pid=$!
i=0
while kill -0 $pid 2>/dev/null; do
  sleep 1; i=$((i+1))
  if [ $i -eq $SECS ]; then
    for t in /proc/$pid/task/*; do
      echo "== $(cat $t/comm) $(cut -d' ' -f3 $t/stat)"; cat $t/wchan 2>/dev/null; echo; cat $t/stack 2>/dev/null
    done > $OUT.threads
    command -v gdb >/dev/null && gdb -p $pid -batch -ex 'thread apply all bt 25' > $OUT.gdb 2>&1
  fi
done
wait $pid; echo "rc=$?" >> $OUT.log
