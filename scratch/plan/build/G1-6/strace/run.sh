#!/bin/sh
# run.sh BIN OUTDIR: each test under strace, alone, the calls of the advice
# classes kept; then normalize.py writes a sorted multiset per test.
BIN=$1; OUT=$2; mkdir -p "$OUT"
export TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-6
mkdir -p "$TMPDIR"
for t in builds_ahead_at_landing_start_past_the_percent_and_only_from_resident_records \
         the_build_drops_the_raw_records_it_passed_when_asked_to_and_not_otherwise \
         the_stride_switch_gives_the_stores_it_reaches_the_stride_form \
         a_build_in_flight_at_the_crossing_has_the_file_it_dropped_asked_back \
         an_attached_form_file_is_warmed_once_per_process_unless_the_warm_is_off \
         a_served_partitions_ordered_index_is_dropped_again_a_second_on \
         a_merge_drops_the_raw_records_of_an_input_its_form_serves; do
  setarch -R strace -f -qq -y -e trace=madvise,fadvise64,mincore,openat,mmap,munmap -o "$OUT/$t.raw" "$BIN" --exact "$t" --test-threads=1 > "$OUT/$t.out" 2>&1
  echo "$t rc=$?" >> "$OUT/rc.txt"
  python3 /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-6/strace/normalize.py "$OUT/$t.raw" > "$OUT/$t.norm"
done
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-6
echo EXIT 0 >> "$OUT/rc.txt"
