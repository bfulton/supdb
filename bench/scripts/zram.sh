#!/bin/sh
# zram.sh [-s SIZE] -- ab [bench ab options] | pair.sh A B SIZE [bench ab options]
#
# A pairing with its stores on a RAM-backed block device, for the counts
# and nothing else. Out of core a rate is the faults an operation takes
# times the device's answer to one: about 40 µs on this fleet's disk and
# about 4 on zram, with the kernel's reclaim under the bound and the
# engine's page touches the same on both (bench/CLAUDE.md). So a pairing
# here reads the disk's faults, refaults and bytes per operation, the
# `zpage_*` counters and the group's memory, in a fraction of the time,
# and its rates are nobody's: `bench ab` says so at the table's head and
# foot, `bench run` refuses the device outright, and this script runs
# only `ab` and `pair.sh`. A row of the series never comes from here.
#
# What it distorts, beyond the rates: the controller prices a fault by
# timing it, so under zram it is a different controller unless its price
# is pinned through its seam; a load is write-bound and gains little; a
# kill whose cause is a pile-up behind a fault-slowed pass shrinks with
# the faults. A count read here is confirmed on the disk when it is close,
# and the figure a note quotes is the disk's.
#
# Needs root and the zram module: /sys/block/$ZRAM_DEV (zram0) writable
# and idle. SIZE is the device's (default 4G; a store of ten million keys
# is under two gigabytes on disk, and the device holds one pass's store at
# a time). The device's pages are the host's memory outside any cgroup
# bound, which is the point: the store is on a device, not in the cache.
# Everything is undone on exit, the device reset to idle.
set -u
SIZE=4G
case "${1:-}" in
  -s|--size) SIZE=$2; shift 2 ;;
esac
[ "${1:-}" = "--" ] && shift
cmd=${1:-}
[ $# -gt 0 ] && shift
HERE=$(cd "$(dirname "$0")/.." && pwd)
BIN=${BIN:-$HERE/target/release/bench}
DEV=${ZRAM_DEV:-zram0}
usage() {
  echo "usage: zram.sh [-s SIZE] -- ab [bench ab options] | pair.sh A B SIZE [bench ab options]" >&2
  echo "       a pairing on a RAM-backed device, for counts; run is refused there" >&2
}
case "$cmd" in
  ab|pair.sh|pair) ;;
  run) echo "zram.sh: run is refused on a RAM-backed device: a row's rates would be nobody's" >&2; exit 2 ;;
  *) usage; exit 2 ;;
esac
[ -x "$BIN" ] || { echo "zram.sh: no bench binary at $BIN (cargo build --release)" >&2; exit 2; }
[ -w "/sys/block/$DEV/disksize" ] || { echo "zram.sh: /sys/block/$DEV/disksize is not writable: zram needs root and the module" >&2; exit 2; }
if [ "$(cat "/sys/block/$DEV/initstate")" != 0 ]; then
  echo "zram.sh: $DEV is in use (initstate 1): another run's, or reset it by hand with: echo 1 > /sys/block/$DEV/reset" >&2
  exit 2
fi
MNT=$(mktemp -d "${TMPDIR:-/tmp}/supdb-zram.XXXXXX") || exit 2
cleanup() {
  umount "$MNT" 2>/dev/null
  echo 1 > "/sys/block/$DEV/reset" 2>/dev/null
  rmdir "$MNT" 2>/dev/null
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
# lz4 where the kernel offers it, the default otherwise: the stores are
# half compressible by the suite's payload either way.
echo lz4 > "/sys/block/$DEV/comp_algorithm" 2>/dev/null
echo "$SIZE" > "/sys/block/$DEV/disksize" || { echo "zram.sh: could not size $DEV to $SIZE" >&2; exit 2; }
mkfs.ext4 -q -F "/dev/$DEV" || exit 2
mount "/dev/$DEV" "$MNT" || exit 2
echo "zram.sh: stores on /dev/$DEV ($SIZE, RAM-backed) at $MNT: the counts are the disk's own, the rates are not" >&2
case "$cmd" in
  ab) TMPDIR="$MNT" "$BIN" ab "$@" ;;
  pair.sh|pair) TMPDIR="$MNT" BIN="$BIN" sh "$HERE/scripts/pair.sh" "$@" ;;
esac
rc=$?
echo "zram.sh: done (exit $rc); the device is reset on exit" >&2
exit $rc
