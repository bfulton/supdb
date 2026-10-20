#!/bin/sh
# Stage 2a, WRITTEN AND NOT RUN, as revised for branch wave2-zproof-fix2: the paired ten-million-key measurement of
# `supdb-nocache` against `supdb-nocachezpages` (the second form forced on, point reads and counts only), the in-core
# control, the untimed fault record of both arms' read cell, the arm that keeps the build's drop of the raw records
# off (`supdb-nocachezpagesnodrop`), and the default path's instruction count.
#
# Read the pre-registration first (it is in the report that came with this branch, and section 0 below corrects the
# centres that review found do not follow from the counted numbers): the predictions, the verdict and the stop rule
# are written there, before any of this has run. Each block is a command to paste, not a script to run whole: the
# pairing is two sittings and the record is a third piece of work, and the machine must be free of every other timing
# run for each (bench/CLAUDE.md: never two timing benchmarks at once, this includes your own agents). Gates are `&&`
# and a verdict line, never `set -e` and never a pipe to `tail` or `grep` that swallows the exit status (CLAUDE.md,
# "Running the checks").

# ---------------------------------------------------------------------------------------------------------------
# 0. Corrections to the registered predictions. These replace the report's text where they differ; nothing else in
#    the pre-registration moves. Each is the counted number the review recomputed, and none is a measurement of the
#    arm: the arm has not been timed.
#
#  P1  The staircase clause is withdrawn. The build leaves the form file in the page cache (at one fifth scale the
#      cgroup held 149 MB of its 200 right after the load), so Z's cells are expected to read about 0.00 in all
#      three, not to fall across them while a cold file warms. The refutation line is unchanged: Z's 3t cell above
#      0.05. A resident file after a build is the expected shape and is recorded as such.
#  P4  The build counts are identities the counters print, not an absolute range. Per pass (the runner sums a
#      counter over the pass's two stores; a gauge is named `*_at_end`):
#        zpage_form files_at_end + zpage_small_partitions_at_end == zpage_partitions_at_end
#        zpage_attached == zpage_builds                 (nothing is attached at open in a fresh store)
#        zpage_builds >= zpage_form files_at_end          (equal when every merge landed before the builds, which
#                                                        flush and the settle's first join make the case: 6 of 6
#                                                        and 12 of 12 partitions in the 2M and 3M shuffled loads;
#                                                        a surplus is a merge that landed during a settle, and is
#                                                        a count to explain, not a failure)
#        zpage_build_failed == zpage_build_orphaned == zpage_swept == zpage_fallbacks == 0
#      The pass itself refuses a store that breaks the first identity, whose directory holds anything but one
#      `.zpg` per form file and no other `zpg-` name, or whose form file is out of service (it checks the ordered load's
#      store and the shuffled load's). The report's 68-93 builds (18-23 for the ordered store, 50-70 for the shuffled
#      store) is withdrawn: its 2-3 rebuilds of a partition do not happen, and its 64 MiB partitions are
#      unmeasured at 10M (the engine wrote 31-36 MB partitions at 1M-2M keys); one build per final partition is the
#      shape counted.
#  P5  `load bytes_on_disk_per_byte` 1.89 (1.86-1.92) against R's 1.304 (678 MB of form file at 67.8 B a key: 1.304 +
#      0.584); `load device_bytes_per_byte` 1.92 (1.89-1.96) against 1.339. The sizes (file, group, pages,
#      resident) are unchanged.
#  P6  `load ops_per_s` of the ordered load 0.40-0.60x of R (centre 0.50): the builds' CPU alone is 4.8-5.9 s per 10M
#      keys (0.48-0.59 s of user+sys at 1M keys, 2,740 instructions a key) against the banked 6.6-7.4 s load, which
#      is 0.53-0.61x before the 678 MB written and, under the bound, the raw segments read back from the device
#      (224 MB read at one fifth scale). `load-shuffled ops_per_s` 0.65-0.85x (centre 0.78), one build per final
#      partition. Both are read by the sum of the phases (section 8), never by the rate alone. The build's drop of
#      the raw records (posix_fadvise DONTNEED) is 29% of its elapsed time (0.273 s of 0.947 s for a settle of 1M keys) and is part of this cost:
#      `supdb-nocachezpagesnodrop` is the arm that prices it (section 8), and no in-core scan cell of the forced arm
#      is read as the form's cost, since the build leaves the raw records cold.
#  P8  The default path's price is measured at this branch's head, with the probe source identical for base and
#      head: +1.9 instructions a point read and +3.0 a count (section 6). Sibling probes read +2.9 to +3.15 for
#      the read; the figure moves by about one instruction with the code after the probe's loop (LTO inlines
#      `State::pieces_over` or not), so it is quoted as +2 to +3 a read and +3 a count, "at most 3" for the read within
#      that spread and with no margin for the count. The default path's verdict is the quick row's smallest rung read
#      pass, not an instruction count. `#[cold]` on `part_read_z` and `part_count_z` was priced (-0.3 a read, -1.0 a
#      count) and not adopted: it would put the forced arm's own read function in the cold section.
#  The retire of a merge's input unlinks its form file on the segment-work thread (`Maint::retire_seg`), a device round
#  trip a file on this filesystem (33-98 ms for a 34 MB synced file). The 2a cells (load, read, read-threaded) retire
#  none; the all-passes run (section 3b) carries it in every cell that merges (the lag sweep, the bursts, the mixes
#  that write), and a lag or burst cell that trails is read with this cost named before it is called the form's.
# ---------------------------------------------------------------------------------------------------------------

# ---------------------------------------------------------------------------------------------------------------
# 1. Where things are.  $TREE is a checkout at the head of the branch wave2-zproof-fix2 (or a tree it was merged
#    into); $Z holds the logs; $TOOLS holds the fault-site bucketing tool stage 1b wrote.
# ---------------------------------------------------------------------------------------------------------------
TREE="${TREE:-/home/user/supdb/.claude/worktrees/wf_d4132f43-9fb-5}"
Z=/tmp/claude-0/-home-user-supdb/43547e2e-b745-59c0-b3a0-b32f7c7e7919/scratchpad/wave2
TOOLS=/tmp/claude-0/-home-user-supdb/43547e2e-b745-59c0-b3a0-b32f7c7e7919/scratchpad/wave1/ordread-tools
PERF=/usr/lib/linux-tools-6.8.0-31/perf          # /usr/bin/perf is a wrapper that refuses this kernel (CLAUDE.md)
cd "$TREE/bench" || exit 2
# The machine's shared bench target holds a built RocksDB; a tree left to its own bench/target builds it cold (twenty
# minutes, several gigabytes, and a Bus error from the linker at the disk's allowance). The shared target also holds
# UNHASHED deps/libsupdb.rlib and deps/libsupdb.so, which a bench build from any other worktree overwrites, and a
# different tree's cargo may then call its unit fresh and link the wrong engine: a binary of another tree's engine is
# a measurement of the wrong store, found after the hour it took. So the sitting's binary is built from $TREE
# immediately before the first pair, with nothing else compiling into the directory (pgrep below), and the first
# lines of the first pair's output show the form is in it (`count zpage_forced 2` for the forced arm and the
# arm-differs tests, below). Do not build while a timing run executes there.
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/home/user/supdb/bench/target}"
export CARGO_TARGET_DIR
BIN="$CARGO_TARGET_DIR/release/bench"
# The stores a pairing writes go under the temp directory (supdb-ab-<pid>), and a run deletes what it made there when
# it ends; a directory of its own keeps another run's cleanup from taking them and theirs from taking ours.
mkdir -p /tmp/wave2-zproof-tmp
TMPDIR=/tmp/wave2-zproof-tmp
export TMPDIR

# The disk. A pass writes about 1.7 GB for the raw arm and 0.7 GB more for the forced one (the form files), and removes
# it when it ends; the fault record adds a recording of some hundred megabytes beside it; a link of the suite's
# binary is a third of a gigabyte. Eight free gigabytes is the least to start with, because the failure at the
# allowance is the linker's, not a write's, and `df` still shows free space when it comes.
need_gb=8
free_kb=$(df -Pk "$TMPDIR" "$Z" | awk 'NR > 1 { if (min == "" || $4 < min) min = $4 } END { print min }')
if test "$free_kb" -ge $((need_gb * 1024 * 1024)); then
  echo "disk: $((free_kb / 1024 / 1024)) GB free, enough"
else
  echo "disk: $((free_kb / 1024 / 1024)) GB free, need $need_gb: stop and make room"
  exit 2
fi

# Nothing else is compiling or timing, and the memory controller this needs is writable (what scripts/ooc.sh needs).
# Expect no process from the five pgreps; a build or a run in flight is a reason to wait, not to go on.
pgrep -a bench; pgrep -a perf; pgrep -a cargo; pgrep -a rustc; pgrep -a valgrind
sed -n 's/^[0-9]*:memory:\(.*\)$/\1/p' /proc/self/cgroup && ls -ld /sys/fs/cgroup/memory/cgroup.procs

# The suite's binary, release profile, built from the branch. The two packages are cleaned first: the bench crate is
# the root of its own workspace, whose artifacts do not depend on the checkout path, so a build from this tree into
# a target another tree last built can be called fresh by cargo (it compares the mtimes of the files the OTHER
# build listed) and link the wrong store. Cleaning them costs minutes; RocksDB, the twenty-minute build, is not
# touched. Read the build's own `Compiling supdb ...` and `Compiling supdb-bench ...` lines for $TREE's path: a
# build that printed neither is the failure this guards.
cargo clean --offline -p supdb -p supdb-bench --release \
  && cargo build --release --bin bench && test -x "$BIN" && echo "bench built"
git -C "$TREE" rev-parse --short HEAD          # the engine commit under test: the first line of every log

# A pairing is a pairing only if the arms can be checked to differ in the option alone and the form to have been used,
# and the refusals the pass makes have been seen to fire. All are tests of the branch under the checked profile
# (the lib test binary, not the suite's); eight tests are expected to pass, by name, and the gate is cargo's exit
# status and the count, not the lines:
GATE_LOG="$Z/zproof-gate-tests.log"
cargo test --profile checked --lib -- \
    engines::tests::the_zpages_arm_differs_from_nocache_in_the_option_alone \
    engines::tests::the_nodrop_arm_differs_from_the_zpages_arm_in_the_drop_alone \
    run::tests::a_forced_form_that_is_not_in_use_is_refused \
    run::tests::a_forced_form_with_a_form file_out_of_service_or_reads_gone_raw_is_refused \
    run::tests::the_read_phase_asks_whether_a_forced_arm_read_through_its_form \
    run::tests::the_shuffled_load_asks_whether_a_forced_arm_covers_the_second_store \
    run::tests::the_pass_refuses_a_forced_arm_that_covers_nothing_and_runs_one_that_does \
    run::tests::threaded_readers_read_the_loaded_store > "$GATE_LOG" 2>&1 \
  && test "$(grep -c '^test .* ok$' "$GATE_LOG")" -eq 8 \
  && echo "gate tests: eight green" || echo "gate tests: RED, stop and read $GATE_LOG"

# ---------------------------------------------------------------------------------------------------------------
# 2. The pairing: 10M keys, a 1 GiB cgroup, the read passes at --ops-div 10 (a million reads), twelve pairs after the
#    warmup pair, on one thread and on the class's two and three. Two sittings, THE ARMS SWAPPED in the second (the
#    columns of the second are A = supdb-nocachezpages, B = supdb-nocache: read B/A there as A/B). `ab` swaps
#    which arm goes first every other rep itself. The load is a pass of the pairing (`--passes load,...`): the
#    forced arm's `sync` waits for every partition's form file, so its load carries the build and the pairing
#    prints what that costs. The arms also differ in what their `sync` joins: the forced arm's calls
#    `Db::zpages_settle`, which joins the segment work (the slot's fdatasync, the WAL's retirement, any
#    promotion), where `supdb-nocache`'s flush returns before them. Z's read pass therefore starts on a quiet
#    engine and R's may overlap the tail of its drain; small beside a 19 s pass, stated here and not removed, and
#    read in the load window's sync phase (section 8).
#
#    Time: 26 passes (13 pairs), each the ordered load of ten million keys (about 7 s raw and a build more for the
#    forced arm, with the drain), the million reads of the single-thread pass (19 s at the banked 54k/s for the raw
#    arm, 6 s at the predicted 180k/s for the forced one) and the two- and three-thread passes (2M and 3M reads),
#    plus the second store the runner opens for the shuffled load (the form files are built for it too, one for each
#    final partition), the store's open, its prime and its removal. Plan an hour a sitting, and read the `rep 0/12
#    (warmup)` line `ab` prints on stderr when the first pair is done: its elapsed seconds are one pair, and
#    thirteen times that is the sitting.
#
#    The instrument variables are unset for the pairing and are not the pairing's to set: the suite turns the scan
#    timers on for every arm under SUPDB_LAG_COUNTERS, and an option the environment sets is set for both arms.
# ---------------------------------------------------------------------------------------------------------------
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_LAG_MARK -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"

env $UNSET "$BIN" ab \
    --arms supdb-nocache,supdb-nocachezpages \
    --size 10000000 --bound 1073741824 --ops-div 10 --reps 12 \
    --passes load,read,read-threaded --pairs 'read ' \
  > "$Z/zproof-ab-1.out" 2> "$Z/zproof-ab-1.err" \
  && grep -q ' quantities: ' "$Z/zproof-ab-1.out" \
  && echo "sitting 1: the table is whole" || echo "sitting 1: NOT whole"

env $UNSET "$BIN" ab \
    --arms supdb-nocachezpages,supdb-nocache \
    --size 10000000 --bound 1073741824 --ops-div 10 --reps 12 \
    --passes load,read,read-threaded --pairs 'read ' \
  > "$Z/zproof-ab-2.out" 2> "$Z/zproof-ab-2.err" \
  && grep -q ' quantities: ' "$Z/zproof-ab-2.out" \
  && echo "sitting 2: the table is whole" || echo "sitting 2: NOT whole"

# The pairing is read only if the form was used, and only the nocache column is zero (the gate in the
# pre-registration's VERDICT). A pass refuses by itself a forced arm whose store is not whole after either load
# (form files + small != partitions, a directory with a name or a count of `zpg-` files the form files do not account
# for, a form file out of service) and one that read through the form for under 99% of its reads; those are errors in
# the .err file, not lines to find here. In sitting 1 the forced arm is column B, in sitting 2 column A:
#   count zpage_forced                 nocache 0   zpages 2          (both stores of a pass)
#   count zpage_reads                  nocache 0   zpages 6000000    (1M by the writer's handle, 2M + 3M by the handles of
#                                                                     the two- and three-thread passes: the slots' sum)
#   count zpage_decodes                0 / zpage_reads minus the reads of a stored page (a decode for every read or count
#                                      of a key that falls in a page's range, present or absent; none for a stored page)
#   count zpage_counts                 0 / 0                          (the passes read, they do not count)
#   count zpage_rawserves              0 / 0
#   count zpage_fallbacks              0 / 0
#   count zpage_raw_scans              0 / 0                          (no scan pass here)
#   count zpage_rejected_damage        0 / 0      count zpage_rejected_identity   0 / 0
#   count zpage_build_failed           0 / 0      count zpage_build_orphaned      0 / 0      count zpage_swept   0 / 0
#   count zpage_declined_block_backed / _oversize / _incompressible / _unplaceable   0 / 0
#   count zpage_declined_small         0 / 0 to 4                     (the tail of a store, per store, 0-2)
#   count zpage_partitions_at_end      the two stores' live partitions, summed
#   count zpage_form files_at_end        == partitions - zpage_small_partitions_at_end  (P4's identity, corrected above)
#   count zpage_builds, zpage_attached builds == attached >= form_files_at_end (P4 above)
#   count zpage_pages_at_end, zpage_file_bytes_at_end, zpage_group_bytes_at_end: the form files' size and what it
#                                      decodes to (the ratio is the pages' compression)
grep -E '^count zpage_' "$Z/zproof-ab-1.out" "$Z/zproof-ab-2.out"

# The verdict's rows, the single-thread cell and the two- and three-thread cells (_2t, _3t), and the load:
grep -E '^read (majfaults_per_op|reads_per_s|read_bytes_per_op|refaults_per_op|p99_us)' \
    "$Z/zproof-ab-1.out" "$Z/zproof-ab-2.out"
grep -E '^load (ops_per_s|bytes_on_disk_per_byte|device_bytes_per_byte)' "$Z/zproof-ab-1.out" "$Z/zproof-ab-2.out"
grep -E '^load-shuffled ops_per_s' "$Z/zproof-ab-1.out" "$Z/zproof-ab-2.out"
# and every pair of a read quantity, rep by rep, in the --pairs block at the end of each file.

# ---------------------------------------------------------------------------------------------------------------
# 3. The in-core control (7.3's 3M row): 3M keys fit the 1 GiB bound, the form is forced on anyway, and the read is
#    the raw read's 0.73 us against the form's decode. One sitting, the same flags, no division of the reads.
# ---------------------------------------------------------------------------------------------------------------
env $UNSET "$BIN" ab \
    --arms supdb-nocache,supdb-nocachezpages \
    --size 3000000 --bound 1073741824 --reps 12 \
    --passes load,read,read-threaded --pairs 'read ' \
  > "$Z/zproof-ab-3m.out" 2> "$Z/zproof-ab-3m.err" \
  && grep -q ' quantities: ' "$Z/zproof-ab-3m.out" && echo "3M control: the table is whole"
grep -E '^(read (reads_per_s|majfaults_per_op)|count zpage_(forced|reads|rawserves|fallbacks))' "$Z/zproof-ab-3m.out"

# ---------------------------------------------------------------------------------------------------------------
# 3b. The rows that must not move (7.6 item 4): the same pairing with every pass, one sitting, six pairs. Scans and the
#    mixes' scans are raw by construction in this stage, so scan entries/s and ycsb-E are expected LEVEL with
#    `supdb-nocache`'s; mixes A, B, C, D, F read points through the form. About three hours. The lag and burst cells
#    merge, and a merge unlinks the form file of every input it retires on the segment-work thread (section 0): a
#    cell that trails there is read with that named before it is the form's. `zpage_raw_scans` is the scans that
#    began in a partition with a form file and no other.
# ---------------------------------------------------------------------------------------------------------------
env $UNSET "$BIN" ab \
    --arms supdb-nocache,supdb-nocachezpages \
    --size 10000000 --bound 1073741824 --ops-div 10 --reps 6 \
  > "$Z/zproof-ab-full.out" 2> "$Z/zproof-ab-full.err" \
  && grep -q ' quantities: ' "$Z/zproof-ab-full.out" && echo "all passes: the table is whole"
grep -E '^count zpage_(raw_scans|rawserves|fallbacks|reads|counts|build_orphaned|swept)' "$Z/zproof-ab-full.out"

# ---------------------------------------------------------------------------------------------------------------
# 4. The untimed fault record of both arms' read cell (7.2b, "Faults by region"). The same cell, once: the warmup pair
#    and one measured pair, the load and the read pass only, every major fault sampled with its instruction and its
#    data address on the clock the phase marks use. Both arms are recorded and the windows are cut by arm name from
#    the marks. Untimed: sampling every fault is not free, and nothing here is a rate. perf runs outside the cgroup;
#    the bench process moves itself into its own (bench::bound). About ten minutes.
# ---------------------------------------------------------------------------------------------------------------
env $UNSET SUPDB_LAG_MARK=1 "$PERF" record -k CLOCK_MONOTONIC -e major-faults -c 1 -d -m 32M \
    -o "$Z/zproof-faults.data" -- \
    "$BIN" ab --arms supdb-nocache,supdb-nocachezpages \
        --size 10000000 --bound 1073741824 --ops-div 10 --reps 1 --passes read \
    > "$Z/zproof-faults.out" 2> "$Z/zproof-faults.err" \
  && grep -q ' quantities: ' "$Z/zproof-faults.out" && echo "recorded"
grep -i -E 'lost|overload' "$Z/zproof-faults.err"          # expect nothing: no sample may be lost
grep -E '^phasemark supdb-nocache(zpages)? read ' "$Z/zproof-faults.err"

# The buckets, per window: the reading thread's faults by the file they landed in and by the code that took them.
# For the nocache arm: the hash slot (`FlatIndex::with_lookup`) and the record (`parse_record`, `record_key`), both on
# `.sup` files. For the forced arm the page of the form file is the only fault the read takes (the `.zpg` file); a fault on
# a `.sup` file in its read window is a raw touch (the stop rule). --files keeps the segments', the ordered indexes'
# and the form files' faults and drops the heap's and the binary's.
python3 -I "$TOOLS/faultsites.py" --data "$Z/zproof-faults.data" --bin "$BIN" \
    --marks "$Z/zproof-faults.err" --arm supdb-nocache --reads 1000000 --perf "$PERF" \
    --files '\.(sup|oidx|zpg)$' > "$Z/zproof-faults-nocache.txt" && echo "bucketed: nocache"
python3 -I "$TOOLS/faultsites.py" --data "$Z/zproof-faults.data" --bin "$BIN" \
    --marks "$Z/zproof-faults.err" --arm supdb-nocachezpages --reads 1000000 --perf "$PERF" \
    --files '\.(sup|oidx|zpg)$' > "$Z/zproof-faults-zpages.txt" && echo "bucketed: zpages"
cat "$Z/zproof-faults-nocache.txt" "$Z/zproof-faults-zpages.txt"
# Remove the recording once the tables are banked:   rm -f "$Z/zproof-faults.data"

# ---------------------------------------------------------------------------------------------------------------
# 5. The page cache's folios (counting, not timing; design 2.6 step 7). The kernel sizes a folio by the write that
#    makes it and the alignment of its offset, so the form file's write piece (`ZWRITE_PIECE`, 1 MiB, the segment's
#    own) is a number a pairing could price. Counted on a 400,000-key store with 8 MiB partitions by walking
#    /proc/self/pagemap and /proc/kpageflags over a mapping of each file (a scratch probe, the engine's piece size read
#    from the environment in the scratch copy only):
#      piece 1 MiB   a 19,188 KiB form file is 18 folios of 1 MiB and a ladder in the tail (128, 32, 16, 8, 4 and 1 pages);
#                    a 7,288 KiB one is 7 of 1 MiB and a ladder (16, 4, 2); the raw segments beside them are mostly
#                    128 KiB folios
#      piece 64 KiB  299 and 113 folios of 64 KiB, and a ladder of three or four
#      piece 4 KiB   every page a folio of its own (4,797 and 1,822)
#    and in all three, after the form file is evicted and 3,000 random pages are read through a MADV_RANDOM mapping, the
#    pages that come back are folios of one page (2,205 of 2,213 and 1,128 of 1,140 at 1 MiB; the rest of two to six
#    pages). So the composition changes during the read pass: reclaim works on 1 MiB units for as long as the built
#    file stays cached and on pages after, and the verdict reads faults a read as a property of the form, with this
#    named. What the piece's size costs in rate is not counted: it is the one line `ZWRITE_PIECE`, priced by a pairing
#    of the load window and the first read cells, and not run.
# ---------------------------------------------------------------------------------------------------------------

# ---------------------------------------------------------------------------------------------------------------
# 6. The default path's price (counting, not timing): instructions a point read and a count of a partition cost with
#    the form Off, the branch's price, as the engine at the base commit and at the branch's head. Callgrind over
#    20,000 uniform operations on a partitioned store of 100,000 keys, the function named exactly (CLAUDE.md
#    "Profiling": the toggle flips at every function the pattern matches), the probe's source identical for both
#    engines (the crate name aside), the release profile (lto, one codegen unit):
#      valgrind --tool=callgrind --collect-atstart=no --toggle-collect='costprobe::one_read' \
#          --callgrind-out-file=cg.out ./costprobe off read 100000 20000        (and one_count, count)
#    MEASURED at this branch's head (the engine's non-test code is the head's; the build is the probe's):
#                              base            head            delta
#      point read              970.44 Ir       972.34 Ir       +1.9
#      count                   608.03 Ir       611.03 Ir       +3.0
#    and `#[cold]` on part_read_z and part_count_z: 972.02 (+1.6) and 610.03 (+2.0), not adopted (section 0, P8).
#    The earlier report's +0.9 was its own probe's number; a probe with more code after its loop reads the same
#    engine a few instructions apart (LTO inlining). nm -C -S on the function that holds the inlined read:
#    5,232 bytes at the base commit and 5,287 at the first head of this branch, +55; part_read_z stands alone.
# ---------------------------------------------------------------------------------------------------------------

# ---------------------------------------------------------------------------------------------------------------
# 7. (Section 6's number is also the P8 line of section 0.) The forced path, for scale: 20,331 instructions a read on
#    the suite's value shape (60.9 records a page; 78% the LZ4 decode, 9.6% group_seek) and 45,686 on constant
#    100-byte values (256 records a page); the raw read of the same partition is 950-970.
# ---------------------------------------------------------------------------------------------------------------

# ---------------------------------------------------------------------------------------------------------------
# 8. The load's phases, for the mechanism and not for the verdict (7.6 item 3: "the sum of the phases last"). The forced
#    arm's `sync` makes the form files, so its load is the commits plus a closing sync that carries every build. Both arms
#    get the instrument, which is what an option set from the environment is for; three pairs of the load pass alone.
# ---------------------------------------------------------------------------------------------------------------
env $UNSET SUPDB_LOAD_PHASES=1 "$BIN" ab \
    --arms supdb-nocache,supdb-nocachezpages \
    --size 10000000 --bound 1073741824 --reps 3 --passes load \
  > "$Z/zproof-loadphases.out" 2> "$Z/zproof-loadphases.err" \
  && grep -q ' quantities: ' "$Z/zproof-loadphases.out" && echo "load phases: the table is whole"
grep -E '^loadphases' "$Z/zproof-loadphases.err"          # the commits' time, the first and slowest commit, the closing sync, by arm

# The build's drop of the raw records priced by taking it away: the forced arm against the same arm with
# `Options::zpages_drop_behind` off (`supdb-nocachezpagesnodrop`, a test asserts the two differ in that option alone).
# Both build the same form files, so the load's closing sync is the quantity, and the bytes the page cache holds after
# it. Both arms carry the phases instrument; three pairs, one sitting, and a second sitting with the arms swapped
# before it is read as more than a position effect.
env $UNSET SUPDB_LOAD_PHASES=1 "$BIN" ab \
    --arms supdb-nocachezpages,supdb-nocachezpagesnodrop \
    --size 10000000 --bound 1073741824 --reps 3 --passes load \
  > "$Z/zproof-nodrop.out" 2> "$Z/zproof-nodrop.err" \
  && grep -q ' quantities: ' "$Z/zproof-nodrop.out" && echo "drop vs no drop: the table is whole"
grep -E '^loadphases' "$Z/zproof-nodrop.err"
grep -E '^load (ops_per_s|device_bytes_per_byte)' "$Z/zproof-nodrop.out"
