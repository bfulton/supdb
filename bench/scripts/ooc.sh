#!/bin/sh
# A bounded run: `bench run --scale ooc` under a memory bound, with the
# bound's group cleaned up if the run dies inside it.
#
#   sh scripts/ooc.sh BYTES [bench run options...]   # e.g. 1610612736 --arms supdb,lmdb
#   sh scripts/ooc.sh --prove-red                     # the gate, proved on a known-red input
#
# The run makes a cgroup of its own under the group this shell is in,
# moves itself into it and removes it when it is done (`bound::Bound`).
# A run the kernel kills for exceeding the bound never gets there: its
# group stays, named on stderr by the run's own `bench: bound ... in
# group PATH` line, and the next run removes a stale sibling only where
# it is empty. The trap below removes it on any exit after that line.
#
# Exit 137 is the kernel's kill (SIGKILL under the group's limit) and is
# passed through as "raise L": a bound the run's own anonymous memory
# does not fit is not a measurement of anything, and the number to move
# is the bound.
#
# Gated on `&&` and on the run's own "banked rungs through" line, never
# on `set -e`: this environment's shell walks straight past a failed
# command under `set -e`, and a chain that relied on it once pushed a
# tree the format gate had rejected. The run's exit status is taken
# from the run and not from the `tee` behind it, since a pipeline's
# status is its last command's. `--prove-red` runs with a bound of
# 64 MiB, which `run::MIN_BOUND_BYTES` refuses, and expects the refusal:
# a gate is trusted only once it has fired on an input known to be red.
cd "$(dirname "$0")/.." || exit 2

bin=./target/release/bench
if [ ! -x "$bin" ]; then
  echo "ooc.sh: $bin is not built; cargo build --release --bin bench first" >&2
  exit 2
fi

work=$(mktemp -d)
err="$work/stderr"
st="$work/status"
group=""
cleanup() {
  if [ -n "$group" ] && [ -d "$group" ]; then
    if rmdir "$group" 2>/dev/null; then
      echo "ooc.sh: removed the run's group $group" >&2
    else
      echo "ooc.sh: the run's group $group refuses removal (tasks still in it?); rmdir it by hand" >&2
    fi
  fi
  rm -rf "$work"
}
trap cleanup EXIT

if [ "${1:-}" = "--prove-red" ]; then
  L=$((64 * 1024 * 1024))
  "$bin" run --scale ooc --bound "$L" --out runs-ci 2>"$err"
  status=$?
  group=$(sed -n 's/^bench: bound [0-9]* bytes in group //p' "$err" | tail -n 1)
  if [ "$status" -ne 0 ] && grep -q "refused: --bound $L is below" "$err"; then
    echo "ooc.sh: the gate fired on a bound of $L bytes (exit $status), as it must"
    exit 0
  fi
  echo "ooc.sh: a bound of $L bytes was NOT refused (exit $status); the gate does not fire" >&2
  cat "$err" >&2
  exit 1
fi

L="${1:-}"
case "$L" in
  ''|*[!0-9]*)
    echo "usage: sh scripts/ooc.sh BYTES [bench run options...] | --prove-red" >&2
    exit 2
    ;;
esac
shift

# The run's stderr is both shown and kept: the group's path is read from
# it and the banked-rungs line is the verdict. Its status is written by
# the subshell that ran it, where `$?` is the run's own.
( "$bin" run --scale ooc --bound "$L" "$@" 2>&1; echo $? >"$st" ) | tee "$err"
status=$(cat "$st" 2>/dev/null || echo 1)
group=$(sed -n 's/^bench: bound [0-9]* bytes in group //p' "$err" | tail -n 1)
if [ "$status" -eq 137 ]; then
  echo "ooc.sh: the run was killed under its bound of $L bytes: raise L" >&2
  exit 137
fi
if [ "$status" -eq 0 ] && grep -q "banked rungs through" "$err"; then
  echo "ooc.sh: bounded run under $L bytes banked" >&2
  exit 0
fi
echo "ooc.sh: the run under $L bytes did not bank its rungs (status $status)" >&2
exit 1
