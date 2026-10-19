#!/bin/sh
# Stage 1b, WRITTEN AND NOT RUN: the paired ten-million-key measurement of `supdb-nocache` against
# `supdb-nocacheordread`, and the one untimed fault record of the baseline's read cell.
#
# Read the pre-registration first (ordread-prereg.md, beside this file): the predictions, the verdict and the stop
# rule are written there, before any of this has run. Each block is a command to paste, not a script to run whole:
# the pairing is two sittings and the record is a third piece of work, and the machine must be free of every other
# timing run for each (bench/CLAUDE.md: never two timing benchmarks at once, this includes your own agents).
#
# Gates are `&&` and a verdict line, never `set -e` and never a pipe to `tail` (CLAUDE.md, "Running the checks").

# ---------------------------------------------------------------------------------------------------------------
# 0. Where things are.  $TREE is a checkout at the head of the branch wave1-ordread-fix (or a tree it was merged
#    into); $Z holds the logs and the two files of tooling that sit beside this one.
# ---------------------------------------------------------------------------------------------------------------
TREE="${TREE:-/home/user/supdb/.claude/worktrees/wf_94be36ac-6aa-9}"
Z=/tmp/claude-0/-home-user-supdb/43547e2e-b745-59c0-b3a0-b32f7c7e7919/scratchpad/wave1
PERF=/usr/lib/linux-tools-6.8.0-31/perf          # /usr/bin/perf is a wrapper that refuses this kernel (CLAUDE.md)
cd "$TREE/bench" || exit 2
# Every cargo command below builds into the machine's shared bench target, whose RocksDB is built: left to its
# default a tree builds into its own bench/target, and the first command here is a cold RocksDB build, twenty
# minutes and several gigabytes out of a disk that CLAUDE.md records dying at the linker with a Bus error. Set
# it to another directory only for a build nothing else uses: a build into the shared one replaces the binary
# that a timing run in it executes.
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/home/user/supdb/bench/target}"
export CARGO_TARGET_DIR
BIN="$CARGO_TARGET_DIR/release/bench"
# The stores a pairing writes go under the temp directory (supdb-ab-<pid>), and a run deletes what it made there
# when it ends; a directory of its own keeps another run's cleanup from taking them and theirs from taking ours.
mkdir -p /tmp/wave1-ordread-tmp
TMPDIR=/tmp/wave1-ordread-tmp
export TMPDIR

# The disk. A pass writes about 1.7 GB (the records, the hash, the heads and the directory, the log) and removes it
# when it ends; the fault record adds a recording of some hundred megabytes beside it; a link of the suite's
# binary is a third of a gigabyte. Six free gigabytes is the least to start with, because the failure at the
# allowance is the linker's, not a write's, and `df` still shows free space when it comes (CLAUDE.md, "Running
# the checks"). Read the figure on the same filesystem as $TMPDIR and $Z.
need_gb=6
free_kb=$(df -Pk "$TMPDIR" "$Z" | awk 'NR > 1 { if (min == "" || $4 < min) min = $4 } END { print min }')
if test "$free_kb" -ge $((need_gb * 1024 * 1024)); then
  echo "disk: $((free_kb / 1024 / 1024)) GB free, enough"
else
  echo "disk: $((free_kb / 1024 / 1024)) GB free, need $need_gb: stop and make room"
  exit 2
fi

# The suite's binary, release profile, built from the branch.
cargo build --release --bin bench && test -x "$BIN" && echo "bench built"
git -C "$TREE" rev-parse --short HEAD          # the engine commit under test: the first line of every log

# A pairing is a pairing only if the arms can be checked to differ in the option alone and the path to have been
# taken. Both are tests of the branch, under the checked profile (they build the lib test binary, not the suite's):
cargo test --profile checked --lib the_ordread_arm_differs 2>&1 | grep -E '^test |test result'
cargo test --profile checked --lib threaded_readers_read_the_loaded_store 2>&1 | grep -E '^test |test result'

# The machine is quiet, and the memory controller this needs is writable (what scripts/ooc.sh needs).
pgrep -a bench; pgrep -a perf; pgrep -a cargo; pgrep -a rustc          # expect nothing
sed -n 's/^[0-9]*:memory:\(.*\)$/\1/p' /proc/self/cgroup && ls -ld /sys/fs/cgroup/memory/cgroup.procs

# ---------------------------------------------------------------------------------------------------------------
# 1. The pairing: 10M keys, a 1 GiB cgroup, the read passes at --ops-div 10 (a million reads), twelve pairs after
#    the warmup pair, on one thread and on the class's two and three. Two sittings, the same command. `ab` swaps
#    which arm goes first every other rep itself.
#
#    Time: 26 passes (13 pairs), each the ordered load of ten million keys (about 7 s at the banked 1.4M ops/s, and
#    its drain), the million reads of the single-thread pass (19 s at the banked 54k/s) and the two- and three-thread
#    passes, which run that count on every thread (2M and 3M reads at the banked 100-147k/s: about 45 s together),
#    plus the store's open, its prime and its removal, whose cost at this size nothing banked prices. On the banked
#    rates that is 35-45 minutes a sitting; plan an hour, and read the `rep 0/12 (warmup)` line `ab` prints on
#    stderr when the first pair is done: its elapsed seconds are one pair, and thirteen times that is the sitting.
#
#    The instrument variables are unset for the pairing and are not the pairing's to set: the suite turns the scan
#    timers on for every arm under SUPDB_LAG_COUNTERS, and an option the environment sets is set for both arms.
# ---------------------------------------------------------------------------------------------------------------
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_LAG_MARK -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"

for sitting in 1 2; do
  env $UNSET "$BIN" ab \
      --arms supdb-nocache,supdb-nocacheordread \
      --size 10000000 --bound 1073741824 --ops-div 10 --reps 12 \
      --passes read,read-threaded --pairs 'read ' \
      > "$Z/ordread-ab-$sitting.out" 2> "$Z/ordread-ab-$sitting.err" \
    && grep -q ' quantities: ' "$Z/ordread-ab-$sitting.out" \
    && echo "sitting $sitting: the table is whole" || { echo "sitting $sitting: NOT whole"; break; }
done

# The pairing is read only if the path was taken, and only the baseline's column is zero (the gate in the
# pre-registration's VERDICT). The columns are A (supdb-nocache), B (supdb-nocacheordread), B/A, the pairs with B
# above A, the sign test:
#   count ord_point_reads             A 0   B 1000000   (the writer's handle: the single-thread read pass)
#   count ord_point_declined          A 0   B 0
#   count ord_point_reads_threads     A 0   B 5000000   (the handles of the two- and three-thread passes, summed
#                                                        as each is dropped: 2M + 3M reads on this class)
#   count ord_point_declined_threads  A 0   B 0
grep -E '^count ord_point_' "$Z/ordread-ab-1.out" "$Z/ordread-ab-2.out"

# The verdict's rows, the single-thread cell and the two- and three-thread cells (_2t, _3t):
grep -E '^read (majfaults_per_op|reads_per_s|read_bytes_per_op|refaults_per_op|p99_us)' \
    "$Z/ordread-ab-1.out" "$Z/ordread-ab-2.out"
# and every pair of a read quantity, rep by rep, in the --pairs block at the end of each file.

# ---------------------------------------------------------------------------------------------------------------
# 2. The untimed fault record of the baseline's read cell (7.2b, "Faults by region"). The same cell, once: the
#    warmup pair and one measured pair, the load and the read pass only, every major fault sampled with its
#    instruction and its data address on the clock the phase marks use. Both arms are recorded and the windows
#    are cut by arm name from the marks, so the ordered-index arm's faults come with the baseline's. Untimed:
#    sampling every fault is not free, and nothing here is a rate. perf runs outside the cgroup; the bench
#    process moves itself into its own (bench::bound). About ten minutes.
# ---------------------------------------------------------------------------------------------------------------
env $UNSET SUPDB_LAG_MARK=1 "$PERF" record -k CLOCK_MONOTONIC -e major-faults -c 1 -d -m 32M \
    -o "$Z/ordread-faults.data" -- \
    "$BIN" ab --arms supdb-nocache,supdb-nocacheordread \
        --size 10000000 --bound 1073741824 --ops-div 10 --reps 1 --passes read \
    > "$Z/ordread-faults.out" 2> "$Z/ordread-faults.err" \
  && grep -q ' quantities: ' "$Z/ordread-faults.out" && echo "recorded"

# No sample may be lost, or the rate below is short: perf says so on stderr when its buffer overflows.
grep -i -E 'lost|overload' "$Z/ordread-faults.err"          # expect nothing

# The windows: the baseline's read passes, warmup first (CLOCK_MONOTONIC seconds).
grep '^phasemark supdb-nocache read ' "$Z/ordread-faults.err"

# The buckets, per window: the reading thread's faults by the file they landed in and by the code that took them.
# A fault at the hash-slot read is a fault on the hash region, one at parse_record or record_key one on the
# record region, and through the ordered index the heads (OrdIndex::head) and the directory word (rd_u32 under
# key_at / with_record_at) have sites of their own: a read reaches a region from its own line of code, so the site
# is the region. --files keeps the segments' and the ordered indexes' faults and drops the heap's and the binary's.
python3 -I "$Z/ordread-tools/faultsites.py" --data "$Z/ordread-faults.data" --bin "$BIN" \
    --marks "$Z/ordread-faults.err" --arm supdb-nocache --reads 1000000 --perf "$PERF" --files '\.(sup|oidx)$' \
    > "$Z/ordread-faults-baseline.txt" && echo "bucketed: baseline"
python3 -I "$Z/ordread-tools/faultsites.py" --data "$Z/ordread-faults.data" --bin "$BIN" \
    --marks "$Z/ordread-faults.err" --arm supdb-nocacheordread --reads 1000000 --perf "$PERF" --files '\.(sup|oidx)$' \
    > "$Z/ordread-faults-ordread.txt" && echo "bucketed: ordered index"
cat "$Z/ordread-faults-baseline.txt" "$Z/ordread-faults-ordread.txt"

# The registered reading: of the baseline's faults a read, the hash-slot site 0.00-0.02 and the record site the
# rest (0.40-0.45); the hash site over 0.04 a read is the refutation (the hash is being evicted, M17's residual is
# real, and holding it resident is worth up to that). The standalone probe read the hash file 91% resident, under
# the 97-100% the registration expected. For the ordered-index arm the heads and the directory word are the new
# sites: the prediction is 0.00-0.03 between them and the record site the rest (0.30-0.36).
# Remove the recording once the tables are banked:   rm -f "$Z/ordread-faults.data"

# ---------------------------------------------------------------------------------------------------------------
# 3. The tooling was exercised once, at 300k keys with the files dropped from the cache by posix_fadvise, so that
#    major faults had the read sites to land on: $Z/ordread-tools/zz_scratch_faults.rs (a test, copied into
#    tests/ and not committed). The baseline path read 0.45 faults a read, 0.402 of them at the record site and
#    0.051 at the hash-slot site; the ordered path 0.416, 0.400 at the record site (the key check is the first
#    touch of the record, in record_key under key_at) and 0.014 at the directory word, none at the hash. To repeat:
#      cp $Z/ordread-tools/zz_scratch_faults.rs "$TREE/tests/" && cd "$TREE" && cargo test --profile checked --test zz_scratch_faults --no-run
#      SUPDB_LAG_MARK unused: the test prints its own phasemark lines
#      "$PERF" record -k CLOCK_MONOTONIC -e major-faults -c 1 -d -o "$Z/s.data" -- <the test binary cargo printed> --nocapture 2> "$Z/s.err"
#      python3 -I "$Z/ordread-tools/faultsites.py" --data "$Z/s.data" --bin <the test binary> --marks "$Z/s.err" --arm off --reads 20000
#    and rm "$TREE/tests/zz_scratch_faults.rs" afterwards.
# ---------------------------------------------------------------------------------------------------------------
