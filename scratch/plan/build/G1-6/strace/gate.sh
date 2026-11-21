#!/bin/sh
# gate.sh BIN OUT: the build-drop test of the zpages binary BIN under strace,
# alone, normalized into OUT, and held to the base's shapes by gate.py; the
# status is gate.py's (and 2 where the test itself failed).
BIN=$1; OUT=$2; S=$(dirname "$0"); mkdir -p "$OUT"
T=the_build_drops_the_raw_records_it_passed_when_asked_to_and_not_otherwise
export TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-6-gate
mkdir -p "$TMPDIR"
setarch -R strace -f -qq -y -e trace=madvise,fadvise64,mincore,openat,mmap,munmap -o "$OUT/$T.raw" "$BIN" --exact "$T" --test-threads=1 > "$OUT/$T.out" 2>&1
rc=$?
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-6-gate
python3 "$S/normalize.py" "$OUT/$T.raw" > "$OUT/$T.norm"
python3 "$S/gate.py" "$OUT" "$T" && [ $rc -eq 0 ] || exit 1
