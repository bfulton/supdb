#!/bin/sh
cd /home/user/supdb/bench
O=/tmp/claude-0/-home-user-supdb/28b9c9ae-2263-5e29-a6fc-74dd8e38e2a1/scratchpad/onefile
BIN=$PWD/target/release/bench
rm -rf /tmp/ab-rw/* $O/run-inv
mkdir -p /tmp/ab-rw
python3 -c "import time;print(time.time()-time.clock_gettime(time.CLOCK_MONOTONIC))" > $O/inv.off
# Nothing of the shell's own `SUPDB_` variables reaches the traced run,
# as `bench/scripts/pair.sh` has it: the names are taken from the
# environment and not from a list.
UNSET=""
for v in $(env | sed -n 's/^\(SUPDB_[A-Za-z0-9_]*\)=.*/\1/p'); do
  UNSET="$UNSET -u $v"
done
# shellcheck disable=SC2086
env $UNSET TMPDIR=/tmp/ab-rw SUPDB_LAG_MARK=1 \
  strace -f -ttt -T -y --seccomp-bpf -o $O/inv.strace \
  -e trace=fsync,fdatasync,sync_file_range,unlink,unlinkat,rename,renameat,renameat2,link,linkat,openat,close,ftruncate,fallocate,munmap,mmap,pwrite64,pwritev,write,writev,pread64,fadvise64 \
  $BIN run --scale ooc --bound 1342177280 --ops-div 10 --reps 1 --top 10000000 --bottom 10000000 \
  --arms supdb --passes load,lag --out $O/run-inv > $O/inv.out 2> $O/inv.err
echo "exit $?" > $O/inv.done
