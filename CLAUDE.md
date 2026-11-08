# Working in this repository

Supdb is a read-optimized embedded key-multivalue store. This repository is
the engine, its reader, and under `bench/` the suite that measures it
against LMDB and RocksDB. `bench/DESIGN.md` says how a thing is measured and
`bench/CLAUDE.md` carries that side's rules; this file is about how a thing
is built.

A measurement the docs cite is given as the command that reproduces it
-- for a pairing, `bench/scripts/pair.sh A B SIZE [bench ab options]` at
the commit it ran at -- never as a log: the logs, stage notes and handoffs
of the sessions that built the compact form were kept under `scratch/` in
the history of PR #6, until the commit that took `scratch/` off it
(`cc9c21f`), and nothing on the main line needs them. The main line is
merged by squash, so a commit the docs cite lives in the history of the
pull request that brought it, which GitHub keeps under `refs/pull/<n>/head`
whatever happens to the branch: `git fetch origin
'+refs/pull/*/head:refs/remotes/pull/*'` brings every one of them.

This file is notes to whoever picks the work up next with no memory of it:
dense, contextual, and it explains a rule by naming the failure that produced
it. `README.md`, a PR description and the crate docs are for people who either
already have the context or do not want it, so keep those factual, current and
simple: no counts that move, no standing figures (cite the claim id and the
reader gets a checked number instead of a snapshot; a figure attached to a
*change* belongs in the PR that made it, rounded), no history, and no narrative
of the change that produced the text.

## Layout

| path | what |
|---|---|
| `src/db.rs` | the engine: WAL with atomic batches, memtable, sealed segments, partitioned compaction, tombstones, `Txn`, and the `SegmentWriter` every segment is written by -- `docs/engine.md` |
| `src/db/zpages.rs` | the compact form's layout: the page, the directory of the groups it holds, the group, the fill that cuts records into groups and packs them into pages, the fence, the trailer and the identity a form file is bound by, with no file, thread or clock in it -- the module's own docs, and `bench/examples/zsize.rs` for what it makes of the engine's bytes |
| `src/db/zfile.rs` | the compact form's file on the engine's side: the builder that writes a partition's form file from the partition's own bytes, the attach that binds it to its segment by content, the cell a segment holds it in, the point read, the count, the scan walk and the cursor through it, behind `Options::zpages`, and the controller's platform side, the cache budget read from the cgroup and the test's seam for it; the block cache's reads of a covered partition go through it too (`Options::zpages_block_cache`) -- the module's own docs |
| `src/db/zselect.rs` | the compact form's selector: `ZSelect::step`, a function of its state and an explicit `Input` (no clock, thread or file), the one definition of the transitions between FITS, SUSPECT, building, COMPACT and the trial, and the replays that hold it -- the module's own docs |
| `src/format.rs` | the on-disk format's fixed quantities, owned by no writer |
| `src/block.rs`, `src/index.rs`, `src/flatindex.rs` | the format itself: blocks, extents, the flat key index -- `docs/index-theory.md` |
| `src/bytes.rs`, `src/blob.rs` | the read path over any byte source; compiles for wasm |
| `src/wasmapi.rs` | the C ABI the browser calls; hand-written because the module's size is budgeted |
| `web/` | the browser reader, its byte sources, the Worker it runs in, and the size control -- `web/README.md` |
| `tests/` | the engine's contract, the read paths held to each other, the format's damage cases; `tests/common` holds what more than one file needs |
| `bench/` | the benchmark suite: its own cargo workspace, the arms, the runner, the gate and the figures -- `bench/DESIGN.md` |
| `experiments/ffi/` | what a C ABI and a Python binding cost a point read, borrowed against copied, beside LMDB's and RocksDB's C APIs; its own manifest and dated results, and not built by the gate -- its `README.md` |

One writer produces the format, `SegmentWriter`, with two callers -- `Db`
when it seals or compacts, and a caller with sorted write-once input -- and
three readers parse what it wrote: `Blob` over a mapped file, `Blob` over a
copying source, and `SparseBlob` over ranges. That is why `format.rs`
belongs to none of them.

No module carries a blanket lint allowance; `block` and `flatindex` allow
`dead_code` on the wasm target only, where their writer halves have no
caller. Nothing is exempt from the format gate, and everything holds to
`-D warnings`.

## Running the checks

`sh scripts/deps.sh` readies a host for every group, installing only
what is missing and ending in an `all deps ready` line to gate on; run it
first in a fresh container, which has no wasm target and libclang only as
versioned files. It names libclang through `bench/scripts/libclang.sh`,
the one definition CI uses, and keeps no variable. clang-sys's build
script declares nothing to rerun on, so the directory it found libclang
in at the first build stays in its cached output for good: a
`LIBCLANG_PATH` pointed by hand at a symlink in a scratch directory
built the comparators once, and with the directory gone the next build
failed at RocksDB's build script, `unable to find library -lclang`.
`deps.sh` cleans an output that names a directory with no libclang in
it.

`sh scripts/check.sh` runs every group -- build, test, lint, wasm, bench --
and CI calls the same script with the same names, so a green run here is a
green run there. Every group denies warnings, the wasm group since it was
found printing six dead-code warnings and reporting green: the module built
under cargo's default of warn, and nobody reads a build log that ends in
sizes. Four were `format`'s superblock constants, which `blob` kept copies
of from the time they lived in the write path, so on wasm the originals had
no reader; the copies are gone and the reader names the one definition. One
was a writer's phase timer that sat in `format` and belongs beside its
caller in `flatindex`, whose writer half carries the wasm allowance; one was
`Blob::id`, whose callers are the engine's caches, compiled where they are. Use a group name to run one. `quick` is a group too, the
suite's quick-scale measurement, twenty minutes or so with half of it
the threaded scans; it is not in the default set because a
timing run needs the machine to itself, and CI gives it a job of its own.

The tests leave their stores under the temp directory, one per test and
process (`supdb-next-<name>-<pid>`), so a day of test runs on one box is
tens of thousands of them and the disk they fill; `rm -rf
$TMPDIR/supdb-next-*` between runs. The disk here is a fixed allowance,
and the first thing to fail when it is spent is not a write with an
error but the linker: `ld terminated with signal 7 [Bus error]` on the
bench group's three RocksDB-sized links, with `df` still showing gigabytes
free. A scratch directory of old bench binaries at a third of a gigabyte
each is where the allowance went; delete those before reading the link
failure as anything else. The stores of pairings killed before their end
are the other sink: `bench ab` keeps its store under
`$TMPDIR/supdb-ab-<pid>` and removes it at its exit alone, so five
bounded pairings at ten million keys killed after their first sweep left
ten gigabytes, and the sixth died at its second load with `No space left
on device`. Remove them by name after a kill; a glob over `/tmp` takes
other processes' stores with them. On a box other runs share, give each
run a `TMPDIR` of its own and clean that: the glob takes every process's
stores, and a run whose store another run's cleanup removed mid-test
fails with a bare `NotFound` from whatever file operation came next --
eight tests of one run, one of them in six runs alone, before a syscall
trace showed the store's directory gone and nothing in the process that
removed it. A pairing's arm is the other way the allowance is found
spent: `bench ab` at ten million keys exited 1 after the comparator's
sweep with its own store left behind and nothing in its table or on its
stderr naming the cause, at zero bytes free; the three gates of the day
had left fourteen hundred test stores in the private temp directories
(1.3 GB), and the bench workspace's `target` held 14 GB, 3.6 of it the
checked profile's example binaries, which the gate's bench group rebuilds
and nothing reads. Read `df` of the temp volume before a timing run, and
delete the test stores and the checked examples first.

The test group's compile was thirty-three minutes of a gate's
thirty-seven, for four minutes of tests, and the cause was in the
tests. `scan` and `read_all` are generic over their callback, so every
closure a test handed one compiled its own copy of the engine's read
path: `tests/db.rs` held about a thousand copies each of `read_exts`
and `values_at`, a 328 MB binary whose optimization was the compile,
466 s of it even at level 1. This note spent a year on the symptom --
the compile's memory at 13.2 GiB against the tool cgroup's 13.4
(`memory.limit_in_bytes` of the `claude-code-bash` group), compiles
serialised one at a time, line tables and then thin LTO over sixteen
units to fit -- and every figure in it was that one crate's size. The
tests hand their callbacks over as one trait object each now
(`tests/common`'s `kv` and `vf`, one copy of the read path apiece; the
binary is 34 MB), and the `checked` profile is level 2 with no LTO, 256
codegen units and incremental builds: every test target builds from
nothing in about a minute and a half with four compiles at once under
two gigabytes, and after a one-line engine change in under one, and the
tests run within a fifth of their time at level 3. A test that hands a
closure straight to a generic read call adds a copy of the read path;
hand it through `kv` or `vf`. And a compile that is slow is measured
before it is accommodated: `cargo build --timings` names the unit, and
`nm -C` over its binary counts the copies. A compile killed for memory
is in `dmesg` (`Memory cgroup out of memory: Killed process ...
(rustc)`), and its peak is read by sampling the `rustc` processes'
resident sizes through it. The gate script that ran the stress on the
binaries left on disk from the tree before, and reported their verdict,
exits when the build failed now.
`cargo test` stops at the first failing test binary, so a red `db` hid
`dict`, `leak`, `ranges` and `segwriter` for a run; `--no-fail-fast` shows
the rest. The dev profile is for a test at a time and never the whole
`db` binary: its longest tests run over a minute under `checked` and
seven times that in dev, and the whole file in dev ran a held-seal test's
watchdog past its minute under the other tests' load, which ended the
binary with code 101 after a hundred and sixty-six tests, no failure
named and no summary, twice at the same count -- the watchdog's message
had gone to the harness's capture of the test's threads. The watchdogs
write to the process's own stderr now (`watchdog_exit`), and a test
binary that ends with 101 and no `test result` line is read as a
watchdog's exit before it is read as anything else. And worktrees that share a target directory overwrite each
other's test binaries, because the root package's artifact hash does not
depend on the checkout path: a run through the shared directory executed
another branch's seventy-seven-test library binary with none of its own
tests in it, and was read as green. A worktree gets a target of its own,
and a result is believed after `--list` has shown the tests it names. The
tooling cuts a worktree from `origin/main` and not from the parent's head,
so the first command in one is `git switch -C <branch> <base>`.

Keep it that way. Every gate this repository has broken has broken the same
way: a check that was not running, or one reporting a verdict it had not
earned. CI never built the wasm module at all, so a link break in
`src/wasmapi.rs` survived until a toolchain update happened to surface it
locally. `scripts/fmt.sh` once swallowed "rustfmt could not run" behind
`|| true` and reported green for never having run, which is why it now tells
"formatting differs" apart from "did not run" and fails both. The tests ran
under the release profile, with overflow checks and debug assertions off,
so a prefetch span that added a rank to a scan's limit of `usize::MAX`
wrapped, passed every check green, and panicked in the first debug build
that reached it; the tests now run under `profile.checked`, the release
profile with both on, and the wasm module and the suite's binaries keep the
release profile because the checks cost code size there and time in a
measurement. A second definition of "the checks" is how the next one of
those starts.

The same failure has a shell-script form, and this environment invites
it: `set -e` does nothing here -- a subshell walks straight past a
failed `grep -q` with status 0 -- so a chain of `checks; then commit;
then push` that relied on it committed and pushed a tree the format gate
had just rejected. Gate on `&&`, on the verdict line and not on a `|
tail` that swallows the exit status, and prove the gate fires on a
known-red input before trusting it with a push.

## Profiling

Two instruments, and they disagree where it matters. Callgrind
(`valgrind --tool=callgrind --collect-atstart=no --toggle-collect='supdb::db::Reader::scan'`)
counts instructions exactly and attributes them without sampling noise,
which is what a fixed cost of a few hundred nanoseconds needs. It is
blind to waiting: on the scan path `Blob::scan_at` is 9% of the
instructions and a third of the time. Count what the toggle collects and
not what the probe times -- a warmup loop inside the toggle doubled every
figure once. Name the function exactly: the toggle flips at every
function the pattern matches, so `*Reader*scan*` turned collection off
again inside `scan_blocks` and counted a scan without its walk, three
runs before anyone read the function list. And valgrind runs one thread
at a time: a state the upkeep thread must build while the probe sleeps
-- the copies after a burst -- is never reached under it, however long
the sleep, so the copy walk's instruction count cannot be had this way.

A hypothesis about where a thread's time goes is tested by a count
before a fix is timed. The upkeep thread's profile over a buffered
burst, cut to the window and the thread, put up to half of it in the
kernel zeroing pages; the largest per-pass allocation was kept across
passes and nothing moved. The lag probe counts each thread's minor
faults now (`flt`, from `/proc/self/task/*/stat`), and the same binary
run with glibc's trimming off (`GLIBC_TUNABLES=glibc.malloc.trim_threshold=...:glibc.malloc.mmap_threshold=...`)
says how many of them are the allocator's give-back: a process's
second burst drops from ten thousand to under four, its first stays at
fifteen, and the difference priced at a microsecond or two a fault was
a tenth of the thread, not the half. Count the faults and alternate the
tunables before pooling anything. And price a fault by the thread's
own kernel share, window by window, not by a count: the count was the
same twelve to fourteen thousand in every rep of the buffered
rewritten point while the thread's samples in `clear_page_erms` were
6-15% in the fast reps and 57% in the slow -- a first touch costs the
guest about 0.7 µs where it still backs the page and about 9 where its
balloon has given the page to the host, and which pages a burst gets
is the host's. The faults that were the engine's to remove were the
copies sized with room, each twice its form's bytes and fitting no
chunk the forms before it had freed (`Options::copy_exact`); the
tunables had said the give-back was not it, and the profile's kernel
share said what was.

For time, `perf record -e cpu-clock`. There are no hardware counters
here: this is a Firecracker guest, `/sys/bus/event_source/devices/` has
no `cpu` and the CPU flags have no `arch_perfmon`, so cycles, cache
misses and branch misses cannot be had at all, and a container cannot
add them -- a container shares this kernel. Software sampling needs no
PMU and works. There is no `linux-perf` package for this kernel;
`apt-get install linux-tools-6.8.0-31` puts a 6.8 binary at
`/usr/lib/linux-tools-6.8.0-31/perf`, which samples fine against a newer
kernel. Callgrind's `--cache-sim=yes` is a model of a cache, not this
machine's, and is worth only what a model is worth.

Memory placement is a hidden variable here. The same bytes in a fresh
anonymous copy read 20% faster than the file mapping of them in three
runs and 20% slower in the fourth, paired window by window with the
same keys, and the engine's scan over the store paired at the blob's
own speed. The guest's pages land somewhere different each run and how
the host backs them cannot be seen from inside, so a question of ten
percent about translation, huge pages or folio size is not one this
machine can answer; `docs/engine.md` has the probe and both outcomes.

A hand-off to another thread costs 15-200 µs here, spawned or parked
alike, and the drain's stamps say so (`SUPDB_DRAIN_STEPS=1`, a
`drainsteps` line per flush and a `finishsteps` line per segment finish:
the writer's hand-over, the seal thread's start, its finish's steps, its
fsync, the segment work's readable landing, the join, the manifest, the
answer). This file said a millisecond for a while, read off the pending
manifest's helper thread, whose fdatasync began 1.2 ms after the slot was
written: that was the device, an fdatasync queued behind the seal's fsync
in flight, not the wake -- a wait a thread's first syscall shows is the
syscall's until a stamp before it says otherwise. Stamp a chain at each
hand-off and each device wait before pricing either; `strace -f -tt`
shows the syscalls' timing and nothing of what runs between them. And a
phase on one thread that ends when a sync of the same file ends on
another is the sync's, not the thread's: the direct run's finish read
1.1 to 1.3 ms on a fresh thread beside the writer's fdatasync of the
records, which this file's own figure for a cold vCPU made plausible,
and moved to the warm writer it read 1.8, because the trailer's write
waits for the journal commit the fdatasync forces, on whichever thread
writes it (`Options::close_finish_inline`, refuted). Stamp the compose
apart from the write before pricing a finish beside a sync.

The page cache's folios are not hidden: `/proc/self/pagemap` over a
mapping of the file gives each page's frame, and `/proc/kpageflags` at
that frame says whether it heads a compound page or follows one, so a
walk of the mapping reads the file's folio composition -- forty of a
megabyte, or twenty-nine and nine ladders of 512 KB down to 4 KB over
the same bytes, which is what the stream's third shape left and the
reads trailed by. The kernel sizes a folio by the write that makes it
and the alignment of its offset, so a change to who writes a file is
read there when its bytes are the same and its reads are not.

The block device has a write-back cache, and an fsync's cost here is
the host's flush of it, not the page cache's writeback: a probe that
wrote forty megabytes with a `sync_file_range` hint after every piece
and waited for the writeback to finish still paid 24 ms at the fsync,
and nothing but an fsync issues that flush. The flush is priced by what
that cache holds: on fresh files with the pieces hinted as the writer
hints them, an fdatasync costs a third of a millisecond and then about
0.85 ms a megabyte, and 0.03 ms with nothing dirty, so a redundant sync
is free and a sync issued while the CPU has a millisecond of work ahead
of it is a millisecond of device time hidden (`Options::close_presync`,
the records synced by the waiting writer while the seal thread composes
the trailer); a write to the file under a concurrent fdatasync stalls
until the journal's commit is through, in some reps and not others, so
the overlap is read in the drain's stamps and not assumed. So price a
sync by who issues the fsync and when, not by how much is dirty, and
before reading a sync phase as the device's, time its steps -- the buffered
load's sync at three hundred thousand keys stayed level after the
fsync inside it fell from 25 ms to under one, because the phase was a
segment close whose checksum row re-read the section (`docs/engine.md`,
the streaming write). The suite's device-bytes quantity is
`/proc/self/io`'s `write_bytes`, which the kernel counts at dirtying
time per folio: a one-megabyte write makes a one-megabyte folio, and a
patch of a hundred bytes made in place after the folio was written
back counts a megabyte again while the device writes a page. Read the
device's own counter in `/sys/block/vda/stat` beside it before calling
such a move a cost, or a win: the streamed ordered load moved the
load's `write_bytes` by three and a half percent at ten thousand keys,
which the quick gate flagged a fortnight later as better than every row
of its class, and the device's counter around the same loads read 4.4 MB
a load against 7.6 before it, two fifths fewer, since the count sees
neither the journal nor a page written back twice. And an inode freed is a device round trip here: the
filesystem discards freed blocks, so a rename over an existing file, an
unlink or a truncate costs 1.2-1.4 ms for a file of one block and 20 ms
for one of forty megabytes, where writing and fsyncing a new file costs
0.3. The manifest publish renamed over the old manifest at every
landing and paid that on every drain's path, until the old manifest was
kept linked under a spare name for an idle tick to unlink
(`Options::manifest_spare`). Price a file operation by what it frees as
well as by what it writes, and read `strace -f -tt -T -y` over a phase
before reasoning about its syscalls: the drain's strace is what showed
the rename beside the fsyncs it was costing as much as.

A pass that runs beside the engine's other threads is not profiled by
sampling the process: four cores shared with a seal, a merge, the
helper and the upkeep put most of a pass window's samples in those
threads, and perf's own cost on every one of them read the durable
arm's rewritten pass at 62-110 ms where it runs in 11-25. Stamp the
window (`SUPDB_LAG_MARK`, `perf record -k CLOCK_MONOTONIC`), cut the
samples to it by thread, and give the probe the scanning thread's CPU
clock beside its wall clock: a pass slower than its CPU was descheduled
or waiting, one as slow as its CPU stalled, and the nine reps that read
13-66 ms split into three shapes on that one column before anything
was changed (`docs/engine.md`, the pass after the rewritten burst).
The same holds for a phase timed inside a thread: a trace stamped the
upkeep thread's conversions by wall clock at 15-35 µs a block, and a
counter on the thread's own clock that both arms report in one
process (`Db::convert_us`) read 11 -- the rest was the thread off its
core beside the writer and the seal. A per-block cost to be priced is
counted on the thread's clock and compared between arms, never read
off a wall-clock trace of a niced thread.

The row has counters of its own: `SUPDB_LAG_COUNTERS=1` makes `bench
run` print one `lagcounters` line per lag point on stderr -- the
scans' phases, the threads' time and pass count, the faults by
thread, the blocks built and the walks by form -- read before the
burst and after the pass and never between. It is how a pass's two
shapes are told apart in the suite's own shape: the slow reps of the
buffered rewritten point at three hundred thousand keys read 4-9
passes of the upkeep thread over the burst where the fast ones read
13-45, with the thread's CPU the same in both, after four probes had
each reproduced one shape and called the row wrong. Find the count
that separates the reps there before building a probe, and read a
verdict at p .04 on one pairing of `bench ab` as a position effect
until a second pairing agrees: the durable arm read a point 6% against
a change in ten pairs of twelve and 6% for it in the replication, and
the buffered arm's rewritten point at ten thousand keys 3% against one
at p .023 and 11% for it the second time. The load has the same
(`SUPDB_LOAD_PHASES=1`, a `loadphases` line per load on stderr): the
commits' time, the first and the slowest commit, and the closing sync,
which split the buffered arm's loss at ten thousand keys into commits
near the comparator's and a drain of 8-9 ms against one fdatasync of
2; a drain is then stamped step by step and read beside `strace -f -tt
-T -y` for the fsyncs and renames it makes, since a rename over an
existing file costs an fsync's worth here. A step that is CPU is priced
by the phases' sum and not by the phase it leaves: the segment finish's
checksum row read every piece of the key section back and hashed it,
0.3 ms of a 10k drain, and hashing the pieces on the writer as they were
buffered moved the whole of it into the commits -- the CRC costs the same
hot or cold, and only the preads were the finish's own -- until the
marker's pass over the same bytes was made to yield the row's pieces
too (`block::crc32_combine`), one pass for both and the finish's share
gone from the sum (`SegmentOptions::row_on_write`). And a step that
reads a section the next phase will read warms the cache for it: the
read-back made the lag pass at ten thousand keys, a hundred scans over
the section it had just pulled through, a fifth faster than without it,
forty microseconds against the three hundred the read-back cost, so a
cell that moves when a step is removed is read beside the phase the step
was in before it is read as the step's. It came again at a merge's open,
a third of the merge's wall under a bound for a second verification of
each input's key section: skipped, the merge took the same faults in its
value pass and no less wall, because the section holds the inline
records the pass reads, so the check was the merge's one sequential read
of its inputs (`Options::merge_verifies_inputs`, refuted off). A step's
faults are priced against the step after it that reads the same pages
before the step is called the cost.

The same counters have a memory half, for what the process holds and
not what a pass costs: `lagmem` (anonymous bytes, the allocator's bytes
in use, free and mapped, the writer's counted cache), `lagburst` (the
anonymous bytes where the burst ends, read from the kernel so it takes
nothing back from the upkeep) and, for an engine that answers
`Engine::memory_census`, `lagcensus` -- the writer's forms by kind with
their contents beside their buffers' capacity, the published and retired
forms, the snapshots, the memtables by owner and the segments' derived
caches, each structure once, the sum held against the allocator's bytes
in use, which is how an owner nobody counted shows -- then `lagarenas`
(the allocator's bytes by arena, which say whose frees the free bytes
are) and `lagmaps` (the anonymous mappings by size). `SUPDB_LAG_TRIM=1`
adds `lagtrim`, what a trim at that moment returns, and changes the
points after it, so it is a probe of its own. The census reads the
writer's tables and takes its upkeep home, so it stands after the pass
and never between a burst and its scans. `lagburst` does stand between
them, a read of a `/proc` file, and `SUPDB_LAG_ARENAS=1` adds
`lagburstarenas` there, the arenas as they are at the burst's end:
`malloc_info` walks every free chunk of every arena under the arena's
lock, milliseconds with tens of thousands of chunks and the upkeep
thread's allocations among the waits, so it is asked for and not given,
and a run that counts is a run whose rates are not read.

A copy taken off the writer's path is priced at every reader of the
copy and in the phase that inherits its pages. The streamed table's
key copies were twelve hundred first touches a load at three hundred
thousand keys, and taking them away moved the commits 3.5 ms; the
close's one pass over the keys then streamed the forty-megabyte record
section where it had streamed five megabytes of copies, 2.5 ms in every
pair of both sittings and nothing at ten thousand keys, and the first
update mix after the load first-touched a third of the pages the load
no longer did, 670 faults in its thread against 60, a cell down 3-5%
at p .04 twice while the read-only mixes sat level. The fault record by
symbol named the copy, the drain's finish steps named the pass, and
the mix counters named the inheriting mix; a rate over any one of the
three would have named none of them.

Profile a probe that does one thing. The scan probe's own `format!` per
iteration was 8% of its samples until the keys were built before the
loop.

A per-key cost written down from a guess is not one until a count or a
probe has it, and an estimate that stands in a note for a month is
believed as a measurement. The finish's trailer carried "the FNV a
third, the slots' random stores the rest" from the stamps that first
split it; callgrind over the finish counted the FNV at sixty-five
instructions a key, a micro-probe timed it at 4 ns, and perf's
annotation put four fifths of the fill loop's samples on the branch
after the slot probe's load -- a load, not the store, missing the core's
cache -- so the lever was a prefetch sixteen keys ahead, priced in a
micro-probe of the loop's own shape before it was built, and neither of
the two things the note named. Separate the pages from the per-key work
before either is priced: a phase's time fitted against its first-touch
faults across a sitting's drains gives both at once (394 µs and 2.2 µs a
page for the trailer at ten thousand keys), where a mean over the drains
gives one number that is neither. And read a pairing's stderr by the
arm each line names, never by the lines' parity: the arms' order swaps
every other rep, and a parity split reads positions. Attribute the
faults themselves before pricing them: `perf record -e minor-faults -c 1`
over a pairing records every fault with the instruction that took it,
and by symbol it put the writer's thirteen thousand a load in the record
stream's copies, the slab's entries and the keys' copies, the
comparator's four hundred in a pool it reuses, and seven thousand in the
scan snapshot's radix histograms. A page a load no longer touches is
touched by whatever runs next in the same store if it needs the memory:
the mixes after a load had been finding the stream's pages warm in the
heap, so the ring's saving in the load shows as faults in the first
update mix, and only the sum of the phases says which way it went.

A fresh page's price here is the host's, and it is what moves the
write cells between sittings. The durable ordered load at three hundred
thousand keys ran in two shapes with the writer's CPU at 2.5-3x in one
and the fault count the same in both; the per-window profile put the
excess in `clear_page_erms`, the copy into the page cache and the
allocator's list walks -- 0.14 µs a page zeroed where the guest backs
it, 6-8 where the balloon has reported the block -- and every slow load
followed the comparator's rep, whose dropped store had freed the blocks.
The suite makes the pages fresh by design (`release_freed_memory`, the
allocator trimmed after every store, since the ten-million rung held
13 GB otherwise), so glibc's tunables move nothing under it, and the
count of pages a load first-touches is the engine's lever
(`docs/engine.md`, the pages a load touches). Count the pages per key
before the instructions per key: the writer's two key copies cost 64
bytes a key of fresh memory and a few nanoseconds of CPU, and the pages
were the loss. And a derivation moved to the finish is one pass -- the
first version of the derived close read the entries five times and the
keys three, and gave back at the drain what it had saved at the commits.

A lever on fresh pages is priced on the smallest write set as well as
the load. A fresh anonymous page costs this guest about a microsecond
by fault and half that populated in bulk (`MADV_POPULATE_WRITE`), and
populating the writer's buffers a window ahead of the fill moved the
buffered load's commits 8% at ten thousand keys in every load and the
load nothing by rate in forty-eight pairs; the lag sweep's one-percent
burst, a hundred writes over four pages, paid a fixed window of
sixty-four pages at its first write and read 0.66x, and a window that
grows with the fill left it within noise and the load still unmoved
(`Options::prefault`, off; `supdb-populate` is the arm). Read the
phase column that names the mechanism beside the rate, and when the
phase moves and the rate does not, look for where the saving is being
spent in the same load; where the code changed is one phase's alone and
no other cell can have moved, two sittings of the phase column are the
verdict and the rate is noted as unresolved -- the record path
(`Options::direct_put_one`) took the commits a tenth lower in both
sittings while the rate read 1.12x and then 0.98x.

An instrument in the measured path is priced by taking it away in the
suite, and the pairing that prices it has to differ in the instrument.
The scan path timed its six phases before the walk on every scan, ten
clock reads, and a probe of the clock in a tight loop read 50 ns a pair;
the first pairing of the gated timers against the old shape read level
everywhere and was called a refutation -- but the pairing script
exported `SUPDB_LAG_COUNTERS` for its counters lines, and the suite turns
the timers on for every arm under that variable, so both arms had them
on. Callgrind over ycsb-E's interval put the clock paths at 17% of its
instructions, and the pairing with a clean environment read ycsb-E 1.30x
and every scan pass 1.16-1.30x at ten thousand keys (`Options::scan_timers`,
off by default; `supdb-ingesttimers` is the arm that times). The rule:
an option the suite sets from the environment is set for both arms of a
pairing, so the arms are checked to differ in the option before a verdict
is read, and a measurement that contradicts an exact count is a reason to
check the measurement before the count. The same interval said something
the counters could not: our E executed 24% fewer instructions than LMDB's
and took 40% longer, and the clean scan workload 38% fewer at level time,
so what a scan of ours pays over LMDB's is stalls per instruction, and the
remaining levers are in the walk's memory access, not its arithmetic. The
same variable came back the other way: the pairings of supdb arms set
`SUPDB_MIX_COUNTERS` for their counters lines, which turns the timers on
for every supdb arm and for no comparator, so a cell read against LMDB
from such a pairing carries ten clock reads a scan on one side -- E at five
million keys under 640 MiB read 0.69-0.75x LMDB with them and 0.76-0.86x
without; a cell against a comparator is read from a pairing with the
counters off.

Price a structure by taking it away, not by what its doc says it saves.
The sparse form's doc says it saves the walk the memtable's value
reads, and the snapshot carries those values in key order anyway, so
the form read as a cache the upkeep thread paid a patch a write to keep
for nothing; a scratch build that walked every written block without
one halved the thread's cost and ran the pass after the rewritten burst
three times longer, because the form's real saving was the fold of the
level-0 pieces under the block, read from their records at every walk
otherwise (`docs/engine.md`, the dense-burst regime). Measure the
removal in the suite's shape, pieces and all, before building the
replacement. The seal's snapshot copy on a drain went the other way:
read as fresh pages paid for readers a drain never has, it was taken
away and the records phase grew by exactly what the copy had cost,
since the copy was the cheaper of two walks over the table
(`Options::drain_snapshot`, the arm `supdb-ingestnodrainsnap`); the
pages were never the price, and only the removal said so.

Give the probe the suite's shape before believing it disagrees with the
suite. Four probes in one day failed to reproduce a figure from the lag
sweep and each was read as a refutation before it was read as a bad
model: one amortised the block builds over a hundred thousand scans
where the sweep does `size / scan_len` of them, one buffered its writes
differently from the mix driver, and all of them used forty-byte values
against the suite's hundred (`VALUE_SIZE`) and sixteen-byte keys. A
fifth skipped the `sync` the suite's load ends in, so its store at three
hundred thousand keys stayed two pieces through the whole sweep, where
the suite's second flush had joined the partitioning, and read every
point at a third of the suite's rate. A sixth read the store's counters
between a burst and its scans, and a counter read takes the writer's
upkeep home, waiting out the thread's pass in flight before the clock
started: it read the lazy freeze's pass level where the suite read it
at four tenths, and the wait it hid was half of the suite's first scan.
Read nothing between the phases the suite times that the suite does not
read. A seventh ran the rewritten lag point alone (`LAG_PCTS=100`) and
read sparse forms and a pass twice as long where the ladder reads
copies: the thread converts a burst's forms only once a scan has
happened since its last pass, and the point before it is where those
scans come from, so a lag point measured without the ladder is a point
of a different store. The
suite's own arms answer most of these questions without a probe at all,
and `bench ab` prices two of them in one process; reach for a probe only
when no pair of arms isolates what you are asking.

A decoder timed on one input is timed on an input it has learned. The
form's LZ4 decode read 1.1 µs a page decoding one page over and over
and 3.4 cycling the file's pages, and the cache level was not it:
cycling 1, 4, 16, 64 and 256 pages of a file that fits L2 read 0.7,
1.0, 2.0, 4.5 and 3.5 µs a page, the plateau the whole 20 MB file reads
at. A decoder is a branch or two a token and the predictor learns a
page's tokens inside a few repetitions, so the hot figure is nobody's
and the cycling figure is a read's; liblz4 against `lz4_flex` read
0.76-0.83 hot and 0.83-0.91 cycling. Time a decoder, a parser or any
branchy walk over more distinct inputs than the predictor holds, a few
hundred pages here (`bench/examples/zdecode.rs`, `--pages`), and read
the figure where it has stopped climbing. And read a cold figure as the
predictor's before reading it as the memory's: the E mix's decodes ran at
exactly that cycling figure, a microsecond a group against 0.27 hot, and
the obvious lever -- the group's lines asked for ahead, at the view, at
the fence, the whole page -- bought a tenth of a decode at most and made
the whole page slower (`bench/examples/zlines.rs`), where fewer tokens
to parse did (`zdecode --hc`: liblz4's high-compression mode, 16% off
the cold decode and 4% off the form at level 9). A software prefetch is
a lever on a stall the profile shows as a load; a cold decode's stall is
its branches. And a decode removed is priced by what it cost: raw tails on
the partition that held half the E mix's decodes took them away and moved
E by two percent, since that partition's groups came back warm, and the
next three partitions' third as many decodes moved it by ten -- a heat
counted in decodes ranks the warm partition first and the profitable ones
after it. And a self time is read as misses only after the
structure's size is read against the cache per instance, not per store:
the fence's search was a twentieth of E's thread and was taken for a
binary search over 93,000 entries missing on two lines a probe, so a tree
over the entries' words was built, one line a level and prefetched three
ahead (`Options::zpages_fence_tree`); profiled arm against arm the search
took more of the thread with the tree than without, because the store's
fences are nineteen of 4,900 entries, a hundred kilobytes each and in L2
for the partitions a Zipfian reads, and a search that was instructions
and L2 hits had no misses for a tree to save. A lever on misses is held to
the profile of the arm pair, where the symbol it is aimed at loses its
samples or does not; a pairing resolves nothing under a tenth and an
instruction count sees no miss. It came again in the settle's resolve
under the pace: the gallop over the heads for a batch's keys two hundred
ranks apart put nine tenths of its samples on head loads, so a seek that
paid one miss into a prefetched stride was built in its place, and kept
the same tenth of the thread in both arms' profiles -- at 200 ns a key
over a dozen probes the loads were L2 and L3 hits, which the self time
divided by the loads had said before anything was built. Price a load by
the latency the profile's self time over the loads implies, against a
miss's, before building a lever on the miss.

A pass that reads bimodal across rounds is two shapes, not noise. The
run keeper's lag pass at ten thousand keys read 2 µs a scan in four
rounds and 15-36 in three, and the rounds split on the probe's own
counters -- blocks built, the forms' bytes -- before they split on
time: the seal the burst triggers at that rung landed at its last
commit in the slow rounds, the writer's tables went with the publish,
and the pass built every block it met. Find the count that separates
the rounds before averaging them, and read the slow shape as a
measurement of its own.

A window of a mix is several kinds of operation, and a rate over it
names none of them. The mixes' windows around a seal read at four to
ten times their neighbours, and two probes read that as the reads
slowed beside a seal in flight -- the shape a known problem has --
until a third split each window into its reads and its commits: the
reads were flat to the nanosecond and one commit in the window was
the whole excess, the freeze's, at five milliseconds. Time the kinds
apart before naming the one that is slow, and when the excess is one
operation, decompose that operation next.

A background thread beside the writer is priced by what it files, and
what it costs the writer is found by taking its work apart, not by
sampling the writer: the alert upkeep thread cost the mixes that
write a tenth to a fifth at ten thousand keys, and a perf cut of the
writer's samples by symbol moved by thirty where the tax was a
hundred. A pass that pinned, parked and left cost A four percent and
the reads nothing; one that filed and built nothing cost the whole
tax; the full pass cost the same -- so the filing's re-reads of the
entries the writer is rewriting are the cost, each an ownership the
next update pays for, and three cache-line separations guessed at
before the bisect moved nothing and were taken out. The mixes have a
timer by kind (`SUPDB_MIX_KINDS=1`, a `mixkinds` line per mix on
stderr for every arm, its reads, scans and commits apart), which read
the E gap as the scans' alone and the tax as the commits' and the
gets'; `SUPDB_MIX_EVENTS=1` prints each mix's form events, upkeep
passes and snapshot builds beside its counters; `SUPDB_MIX_CENSUS=1`
prints the lag pass's census of what the process holds, by owner, after
each mix (`mixcensus`), which is where a form file's fit beside the
process is decided -- a trim before the mixes read the process 15 MB
lower in use and level in anonymous bytes once the mixes had taken the
return back, so a lever on the process's memory is read in the phase
that competes for it, never at the lever; and a trim counted by the
allocator's free bytes (`trim_due`) never fires on pages that come back
resident under chunks already free, which is what a thread re-using a
dead arena does, so a look that found nothing to trim nine hundred times
is read against the arenas line (`SUPDB_MIX_CENSUS=2`) before the
mechanism is read as having nothing to return; and a count that does
fire there -- the resident bytes beyond the allocator's use
(`Options::trim_by_resident`), four to nine trims through the mixes --
lowered neither the process's anonymous bytes in the mixes nor the
faults, since what a trim returns the mixes touch next, so a trim is
priced in the phase's census and never by its own count. And a pairing of two
`Auto` arms is read per sitting with each arm's builds and installs
(`zpage_builds`, `zpage_sel_installs`, the `zsample` trace line's pick)
beside its rates, never as the table over both orders: the ski rental's
buy moves with the order -- at five million keys under 736 MiB the default
built nothing in one sitting's median rep and its block form in the
other's, and the gear's pick at 720 MiB flipped with which arm ran first,
since the budget subtracts the process's anonymous memory at the sample's
moment -- so the table's gmean mixes arms that bought different things at
different times, and the coding alone is read in a forced pairing where
both forms stand from the start. The lag sweep under a bound carries the
same buy: the controller bought its form during the ten-percent pass in
one arm and during the full rewrite's in the other, and that pass read
12.8 s against 17.4 between arms that differed in their merges alone, so
an option that is not the controller's is priced there with the form off
in both arms (`supdb-nozpages` and a sibling of it). `SUPDB_SCAN_OPS=1` times
every scan of the single-threaded scan pass on its own, for every arm,
and prints a `scanops` line a pass with the first scans beside the
median: a pass of a hundred scans at the smallest rung is a few hundred
microseconds, and a first scan that builds what the pass needs once is a
share of it no rate shows. Its median prints to the nanosecond: printed
to the tenth of a microsecond it hid a fifty-nanosecond move behind four
sittings of pairs, and the move was read off the pass sums instead. Under
the same variable the lag sweep prints a `lagops` line per point -- the
first four scans, the median and the longest, the pass in tenths, and
the thread's CPU and context switches beside its wall -- and
`SUPDB_LAG_COUNTERS=1` a `lagdev` line beside `lagcounters`, the pass's
device and group side: bytes read and written at the device and by the
process, and the group's faults, refaults, dirty pages and cache. The
two told the full-rewrite pass at ten million keys under a bound apart
from its rate -- a first scan of nineteen seconds and one tenth of
seventeen in a pass of fifty, half the device's eight gigabytes under
them -- where the row's own quantities for the point, bytes read by
background threads and major faults per operation, read 371 KB and
zero: the first counts the merge's reads as the pass's, and the second
is blind to a fault on a page the cache holds and has not read yet,
which the kernel counts as minor. Read a thread's
passes per rep there before reading its cell: a thread that ran in
two reps of five made a cell that read neither shape.

## The suite lives in bench/, and it gates this repository

`bench/` is a time series. `bench run` measures every arm -- supdb's
shipping configurations and the comparator a user would otherwise pick,
durable against durable and buffered against buffered -- over a ladder of
store sizes, and writes one row of raw per-rep samples under
`bench/runs/<scale>/`. Nothing in a row is derived. The gate compares a new
row to the last ten rows of its machine class and fails when a quantity's
error bars lie entirely on the worse side of every one of them; a row
entirely on the *better* side is flagged rather than passed, because a
measurement that is implausibly good is a broken measurement until someone
looks. There are no claims and no expected states. The suite that had them
-- 183 claims adjudicated by `verify` -- was retired when its gate went red
on an engine head that had not changed; it is in the supdb-bench
repository's history. Nothing here cites one of its claims by id: an id
whose checker is gone is a pointer to nothing, and it comes back as a
number.

The ladder's small rungs are not a formality. A fresh memtable's first
write zeroed eleven megabytes of blocks it would never read, 3.7 ms that
five interleaved rounds of a probe at 300k keys could not see and the
quick row's ycsb-B at ten thousand keys, a pass under a millisecond,
showed as 3x. The scan snapshot's radix sort zeroed two histograms of
half a megabyte on every build, over a memtable that after the load
held nothing, and the faults on those pages were 400 us of the first
scan at every rung: the row's scan pass at ten thousand keys, a hundred
scans, sat at 0.64x LMDB's while a thousand scans through the probe
ran 1.4x it. Take the quick row before a change is called flat, and
read its smallest rung, and when a small pass trails, time its first
operation on its own.

Two consequences for code in this repository:

- **The comparison arms in `Options` are not dead code.** `cursor_merge`,
  `scan_merge`, `scan_snapshot_arena`, `flush_ranges`, `compact` and their
  kin each keep an older shape alive behind a flag because the suite prices
  the new shape against it in one process -- comparing two separate runs
  does not work, the unchanged comparators move by tens of percent between
  them. Removing an arm removes the experiment; check `bench/src/engines.rs`
  first.
- **No standing figure is written down here.** A number belongs in a row and
  a figure is drawn from the rows by `bench figures`. A figure attached to a
  *change* belongs in the pull request that made it, rounded.
- **A cell a comparator wins is a workload the engine has no strategy for.**
  The file may carry more than one form of a structure and the reader choose
  the one the workload calls for, because the cache is charged for the pages
  a path touches and not for the file's size: a form the chosen path never
  reads costs disk and the time to write it. Out of core that is the lever.
  A point read of inline values touches the hash slot and the record; the
  slots are touched by every read, so they stay resident and crowd the
  records out, and the records are the bytes a read misses on. Both have a
  compact form that the raw one does not preclude. Two rules keep it honest.
  A second form that accompanies the records may only be one the other paths
  never touch -- `src/ordindex.rs` records an over-store that scans touched
  as well, which enlarged the working set it was meant to beat. And the
  choice is made from what the store measures of the workload, the share of
  its lookups that blocked and the limit it runs under, the form is built
  only when that measure calls for it so the in-memory rows cannot move, and
  the choice is taken back when the measure turns. The first version of this
  note argued the other way, that past the memory line trading space for
  speed was no longer free, because it priced the file by its size.

## Invariants a change must not break

**The read path is synchronous, and that is the constraint rather than an
accident.** `flatindex::lookup` returns a borrow into the index section, and a
borrow cannot survive an `await`, so `Bytes` is synchronous and the `await`
lives in JavaScript: the browser downloads the object into OPFS once and
every read after that is `FileSystemSyncAccessHandle.read`, or it asks the
module for the ranges a read will touch (`Blob::ranges_for`, `open_ranges`,
`SparseBlob::dictionary_plan`), fetches them, and then the read runs
synchronously and cannot miss. An `async fn` anywhere under `blob`,
`flatindex` or `bytes` turns the API inside out and buys an Asyncify rewrite
against a module size that is budgeted. Format knowledge stays in Rust for the
same reason a plan is computed there: a superblock constant hand-copied into
the JS side has drifted once already.

**Two publishers, one pointer, and nothing a reader follows moves.** A
`Db` is the writer and one thread's; its segment work (`Maint`) -- a
seal's landing, the merges, the promotions, the manifest and the WAL
retirement -- runs on a thread of its own (`publish_in_background`), or
inline where the writer drives it; a `Reader` from `Db::reader` is
another thread's, `Send` and not `Sync`, with the scan snapshot and the
block tables of its own. What they share is published whole: the segment
set and the memtables, the live one and the frozen list, as one `State`
behind one pointer, swapped by a
compare-and-swap against the state it was made from and made again over
whichever won. The writer changes only the memtables (a freeze, a switch
of memtable) and the segment work only the segments -- with one exception
the writer consents to at the hand-off: the landing that retires a table
the writer handed to a seal without freezing it installs an empty live
table in its place, and the writer, which writes nothing into a handed
table, takes whichever table it finds live at its next write -- so a
retry takes the other's half afresh and nobody holds anybody off. A
memtable's arenas, entries and index never move once published -- with
one exception the epoch licenses: a direct run's record stream runs in
a ring (`Options::direct_recycle`), and a block of it the file holds
whole is taken out under a fresh epoch, its pointer nulled, and reused
for a later block only once no reader is pinned at or before that epoch;
a reader that finds the block gone reads the record through the run's
mapping, and one that loaded the pointer first is pinned below the
epoch and keeps the memory until it leaves. Everyone
who reads a state
pins the epoch it reads in through a slot in the reader table -- a handle
for each read, the writer for each of its operations, the segment work
and the upkeep thread for each of theirs -- and whoever replaces a state
frees it only past every pinned slot; nobody waits for a reader. The
pin fences, a sequentially consistent store on every read;
`Options::asym_pins` is the arm where the sweep fences for it instead,
once a publish through `membarrier`, priced on a point read and not
resolved (`docs/engine.md`). A read
takes the state once, at its start, and holds it: a read that loaded the
state twice took an entry from one memtable to the chains of another when
a freeze landed between, and three reader threads found it in their
first minute. An operation that publishes moves what it holds to what it
published, and the writer's operations hold one state each: what the
segment work publishes meanwhile, the writer sees at its next. A table
the writer hands to a seal without freezing it -- `sync` under
`adaptive_shape` hands the live table whenever it holds anything, no
table is handed already and the segment work has a thread of its own, a
seal in flight or not, since seals in flight may fill the frozen list --
is replaced by whichever side publishes first: the
writer at its next write, freezing it under a fresh table once the frozen
list has room, or the landing, installing an empty one, each by
compare-and-swap on the same pointer; and the writer writes nothing into
it after the hand-off. The
isolation is the reader's: `Latest` honours the memtable's watermark at the
last commit, `Snapshot` pins a state and a watermark, `Dirty` honours none,
which is what the writer's own reads do. A `#[cfg(test)]` module asks the
compiler that `State` is `Send + Sync` and `Reader` is `Send`; that is what
keeps a cell out of a segment, a blob or a memtable, since a raw pointer
behind an atomic would let one in unasked.

**The writer's upkeep is lent, never shared.** By default
(`Upkeep::Background(1)`) a commit lends the writer's `FormsState` --
its block tables and scan snapshot -- to a thread of the store's own and
returns without waiting for it; at level 2 (`supdb-hold`) a commit that
would leave its batch to the next scan returns only once the thread has
filed it. A commit with no fsync to work beside files a burst's first
batch after a scan itself (`Options::upkeep_inline_bursts`): the
thread's window is the barrier, and a buffered burst of one commit left
it none, so the burst's whole filing fell to the first scan after it --
the buffered lag cells read 0.2-0.6x of LMDB with the scans themselves
at the drained store's rate. The batches after that one, each at most
a hundredth of the store's keys, it lends paced
(`Options::upkeep_pace_pct`, `Hand::Pace`): the commit returns once the
thread trails by at most that hundredth, so the thread's core files
behind the writer's commits, the writer waits only where the thread has
fallen that far behind, and the first scan after the burst meets at most
the share -- filing every batch inline had put the whole fill on the
writer, a third to half of a buffered burst from a hundred thousand keys
up, and lending every batch had left the scans after a burst 1.8-3.5x
slower. The share is the first scan's wait for the pass in flight
against the pass over the store: at ten thousand keys a batch of a
thousand is a tenth of the store and its fill several passes, and paced
there the burst gained a third while its scans after lost two thirds.
It is the pipeline's depth too: paced a batch deep, every batch was a
pass of the thread's, whose fixed part doubled the thread's cost a batch
at three million keys, and the sweep's weighted sum read a third worse
than lending at a million; and a thread gathering half the share before
a pass, for longer passes while the writer ran ahead, read level, since
the trailing thread took several batches a pass already and the share
bounds the trail that lending's long passes amortise, so a lever on a
pass's length is read against the passes counted before it is built.
`supdb-ingestlendbursts` is the arm that lends every batch and
`supdb-ingestnopace` the one that files every batch. A syncing commit
lends every batch, since its thread files through the fsync, and in core
it is paced too, within a quarter of a percent of the store's keys
(`Options::upkeep_pace_syncs_bp`; `supdb-nopacesyncs` lends unpaced):
unbounded, the thread's trail at the end of a full rewrite was the first
scan's to wait out or settle, the pass's largest part. Out of core it is
not paced, since there the thread's resolves are major faults and the
hold paced the writer by the device.
One thread holds it at a time, and one word says which:
every hand-over is a single operation on that word, the side giving it
up fills the cell before it and the side taking it empties the cell
after, so nothing in it needs to be `Sync`. A writer's touch of it takes
it back, waiting only for a pass in flight, and never on a lock: the
thread is niced, and a lock the writer took at every commit was one the
thread could be preempted holding. The thread pins a slot for each pass,
so a publish from the segment work needs nothing of it: the forms go
across as copies the publish makes (`State::carry_published`) and the
writer's own tables follow at its next look at the log
(`Reader::rebase_tables`). The writer's other publishes take it back
first; the freeze does not, swapping the tables and touching nothing
else (`Options::freeze_settles` is the freeze that did, kept for
pricing), and the next look carries the
tables across it as it does a landing, the frozen table's unsettled
writes kept to settle through that table and every table live since the
look kept for it in `State::replaced`. The forms a pass publishes carry the commit the writer named,
never the latest, or a handle at the latest takes forms without the
writes between; and `settle` joins it before anything else, because
every other join touches it. The thread keeps its own timer: for a
hundred milliseconds after its last pass or hand-over it parks for at
most `Options::upkeep_alert_poll_us` at a time while reads are around
(`Options::upkeep_alert`; `supdb-noalert` is the back-off as it was),
since a mix's batch of a hundred is lent and too small to wake it for,
and the back-off it had climbed across the read passes slept through
the mixes; the writer wakes it once, where reads come around or it
finds the thread parked past its poll, because a cap a sleeper applies
when it chooses its park cannot shorten the park it is already in. The
threaded test runs at each level, since a level that holds hides what
a level that lends exposes.

**`Blob::zero_copy()` stays true on the native path.** `Bytes` has two halves
for one reason: `read_at` copies and every source can answer it; `slice_at`
lends and only a source backed by memory can. Native takes the second for
every access and copies nothing, which is the axis `flatindex` exists to win
and the one a byte-source abstraction most easily loses. `tests/blob.rs`
pins it, because a native reader that started copying would still pass every
correctness check.

**The three readers agree, and are tested against each other rather than
against themselves.** The failure mode of a second read path is not a crash
but a browser quietly answering a different question from the server.
`tests/blob.rs` requires a lending source and a copying one to agree on
every key, value and count, and `tests/dict.rs` holds `SparseBlob`'s ranged
dictionary walk to the whole reader's `scan_counts`. Those checks have caught
real differences: a reader reporting the superblock's generation where
another reported the index section's, and a `value_bytes` that counted the
varint length prefixes it claimed to exclude.

**The magic moves when a reader from before the change would misread rather
than error.** The question is never "did the format change" but "what does an
old reader do with the new file". The per-extent count word and the `FIXED`
flag each re-decode a run under the wrong encoding in an old reader, so the
magic moved for both, and so did the compact record, whose flag bit an old
reader takes for 32,768 extents and may find bytes enough to read as them.
The inline extension did not move it, because a reader
from before it errors on `Ext::INLINE` as an impossible block id; nor did the
key-section checksum row, whose header words are zero in every older file
and unread by every older reader, nor the segment's payload in a spare word
of the superblock extension, for the same reason: a zero there reads as
"not recorded" and the engine estimates it. Decide which case a change is
before writing it.

**A checksum that cannot see a corruption is not a checksum for it.** Block
checksums cannot see a flipped bit in an index record -- a flipped `FIXED`
bit re-decodes the run with no error -- which is what the key section's row
of per-piece CRC32C words is for. A store's in-place-editable index carries
no row, because a record is published there with one aligned store into a
mapping readers already hold and a piece checksum cannot follow that
lock-free; `index_checksummed()` says which kind a reader has.
`tests/segwriter.rs` flips every seventh byte of a segment's key section and
requires each to fail the open. Its first run found a flip of the piece-shift
word that made the row look *absent* and opened clean, which is why a row
named with an impossible shift is damage rather than absence.

**Both callers of the writer emit the same shape unless a measurement says otherwise.**
`Db` passes `Options::inline_bytes` to its seal and its compaction exactly as
`SegmentWriter` does. This file once said the opposite, and three findings
were built on a difference that did not exist. If you are relying on a shape
difference between the two writers, measure it.

**`count_fixed` claims a count only when two independent quantities agree**:
the run is a whole number of strides *and* `Ext::last` is exactly
`(n-1)*stride`. Divisibility alone was tried: a run of 17 variable-length
values divided exactly by a stride of 4 and the first version answered 23.
Two quantities is still not a proof, so the contract is that the caller
knows its schema; the `FIXED` flag makes it exact where the writer could
prove it.

**Crash discipline is an order, and every window in it is survivable.**
Commit is a WAL append and one fdatasync; the batch is durable or its tail
frame fails its CRC and replay stops before it. Seal is write to a temp name,
rename into place, publish to readers, fsync, then the manifest -- one of
two slot files written in place and fdatasynced, the newest valid slot by
its publish sequence being the store at open, so a torn publish is the
slot an open ignores (`Options::manifest_slots`; the shape before it wrote
a temp name, renamed it over the old manifest and fsynced the directory,
`supdb-ingestmanifestrename`). A flush's drain is answered once its
seal's fsyncs are done and the slot is written, and the slot's fdatasync
follows on the segment work's thread with the WAL's and temp names'
retirement behind it (`Options::flush_waits_manifest` is the shape that
waited, `supdb-waitmanifest`): the round trip was a tenth of a
ten-thousand-key drain, and a crash inside it is a torn newest slot, the
case the slots were made for. A buffered store's flush commits what is
staged under its policy, with no sync (`Options::drain_syncs_wal` is the
shape that fsynced the whole log first, `supdb-ingestwalsync`), so its
drain is answered behind the slot's fdatasync instead: the log's
durability was what covered that window, and without it the slot's own
does. The flush's close of a direct run
fdatasyncs the run's records on the writer's thread, which waits for
the drain anyway, while the seal thread composes the trailer, so the
seal's fsync flushes the trailer alone (`Options::close_presync`;
`supdb-latesync` is the shape with the one fsync at the end): nothing
of the order moves, the landing still waits for the fsync, over less.
The directory fsync the seal's names need
runs at the seal's readable landing, on the segment work's thread beside
the seal thread's fsync of their data, and the two wait on one journal
commit here: the slots' first version dropped it as the manifest's own,
and it was the segment's too -- an fsync taken off a path is first asked
what else it made durable. The seal leaves the WAL open and ends at
its sequence, since replay skips every record a manifest covers; the log
rotates by size at a commit, leaving what the policy left unsynced to
the policy's next sync -- the file it begins says in its header kind
that its predecessor was left loose, the first sync by policy syncs that
file before the live one and chains the kind, and a replay that meets a
sequence gap at a loose boundary ends the log there where at a chained
one it refuses the open -- and a closed file retires at the next seal's
durable landing, whose manifest covers it (`seal_rotates_wal` is the arm
that syncs and rotates at every seal instead). Readers wait for no
fsync and the manifest does, and no manifest is written while a seal's
segments are published and unsynced -- it would name a segment that may be
torn and cover a sequence the WAL still has to hold -- so a merge that
finishes in that window lands after the seal (`Maint::collect` holds it,
`publish` asserts it). The arm that writes the seal's own slot in that
window, naming its segments pending (`Options::manifest_pending`,
`supdb-manifestpending`), trades the order for a rule at the open --
a pending name counts only once the segment reads back whole, else the
slot before is the store -- and retires the WAL only behind a slot that
names the segments durable; it bought nothing here, because the thread
its fsyncs were handed to woke a millisecond late. The seals in flight are a queue (`Maint::sealing`),
landed in the order they were handed and each in its two phases, and only
the oldest lands at all until it is durable, so one seal at most is between
its phases and no manifest covers a later seal's records before an earlier
one's are durable; the frozen tables are a list in the queue's order,
up to `FROZEN_CAP`, and a landing retires its front; a table `sync`
hands without a freeze is a seal in flight beside theirs that joins the
list's back only when the writer freezes it; the writer freezes into
room and waits only when the list is full, for the oldest's readable
landing; and the count of seals
(`in_seal`) is read before the state whoever decides against it, so a
count of zero is one whose landings the state shows. A crash between any
two of those leaves either a WAL that replays the whole memtable, with a
segment the manifest never named --
possibly torn -- swept at open, or a complete, named segment plus a WAL
whose sealed prefix is skipped by sequence. The ordered index beside a segment
is written unsynced and never fsynced (`Options::ord_durable` is the
arm that syncs it): it is a function of the segment's keys and nothing
else, so an open that finds it missing, torn or describing another
segment composes it again as the writer did, byte for byte, writes it
synced and counts the rebuild (`Db::ord_rebuilt`); its fsync was a
device round trip on every seal's, merge's and drain's landing for the
durability of a file the store regenerates. Every store has a manifest from
birth for that reason: without one, open takes every `seg-` file as live
and skips the WAL behind it, which was safe only while a segment was
fsynced before it was renamed, and that scan is kept for stores from before
manifests alone. A store's first seal under a flush
that partitions writes the partition's name directly when the piece is
tombstone-free and fits one, since that is what the flush's promotion would
link it as under a second publish; the first version named it so for a
store that does not partition on flush too, and the suite's ingest arm,
whose seal must leave a piece, scanned three times faster for one row.
Such a flush now promotes the piece itself, by link, and hands what
promotion cannot tile to a background merge the writer publishes at its
next commit (`flush_schedules`); and its drain's own last seal, the one
whose piece it would promote alone, is named the partition at the close
(`drain_names_partition`), since the promotion was a second manifest
publish for the same store, 2.3 ms of a drain of 8.5 at ten thousand
keys: before it the background waited for
`l0_trigger` pieces, a store of fewer stayed pieces for good, every scan
took the merge path, and the block cache had no partition to cache.
Replay applies the frames between commit frames whole or not at all -- a
partial batch used to replay as whole, and the first test written against
the contract found it. `settle` is what joins an in-flight seal; `sync` does
not, and an experiment that assumed otherwise measured a 286,000-key seal it
had never joined.

Ordered ingest has its own log and one more window. A run of keys above the
store's greatest goes to an ordered memtable and, at each commit, to a
segment open for append: the batch's records, then a commit marker carrying
their CRC, length and count, then one fdatasync on that file. The WAL never
sees them (`direct_ingest: false` is the arm that still sends them there).
Recovery walks the temp file's record stream to the last marker whose three
quantities agree with the records before it and rewrites what precedes it as
a piece; what follows is a batch nobody was told was durable. The CRC is
there because a crash can land the page a marker is on and not a page of its
batch, and a record whose bytes came back zero still parses. The close is
the seal's shape with one difference: the finished segment is hard-linked
under its piece name rather than renamed, and the temp name stays until the
manifest names the segment, because a name the manifest lacks is swept at
open and the temp name is what recovery reads; a temp name whose id the
manifest already names is that window's leftover, removed at open. The first
version of this path kept the hash memtable for reads and closed the segment
on the commit thread, and measured slower than the WAL it bypassed at three
million keys and at thirty: the memtable insert was a quarter of the load
and the close a tenth. A path that keeps the structure it was built to
bypass has bypassed nothing. The ordered table the run keeps for the
reads a run in progress serves holds a copy of every value, and the copy
is a third of the writer's fresh pages at three hundred thousand keys,
each first touch a fault the host prices; a table that holds references
into the run's own segment instead (`Options::direct_table_refs`,
`supdb-refvals`) took two fifths of those faults and a twentieth of the
writer's CPU and moved the load 4% in twelve pairs, short of the tenth it
was predicted to, so the references are an arm. What ships is the table
whose arena is the record stream (`Options::direct_stream`): a value is
laid out as its record at the append and the commit writes the batch to
the file from the arena, a whole write piece at a time as the buffer
path's pieces reached it, so the staged copy and the piece buffer's pages
are gone from the writer's faults and the copies are the arm
(`supdb-copyvals`, `supdb-ingestcopyvals`). The segment writer kept two
more copies of every key for the close -- its own key, span, offset and
hash for the key section and the Bloom, and the ordered index builder's
-- while the table it streams from held every key with its record; the
close composes the trailer, the Bloom and the ordered index from the
table in one pass now (`Options::direct_derive_keys`,
`flatindex::stream_trailer` by position, `ordindex::Composer`), a quarter
of the load's first-touched pages gone, and the kept copies are the arm
(`supdb-keycopies`, `supdb-ingestkeycopies`). The close's two fills,
the key section's slots and the landing's Bloom, ask for each key
sixteen ahead and hint its slot and its block meanwhile
(`SegmentOptions::lookahead`; `supdb-fillplain`, `supdb-ingestfillplain`
place as they are asked), since each is a random load into a table that
outgrows the core's cache at a few hundred thousand keys. The stream's
arena is a ring of megabyte blocks recycled once the file holds them
(`Options::direct_recycle`; `supdb-norecycle`, `supdb-ingestnorecycle`
keep the stream whole), since the stream's pages were two thirds of the
load's first touches and the comparator's dirty pages come from a pool
it reuses; the mixes after a load in the same store then first-touch
the pages the whole stream had warmed for them, which the sum of the
phases carries. The table's copy of every key was sixteen bytes a key of the
same kind, a quarter of the pages left; an entry names its key inside
its record in the stream now (`Options::direct_keys_in_stream`;
`supdb-keysarena`, `supdb-ingestkeysarena` keep the copies), and the
close's one pass reads the keys out of the record section through the
run's mapping, which is the bandwidth of the whole section where the
copies were an eighth of it. An entry is three words now -- the hash's
high half, which the index draws the slot and the tag from and a
rebuild reinserts by, the key's span and a thirty-two-bit count beside
the head -- and an ordered table's entry is one word now, the record's
offset in the stream with the key's and the value's lengths
(`Options::direct_slim_entries`; `supdb-fatentries` and
`supdb-ingestfatentries` keep the twenty-four bytes), the head and the
key's place arithmetic over it and the key and value read in the
record, where a lookup reads them anyway; a record that straddles a
block, a key copied past the bias or a value over sixty-four kilobytes
keeps a fat entry behind a flag bit in the word, and the writer's
stores to a hash table's entries go to the fat entry itself
(`MemTable::fat`), which a reader never reaches through the view
(`Entry`). The write log is beside them: every write to an ordered
table makes a new
entry, each above the last, so write `i` is entry `i` and the log is
the entries' range (`Options::direct_log_range`; `supdb-ordlog` and
`supdb-ingestordlog` keep the slab a hash table logs into) -- a word a
write and a fresh page a thousand that said nothing the entry count
did not, and whose tail the truncation of an uncommitted run never
trimmed. A run that kept no table at all is the shape left to price.

## Shapes the bugs come in

The reproducers for the previous engine's defects retired with it. The
shapes are worth keeping, because this engine can take them too.

**A derived thing keyed by position outlives the thing it was derived
from.** A level-0 piece's ranks -- each key's cut in the partition it is
aligned to, taken once when the piece is published -- were kept for the
piece's life, and a piece sealed while a merge of its range ran was kept
across the merge's publish under a new partition over the same fences, so
every cut below a key the merge folded in was behind by that key. Nothing
raised in release: a block built through a stale cut emitted the piece's
key where the old partition had it. The checked profile's tests reached
it as a debug assertion in about one run in twenty, once the seal's
timing put a piece inside a merge, and the chain that landed the reader
handle went red on a change of four ampersands. The ranks now carry the
blob id of the partition they were taken against and are taken again at
a merge's publish; `tests/db.rs` makes the merge long and the seal short
so the case is reached every run. The rule: a cache derived from one
object is keyed by that object's identity, never by its place or its
range, because a publish that keeps the position and replaces the object
is exactly what a merge does.

**A replay that builds the thing without the last step of the live
path.** A reopen replays the WAL into the memtable through the same
`append` and `delete` a live write takes, and stopped there: the live
path ends in a commit that records the watermark, so the replayed table
carried none, and a reader handle under `Latest` saw nothing of what the
WAL had committed until the writer's first commit after the open. Every
test read a reopened store through the writer's own handle, which
honours no watermark. The builder ahead of the reader found it, reading a
reopened store at the watermark and building as if the memtable were
empty; the open commits what it replayed now, and a test reads a
reopened store through a handle. The rule: a path that rebuilds a
structure ends where the live path ends, and a reader that honours a
mark the live path sets is the test that tells the two apart.

**A path only one arm exercises is a path nothing tests.** A delete was never
marked dirty, and the checkpoint asked to carry it dropped it, leaving the key
readable at its old extents. It was invisible for as long as every insertion
forced a full rewrite, because a rewrite reads the tombstone directly. Turning
a flag on is what exposed it, and the bug was older than the flag. It came
back with the record stream: every consumer of a table's head decoded the
stream's tagged form but the block-cache scan's prefetch, which indexed the
arena's block table by the tag, and six sittings of the stream as an arm
never ran a scan over a run in progress with the cache on; the gate's first
run with the stream as the default did, in a test written for another bug.
Flipping the default is the test an arm never gets, so flip it under the
gate before pricing it as the default.

**The sharp edges of a log are in the bookkeeping around it, never in the
append.** A value-carrying log queued a key twice when it sealed, re-queued
and sealed again inside one interval, and logged the same delta twice. A
replay applied records over newer index state because nothing said which was
newer. A durability point acked before the table that named its blocks was
synced, so a crash at exactly that point left a log naming blocks the
recovered table did not have. The WAL recycler has an edge of the same kind
at the device: the page cache sizes a folio by the write that creates it, so
a WAL pre-written in 1 MB pieces made every 100 KB commit after it cost 11x
its bytes; in 4 KB pieces, 1.04x.

**A clean test result proves nothing about a path the test never took.** The
first reproducer for the replay-ordering bug came back green and was
inconclusive until a path trace showed it had never reached the arm it was
written for. `tests/db.rs` emulates every crash window by constructing the
exact on-disk state the window leaves behind, for that reason.

**A size chosen before the thing is measured is a guess, and a wrong guess
here is never a fault.** A segment's head reserve has to be sized before the
first key is written, and it holds the block table, the checksum row and
copies of the fence and the directory. Too small and the pieces that do not
fit go after the data, costing the sparse reader a round trip; too large and
every segment carries zeroes forever. Neither raises anything, so a floor
under it was wrong in both directions at once and nothing said so. `reserve`
computes it instead, and it computes it by *calling the planner the writer
calls* rather than by restating the layout -- a second copy of that
arithmetic is a second definition of the format, and two definitions drift
the first time one is edited. What is left over is one four-byte rounding,
because the checksum row's length depends on where the section lands and
that depends on the answer.

**Arithmetic that underflows fails quietly and expensively.** A size-class
calculation underflowed for every block of 4 KiB or less, which is every
block a store of short postings produces. Debug builds panicked; release
builds wrapped and reserved 7,680 bytes for a tiny placement, so every small
store paid about 1.9x on every section it wrote -- visible as size, never as
a fault.

**A zero-length compare is not free when its pointer is dangling.** An
open fence was `Vec::new()`, and every read that reached the first partition
or a level-0 piece, and every scan that started there, compared its key
against that empty slice. The compare is a `memcmp` with a length of zero,
which glibc's AVX-512 variant answers by building a byte mask from the
length and issuing a masked load through one operand and a masked compare
through the other before anything looks at the length: a zero mask
suppresses the fault but not the address translation, and an empty `Vec`'s
pointer is a dangling non-null address no page table maps, so every call
paid a failed page walk that no TLB entry could cache. Measured with a
run-time length of zero: 81 ns with the dangling pointer as the left
operand, 19 ns as the right, 2 ns on a real pointer. The scan's cursor
compare had the fence on the left, a fifth of a scan's fixed cost at 300k
keys; the read path's `may_hold` had it on the right, and the read
measurement did not move. It was found by peeling a scan apart one call at
a time after every layer of the engine had been cleared. The fence
compares now go through `Seg::below_lo` and `Seg::cursor_from`, which do
not compare an empty fence at all. The rule is general: a slice that may be
empty, and whose bytes live in a `Vec`, must not be handed to a byte
compare without an `is_empty` check first, and a zero-length operation is a
real operation until a measurement says otherwise.

**A log read past the mark it honours.** A reader handle under
`Latest` reads the memtable's write log to settle its cached blocks,
and read it to its end: a key's write staged and not yet committed was
settled under the committed watermark, which left the value out as it
must, and when the commit landed the log had not moved, so the block
kept the old run for as long as it was cached while the point read
beside it, through the memtable, answered the new value. Every test
read a written key through the writer's own handle, which honours no
mark, or through a handle that had not cached the block. The handle
reads the log only to the length it had at the commit whose watermark
it holds. The first version of that took the length and then the
watermark, two loads of words the commit stored in that same order,
and a load that sees a store learns nothing of the stores after it: a
handle took one commit's length with the watermark before it, settled
that commit's writes under a watermark that hid them, read on past
them, and kept the old values for the rest of the state -- the same
failure, one commit wide. A test that reads a key and then scans from
it found it in about one run in two. A commit now writes its three
quantities into the mark the last commit did not write and then names
it, and a reader takes a mark only while it stays named. The rule: a
structure kept current by a log is current to a position in it, and
the position a handle may read to is the one its isolation names,
never the log's end; and quantities a reader must take together are
published as one, because no order of separate stores and loads makes
them one.

**A slot table whose slots share a line.** The reader table's slots
were adjacent words, eight to a cache line, and every read stores its
handle's slot twice, at the pin and at the unpin, so four handles
claimed in order stored to one line from four cores on every read, and
four threads read a partitioned store of ten thousand keys at one
thread's rate. Nothing raised and nothing was wrong: the first row
with threaded reads showed four threads at 2.5x one, and the probe with
the suite's shape showed 1.0x. A slot per line reads 3.8x. The rule: a
word one thread writes on every operation lives on a line no other
thread writes, and adjacent words, the layout nobody chose, are the bug
until the layout is chosen. It came back once as counters: every scan
through a handle bumped seven to thirteen shared statistics, and four
handles read the threaded scan mix at ten thousand keys at 0.46x of
LMDB, 0.91x with the counters off. A handle's statistics live on its
slot's line now, and the one word the writer must see, the regime's
scan signal, a handle bumps once per commit and not once per scan.

**A join that walked the long side for an empty short one.** A block
table maps a source's positions against a partition's block boundaries
by walking the source and reading each boundary key once, and read the
boundary before asking whether the source had a position left. A
snapshot of no unsealed keys is what every scan pass over a store just
flushed starts from, so the first scan read a cold line per block for
nothing: 500 µs of a first scan of 700 at three hundred thousand keys,
a tenth of the suite's scan pass at every rung from a hundred thousand
up, and nothing raised because every bound was right. The suite's own
rule found it -- time the pass's first operation on its own -- and the
same scan carried two more costs of the kind: the ordered index's top
level built at the first seek, and a builder thread spawned to find
nothing to build. It came back at once for the store the mixes leave:
the same walks with sources that have keys, a record read per boundary
and per piece key, made again by every handle and by the writer at
every published state, an eighth of each thread's pass in the threaded
scan mix. The rule: a walk over two sorted sides costs the side the
loop runs over, so it stops the moment the other side is spent, reads
the cheapest form each side has (the index's heads before the
records), and its answer, a function of two immutable objects, is
taken once and kept with one of them under the other's identity; and a
structure a pass needs once is built where the pass is not timed -- at
open, by the seal that made the segment -- or not at all.

**Two merges over one range that did not take pieces by age.** Level
0 is newer than the level below it, every piece of it, so a merge that
writes the level below may take only a prefix of a range's pieces by
age. A piece merge held a range's pieces as inputs; a partition merge
started beside it excluded those inputs and took the piece sealed
after them, and folded the newer piece under the older ones. A key's
values came back out of order, and a key whose older values a
tombstone had masked lost them when the partition merge dropped the
tombstone as one that had nothing older left to mask. The crash oracle
found it in its first run: nothing raised, every file was well-formed.
No partition merge starts while a piece merge runs; a piece merge may
start beside a partition merge because that merge's inputs are the
range's oldest pieces. The rule: two writers of one level order must
agree on which of them takes the old end, and an exclusion by name is
not an ordering by age.

**An order that held by name.** The live segments sort partitions first and
then the level-0 pieces, and the pieces sorted by fence and then by name.
Every piece a seal makes after the first partitioning is named `pcs-` with
its fences; one sealed before it is named `seg-` with the empty fence, and
so are the pieces aligned to the first partition, and `pcs` sorts before
`seg`: an older piece came after a newer one, a read took the older piece's
tombstone as the newest source and answered a version it had already
answered past. It held for as long as the merge took the unaligned piece
before a read met the pair, and reader threads beside a writer that sealed
every few hundred puts met it in their first minute. The pieces over one
fence now order by the sequence their names carry, and a read's
"oldest to newest" is a property of that order and not of the names.

**A form dropped by its writer stayed published.** The writer keeps
its own copy of every block form and publishes a clone for readers to
take; a reader takes a published form as the block and, with the table
complete, an empty slot as clean. When a block's overlay outgrew every
form but the wide one, the writer dropped its copy and built the block
wide, a form it never publishes, so the block's published slot kept
the form from before it went wide, and a reader handle read the last
block of a store short of three hundred of the five hundred keys
inserted past the end. No test read a block gone wide through a
handle; the writer's own reads walk its own tables and were right. The
slot now carries a mark whenever the writer drops a block's form or
holds it wide, and a debug assertion at the settle holds the rule: a
block the writer has no form for has none published as the block. The
shape is general: two copies of a structure, one kept current and one
handed out, drift the first time the current one is dropped rather
than replaced, and the handed-out copy needs a tombstone for that.

**A flag set before the call that resets it.** `maintain_forms` set
"the writes are filed into the tables" and then read the log, and the
log read, finding the generation moved, dropped the tables and cleared
the flag. Nothing set it again when the commit's own fill made the
tables, because the two paths that had always made them, the scan and
the install, set it themselves beside their call. So a writer whose
first table over a state was the commit's fill -- a handle's scan asked
for the maintenance, not the writer's own -- filed nothing into its
forms for the rest of the state, and its scans answered a key's values
short of every write since while the point read beside them answered
right. Every test scanned through the writer before it wrote, which
made the tables the other way. The flag is set where the table is made
now. The rule: a flag that means "this structure exists" is set by the
code that makes the structure, never by a caller that expects to, and
a reset on one path is a reset on every path that shares it.

**A share taken of a sum that counts one thing twice.** The settle
bound is a share of the store's keys, and it summed every live segment:
a level-0 piece sealed from a burst of updates holds keys a partition
holds already, so each seal grew the bound, a commit's batch stopped
crossing it, and the commits after a seal's publish settled every other
time. Which of them was the burst's last came down to the seal thread's
timing, and the lag pass at thirty thousand keys read two shapes, one
in three reps at half speed, with every count the probe kept identical
in both; the first scan was filing a thousand writes no counter
counted. The rule: a share of the store is taken of its distinct keys,
the partitions', never of a sum over versions of them; and a pass that
reads two shapes while its counters agree is paying for something no
counter counts, so time its phases.

**A divide an entry in a walk that needed none.** The record walk
divided a run's length by its count for the stride, and `chunks_exact`
divided again for the remainder: a dependent chain of two 32-bit
divides, in a loop of twenty-five cycles an entry, for a quantity that
is one in the common record. Callgrind showed it as five instructions
on one line; the divider's latency is what it cost. The rule: in a
loop measured in cycles an entry, read the assembly for the
instructions whose cost is not their count -- a divide, a call through
a pointer, a store the next load depends on -- and give the common
shape a path without them.

**A value built a field at a time and handed back whole.** The scan
snapshot's cursor, generalised to a base a frozen table, folded each
run's head into one value -- a key's slot and run in every table -- and
returned it with the mask: the fold wrote it to the stack in stores of
one, four, eight and sixteen bytes, and the return copied it out in loads
of eight and sixteen that spanned them, and a load that spans stores of
other widths is not forwarded from them, twice a key. Callgrind counted
the same instructions in the cursor before and after; perf put four and a
half times the samples on it, 42% on one sixteen-byte load from the
stack, and a lag point read 0.77x in every round of a sitting while the
point after it read 2.8x, its blocks built by the upkeep thread in the
time the slower scans gave it. The cursor finds the key and the mask as
two scalars now and reads the entries only where a caller asks. The rule:
a value assembled field by field and then moved whole costs a stall per
move that no instruction count shows; keep a hot path's results in
scalars, and when time moves with no instruction to account for it, look
in the annotation for a load from bytes just stored in pieces.

**A join that took back what it was about to wait for.** Under
`Upkeep::Background` the writer's upkeep is lent to a thread, and any
touch of it by the writer takes it back, waiting only for a pass in
flight and taking whatever the thread has not begun untouched. `settle`
joined the seal, the merges and the builder first and the upkeep last,
and each of those joins touches the upkeep, so by the time `settle`
asked the thread to finish, the writer already held it and nothing had
run. A model test that asked for the thread's work after a settle found
no pass at all. The rule: when joining several workers and joining one
resets another, join the one that would be reset first; and a test of
a background worker asserts that the worker did something, since every
check is green on work that was never handed over. The same test found
the converse once the commits held for their pass: under a hold the
writer commits nothing past the commit it named, so a pass stamping its
forms with the writer's latest commit instead of the named one is
invisible there, and only the level that never holds catches it. It
came again when that level became the default: the builder's check,
after the commit had lent its upkeep, read the upkeep for whether a
builder ran, took it home and dropped the commit no pass had begun,
so the arm that builds ahead at every publish had none of its commits
filed; and at a new generation it started a builder over the one whose
posted forms were the reason that commit's maintenance was due, which
the commit's own maintenance, running before the check, had always
taken first. A commit that lent hands the upkeep back if anything
after the hand-over took it home, and a builder whose forms wait to be
installed is left alone, as a running one was. The test that found
both failed 28 runs in 48 under contention and none alone.

**A format change priced on the path that reads it in bulk.** The
compact record was priced against `supdb`, whose scans walk a
partition's records in one loop that reads the compact form in place,
and read faster there. The buffered arm keeps its level-0 piece a
piece, so its scans take the merge path, which asks each source for a
key and then for its values, and `key_at` decoded the whole record for
the key: every key compared rebuilt the compact extent, twice with the
values read after, and those scans ran at half their rate for a week
with no row taken to show it. The quick row's gate named the arm; a
bisect in one sitting against its comparator named the two commits.
The rule: a change to what a record holds is priced on every path that
reads one -- the bulk walk, the merge's per-key calls, the point read
-- and an accessor answers only what it is asked, since a key read is
the most frequent call a merge or a seek makes. The point read was the
path not priced: the same commit made every lookup return the record's
extents as a two-variant value, borrowed or rebuilt from the compact
header, by value through every layer above, and the C mix read 0.84x
of the commit before in a bisect by alternation three weeks later,
with the uniform read, whose misses hide a fixed cost, level. The scan
had already been given a callback that lends the rebuilt extent on the
index's frame; the point read has one now, and the by-value shape is
the arm `supdb-extsval`, which reads the C mix at 0.89x in twelve
pairs of twelve. The corollary: when one path is given the fix for a
shape, every path that shares the shape is checked for it then, not
when a row finds it. It came again with the in-place walk of a raw-tail
group: given to the cursor, whose walks E's overlay and the sparse
builds take, it read 74,000 groups in place over the mixes while the
scans decoded 14 million through the form file's bulk scan path, which
the block cache's clean walks take, and the rates alone read as a gain
of a tenth. Count the paths' own counters beside a pairing before the
rates are read as the change's. A third path was found a commit later by
reading the code: the merge path's seek into a covered partition
(`ZCursor::seek`) opened the group in place and then sought it by the
walk of its records, which a group read in place answers no bytes for,
so the seek failed the page and `ZFile::fail` took the form file out of
service for the rest of the store's life, every key still right through
the raw form and nothing raised; the cursor's seek test ran on
block-coded fixtures only, and the pairings' stores had no pieces while
they were measured. The seek goes over the coded heads now
(`record_group_seek`), the test's fixtures carry the raw tails, and the
oracle, whose store has pieces, runs under them and counts a fallback or
a damaged form file as a failure. The corollary of the corollary: a
shape given to a kind of group is checked at every site that reads the
group's bytes, and a reader that answers the empty slice for a kind it
does not hold is a reader every such site has to be read against. And a
fourth site by the profile: the form file's own seek, the scan's start
and the cut's, still opened a coded group with the decode and sought the
decoded bytes, 87,000 whole-group copy-decodes a mix in the lean arm's E
for walks that decoded nothing, a fifth of its thread; the profile of the
arm named the caller where the counters had only the count.

**A list of raw pointers drops the pointers.** A replaced canonical
form and a replaced scan snapshot wait in lists until every reader
that could hold them has left, and the sweep freed each by hand as it
removed it. Nothing freed what was still waiting when the store
closed: the lists dropped their wrappers, and a wrapper of a raw
pointer frees nothing. The suite's stores closed with up to two
hundred forms and a snapshot waiting -- a pass's closing threaded
scans republish the blocks the mixes dirtied, and nothing sweeps after
them -- and `tests/leak.rs`, which counts every allocation, kept a
quarter of a megabyte per closed store until the wrappers were given a
`Drop` that frees. It was the suite's runner that showed it: with
freed memory returned between passes, what the process still held
climbed about 35 MB a rep at a million keys over every arm, and held
level once the wrappers freed. The rule: whatever owns a pointer it will free is a
type with a `Drop`, so the owner's end frees it, not only the path
that remembered to.

**A carry that required nothing to have moved.** The freeze carried
the writer's tables only over the state it had prepared them against,
and the prepare was a settle of the whole backlog, milliseconds on the
writer's path. A seal's landing from the segment work fell inside it at
the second freeze of the buffered lag sweep's largest burst, so the
freeze carried nothing and dropped the tables; every freeze and look
after it then refused for want of a table some scan had used, the
burst ended with none, and the pass built every block of the store at
a seventh of the rate the rows had shown. Retrying the prepare over the
landing restored the pass and doubled the burst, the settle being the
writer's; the freeze swaps the tables and nothing else now
(`freeze_settles` off), and the look carries them. The rule: an optimistic check
that fails falls back to work, never to a state nothing recovers from;
and a fallback that drops a structure must not also be what stops
anyone rebuilding it.

**A publish that dropped the marks an empty slot is read by.** A reader
takes a published form as the block and, with the table complete, an
empty slot as clean, so the writer publishes a mark for every block it
holds no form for, and the marks travel from state to state with the
copies a publish carries. The freeze that carries nothing published a
state with no form and no mark while the writer's tables still called
themselves complete, and a handle read the last block of a store
without one key the frozen table held for it -- the writer, walking its
own tables, was right. The tables carried across a freeze are
incomplete now until the fill makes them whole. The rule: where an
absence means something, whatever drops the things it is the absence
of drops the claim that gives it the meaning.

**An absence whose meaning a publish changed.** A reader at the
writer's commit takes a published form as the block and, with the
table complete, an empty slot as clean. With the forms holding the
memtables' keys, a seal's landing changed no block's key set -- the
piece's keys were the frozen table's, filed at the freeze -- so a slot
empty before the landing was empty after it, and the rebase dropped
only the forms a rewritten partition made stale. The arm whose forms
hold the pieces' fold alone (`forms_pieces_only`) made the landing the
one publish that adds keys to a block, and its rebase dropped the forms
of the blocks the piece covers and left the table complete where a
covered block had no form: a handle took the empty slot as clean,
walked the partition alone, and read a key's sealed value as the one
before it, in the reader threads' first seconds, while the point read
beside it, through the piece, was right. The rebase marks the table
incomplete for every block a landed piece has keys over, form or none.
The rule is the one above, turned around: where an absence means
something, whatever changes the facts the absence is about drops the
claim that gives it the meaning, and a change that adds to a set is as
much a change as one that drops from it.

**A check for what landed that read the sets on either side of it.**
The same arm's rebase asks which pieces landed since the writer's last
look by comparing the table's pieces with the state's, and drops the
forms of the blocks a new piece has keys over. A piece that landed and
was merged into its partition between two looks is in neither set: the
copies over the rewritten partition were carried across the rewrite as
the default arm carries them -- there the copy holds the frozen table's
keys already, so a vanished piece changes nothing -- and republished,
and a handle at the writer's commit read a key's merged value as the
one before it, with the key in no memtable and no piece for the point
read to disagree about. It surfaced in one gate run of two and in none
of ten quiet runs; four processes stressing the two threaded tests found
it in minutes, and a debug accessor on the handle naming the key's
every source said where it was not. Under the arm a copy now survives a
partition's rewrite only when the rewrite was the one publish since the
look, by the state's generation. The rule: a difference of two sets
shows nothing that joined and left between them, so a check for what
happened between two looks counts the publishes between them, not the
members present at either end.

**A note that held one of the things its look would need.** The look
that carries the writer's tables across a freeze reads the writes it
has not read from every table live since its last look, and its note
held the one live at that look and nothing after it. At the buffered
lag sweep's largest burst a table froze and landed every few
milliseconds while a pass ran longer than that, so a table went live,
froze and landed between two looks, nobody held it, and the look
dropped every table: the thread rebuilt the store in the burst, and
when the writer's first scan was the look, the pass built every block.
The state keeps the tables replaced since the upkeep's last look now
(`State::replaced`, trimmed at the count the look publishes). The rule:
what a reader that runs behind will need is kept by the side that
retires it, until the reader says it has passed it, never by the
reader's own record of where it last was.

**A fast path gated on the case it was written for.** A settle copies a
written key's run from its own chain when the chain holds a tombstone,
since a tombstone masks every older source, and the gate asked whether
the write was the live table's. A write read from a table frozen since
-- what the freeze that leaves its carry to the next look settles --
has the same chain and the same tombstone, and took the general path,
a seek of every piece standing, at about six times the price: the
writer's first scan after the buffered lag sweep's largest burst spent
thirty milliseconds there. The rule: a fast path is gated on the
property that makes it valid, not on the source it was first written
for.

**A pass that reported done for a turn it skipped.** The upkeep thread's
pass checked that the state was the commit's by its generation and, where
a landing from the segment thread had moved it between the hand-over and
the pass, filed nothing -- and reported the commit filed all the same. A
commit holding for the thread (`Options::upkeep_lag_pct`) was released by
those reports, the writer ran on through its freezes, and the thread's
real look fell past the tables the state keeps for it (`State::replaced`):
the writer's first scan after the burst dropped its tables and built the
store, in one bounded burst in four, with every count the probe kept the
same in both shapes but the blocks built. The pass runs now against
whatever state holds the table its bounds are of -- a landing moves the
generation, not the table -- and reports filed only for a pass that ran.
The rule: a worker reports progress for work it did and never for a turn
it took, and a check that a thing is the one named compares the quantity
the work depends on, not a counter that other things also move. It came
back as a bound: the pass that files at most a share of the store took
its share from the commit's generation against the log position it had
read to, a landing between the two read as a position of zero, and the
pass read nothing and re-posted itself seventy thousand times in one
burst. The bound is applied where the position is known now, in the log
read and the frozen tables' settle, and never computed from a count
beside it.

**A derivative walked for every key when a few blocks asked.** The
snapshot's cuts -- where each unsealed key cuts the partition's walk --
were taken for the whole run in one forward walk when the bounds were,
since the first version built every overlaid block and needed them all.
An upkeep pass that replaces the snapshot took them again: half a
millisecond over seven thousand keys, on every other pass of a dense
burst, for the eight or twenty blocks the burst had the thread build. A
block's keys cut inside its own sixty-four ranks, so each block's cuts
are walked on its first build or walk and no other's, seeded at the
block's first rank, behind a per-block flag published after them since
the bounds are shared through the snapshot across threads. The rule is
the one the empty-join entry gave, read the other way: a derivative of
one object per item is taken per item at the item's first use when the
items used are a fraction, and the whole-run walk is a choice to be
re-made when the fraction changes.

**A form dropped after the copy stood as the block.** A publish from
the segment work copies the old state's published forms into the new one,
and the writer marks a block dirty at its next look unless the new state
holds the very form its table does. The first version compared only the
blocks the table held a form for: a pass that shed or widened a block over
the old state after the copy left the old form standing as the block in
the new one, and three reader-thread tests read a key's older value after
its newer one. The rule is the one "a form dropped by its writer stayed
published" already gave -- a copy handed out needs a tombstone when the
kept copy is dropped -- and it applies to every copy, however it was made.
It came a third time in review, before it landed: the landing that
installs an empty live table for a table `sync` handed without a freeze
copied the published forms as every landing does, and a freeze files the
writer's backlog before it publishes where a hand-off files nothing, so
the copies were current to a position short of the handed table's end
while the piece the landing published held the rest; the writer's next
maintained commit stamped the new state's position over them, and a
handle's scan at that commit read a key short of the last batch while the
point read beside it, through the piece, was right. That landing carries
no form now, as a freeze that carries nothing does. The corollary: a copy
is current to the log position its source was filed to, and a publish that
carries copies across a change of the live table has to know that position
was the table's end. And a fourth time, as a deferral: the look that finds
a carried form its table holds none for marked the block dirty for the next
publish, and the settle in the same look wrote into the block with the form
still standing, which the check that a block with no form has none
published caught in the differential fuzz under the promotion arm, four
runs in a hundred with four processes and none alone; a publish that builds
nothing at a commit (`commit_forms_build` off) would have moved the forms'
position past the dirty block without publishing it. The look publishes the
marker itself now, as its sibling branches do, and the test leaves the
state the race leaves through a seam (`Reader::forget_form_unretracted`).
A retraction is made where the staleness is found, not queued behind a
publish some other step may run first.

**A rename under a merge.** A promotion hard-links a partition under a
new name to close its fence, and a merge's landing removes its inputs by
name. A promotion decided while a merge held that partition left it live
beside the outputs that replaced it: two generations of partitions over
one range, and the next merge over them found keys outside every fence it
was given. Inline it needed a merge still running at the next landing;
with the landings on a thread of their own it happened in the first run.
Nothing a merge in flight holds is promoted now. The rule: an identity a
job holds by name is not renamed under it, and a check that the job's
inputs are still what it took is the job's to make, not the reader's.

**A piece cut at fences a merge replaced before it landed.** A seal cuts
its table at the partitions' fences as they stand at the freeze, and a
merge may land before the seal does. Reads were ready for a piece over
ranges it was not cut at -- a store with one consults every piece -- and
merges and promotions were not: a merge over some of the ranges such a
piece covers took it as an input and refused its keys past them, and a
promotion beside it would have made pieces younger than it into
partitions, which read as older than every piece. A merge's fences are
grown to cover every input now, and a range such a piece overlaps is
merged, never promoted. The rule: when two jobs cut one space and either
may land first, whatever reads the result must take either order. A
promotion at every landing made the stale cut the common case: an
ordered load's tail was named against the last partition while the
piece ahead of it was in flight, that piece's landing promoted it and
closed the partition, and the tail landed wide over two ranges and went
to a merge of both, at every landing, where a link would do. A piece
open above and cut at least as low as the last range is promoted on its
keys now, the check a promotion makes anyway, since the fence in a name
says nothing the keys do not.

**A name decided against a store a seal in flight will change.** The
seal that names the first partition asked only whether the store had a
segment. With a table handed to a seal beside one already in flight,
both asked over an empty store and both were named the first partition,
two partitions over the whole range, and the landing's tiling assertion
was what said so. The count of seals in flight is asked first now, and
the state after it, so a seal counted out is one whose landing the
state shows. The rule: a decision about the store's shape is made
against the store plus everything in flight that will change it, and
the order of the two reads is the order the other side wrote them in.

**A precondition left to the callers.** Promotion without a merge is for
a store with no partitions yet, and two of its three callers asked
first. The full flush asked on every round, and it stayed latent inline;
with the merges collected on a thread of their own, a flush met five
pieces a merge in flight had held back, disjoint in key order, and made
them partitions from the bottom over ranges partitions already held.
Nothing raised, and a quarter of the keys read back empty. The function
asks now, and every segment publish asserts that the partitions tile the
key space. The rule: a function correct only in some state checks that
state itself, and an invariant reads route by is asserted where it is
published, not found where it is read.

**A join that blocked the thread everything lands through.** The
segment work's thread took each seal as it was handed over and joined
its thread at once, so for the seal's whole run -- 700 ms at three
hundred thousand keys -- it landed nothing else: a merge that finished
waited, and a piece that could have been promoted was not, while every
read took the merge path. Nothing raised, and the writer never waited,
since it waits only on the flag the landing clears. The thread polls
the seal as it polls the merges now. The rule: a thread that serialises
several publishes blocks on none of them; it waits on the set, and a
join is for a caller that asked for that one thing.

**A count that began only once there was something to count.** Shaping
waited for a read that had counted, and a read counted only when it saw
a piece, so a store read while its first seal was in flight -- no
segment at all -- had nothing counted when the piece landed, and the
shaping waited for the next read and the poll after it; at ten thousand
keys the sweep was over first. Any read the block path cannot serve
counts now, and a promotion, which rewrites nothing, waits for no count
at all. The rule: a signal that gates a decision is raised by the
condition the decision is about -- here, a read the shape made slow --
and not by a proxy for it that happens to be easy to test.

**A sort priced by its compares.** The seal sorted the frozen table's
entries with a comparison sort keyed on the arena's keys, two random
reads a compare and five million compares at three hundred thousand
keys: 147 ms of a 715 ms seal, a fifth, while the scan snapshot over the
same keys sorted them by radix over their sixteen-byte prefixes in 30.
Nothing pointed at it; the seal was off the writer's path and its time
was nobody's until the reads waited on it. The rule: a sort over keys
that live behind a pointer is priced by its misses, not its compares,
and when one structure already sorts the same keys the way that costs
less, the other takes that way.

**A publish that left its derived work to whoever looked first.** A
level-0 piece's ranks -- each key's cut in the partition it is aligned
to -- and its bounds -- where the partition's block boundaries fall in
it -- are functions of two immutable segments, taken once and kept on
the piece under the partition's id. The segment work took them after it
published the piece, so the writer's next scan found them missing at its
rebase and took them itself, on the read path: 0.6 ms at ten thousand
keys and 4.4 at a hundred thousand for a seal's piece, 17 for every
piece over a partition a merge had rewritten, in scan passes of 0.2 and
4.5 ms; a promotion, whose re-opened partition has a fresh id, took none
at all. It hid for as long as a landing came after the fsyncs, which put
it after every pass of the sweep; the two-phase landing put it inside
them, and the flush arm's fully-unmerged lag point read 0.3x of what it
had, at every rung, with the shape arm beside it flat. A host-normalised
bisect against LMDB named the commit, a probe timing each scan of the
pass beside the store's counters named the scan, and timers in the
scan's phases named the call. The ranks and the bounds are taken before
the publish now. The rule: what a publish is to derive is derived before
the publish is visible, since the first reader to find the cache empty
is the one that pays for it; and a cost that timing hides is a cost --
price a landing where the reads are, not where it happens to fall.

**A merge priced for a batch of a hundred.** The scan snapshot's
extension found each new key's place in the run by a binary search over
the rest of the run: right for a batch of a hundred over a hundred
thousand, and fourteen cold lines a key for a thousand over ten
thousand, which is the shape of every lag point's first scan and of the
switch to the seal's snapshot -- two milliseconds of a scan pass of four
at a hundred thousand keys. It sorted the batch by comparing keys through
the arena, two block lookups a compare, a millisecond for five thousand.
The batch sorts by its prefix words now, as the build does, and the
merge walks the run where the batch is dense in it and gallops where it
is sparse. The rule is the one the seal's sort gave, applied to a
search: a search over a run that lives behind a pointer is priced by its
misses, and a merge of two sides within a factor of a hundred of each
other walks the longer side rather than searching it, since a walk
streams and a search does not.

**A batch resolved in arrival order.** The settle resolved each of a
batch's keys to its block with a seek from the ordered index's top,
in the order the log held them, and the index's heads below the top
level are a cold stride a seek -- eight lines, issued at once, one miss
each key -- so the resolve was a quarter of the upkeep thread's time
over a dense burst, as much as the splices themselves, with nothing
raised and nothing wrong. Sorted by their sixteen-byte prefixes first,
the keys resolve in the index's order and each seek gallops from the
rank the key before it reached, over heads the last seek left warm; the
pass after the buffered arm's fully rewritten burst read 0.68x and
0.84x at a hundred and three hundred thousand keys. The rule is the one
the snapshot's merge and the seal's sort gave, applied to lookups: a
batch of searches over a sorted structure is made in the structure's
order, each from the last answer, since a search from the top pays the
structure's depth in misses for every key and a walk pays it once.

**A read measured fast because the write paid for it.** A first seal
at the floor left a partition behind a load, and the pass after the
load read eleven times faster -- because every commit of the load, with
a partition in place, filed its batch into block forms that the next
seal or merge invalidated: 890 ms of commits at three hundred thousand
keys against a load of 155. The row showed a read win and a load loss
in two cells, and only their sum said the change was a loss. The rule:
price a change that moves work between phases by the phases' sum; a
structure built on a commit in a write-only stretch is paid for by the
writer whether anything reads it or not.

**A page made for the reader on the writer's path.** The segment
writer wrote 2 MB pieces at 2 MB offsets so the page cache would hold a
segment in folios a mapping takes with one entry each, a gain to a scan
this machine never showed. A 2 MB folio takes a whole free 2 MB block,
and this guest's balloon hands free 2 MB blocks back to its host two
seconds after they free, so every such write faulted on the host for
each of its pages: the buffered ordered load ran at a third of LMDB's
from that commit on, 45% of a fresh process's load in the kernel's copy
into the page cache, and nothing the engine counts could see it. The
rule: work one path does for the other side's benefit is priced on the
path that pays it, and a gain never measured is no reason to pay
anything.

**A test that bet a seal would still be running.** Tests that needed a
seal in flight across a commit left it unheld, on the bet that the seal
thread could not finish first, and with the segment work inline a commit
lands a seal that has. When seals got faster the bets lost, a different
test each gate run, and the first of them had been losing one run in
three under load before anything changed. They hold the seal now
(`hold_seal_landing`, and `hold_seal_durable` where only the oldest may
land). It came back as a count: the segment writer's helper thread syncs
once an `ahead` of the file is unsynced and begins no sync once the
finish asks it to stop, and its test asserted a sync after writing a
quarter megabyte, which a host twice as fast wrote before the thread's
first turn; the test waits for that turn now. And as a merge: the test
of a piece kept across a merge bet that a merge of four hundred thousand
keys would outlast a seal of twenty, and the bet lost once under the
gate's load, with a guard naming the case not reached; it holds the
merge's landing now (`hold_merge_landing`) until the piece has published.
The rule: a test that
needs a state holds it, and a bet on timing loses the first time the
thing it bets against gets faster.

**A test whose answer was a function of the clock, or of a thread's
turn.** The superblock records the second a segment finished in, in each
of its two slots, and the checksum over each slot covers it: two seals of
identical input differ in four aligned words whenever a tick falls between
them. `tests/segwriter.rs` had learned that and kept the fix to itself, a
private helper; two tests in `tests/db.rs` that compared the files of two
stores were written without it, one whole-file and one allowing a single
aligned word, which is not enough for a second slot and a checksum. They
failed one run in a few dozen under load and every time the binary was
started within the test's own length of a boundary: a script that starts
it at chosen offsets before a second boundary (50-110 ms, in a test that
takes 0.2-0.5 s) failed four starts of thirty-five before and none after.
The helper lives in `tests/common` now, one definition for every test file,
and a file the superblock is not in is compared as it is. Beside it a test
counted the cache's kinds and size right after a commit under the default
upkeep, which lends the writer's tables to the thread: a counter read takes
the upkeep home, waiting for a pass in flight and taking back a commit's
batch the pass has not begun, so the count was of tables nothing had been
filed into, one run in thirty; the test runs the commit's own fill inline,
as its siblings that count what a commit filed already did. The rule is the
one above, applied to two more states a test cannot see it needs: the
second a store was finished in, and which side of a hand-off a counter is
read from. A flake whose trigger is a phase is reproduced by choosing the
phase, not by repeating the run.

**A mark taken whole and then read in part.** A commit publishes the
log's length, the entry count and the watermark as one mark, and a
handle under `Latest` or `Snapshot` took the length and the watermark
and left the count: it read the live table's entries to their raw
length, the writer's staged keys among them. A staged key's values sit
past the watermark, so nothing read back wrong by value and every test
that compared values passed; but a scan's snapshot held the key, and
the walk counted it against the limit with nothing to emit -- a scan of
ten beside ten staged keys answered five. The snapshot went into the
state for other handles, and a handle adopted any published snapshot at
or past its own length, so a handle pinned at one commit counted a later
commit's keys the same way. It surfaced as a debug assertion in the
reader-thread test, where the store's first key opens a direct run and
the writer's next, smaller key leaves it: the run's uncommitted tail is
truncated from the ordered table, and a handle whose snapshot named that
entry read past the table's length. A bisect named the commit where the
test began to catch it; the three tests written for the fix fail on the
commit before it too. Every pin now takes all three quantities, a
live-table lookup honours the count, and a handle adopts no published
snapshot past it, as the builder ahead and the keeper already refused
one. The rule: a mark published as one is taken as one, every position
a read names -- in the log, the entries, the arena -- is bounded by the
mark's quantity for it, and a value the watermark hides is still a key
until the count hides it too.

**A carry that took its own publish for the only one.** The writer
carries its tables across a publish it makes itself -- a freeze, the
switch to an ordered table at a direct run's start -- since it knows what
it changed, and leaves any other publish to its next look at the log,
which rebases the tables across it. The freeze carried only over the
state it had settled; the switch carried over whatever state it
replaced. A seal emptied the live table, the segment work landed it
readable, and before the writer looked at its log a key above the
store's greatest opened a direct run: the switch carried the tables
across the landing as well, so they had no bounds for the landing's
piece and the snapshot named the frozen table the landing had retired,
and every scan through the writer walked the last block without a key
of the piece -- all of them, since every key sat above the last
partition's last key -- while a point read and a handle's scan found
them. Nothing raised. The test that caught it passed alone and failed
under load, in about a third of its runs with seals that keep the log
and a twelfth with seals that rotate it, so it surfaced only when the
default changed. The switch carries now only
when it is the one publish since the tables' state, and hands the rest
to the rebase. The rule: a carry across one's own publish holds only
over the state it was prepared against, and the check is that state's
identity or generation, never the one field the carry changes.

**An arm sound only beside the default it was priced with.** An arm
keeps an older shape alive for pricing, and `supdb-nopin` kept the
writer's reads unpinned, which is sound only while the writer is the one
thread that publishes. The segment work then moved to a thread of its
own and publishing in the background became the default, and the arm
kept running beside it: the segment work replaced a state and freed it
while the writer's commit built a block through one of its segments, and
the suite faulted in about a third of the arm's runs. No test ran the
arm at all, so nothing said so until a pairing crashed. A store now
refuses an unpinned writer beside another publisher, and the arm runs
its segment work inline as it did when the pins were priced. The rule:
an option sound only under another's value is checked against it where
the store opens, and an arm that keeps an old shape keeps that shape's
other settings with it.

**A sweep that read the pins before it took the list.** A retired
item is freed once every pinned reader is past the epoch its
retirement bumped, and the sweep read the pins first and took the list
of items second. Two threads retire -- the writer at its freezes, the
segment work at its landings -- so an item the other thread pushed
between the sweep's two steps was judged against pins read before its
bump, and a reader pinned at the epoch before, holding what the item
was, is one those pins could miss: a window of tens of nanoseconds a
landing, with nothing raised. No test found it; the pin protocol's
proof did, when the pin's fence moved to the sweep, since the proof
needs every bump the sweep frees for to precede the barrier the sweep
issues, and the order gave it only the sweep's own. The list is taken
first now. The rule: a judgement over a set against a condition reads
the set before the condition when the set can grow between the two,
since a new member's condition is one the old reading cannot know.

**A barrier on every operation for a sweep once a publish.** An arm
made the pin a plain store and a load with the sweep fencing for it
through `membarrier`, twelve microseconds a call with a sibling thread
running, once a publish by design. The writer's every operation asks
at its end whether a replaced state can be freed, and while the upkeep
thread's pass pins the state a landing replaced the answer is no for
the pass's length: three thousand barriers in a burst of a hundred
thousand puts, forty milliseconds on a burst of fifty. The sweep peeks
at the pins without the barrier first and fences only when the peek
cannot rule a free out, since a pin the peek sees may be gone, which
frees nothing early, and a pin it misses is the barrier's to find. The
rule: a cost designed to be rare is priced at every site that can
reach it, and a site that asks on every operation needs an answer that
is cheap to say no with.

**A bisect across a change in the measurement.** The quick row read
point reads a quarter behind the rows of a fortnight before, and a
bisect by alternation named the commit that pinned the writer's
operations, with a full fence a read. Priced as an arm pair in one
process, the fence was nothing the pairs could resolve: two sittings
at a hundred thousand keys disagreed in sign over a tenth. Between the
bisect's two neighbours sat the commit that made the harness copy every
value it reads into a sink, on every arm: a fixed cost a read that is a
larger share of a shorter read, so supdb's ratio to LMDB fell at that
commit with no change in the engine, and the rows before it are not
the rows after it for a point read. The rule: a bisect of a ratio walks
the harness's commits as well as the engine's, and a step that changes
what is measured ends the comparison there, whatever the engine did.

**A locked exchange to look at a variant.** A scan asked, of every
block it walked through its own table, whether the block's form was the
wide one, and asked through `Arc::make_mut` so that it could update the
form if so: `make_mut` proves the pointer unique before it looks, with a
compare-and-exchange on the weak count, so every walked block paid a
locked read-modify-write to find the one in a thousand that was wide --
4% of the scan mix and 2% of the pass after a burst at three hundred
thousand keys, visible in a profile of the pass as `Arc::make_mut` with
no clone behind it. The variant is read through a shared borrow first
now (`wide_mut`). The rule: a question about a shared value is asked of a
shared borrow, and the mutable one is taken only once the answer says
there is something to change; and a function that promises uniqueness is
priced by what it does to prove it.

**A check that never fired, fixed to fire.** The upkeep thread's
conversions were to yield to a posted commit once the thread ran a
thousand writes behind, and the measure of "behind" was the log
position the pass may read to less the position it had read -- on the
thread the pass's own bound, which the log read has just reached, so
zero in every pass: the yield never fired, and the thread converted
every dense form it met, behind or not. Fixed to measure the writer's
lead as the doc said, it fired as designed and lost: the thread
converted a third of a rewriting burst's forms, the scans after walked
the rest as sixty-four-delta sparse forms at 0.65x, and the forms left
sparse cost the thread a tenth more to patch. The default is the
behaviour the dead check had given, said plainly, and the fixed
measure is an arm. The rule: a check that was never firing has a
behaviour of its own that every row so far has measured, and the doc's
intended behaviour has been measured by nothing; fix the measure, but
price the intended behaviour as a new shape against the one the rows
know before making it the default.

**A first look that read nothing, trusting what came next to cover
it.** A handle's first scan reads the write log to list the slots
created since its snapshot, and a handle with no snapshot yet skipped
the read: the snapshot it builds next covers every entry, so the list
would be empty anyway. A snapshot it *adopts* may stop short of the
memtable by up to `snapshot_adopt_behind` slots, and the switch to it
trimmed the list to the slots past its end -- an empty list trimmed is
empty, and the newest keys were gone from every scan of that handle
while the point read beside them, through the memtable, was right. It
surfaced one run in a hundred or two under eight-process load, because
the upkeep thread's pass normally published a longer snapshot and the
forms first, and under load the niced thread had made no pass when the
handle was claimed. The diagnostic that named it printed the key's every
source on the failing scan: the adopted snapshot's length, the handle's
empty added list, and no form published. The list is made from the slot
range now, the snapshot's end to the handle's bound, which is what the
log read would have listed. The rule: an invariant one path keeps by
construction is not kept by a path that skips the construction, so a
shortcut taken on one assumption is checked wherever a later step
depends on what the shortcut did not do; and a race that a background
thread's work hides is reproduced by taking that work away, not by
waiting for the timing.

**A sentinel that crosses the wasm boundary changes sign.** A wasm `u32`
arrives in JavaScript as a signed i32, so a failure sentinel of `u32::MAX`
arrives as -1 and a comparison against 4294967295 can never match. Every
error check in `web/supdb.mjs` was dead for as long as it compared raw, and a
reader over an object that failed to open answered `[]` for every key. The
convention is normalize to unsigned at the boundary, compare unsigned. In the
same file: the host imports are named with `wasm_import_module = "env"`
because the bare `extern` block silently stopped linking on a toolchain
update, and the host ABI's 32-bit offsets refuse an object at or over 4 GiB
at open rather than wrapping.

**A constant with two meanings, used for the other one.** The key
section's checksum row hashes the section in pieces of 16 KB
(`flatindex::PIECE_SHIFT`), and the segment writer writes the file in
pieces of a megabyte (`SegmentOptions::write_piece`); the first shape
of the direct run's stream hashed, wrote and hinted at the row's piece,
and a load of ten thousand keys made ninety-seven writes and
seventy-seven writeback hints where the buffer path made three and two.
Nothing raised and the bytes were right; the commits ran at three times
their time, and a profile named the doorbell write and the kernel's
copy before `strace`, per file, counted the calls. The rule: a quantity
with a name is used under that name, and a count of the syscalls a
phase makes is read beside its profile before the phase is priced.

**A write pattern priced on the bytes it wrote.** The stream's second
shape wrote a commit's batch as one write at whatever offset it fell
on, and its third wrote a piece that spanned an arena block as two.
Both left a file byte for byte the buffer path's, dumped and compared,
and every read over it trailed by a few percent at every rung, with the
read pass's profile identical to half a percent per symbol. The kernel
sizes a page-cache folio by the write that creates it and the alignment
of its offset, and `/proc/self/pagemap` with `/proc/kpageflags` over a
mapping of the file read the composition: forty folios of a megabyte
under the buffer path, twenty-nine and nine ladders of 512 KB down to
4 KB under the stream's. The rule: a file is its bytes and the page
cache that holds them, and a change to who writes the bytes is priced
on the reads as well as the writes, with the folios read when the bytes
are the same and the reads are not.

**A close that took the spare before the thread offered it.** `close`
flushed, took the WAL spare and removed it, and the flush is answered
before the slot's fdatasync and the WAL's retirement, which follow on
the segment work's thread; the retirement is what offers the spare. On
a loaded machine the thread's retirement fell after the close's take,
the spare was offered to nobody and outlived the close, and the test
that asserts a closed store leaves no spare failed once in a run beside
a build, and in none of fifteen runs alone or four at a time. The close
settles the segment work before it takes the spare now. The rule is the
one the settle's own join gave: a step that waits for a worker's answer
has waited for the work the answer names and nothing after it, so a
caller that needs what the worker does afterwards joins the worker, not
the answer.

**A poll capped at the next park slept through the regime it was for.**
The upkeep thread was to stay alert while reads were around, parking for
at most two hundred microseconds at a time; the cap was applied where the
thread chose its park, and reads came around while it was already inside
a park of several milliseconds it had chosen during the read passes
before the mixes, which last two. It ran during A or F in two reps of
five and E built every block of the store in the other three, with the
counters saying so rep by rep and nothing raised. The commit that finds
reads newly around, or finds the thread parked longer than its poll,
wakes it once now. The rule: a bound on how long a sleeper sleeps is
enforced by whoever changes the condition the bound is for, since the
sleeper cannot read a condition that changed after it chose; and a
background worker's effect is read per rep in its own counters before
its cell is read, because a thread that runs in some reps makes a cell
that reads bimodal, and a median over it reads neither shape.

**A cell one rung under a threshold the rungs above it cleared.** The
lag sweep's one-percent scans at ten thousand keys read level with
LMDB in twelve rows while the point read 1.3-1.4x at every rung above,
and the first two decompositions went after the walk: the alert
thread beside it (nothing moved), then the sealed walk against the
copy walk the sweep's later points take, which callgrind counted at
139 instructions an entry against 129 -- the copy's 1.4x was the
sealed segment cold on the scanning core after the seal thread wrote
it, and LMDB's first pass over its store is cold the same way. The
level point counted 196: the first scan's settle of the burst's 99
keys, 145,000 instructions, the forms materialized block by block,
150,000, and 150,000 in some 550 allocator calls between them. The
burst's commit files its batch itself only from `UPKEEP_WAKE_WRITES`
writes, 256; the sweep's one-percent burst is one batch of `size/100`
-- a hundred at ten thousand keys, lent, and taken back untouched by
the scan that starts a microsecond after the commit returns; three
hundred at thirty thousand, filed inline, the cell a win -- and the
pricing note that set the threshold had already read the point as
"0.99x (a batch of 99, lent)". Filing every batch inline
(`Options::upkeep_inline_min`, `supdb-ingestinlineall`) took the point
to 1.6x in both orders and the burst to 0.47x, below LMDB's, and cost
ycsb-A and F a sixth to a half at both rungs, since a mix's batches of
a hundred cross the same threshold and the thread files those beside
the reads. The rule: before a cell's walk is taken apart, read which
regime of the engine the cell's inputs fall in against every fixed
count on the path -- a batch size, a bound, a wake -- since a ladder
whose rungs straddle one measures two engines and names neither; and a
threshold moved for one cell is priced on every cell that crosses it.
What won both cells was the split the count suggested: the batch's
settle, a third of the fill, runs at a commit a scan preceded, and the
forms stay the scans' (`Options::upkeep_settle_after_scans`), which the
mixes without scans between their commits never reach; the allocations
were worth 5 instructions an entry of the 57.

**A sync the policy never asked for, on the one path that waits for
it.** `Wal::commit` writes the frames and fdatasyncs, and the flush's
commit of what is staged called it whatever the sync policy, so a
buffered store's drain began with the whole log fsynced on the writer's
path before its seal was handed over -- 2.4 ms of a 9 ms drain at ten
thousand keys, 3.5 of 17 at thirty -- for frames the segment would make
durable before the flush was answered, in a file the landing retires.
Nothing raised, and every decomposition of the drain before this one
had read the fdatasync as the device's: the durable arm, whose commits
sync, pays nothing there, so the pairing of the two policies never
isolated it, and the shuffled load at ten thousand keys read level with
a comparator whose sync is an fsync of its log alone. `strace -f -tt -T`
over the drain named it as the sync phase's first syscall, and the
stamps put it before `handed`. The flush commits under the policy now,
and a drain whose log holds unsynced frames is answered behind its
slot's fdatasync, since the log's durability was what covered that
window (`Options::drain_syncs_wal`). The rule: a sync is issued by the
policy, never by the shape of the call that happens to need the write;
and a drain's syscalls are listed under `strace` beside its stamps
before any wait in it is read as the device's floor.

**Two counters that start apart, compared as if they started
together.** The drain's slot wait asked whether the log had frames no
sync had covered as `synced < written`, and a log's `written` starts at
its header's length while `synced` starts at zero, so the pair read
"unsynced" from the file's birth until its first sync -- never, for a
buffered store's direct runs and for a durable store's, whose records
sync in the run's own file -- and every direct run's drain waited for
its slot's fdatasync, 0.4 to 0.9 ms where the publish had followed the
landing in 0.05. The change was priced on the shuffled load, the one
drain whose log holds frames, and read as a win there; the ordered load
beside it read 1.07x and 1.33x for the comparator across two sittings
and was taken for the host until a third sitting, with the stamps of
the direct run's drain beside it, put the slot wait where none had
been. The rule: a condition over two counters is written against the
values both have at birth, not the values they share once running; and
a change priced on the cell it was made for is read on every cell that
shares the path it changed, before its first verdict is banked.

**A mask over an empty run that said every byte varies.** The scan
snapshot's radix sort takes a pass only over the bytes that differ
among the records, found by folding every record's prefix words with
`and` and `or`; over no records the folds keep their identities, all
ones and all zeros, the mask reads every byte as varying, and the sort
ran sixteen passes of a histogram zeroed and summed for nothing: 18,000
of the 42,000 instructions of the first scan over a store just
drained, whose snapshot is built over the empty table, in a pass of a
hundred scans at ten thousand keys. Nothing raised and nothing was
wrong, and the pass read level with the comparator where its scans
after the first read faster. The harness's per-scan timer
(`SUPDB_SCAN_OPS`) named the first scan, a probe with the engine's
timers named the snapshot, and callgrind over the probe's first scan
named the sort. The rule is the small rungs' again, with a corollary: a
routine whose work is shaped by a fold over its input does the whole
of the work for an empty input, since the fold's identity is the case
that selects everything, so a run of fewer than two returns before the
fold; and the first operation of a pass is timed on its own at every
rung where the pass is short, not only where it reads slow.

**A sync whose error the next sync could not see.** The close's presync
of a direct run's records ran on the writer's thread while the seal
thread composed the trailer, and discarded its result on the reading
that the seal's fsync of the same file would report the error again.
Linux reports a writeback error once per open file description, and the
presync's handle was a clone of the writer's -- one description, one
sample -- so a failed sync of the records was answered as a flush that
had made them durable, under both policies, for as long as the arm
shipped. The flush answers with the presync's error now, and a hook
injects the failure for the test. The rule: an fsync's error is consumed
by the call that sees it, so the result of every sync on a path is
checked by that path and never left to a later sync of the same file.

**A length read off the slab that held the exceptions.** The ordered
table's write log became the range of its entries, its length the
entries' count, and read that count off the entries' slab; the one-word
entry then moved an ordered table's entries into a slab of words and
left the old slab holding only the straddlers, so every commit's mark
named a log of nothing, and three tests saw the inserts a run had
staged, since a handle that finds no log since its snapshot takes the
table whole. The count comes from the table's own `len` now, which knows
which slab holds the entries. The rule: a quantity derived from one
container is derived through the accessor that knows which container
holds it, since the next change moves the contents and not the name.

**A structure maintained for a reader who was never coming, found by
its memory.** The forms were maintained at every commit whoever was
reading, and a batch of random writes overlays every block of the
store, so a shuffled load had the upkeep thread building and patching
forms over the whole store that the next landing dropped, anonymous
memory of about the store's own size beside it. No rate named it: the
thread is niced and ran beside the writer on a spare core, every row was
in-core on a box with memory to spare, and the one gap it left, the
first lag pass after a shuffled load, read as a first-scan cost. The
first bounded rung found it as a kill, `anon-rss` at the
limit with the page cache reclaimed to nothing under it. The commit's
maintenance files nothing far from a scan now (`Options::forms_settle_far`
is the shape that did). The rule: a structure kept for readers is
gated on the readers being there, by the signal that already says so,
on every path that builds it -- the hand-over asked and the settle did
not; and a process's anonymous memory is read by phase, the group's
`rss` beside the suite's phase marks, before an engine is priced under
a bound, since what it holds is page cache it will not have. Beside
it: a pass divided for time weights its first operation by the divisor,
so a divided pass that trails is read against the whole pass before it
is read as the engine's -- at a tenth of the scans three of the four
lag points at three million read 0.25-0.4x of LMDB, and whole counts
read all four at 1.0-1.9x.

It came back on the seal's side, twice in one load, once the forms were
gated: a shuffled load of five million keys under 640 MiB killed the
default arm and the combined `Auto` arm while the arm with no cache ran
in 318 MB. The census by owner, taken every hundred batches
(`bench/examples/load_census.rs`), named the first: every seal built
its frozen table's snapshot copy, the table's keys and values again,
for scans while the seal ran, and three frozen tables stood with three
copies, 380 MB of the bound; a seal far from every scan builds none now
(`Options::seal_snapshot_far`), a drain's excepted. With the copies gone the default still
peaked at 529 MB in some loads and not others, and the count that split
the loads was the writer's wait at its freezes, 1.3-2.4 s in the slow
ones and none in the fast: the landings fell behind. The segment work's
thread took three times the no-cache arm's CPU, a fifth of it in the
pieces' ranks against their partitions, taken before each publish so
the first scan would not pay for them, and the rest in the device
doorbells of the major faults the walk took over partition pages the
bound had pushed out -- for pieces the next merge folded away unread. A
landing far from reads publishes its pieces without them now, a flush's
counted near (`Options::land_ranks_far`), and the default's loads peak at
256-312 MB. What the sync then held was the drain's merge placing its
cuts from a copy of every distinct key of the store, 120 MB at five
million, where a cut reads a key's first sixteen bytes and only within
its fence (`db::Slicer`). Neither copy nor ranks moved the census lines: the census
counts what the store holds and not what a worker is doing, and the
allocator's bytes in use beside it (`mallinfo2`) were what said the
excess was live. The rule's "every path that builds it" includes the
seal and the landing, and the signal is the same one: a scan on either
path counts toward it, since the merge path's scans were counted by
nothing (`Shared::scans_merged`). And work on the thread every landing
goes through is priced under a bound by the pile-up it makes behind it,
since its own time is what a fault costs there, which nothing in core
shows.

**A budget that counted what a structure held, beside a process that paid for what it cost.** The
block cache's budget (`scan_cache_bytes`) counted a form's keys, values and entries, and the arm
budgeted at 256 MB held a median of 935 MB of anonymous memory at the lag sweep's full-rewrite point
at three million keys, as much as the default with no budget and four times the arm with no cache
(234). The budget was exact about what it counted. `lagmem` could say that the count was 255 MB and
the allocator's bytes in use 483-538, and nothing about the 230 MB between; a census of the process
by owner (`Reader::memory_census`, the `lagcensus` line: the writer's forms by kind with their
contents beside their buffers' capacity, the published and retired forms, the snapshots, the
memtables, the segments' caches, each structure once) summed to 92-106% of the in-use bytes where
they passed 200 MB, which is what made the rest a question with an answer, and the answer was four
things, none of them a leak. The forms' buffers cost 1.6-1.8x what they held: a build asked for room
for every overlay key, the ones that replace a record as much as the ones that add, and a patch
doubled a vector that held forty deltas into room for sixty-four. Nothing shed for a patch: the
budget was kept where a scan built a block, which a burst does not do, so a burst the upkeep thread
kept pace with built the cache to the size of the store and the first scan after it shed the
difference into the allocator's free lists. The shed of a block in another partition's table left
the form it had published in the state, where a handle's read could take it, and that was the
engine's before the flag: under a budget small enough to shed, a claimed handle's scans answered the
values from before a write (a checked build tripped its assertion at the write instead), and the
suite never showed it because its harness does not check the values it reads and its lag sweep
claims no handle; the retraction is unconditional now, and a test with a small budget and a claimed
handle builds the shape. And what the sheds freed stayed resident: a freed chunk goes back to the
arena that allocated it, glibc gives an arena back only from its top, and the forms the upkeep
thread built over a burst were freed by the writer's scans, 150 MB of 8 KB chunks in an arena that
nothing allocated from any more, beside a cache a quarter of that size. `Options::forms_to_fit`
counts by capacity and builds and grows to fit, and carries three parts that are options of their
own, so that each is priced by taking it away: the shed after a patch (`supdb-cache256tofitnoshed`),
the sweep of the retired lists at the end of an operation (`...nosweep`) and the trim of the
allocator, by a thread of its own, after a stride of frees (`...notrim`). The arm read a median of
578 over twelve reps, against 490 for its budget and the arm with no cache, which has no trim, so
some of the difference is the load's retention that the trim returns and not the forms'. Only the
trim was priced apart then: fitting took 133 MB off the pass's end and 189 off the bytes in use, and
the trim the other 224; the sweep, which had freed nothing the census could show, had a guard that
compared the number waiting with the number its last sweep left and skipped on a match, which a pin
that stays and a pin that has gone leave alike, so nothing swept again until a publish, and it
retries on a span of operation ends now, unpriced. The rule: a budget is held against the
allocator's bytes in use, by owner, before it is called a bound on anything, and against the
process's anonymous bytes before it is called a bound on the process; a bound is kept at every site
that grows what it bounds and not only at the one that builds; and the allocator's free bytes are
read by arena, and as the resident bytes beyond use and not as `mallinfo2`'s free count, which
counts pages it has already returned; and a fix for a wrong answer is not an arm, since the arm's
off position is the bug and the default that runs with it off keeps it. What is left is not the
forms': 112-226 MB of resident free bytes in every arm, the arm with no cache included, that the
load's threads left in their arenas, and a write path that holds 60-260 MB beyond the cache while a
seal in flight or a reader's pin holds what a publish replaced, which swings the same arm's pass by
that much between reps: the rep that read 1,019 MB where its arm reads 580 was 215 MB of snapshots
and frozen tables beside the cache, a seal that had not landed, and the census said so. A request to
the trim on builds as well as frees, to return the load's retention at a burst's start, read no
lower and cost a look of 30 ms: refuted. Read a bound's effect as the median of several reps with
the census of each, never from the rep that happened to be run.

**A library's features, unified across a build.** The bench crate's
dev-dependency on `lz4_flex` asked for the defaults, cargo unified them
with the engine's pinned `std` and `frame`, and every example and test of
`bench/` linked the safe decoder: the compact form's probe timed a decode
1.3-1.6x slower than the engine's, and nothing printed which codec had
run. The manifest pins the features the engine ships with now, an
example builds with the safe codec only when asked (`lz4-safe`), and
`zsize` runs the codec and the encoder's fingerprint in its own binary and
exits when either is not what the build asked for. The rule: a measured
library is built with the features it ships with, and the binary says
which it linked.

**A budget that counted the cheap thing, and a step that replaced a fit
with less.** The page filler's loop was bounded at twelve turns and
counted its staging passes, which cost nothing, as turns, so the
fallback that was never to be reached could be; and a growth step that
met a page it overshot kept the smaller page it had and ran on past the
end of the group. A budget counts the expensive operation it exists to
bound, and a step that grows a fit never replaces it with a smaller one.

**A guard no input can reach.** The builder decodes every page it has
written and compares the result with the images, against a defect the
compressor cannot produce, so no input made the comparison fire and a
version of it that was constant true left forty tests green. The tail of
the builder takes a fit and a scratch buffer now, which a test damages
after the fit was judged, byte by byte against an oracle that decodes the
damaged block itself. The rule: a check against a defect the code under
test cannot produce is held to firing by damaging its input after the
fact, and a conditional assertion that skips itself when its fixture has
no padding passes for the wrong reason.

**A controller tested on constants its own answer moves.** The selector's
replay fed its pressure term as a constant, and the term is the group's
refault counter, which holds the store's own refaults, and which form
serves decides how many there are. With the constant, making the exits
inert under pressure looked like the fix for a loop; fed as the group has
it, one constant workload made eighty-two installs and eighty-one exits in
three hours, and a compact form that faulted more than the raw form was
never left, while the checker stayed green because the loop makes one
switch a dwell. A controller facing constant inputs reaches a fixed point,
so the replay now feeds it what the world would give back and a grid of
four hundred and eighty constant workloads asserts that none of them
switches in its last hours. The same module found a model believed again
after the form it was wrong about had been seen not to pay, and a census
read in the state where it means nothing: the raw form's rate over the
intervals that were capacity misses and not the first touches of a restart,
and nothing of the census read at all while the model says the store fits.

**A tally that missed the case it was kept for.** A running balance that
was to be a cumulative sum (an excess that must floor at zero or a long
hot stretch is owed back), a budget that saw only the builds it expected
(superseded builds collected after their build was cancelled were lost,
and installs tripped it again), a window sized by its type and not by its
constant (a `u8` whatever `BUDGET_BUILDS` said, found by moving the
constant and seeing nothing change), and a threshold stated two ways in
the design. The rule: move each constant both ways and see what notices.

**A test that reads what it tests.** Twenty-odd constants of the selector
survived until their tests wrote the registered number out instead of
reading the constant, and five holes in the first traces (the band between
a threshold and half of it, a budget suspension that ends with the dwell,
a rule that ignored the compact form's faults, an evidence window that
did not restart, silent intervals) were found only by breaking the rule:
a trace that never crosses a rule is not a test of it. A hundred and
thirty-two mutations picked by the author left holes that two hundred
mechanical operator flips found, so the flip pass runs beside the list,
since a list finds only what its author thought of.

**A sibling function without its sibling's guard.** `reserve_contig` made
the arena's first block when `block_end` was still zero; `reserve`, which
the memtable's keys and values go through, tested only whether the bytes
fit, and a reservation of no bytes always fits: it answered offset zero
without a block, and the write of nothing at that offset asked for a block
nobody had published ("an offset past what was published"). So the first
write of a fresh store, and of every table after a seal, panicked if its key
was empty, in release as in checked, and no test wrote an empty key first;
the second form's reviewer found it because its fuzz allowed one. The rule:
two functions that hand out the same resource take the same guard, and a
fuzz that draws its inputs from the whole of the type (the empty key, the
empty value, the key of 0xff bytes) finds what a table of examples chose
not to. It came back in the scan
snapshot's arena: the batched reservation answered no bytes with the tail
and no block, the per-key one made the block, and a snapshot built over
the empty key alone reserved its one key's no bytes the first way; the read
of them, the fence sought in the snapshot at the next partition's start,
asked for the block nobody had made and the upkeep thread panicked, one
seed in three hundred of the differential fuzz once a new option draw had
moved every seed's operations, with a trace that named the arena's tail at
zero and no block. A read or a write of no bytes touches no block now,
and the two reservations agree (`tests/settle.rs`). The corollary: a fuzz
whose seeds share one random stream changes every seed's operations when
a draw is added, so a failure that appears with a new option is attributed
by running the commit before with the same draws, not by reading the
option's name.

**A check spelled twice, and the copy nothing reached.** The form file
reader's first decode compared a page's rank and count with its fence entry
and walked its group in code beside `verify_page`'s, and the attach's sample
compared them in a third place. Every damage test flips a byte and the
page's CRC refuses it before any later check runs, so deleting the compare,
skipping the walk or passing no first key survived twenty-six integration
tests and fifty-seven unit tests. The three share `check_entry` and
`check_group` now, and the tests forge a page whose CRC is good under the
form file's own binding and whose records are wrong (one short, two swapped, a
first key that is not the fence's, a rank or a count the entry does not
name). The rule: a check against what a checksum already catches is reached
only by an input that passes the checksum, so the test makes one; and a check
is written once.

**A mapping's advice decided from a mode that moves.** A form file was advised
from the store's mode at the instant of its attach, so one attached after a
scan kept the kernel's readahead for its life: 806 faults of 4 KiB where the
other read 34 of 98 KiB, in the review's probe. Only point reads and counts
read a form file, and they are random in every phase, so it is advised random
once, at attach. The rule: the advice for a mapping one access pattern reads
is decided by that pattern and not by the phase the store happens to be in,
and the kernel's answer (the `rr` flag in `smaps`) is read, not the store's
record of what it asked. The advice went on after the attach's own sampled
reads for a while, and under the default advice each of the attach's own
reads -- the trailer, the resident part, the sampled pages -- pulls a
readahead window in around it: a 280 KB form file came in 67 pages of 69 at
its attach, a 4 MB one 522 pages of 1,004, two megabytes of a large one
that the reads may never want, and the warm's test read 8 KB where it was
written to read the file. The advice goes on as the mapping opens now, 42
pages of the 1,004 (the sixteen sampled, the resident part and the
trailer), and the test holds the attach to a quarter of the file. The rule
again: the advice is set before the first read it is for, and what a step
leaves resident is read from the kernel between the steps, never taken
from the file's size. And a page a mapping holds is not one a drop of the
file's cache takes: `POSIX_FADV_DONTNEED` skips it and `MADV_COLD` only
ages it, so the raw key section the open's checksum check maps stayed
resident beside a form that no read sends there, 85 MiB of the 496 a store
of five million keys held under 640 MiB, with 38 MiB of ordered indexes
beside it, and the form the bound could not hold faulted for the want of
them; `mincore` over the store's files read it
(`scripts/resident.py`), and the drop unmaps first
(`MADV_DONTNEED`, `Options::zpages_drop_raw`).

**A second mechanism that masks the first in a test.** Once the settle swept
the directory, the test that a merge takes the form file of the segment it
retires passed with `retire_seg`'s unlink deleted, because the sweep cleaned
up before the assertion; two mutants survived. The test asks the directory
after the merges land and before any settle. The rule: when two mechanisms
can reach one end state, the test asserts at the moment only the first has
run. It came back in the leak test as the other case: a retired form file is
freed by the segment's own drop when its segment retires with it, and by
the retire list's wrapper (`RetiredZ`) when it was taken out of a live
segment after a page failed a check, and the test written for the second
retired only through merges, which never reach the list; a wrapper whose
drop freed nothing passed it. The test flips a page in place under the
mapping now, so the settle takes the form file out of its cell into the
list under the pin, and the handle's drop with no sweep after is what
frees it. The corollary: a test of one of two mechanisms that free the
same thing is held to a mutant of that mechanism, since the other frees
it just as well.

**A process-wide side effect and a test that counts it.**
`release_freed_memory` is `malloc_trim` of every arena, and the first library
test to run a pass trimmed under the test that measures what a trim returns:
two of twenty-five whole-binary runs failed, with 148.6 of 300 MB released.
A lock now, taken for write by the measurement and for read by every test
that reaches a pass, through `into_inner` so that a failure holds nobody. The
rule: a test that counts a process-wide effect excludes every test that can
cause it. A lock excludes the tests that trim and not the ones that
allocate, and the count of anonymous bytes resident beyond the allocator's
use is lowered by any allocation from a free chunk already resident: a
dead arena of thirty-two megabytes read as nine in one run of the whole
`db` binary, so that test has a binary of its own (`tests/resident.rs`), as
the allocation count has (`tests/leak.rs`).

**A call site no real input reaches red.** The suite's refusals after the
read pass and after the shuffled load were green-side only, because a real
forced arm reads through its form files and the refusal is for the arm that did
not. Each is a function of its own now, driven by a stub engine that lies,
and the state a real arm reports is read from a store damaged and strewn for
the purpose. The rule: if a check's red input cannot be made by the real
thing, the call is a function the test can hand a liar.

**A prediction an assumption supports.** The first registered builds for the
forced arm rested on two or three rebuilds a partition that a flush and a
settle's first join make impossible; one band sat 0.4% from its own edge; the
load's registered cost ignored a build CPU already counted; and a staircase
was predicted for a file the build leaves resident in the page cache. A
prediction that rests on an assumption marked unmeasured is registered as an
identity the counters print, or not at all.

**A piece named by its position among the bounds that cut it.** A merge
cuts its output into pieces at bounds, each a key at an even rank cut to
the fence's length, and named each piece's fence by the piece's position
among the bounds. A bound cut from a key that shares its first sixteen
bytes with every key before it is at or below the first of them, cuts
nothing and makes no piece, so every piece after it carried the fence of
the slice before it, and the merge refused its own first key as one
outside its fence -- after which the store's segment work had stopped
for good and every commit answered with the refusal. The first
partitioning had it in one form, and a partition rewrite's split into
sub-pieces in another, where the joins were cut by count and named by
the truncated key at the cut, which separates nothing from the keys
that share it. The suite's keys are sixteen digits, the fence's length
exactly, so no row reached either; the differential fuzz (`tests/fuzz.rs`)
reached the first at six seeds of sixty with keys of a long common
prefix, and the refusal, made to name the key, the rank, the piece and
its fence, said which. A slice is named by the bounds that cut it now
(`cut_by_bounds`), a bound that cuts nothing names nothing, and a join
is made only where its cut separates the two sides; `tests/fences.rs`
builds both shapes. The rule: a fence derived from a key by cutting it
short is at or below every key that shares the cut, so it separates
nothing inside such a run; and two lists kept in step -- the cuts and
their names -- are built by one loop, never one by index into the other.

**A promotion that closed a partition at its own lower fence.** A
promotion keeps the partition and closes it at the first piece's first
key, cut to the fence's length, and checks that bound against the
partition's last key. A partition emptied by a merge -- every key of its
range deleted -- has no last key, and a piece whose first key was the
fence itself closed it at its own lower fence: a partition of no width,
which the tiling check accepted, since it opened where the last closed
and closed where the next opened. The pieces a scan walks over a
partition are bracketed by lower fence, and the partition of no width
shared its lower fence with the one after it, so a scan across the
boundary walked that one's pieces twice and listed a value twice, while
the point read, which routes by upper fence, skipped it and was right.
Three seeds of the differential fuzz read it; the reopened store read it
every time, and a scan from the key itself did not. The first bound
stands strictly above the range's lower fence now, the tiling check
refuses a partition of no width, and `tests/fences.rs` builds the shape.
The rule: a fence an operation closes is held strictly above the one it
opens at, by the fences themselves and not only by the keys between
them, since a partition may hold none; and a check that a set tiles
checks every member's width.

**A merge that wrote nothing for a fence no input key lay in.** A merge
cuts its output by the fences it was given and writes a piece for each
slice that has a rank; a fence with no rank -- the partition that names
it emptied by an earlier merge, and the merge over it selected by a
wide piece with no key inside it -- got no output, while the partition
was an input and went with the landing. The set that landed had a hole,
the tiling assertion killed the segment work's thread, and the store
answered every flush after with "the segment work has stopped"; the
differential fuzz reached it at one seed in two hundred, every run. An
empty partition is written for such a fence now, as the emitter writes
one around a slice whose every key is dead. The rule: an operation that
replaces a set of partitions produces one output per fence it took,
keys or none, since the tiling is a property of the fences and not of
the keys.

**A key's identity taken from where its bytes begin.** The settle folds
a key's consecutive writes into one pending entry and skips its
duplicates in the resolve, both by the key's arena offset. The empty key
takes no bytes, so the key created after it begins at the same offset,
and that key's creating write folded into the empty key's entry: never
filed into its block's list, never patched into its form, and a scan
through the block cache answered the key absent, or its values short of
the one appended after an empty-key write, while the point read beside
it, through the memtable, was right. Twenty of a hundred and twenty
cache-on seeds of the differential fuzz, and the conversion's check that
a copy from the form agrees with a build from the sources, which is the
same loss seen a second way. The writes fold and skip by the slot now,
the write's entry in its table, which is what a key is to the table
(`tests/settle.rs`). The rule is the one the arena's first reservation
gave: a thing of zero length has an address another thing shares, so an
address is not an identity, and a fuzz that draws the empty key finds
every place one was taken for one.

**A partition with no record had no block to file a key under.** The
block cache counts a partition's blocks from its records, and a
partition emptied by a merge -- every key of its range deleted, or a
fence no input key lay in -- keeps its range with no record: counted as
no blocks, the settle dropped every write resolved into it, the scan
walked past it, and the builder made it a table of nothing, so a key
written into the range was in no scan through the cache, before and
after its seal into a piece over the range, while the point read beside
it, through the memtable or the piece, was right. Five of a hundred and
twenty cache-on seeds of the differential fuzz. An empty partition has
one block now, over no record and whatever the memtables and the pieces
hold in its range, and every table, bound and form over a partition is
sized by that one count (`Seg::blocks`; `tests/settle.rs`). The rule:
a structure that files things by the range they fall in has a slot for
every range the space is cut into, whether or not the range holds
anything of its own, since what is filed under a range is not what the
range holds.

**A rank kept across the rewrite that moved it.** A table is carried
across the merge that rewrites its partition over the same block
boundaries (`same_blocks`), and keeps its copies, since a block's key
range is the same on both sides; its sparse forms are dropped, since a
delta's cut is a rank. The keys filed since the snapshot were carried
too, each with the cut it had resolved to -- a rank in the partition
before the rewrite -- and a key the merge folded in or dropped inside a
block moves every rank after it. The form built from the kept cut put a
filed key at the wrong record: the scan emitted the record the key's
tombstone was to mask, then the key itself after its successors, and
the upkeep's check of a copy from the form against a build from the
sources, and its check of an entry by cut against the search by key,
fired on the same cut. Four of a hundred and twenty cache-on seeds, each
with the partition rewritten once or twice between the filing and the
scan. A rebased table resolves every filed key again against the
partition as it stands (`tests/settle.rs`). The rule is the one the
piece's ranks gave, applied to the keys a table files: a position taken
against an object is kept only under that object's identity, and a
carry that keeps the structure across a replacement of the object
re-takes every position it holds, not only the ones a form holds.

**A cache a thread kept of things a store had closed.** The memo of
decoded groups was the thread's, keyed by the form file's attach identity
and the page, and a thread that read through one store's form files and
closed the store kept their groups until other stores' displaced them:
ninety kilobytes a store in `tests/leak.rs`, bounded by the cap, freed by
nothing the store's close could reach, and invisible to the memory
census, which counts by handle. The memo is the reader handle's now
(`zfile::ZMemo`), dropped with it and counted with it. Beside it, the
memo's file compare -- an entry is the form file's identity and the page --
is reached only when two form files' pages share a slot, and the test that
said "another form file's page" passed with the compare deleted until it
built the collision (the slot is the page plus a multiple of the identity,
so over eight slots sides eight apart collide). The rule: a cache that
outlives what it caches is a leak with a cap, so a cache is owned by the
thing whose life it should share and counted where that thing is counted;
and a compare a hash reaches only on a collision is tested with the
collision built, as a check behind a checksum is tested with a forged
checksum.

**A count bumped after the thing it counts began.** A form file's build was
counted started after its thread was spawned, and a test that waited for
every started build to be collected returned between the thread's first
write and the count, and read the temp name as a build nobody had
started: one run in twelve under the lifecycle stress, none alone. The
start is counted before the thread exists, and a spawn that fails counts
a failure, so the sum the wait reads is whole at every instant. The rule:
a count another thread's wait sums with others is bumped before the
thing it counts can be seen, never after.

**A plan made against a state the plan's own first step replaces.** The
merge planner takes the ranges due from the partitions standing, and the
merge it then starts first joins a merge already finished, whose landing
replaces the partitions that merge rewrote: a due range whose partition
the landing had split named a range two of the new partitions tiled, the
planner took it as it was beside them, and the merge cut its one key list
at both -- pieces [0, 1500) and [0, 750) -- and refused its own key
("merge slices leave a key unassigned"), which stops the store's segment
work for good. One run in a few hundred of the reader-thread tests, once
in an out-of-core sitting's shuffled load that was run again and read as
the host's; eighty runs of the test four at a time did not reach the
window, which is microseconds wide. It was reached by holding the first
merge before its landing and making the planner join a merge in flight
before it plans (`Db::hold_merge_landing`, `Db::plan_joins_merge`,
`tests/fences.rs`), and closed by resolving the fences a caller gives
against the partitions standing after the join (`resolve_fences`), with
a debug assertion that a merge's fences are a sorted run of disjoint
ranges. The rule: a plan made before a step that can change what it was
planned against is re-resolved against the state after that step, and
never carried across it as data; and a refusal that fires once in a
sitting's run and once in a long test is one bug with its window named
by both, not two accidents.

**A walk that reopened what it held at every interruption.** The
overlay walk under the compact form cut the partition's walk at each
unsealed key inside the window: a seek to the key, a fresh walk after it
and a values read at its emit, each opening the key's group again, three
opens a key where the walk had the group open already. Nothing read
wrong, and the scan pass never showed it, since a drained store's walk
meets no unsealed key; the E mix did, 1.12x of the raw form out of core
with its faults down twenty-fold, because the suite's Zipfian is not
scrambled and the mixes' updates pile onto the lowest keys, where E's
scans also start, so a window of fifty there crosses about sixteen
unsealed keys. The time by kind named the scans, the counts per mix
named the opens -- thirty-four a scan against four and a half for the
pass's scan of a hundred -- and the merge path already had the cursor
that keeps its group across its turns; the overlay walk goes through it
now, 1.8 opens a scan (`Options::zpages_walk_cursor`;
`supdb-nocachezpagesrankwalk` is the walk by rank). The rule: a walk
interrupted by another source keeps what it holds across the
interruption, and the test of a scan path is a window dense with the
other source's keys, since a drained store's scan exercises none of its
seams; and a key distribution's shape is read before a mix's cell is
decomposed, since an unscrambled Zipfian's hot keys are contiguous and a
window at its head is a different workload from one anywhere else.

**A dispatch gated on the one option its tests ran under.** The point
read and the count went to the form only when the option was `Always`,
with the controller's gate asked inside the dispatch; the gate's first
piece tested the gate under `Always` alone, so a store under `Auto` in
COMPACT read every key raw with the gate at the form and the cells
installed, and nothing raised, since the raw answer is right. The first
`Auto` store driven to COMPACT found it in its first run, by the count of
reads through the form. The rule is the one "a path only one arm
exercises" gave, for a mechanism with two modes: a test of the mechanism
runs under every mode that reaches it, and the condition at the call
site is the mechanism's own (the gate) and not the mode's name.

**A hook priced behind its flag, with the gate behind the call.** The
census hook at every read's and scan's start was a flag inlined into the
operation and the rest out of line, and the flag said the controller was
on where the work it guarded ran only under the census gate: a store
under `Auto` in core is FITS for its life, and every read paid the call,
the window's copy out of its cell and the gate's load to be sent straight
back, 56 instructions of a read's 830 by callgrind, the end's hook 38
more and the form dispatch's own gate check, out of line too, 23 -- 124 a
read, 15% of a point read's instructions, under a comment that said FITS
pays "the two loads and branches above and no more", above a copy it was
below. No pairing could name it: the arm's pairings against the default
at a hundred and three hundred thousand keys read the point-read mixes
5-28% slower in three readings of four and level in a fourth, since a mix
at those rungs is three to fifteen milliseconds and reads in two shapes
with the host, 3.5 and 5.5 million reads a second within one sitting, so
the readings were booked as the host's and the default stayed `Off`. The
hooks test the state they work on now, a window open on the handle or
the gate away from raw, and the dispatch asks the gate inline: 12
instructions a read over the default (`bench/examples/read_only.rs` under
`valgrind --tool=callgrind --collect-atstart=no
--toggle-collect='<supdb_bench::engines::Supdb as supdb_bench::engines::Engine>::get'`,
`supdb` against `supdb-autozpages` at 100000). The rule: a hook's inlined
check tests the condition under which the hook does work, never the
option that makes the work possible; and a cost "behind a flag" is
counted under callgrind with the flag on and nothing to do, since a
pairing resolves nothing under a tenth and a tenth is what the hooks
cost.

**A build priced by its class and limited by its window.** Under a
bound the controller's build of a store's form files took 136-196 s of
wall for 8-22 s of CPU, and the note that first timed it blamed the idle
I/O class the build ran at, behind the readers' faults that held the
device at 99-100% busy. The class made an arm, and the arm read level
in two sittings, the build's wall 80-93 s under either class. The
diagnostic's sampler had the build thread at forty-odd major faults a
second and the device's readahead is 128 KB: the build walked a mapping
by rank, one readahead window a wait, each wait a device round trip
behind the readers' queue, 5-15 MB/s whatever the request's class.
Asking the raw records ahead of the walk in four-mebibyte windows
(`Options::zpages_build_ahead`) took the wall to 29-60 s in every rep of
both sittings and the install 30-45 s earlier, and the passes before any
build did not move. The rule: a reader's rate under contention is the
bytes a wait fetches times the waits a second, so a change to a wait's
class or priority is priced after a count of the bytes it fetches; and a
sequential walk through a mapping asks for what it is about to read,
since the kernel's readahead is a window sized for nobody.

**A population taken from the section that holds the other
population.** The model of the raw form's faults takes two populations
a point read touches one page of each, the hash slots and the records
(`zselect::model_raw`), and the engine handed it the index section for
the first. The record region lies inside the key section for a record
whose value is inline, every record of the suite's shape, so the records
were counted twice, and the model predicted 1.25 faults a read where the
census measured 0.27 -- five million keys under 640 MiB, 4.6x -- for as
long as the controller shipped. Nothing raised: a prediction nothing
holds to a measurement is wrong for free, and the controller's verdicts
under it had read as cautious rather than wrong, since the rent is
measured and the inflated prediction only let it in. The eager entry
found it: bought on the prediction, every build was cancelled within a
second by the vanish rule, which holds the measured raw rate against the
modelled compact rate, and the trace, extended to print the census and
the evidence beside the model, put 0.27 beside 1.25 on one line; the
first sitting's two cancels had been read as a merge's, from the
stamps, until the event's kind was read. The populations are the hash
region and the record region now, the counts carry both, and the test
holds them to the files' headers and a store whose populations fit its
budget to FITS. The rule: a population handed to a model is the region
a read touches a page of, never a section that contains it; and a
model's prediction is printed beside the measurement of the same
quantity before its verdicts are read, since a controller that decides
on a prediction four times the truth decides the right things late and
the wrong things fast.

**An estimate from the first call of a chain whose steady state is the
third.** The sampler sized the form from thirty-two groups, each filled
from a rank with no hint, and the filler's own doc says what such a fill
makes: a page well short of full, the records' uncompressed count, where
a page hinted by its predecessor holds what its group compresses to --
"whoever sizes a partition from such calls has to chain a few first".
Nobody did, and the estimate came out at 1.7x the file (626 MB for files
of 360 at five million keys), exactly the uncompressed count a page; the
model's compact rate read 0.22-0.28 a read for a form that fit its budget
whole, where the forced arm measured 0.000-0.008, and every rate drawn
from it -- the predicted saving, the measured saving the rent and the
vanish rule take, the eager entry's verdict -- was off by that. Its test
asserted the estimate's own arithmetic and that the form was under the
records, which a 1.7x estimate still was. The sample chains three fills a
rank now and counts the last, and the tests hold the estimate to the
build's page count and the store's files within a tenth. The rule: a
quantity whose doc says it needs a predecessor is sampled with one; and an
estimate is held to the thing it estimates -- the build's pages, the
file's bytes -- never to its own arithmetic, since the arithmetic was never
the question.

**A refresh gated on who read the log.** The scan on the merge paths
files the keys created since its snapshot into the snapshot's side runs,
from the list the log read keeps, and asked for the filing only when its
own log read had moved. A handle claimed between two scans reads the log
for the forms (`publish_for_handle`) and, with no partition to hang them
on, refreshes nothing, so the writer's next scan found the log read
already, took the refresh for unneeded, and walked a snapshot short of
every key created since the claim, with nothing raised. With a partition
in the store the claim rebuilds the snapshot, and with the cache off the
claim reads no log, so it needed the block cache on in a store with no
partition yet -- every store's first stretch -- which no test built and
the differential fuzz drew in four seeds of forty-eight with the cache on
in every seed (`ZFUZZ_CACHE=all`, with `ZFUZZ_OPTVARY`); the probe that
named it stamped each log read with its caller. The scan asks whether the
list has grown past what the snapshot has filed now (`tests/db.rs`). The
rule: a step another operation may take on a handle's behalf leaves that
operation's own work undone, so the operation that owns the work asks the
state it is about -- the list against the filed count -- and never the
event, whether this call was the one that read the log.

**A window whose two readings were of two counters.** The census
window reads the thread's major faults at its open and at its close and
counts the difference, from the test's seam where one is set and from
the platform otherwise; the seam's count began as a sentinel meaning
"the platform's" until the test's first `add_faults`, and a test adds a
read's fault after the read. So the first window of every faulting test
opened on the platform's count and closed on the seam's, and read right
only while the thread had taken no real major fault before its first
read: the gate's test group runs straight after the checked build has
pushed the page cache out, the test binary's own text pages in by major
fault, and `a_cold_start_is_not_a_capacity_miss` counted 12,799 faults
of 12,800, once, with every count beside it exact. The seam's count is
zero from its creation now. The rule: a difference of two readings is
taken from one counter, chosen once at the first reading and never
re-chosen between them, and a sentinel that selects a source is a
second counter. Beside it, the same test's premise stood on a
threshold the dev profile's timings drew: the relax line is a function
of the sampler's timed decode of a page, a hundred microseconds with a
third of noise between dev runs against five optimized, and the test's
model rate of 0.69 faults a read sat between the line's 0.57 and 0.78,
so one dev run in eight relaxed the rung to FITS for two seconds and
the census it was counting stopped with it; under `checked` the line is
at 0.05. The seam prices the decode now (`ZInject::set_decode_cost`),
as it prices a fault. A threshold a test's premise depends on is pinned
through the seam, never left to a measurement whose profile the test
does not choose.

**A lie only the group nobody read could refute.** The form's page became
a container of groups, each decoded on its own, the directory naming each
group after the first by its first record's key, and the first version
verified a group at its own first decode: the walk of a group holds every
key of it below the next group's named key and its first key to the key
named for it. A directory that names a group by its *second* record's key
passes the group before (every key of it below the name) and is refused
only when the named group is decoded, and a read of the group's true
first record never decodes it -- the name sends the read to the group
before, where the record is absent, and the read answers nothing with
nothing raised. The forged-page test found it on its first run, under a
good CRC. A page is verified whole at its first decode now, every group
walked, and a read after decodes the one group it wants. The rule: a
check that a claim about `B` is true, made by walking `A`, protects a
read of `A` only; when a lie about `B` sends reads of `B` to `A`, the
check has to run before any read of the page, not when `B` happens to be
read.

**A slot keyed by a number a format change made sparse.** The memo of
decoded groups is direct-mapped, and its slot was the group's number in
the form file, the page times the most groups a page holds plus the
group, mod the slots. When the page became a container of up to sixteen
groups the number took sixteen values a page while the shipped cut puts
about four groups on one, so a form file's groups reached a quarter of
the slots and pages 256 apart collided; nothing raised, the memo's hit
rate was what it was, and its test asserted the collision arithmetic the
mapping implied. A Fibonacci hash of the number spreads the groups over
every slot (`Options::zpages_memo_hash`), and the test finds its
collisions by search and holds the slots reached to more than half. The
rule: a direct-mapped table is keyed by a function of the key's
information, not of its encoding, and a change to how densely the
encoding is used -- here the most groups a page may hold against the
groups it does -- is a change to the table's capacity.

**A probe's total over a whole read is an inlining result.** The same engine
read +0.9, +1.9, +2.9 and +3.15 instructions a point read across four probes
and a count +3.00 exactly, because the code after a probe's loop changes what
LTO inlines. Quote a price from the function that holds the inlined read,
with the probe's source identical for the base and the head, and never set a
threshold at the figure.

**A price read off a thread that was not running.** The controller's
sampler times its own touches and reads its wall less its CPU as the
price of a fault, and it ran under `SCHED_IDLE` beside the read pass:
five to seven milliseconds a fault, where the readers' own faults took
forty microseconds and the sampler's took thirty with no reader beside
it. The wait was the scheduler's, and the controller priced the attach
of form files already on disk above their build with it, and took nine
to sixteen seconds to sample at all. Its I/O class changed nothing; its
CPU class did (`Options::zpages_sample_idle`). The rule: a wait measured as wall less CPU on a
thread of a low class is the class's until the same quantity on a thread
at the readers' class says otherwise, and a cost taken from it is read
beside that before anything is priced on it.

**A wait that read its baseline after causing what it waits for.** The
tests' tick wait moved the injected clock and then read the step count it
waited to see move; a tick the segment work's thread took between the two
was in the count already, and the wait sat out its minute for a step
nothing would make. It failed a run of the file in three once the builds
ahead woke that thread at every build's end, and a stack dump of the
stalled run, every segment work thread idle in its sleep and none blocked,
is what said the wait and not the engine was wrong. The count is read
before the clock moves now (`advance_tick`). The rule: a wait reads its
baseline before it causes the event, never after.

**A bet whose loss cost what it was placed beside.** The builds ahead at
landing bet that a store passing a share of its budget will pass the
whole, and the first version built as a bought build does: it dropped the
raw records behind it, and its file took the page cache. A store of three
million keys that fits read at a twelfth of its rate after the load, since
its raw form had lost the cache, and the refaults that followed read as
the group's pressure, which bought the files, rebuilt them in the coding
the pick chose and installed a form a store in memory loses to. A build
ahead leaves the raw form's cache as it found it now: no drop behind it,
and its own file written back and dropped a piece at a time while the raw
form fits. Dropped only at the file's end, its dirty pages still pushed
the raw form out and the store still bought in two reps of three. The
rule: work done on a bet leaves what the present form needs as it found
it, so that losing the bet costs the work and nothing else, and the case
priced first is the one where the bet is lost.

**A bet placed where nothing could pay it, beside the reads it cost.**
The same builds ahead ran on in a store that was only scanned: the lag
sweep's store at ten million keys under 1.25 GiB, past the share of its
budget the bet is placed at, built a partition's form file at every
landing of its bursts' merges, ninety-three through the sweep, each a
partition read from the device and a file written beside the scans, and
the controller never bought one -- it buys on its point reads' faults,
and the sweep has none. The ten-percent pass read and wrote half a
gigabyte at the device where LMDB's read seventy megabytes, and the
store with no builds ahead read the pass in half the time, a burst no
slower. Two readings came before that one, and each was a measurement
of something else. An allocator held 380-490 MB free beside 78 in use,
and glibc told to map every allocation of 128 KB or more read the pass
1.7x faster -- in a run of its own, an allocator's policy being no arm;
the shipped binary across six runs read the pass from 1.9 to 3.8 s,
by the device bytes inside it, under both policies. Merges chained
through the bursts were to move the device bytes out of the pass; a
merge trace on the lag marks' clock then put no merge inside any
ten-percent pass at all. The gate (builds ahead wait
while scans are near) first read nearness off a flag the writer's seal
set, and the one-percent burst is too small to seal, so the flag kept
the write-only load's reading through the passes after it and the
builds ran on; the gate reads the scans' count now, and a seal records
only that the writes since the last scan have made the store far. The
rule: a bet is placed where the event that pays it can happen, and the
event's own signal says so -- a buy that comes from point reads is not
bet on beside scans; a quantity that separates a cell's shapes is read
within one run before an intervention outside the run is credited with
the difference; and a signal published only at an event is stale for
as long as the event does not come, so a gate reads the count the event
is about, not a flag the event happens to set.

**A planner that ran only where a merge landed, and a verdict summed
by hand.** The merge chain plans the next merge at a merge's landing
while reads are around, so it needs a merge in flight to go on: where a
burst's last merge landed before the pass's first scan, nothing started
another while the store was only read, and the full-rewrite pass at ten
million keys under 1.25 GiB ran 76 s with 286 pieces at its end where
the round before ran 32 with six. The segment work's loop plans a merge
when none is in flight and a scan has come since its last look now
(`Options::merge_plan_idle`), and the test holds the state the race
leaves: the merge in flight released with no scan, a range still due.
The arm that chained through the bursts as well (`merge_chains_writes`)
read ahead by the suite's own sum because its chain always had a merge
in flight; once the default's loop started one, it read 1.06x behind.
And it was first written off by a sum of the burst's milliseconds and
the divided pass's, which weighs a pass a tenth or a thirtieth of what
it stands for; the suite's `weighted_ms` (the burst and twice the
undivided pass) read the same pairing the other way, and the undivided
pass, with nothing extrapolated, read the stall. The rules: a planner
that runs at an event needs another way in when the event can stop
coming while the work stands, so the loop that owns the work looks for
it itself; a trade between a burst and its pass is read off the
quantity the suite defines for it, never a hand sum of a divided pass;
and where a divided pass and the undivided one disagree, the undivided
one is the measurement and the divided one the model.

**A verdict on an option whose condition the run never met.** The attach
bought on the prediction fires where the sampler finds the form files on
disk, and it was paired at five million keys under 640 MiB and read
"level, the rest by position", and turned off -- while the builds ahead
left four to seven of nineteen partitions to the controller in every rep,
so the condition, every file on disk, held in none, and the two arms ran
the same code. The figures the shift's paragraph quoted for the builds
ahead came from the build before the fix that made them leave the raw
form's cache alone, which cost them the partitions the fix's kept records
pushed out; nobody re-took them, and the trace's own count of files on
disk at the sample said 12-16 where the paragraph said nineteen. The
rule: a pairing of an option is read after the count that says its
mechanism fired in its arm, and a figure written down for the shipped
shape is taken again after any change to the path that made it.

**A pass that reads what it replaces keeps it.** A build ahead reads a
partition's records to write its form, and the reads make those records
the page cache's most recently used: past the budget the kernel evicted
the records of the partitions still to be built, written once by the load
and never read, before the ones already built, and the builder, oldest
first, found its next partition cold every other time. Dropping the
records behind the build once the store is past the line the controller
would buy at, and at the look that first finds it there swapping every
built partition's records for its form file, took the files on disk at
the sample from 12-16 of 19 to 18-19 (the entry after this one says where
that line is). The
rule: a pass that reads one population to replace it leaves the read one
the most recent, so where the cache cannot hold both it gives back what it
has read, or the eviction takes what it has not.

**A posture taken at the budget's line that pays only far past it.** The
builds ahead stop sparing the raw form's cache once the store is past its
budget, since past it the raw form cannot be held; the first version drew
that at the budget itself. The suite's process grows its own memory across
a sitting's reps, and a store of three million keys under 640 MiB that had
fit ended its load a few percent past a budget of 380-414 MiB, the model
reading a few faults in a hundred reads: the crossing swapped out a quarter
of its records, and its read pass ran ten times longer than the
controller's without the builds ahead, which kept nearly all of itself
resident. The same store's refaults from the load bought an attach the
vanish rule cancelled a second later. The line is the selector's own to
enter now, by the model's prediction alone. The rule: a decision that
gives up part of the present form is drawn where the controller would
give up the form, not where the form first stops fitting, since just past
the budget the form still holds nearly all of itself; and a line read
from a budget that moves is checked at both its ends of a sitting, since
the process's own memory moves it.

**A write that asked for no flush, paid by the next one that did.** The
builds ahead write their form files back as they go, by
`sync_file_range`, so that a store that fits keeps its cache for the raw
form, and never fsynced them, since a form file is regenerable and
checked page by page at its attach. `sync_file_range` puts the pages in
the device's write-back cache and asks for no flush, and the next fsync
anyone issues pays it: at three million keys the builds ended after the
load's last commit, and the first twenty-five commits of ycsb-B after the
read passes took 74-1,690 ms where they take ten, while its reads were
level and the A mix after it was level too -- the mixes' time by kind
named the commits, and nothing else in the trace moved. The builder
syncs each file it wrote back once the file has its name now
(`Options::zpages_build_sync`), on its own thread, for the flush alone.
Syncing the files kept dirty past the line as well put 50-160 ms each on
the builder, which races the eviction front there. The rule: a write
handed to the device without a flush is a flush owed, and it is paid by
whoever syncs next; a background writer that leaves its flush owed puts
it on the foreground's next commit, so it pays its own where that costs
it nothing it needs.

**A position checked once, of slots swapped under it.** A handle's
scan takes the published block forms when they are current to the
commit it reads at, and it asked so by the table's position
(`State::forms_at`), once, at the scan's start, while the forms stand
in slots whoever publishes swaps in place, the slots first and the
position after. A publish of the next commit's forms landing inside a
scan -- the upkeep thread's, beside a handle's read -- handed the scan
forms of a commit after its own, and a handle holding a snapshot read a
write made after the snapshot, about one run in five hundred of the
windows test with the block cache on. That half of the test had been
skipped with the cache on since the hunt that first saw it, "the cause
is not known here", which is the configuration that ships; the
differential fuzz never holds a snapshot across a commit, and nothing
else read a handle's scan against the commit it named. Each form
carries the position it was published at now (`CanonicalForm::at`,
carried with it, and the fresh table's first position across a freeze),
and a reader takes one at or before its own: a reader that found the
table at its position found every form published up to it, so a form
past it is a block the reader builds (`Reader::canonical`). The test
leaves the state the race leaves (`Db::hold_forms_position`: the next
commit's forms in their slots, the position not moved) and fails on the
engine before the fix. The rule: a version checked once and the reads
it licenses are one step, or the things read carry the version; and a
test that skips a configuration because it fails there is a known bug
with its alarm off -- the skip names its bug, and the bug is found
before the skip is forgotten.

**A pin held for a pass the bound made long.** The upkeep thread pins
one state for its whole pass, and whatever a publish retires while it
runs -- a landed table, the seal's snapshot copy of it, the states over
them -- waits behind the pin. In core a pass is milliseconds. Under a
bound every key its settle resolves is a major fault on a partition page
the bound pushed out, and the lag sweep's full rewrite at ten million
keys under 2 GiB ran passes of seconds, one of twelve with eighty
thousand faults, while table after table landed and waited behind it
with its copy until the group killed the process. Two steps faulted: the
install of the builder's forms read two keys of the partition a form,
which the builder now sends with each form (`ahead_sends_bounds`), and
the settle itself. The look stayed current through all of it, so the
tables kept for it (`State::replaced`) were never the owner, and the
first trigger, built on how far the look lagged, never fired. The pass
now prices what its own pin holds -- the tables landed since it pinned
-- against an eighth of the memory available and gives its forms up
past it, and the store reads as far from reads until it is scanned
(`Options::upkeep_abandons`). The rule: work done under a pin is priced
by what the pin holds as well as by its time, and a backlog's length is
not what a long operation holds. Read the owner by counting while the
operation runs and from a thread that touches nothing of the writer's:
a census taken between batches took the upkeep home, which ended every
pass, and that run survived where the one beside it was killed.

**A bound drawn for one regime that held the other, and a sum read
where a size was meant.** The settle's backlog bound
(`forms_settle_backlog_pct`, two percent of the store's keys) was drawn
for the mixes, whose commits are a hundred operations with reads between
them, and the upkeep thread applied it to a burst's batches of a thousand
as well: lent every batch, it filed the first, by the term that files the
first commit after a scan, and then nothing until the backlog crossed the
bound, and whatever was under the bound when the burst ended -- the whole
burst at one percent, sixty thousand writes at ten -- waited for the first
scan. In core the residue was two or three thousand writes at three
hundred thousand keys, a couple of milliseconds the rows read as the
first scan's; under the bound at three million keys, with the pass
divided by thirty, it was 14 of the pass's 26 ms at one percent and 50
of 69 at ten, the cells at a quarter of LMDB's, and the thread's own
counters said so before anything was changed: thirty passes and 2.7 ms
of filing over a burst of thirty thousand writes. The thread files a
burst's batch at once now (`Options::upkeep_files_bursts`), and the
first version measured the batch as the sum lent since the thread last
filed, which three of a mix's commits unfiled also cross: the first
pairing read the mixes moving in both directions across its two
sittings, and the measure is the largest commit lent since the thread
was last caught up (`UpkeepTo::big`), a quantity a mix never reaches.
The test holds all three: the burst filed on the thread, deferred to the
scan without the option, and five commits of a hundred filed by the
thread under neither. Priced under the bound the filing read two shapes:
the pass after the ten-percent burst at 8-16 ms where the thread had
finished and 30-85 where the first scan waited for the pass in flight,
which with the thread filing every batch covers whatever was lent since
its last, half the burst at times; the arm that waits settled the same
writes itself in 20-35, one shape. The writer waits for a pass because
nothing in a pass asked whether it was wanted: a pass on the thread can
read and settle the log a step at a time and ask between steps, giving
the upkeep back marked short with the rest the writer's own
(`Options::upkeep_yields`), the writer's wait in the test from 40-55 ms
to 1-2 -- and priced under the bound against the pass waited out it is a
trade, the one-percent point up and the ten-percent point down in both
orders and their sum 8-10% worse, since the scan then settles a pass's
worth of residue itself where the thread had finished it with the blocks
built, so it is the arm `supdb-passyield` and the default waits. The
first version of it cut the settle itself at a step and left the cut's
writes pending across the snapshot's refresh, which carried the snapshot
past their keys, so the next settle filed them as new into blocks the
snapshot held them in already and three tests read a key twice: the rule
already written at that refresh -- settle before the snapshot moves --
holds for every step, so a step is a read and its settle, never a read
alone. What is left at the bounded rung is the thread's phase: a pass
settles a batch and then builds a form for every block the burst
dirtied, thirty to fifty milliseconds a pass at three million keys, so
the residue the first scan meets is anything from nothing to a pass,
and the divided pass of the out-of-core rows weights that first scan
thirty times; the undivided pass reads the eager builds as what they
are, forms the scans find built. The rule: a bound drawn for one regime's
operations is applied by the size of the operation in front of it, read
off that operation and never off a sum that other regimes grow too;
work a thread was lent and did not do is counted, per pass, at the
reader that did it, before a pass is read as the reader's own; and a
wait for a worker's step is a wait for the whole step until the worker
asks, at a place where stopping leaves every invariant standing.

**A block freed into a heap that kept it.** The memtables' arena
blocks, their slabs and their hash indexes, and the scan snapshots'
arenas, came from the global allocator in blocks of four megabytes, one
and half of one, and glibc's heap kept every one it freed: its mmap
threshold starts at 128 KB and rises to the size of the first large
chunk freed, so the first arena block a landing freed raised it past the
block, and every block after came from the heap and went back to it.
Nothing was wrong and nothing raised. The lag sweep at ten million keys
held 150-620 MB beyond its bytes in use (`lagmem`: anon less in use less
the allocator's own mapped chunks), 1.3 GB of anonymous memory at the
full rewrite with 434 in use, which under 1.25 GiB killed the roster
twice; and the compact form's budget subtracts the process's anonymous
memory, so the retention bought forms late as well. The runner's trim
between phases returned what was free at a phase's end and nothing
inside a burst. The owner was named by taking the allocator's choice
away for one run: with glibc told to map every allocation of a megabyte
or more (`GLIBC_TUNABLES=glibc.malloc.mmap_threshold=1048576:glibc.malloc.trim_threshold=4194304`
around `bench ab --arms supdb,supdb-nocache --size 10000000 --passes
load,lag --reps 1 --ops-div 10` under `SUPDB_LAG_COUNTERS=1`, beside the
same run as shipped, at `f2381c3`) the same sweep held 0-135 beyond use,
so the retained memory was the large blocks and not the forms' small
chunks. The engine maps those blocks from the kernel now, from 128 KB,
and unmaps each at its drop (`src/db/blocks.rs`, `Options::blocks_mapped`;
`supdb-heapblocks` is the heap shape); the bytes mapped are counted and
held to zero past every store's close (`tests/leak.rs`, `tests/mapped.rs`),
and the resident count the trim reads takes out what of them the kernel
has resident (`mincore` over the blocks alive), never the blocks whole:
taken out whole, a store whose tables in flight had mapped tens of
megabytes they had barely written read a dead arena of thirty-two
megabytes as seven, and the trim test that waits for the second trim
failed one CI run in five with the engine unchanged. The rule: a
library gives its large transient buffers back itself, since what an
allocator does with a freed chunk is the process's policy and not the
store's; a process's memory beyond its bytes in use is read against the
allocator's thresholds -- what it maps, what it keeps -- before its free
bytes are read as anyone's; the owner of retained memory is named
fastest by changing the allocator's policy for one run and reading the
difference; and a quantity taken out of a kernel count is itself read
from the kernel, since a page mapped is not a page resident.

**A cache taken whole for a reader who wanted a few of its entries, and
the fallback that read a page per entry.** A level-0 piece's ranks
against its partition -- every key's cut, in one walk, kept on the piece
-- were taken whole by whoever first needed one: the segment work before
a landing near reads, and otherwise the reader whose table met the
piece. Under a bound the full rewrite at ten million keys landed three
to four hundred pieces far from reads, and the first scan after it
ranked them all through major faults for nineteen seconds, with the
first scan after the merge's landing ranking them again against the
rewritten partitions for seventeen, in a pass of fifty; in core the same
walk is milliseconds, and no row showed it. Nobody ranks a piece whole
now (`Options::piece_ranks_whole`), and a build finds the cut where its
walk stops at the key, the path a piece without ranks always took. That
path then cost the ten-percent pass a quarter more faults than the ranks
had: the cut's compare read the partition's record at the cut, a page
the sparse build touched for nothing else, for an answer the ordered
index's heads had wherever a head is a whole key
(`OrdIndex::seek_in_exact`). The rule: a derived table over a source is
taken for the entries a reader asks for, in the reader's order, or not
at all, since a whole-table pass costs the source's size however few
entries are read; and a fallback priced in core is priced again by its
pages under a bound, where a line touched for a compare is a fault.

**A fault taken alone for pages known before the first was read.** A
block's build over a burst's pieces reads one page of each piece its
range touches, and read them as it met them, each a synchronous fault
and a device round trip, while the table's bounds had named every
piece's run over the block before any record was read: ten pieces a
block after a full rewrite at ten million keys under a bound, ten round
trips where one wait would do. The build asks the kernel for every run
first now (`Options::pieces_willneed`), and the full-rewrite pass reads
1.5x; a store in core asks nothing, since the ask is a syscall a piece a
block, and the gate that says the store is out of core (`Options::zpages`)
is what turns it on. The partition's own spans, asked for the same way
(`Options::blocks_willneed`), lost the one-percent point 1.4x before the
ask was gated on residency as well: that point's pages the burst's
settle had just pulled in, and an `madvise` over resident pages still
walks them in the kernel, ten to twenty microseconds on a scan of five;
one `mincore` of the span's first page first, and nothing asked where it
is there, kept the gains and lost the loss. The rule: a walk that knows
its pages before it reads them asks for them together where a page is a
fault, and the ask is gated on the fault being there to save -- the store
out of core, and the page not resident -- since in core, or on a resident
span, the ask is the whole cost; and the thread's major-fault count is
not the measure of such a change, since a page asked for and reached
before it arrives faults as minor -- read the pass's time.

**A prefetch issued the microsecond before the fault it was to save, and
a counter it satisfied for the path it was not for.** The heads a block's
cuts seek were asked of the kernel at the block's build, beside the ask
for its records, and seized by the first cut: an ask with no lead is the
fault with a syscall before it, and the kernel said so -- the pass took
more major faults an operation with the ask than without, 9.0 against 7.4
at the full rewrite under 1.25 GiB at ten million keys and 1.1 against
0.9 at the drained store, and the full-rewrite pass a tenth longer --
where the fault it was aimed at already overlapped the records' ask issued
before it. The test that held the mechanism counted the asks after a
commit and a scan, and the build's asks satisfied it while the settle's,
the site the asks were for, never ran in that test: a store whose commits
keep no forms has nothing to settle, and its writes are cut at the build.
Removing the build's ask read the counter at zero and named the test. The
rule: a prefetch is placed by its lead, the work between the ask and the
touch, and priced by the faults the kernel counts against the arm without
it; and a mechanism with two sites is held at each site on its own,
driven by the path that site is for, since a counter the wrong site
satisfies passes with the right site dead.

**A lead sized in pages for a walk whose work is in keys.** The settle's
run asks kept the heads' frontier half a run of 256 KB ahead of the rank
being resolved, thirty-two pages, which at the one-percent burst's density
of five keys a page is 0.8 ms of resolves -- under the run's own round
trip and transfer -- so the settle waited on most of its runs and the asks
saved three faults in ten where an ask a key had saved nine. The lead is a
time only through the batch's density, so it is set for the sparsest
density the dense shape admits and kept as several requests in flight
(`SETTLE_LEAD`, eight runs of 64 KB): the first scan then reads what the
ask a key read, 248 and 276 ms against 427 and 460, with the pass's
faults halved. The rule: a lead ahead of a walk is sized by the work
between the ask and the touch and not by the pages it covers, and it is
read off the faults the walk still takes, since a lead that is short by
half looks like a lever that half works.

**A planner that ran only where the writes were, and a chain at the pace
the writes set.** The merge planner ran at a seal's durable landing alone,
so a store written in a burst and then only read kept the burst's pieces
for as long as it was read: under 1.25 GiB at ten million keys the pass
after the full rewrite met 336 of them, seventeen a range, a page of each
in every block's build, with one merge over every due range in flight and
nothing to start the next when it landed. Three shapes of the fix each
taught something the previous had hidden. A merge's landing planning the
next, with every due range in one merge as before, met the same 336
pieces: the one merge took the whole pass to run, so the chain never got
its turn, and a cap on the bytes a merge rewrites is what drained them
(`Options::merge_bytes_cap`). Chained through the burst as well, the
merges ran beside the burst's writes and the burst paid a seventh where
the pass gained; the chain is gated on reads around, a scan since the last
landing, and a write-only stretch keeps the seals' plans
(`Options::merge_chains`). And a chain at the trigger reads lower
(`l0_trigger_now`) rewrote partitions of seventy megabytes to fold a piece
or two each, one after another, through the ten-percent pass -- 2.9 major
faults a scan against 2.0 with the same pieces standing, the pass a
quarter slower -- because that trigger paces the seals' plans, a merge a
seal at most, and a chain has no such pace; it plans at the base trigger.
The rules: a background job's planner runs wherever the condition it
serves can hold, which for the pieces readers walk is while the store is
read, and not only where the writes happen to call it; a threshold tuned
for one caller's cadence is retuned for a caller with none; and the
measure of a merge policy is the pieces a reader meets, read off the pass
(`pieces` in the lag counters, `merges_landed`, `merge_plans`), beside the
burst's rate and the pass's together, since a policy that leaves the
pieces for later wins both cells and pays in the pass nobody measured.

**A fault removed from a producer whose output the reader pays for.**
The block cache's splice bounds -- a block's first key, which the builder
ahead asks for every block it builds -- were record reads, a page each out
of core and over half the ten-percent lag pass's major faults at ten
million keys under 1.25 GiB, and the ordered index's top level holds those
keys in memory. Read there, the faults fell by twenty-five thousand in
both rounds of a pairing and the pass ran slower in both: the builder,
freed of its faults, posted its forms faster, and the scanning thread's
install of them grew from 25 and 21 ms to 133 and 148
(`Options::boundary_keys_from_top`, off). The faults had been on a thread
off the reader's path, pacing a producer whose output the reader consumes
at a price. The rule is the phases' sum again, across threads: a cost
removed from a background thread is priced on the thread that consumes
what it makes, and a fault count that falls while the pass rises names a
pace that changed, not a saving. And the refutation named the lever: the
producer itself. Out of core a burst dirties every block, and the builder
built each from pages the bound had pushed out for scans that read a few;
started in core alone (`Options::ahead_out_of_core`, off) the ten-percent
pass took half the faults and nine tenths of the time in two sittings. A
producer whose faults pace the reader's install is priced by removing it
before making it faster.

**A sync the policy never asked for, at the rotation this time.** The
log rotates by size at a commit, and the rotation fsynced the file it
closed before starting the next, since replay refuses a sequence gap
across files. Every frame of a buffered store is unsynced, so every
rotation was a 32 MiB fdatasync on the writer's thread, 43-70 ms here,
in a store whose commits never sync; and at a million and at three
million keys the lag sweep's shuffled load left the live file within a
few commits of the boundary, so the one-percent burst's first commits
paid it in every rep, two thirds of a cell that read 0.33x of RocksDB
without sync. The notes had decomposed that cell to the inline fill and
listed levers for it; the profile was of the fill's window, and an
fdatasync is not in a CPU profile. Running the lag probe with two more
one-percent bursts behind the first (`LAG_PCTS=0,1,2,3,10`) read the
first at 5 µs a write and the rest at 0.6-1.2 on the arm that filed
nothing, and `strace -f -tt -T` over it named the call: three 168 KB
WAL writes, an fdatasync of 43.5 ms, the next file. A rotation leaves
the frames the policy left unsynced now, the file it begins says so in
its header kind, the first sync by policy syncs the closed file first
and chains the kind, and replay reads a gap at a loose boundary as the
log's end and at a chained one as a record lost. The shape before is no
arm: a sync the policy never asked for is a wrong answer and not a
trade, and the pairing that priced its removal kept it as one only in
the tree that measured. The rule is the drain's, again: a sync is issued by the policy, never by the shape
of the call that happens to need the write; and a burst that costs more
than its commits is listed under `strace` before its profile is read,
since the profile of a thread that waits shows the work and not the
wait.

**A point that was not the store it said, and a burst as wide as the
share it is held to.** The buffered arm's one-percent burst at three
million keys read bimodal in every pairing, 24-70 ms in five reps of six
and 110-290 in the sixth, every slow rep with level-0 pieces standing,
its thread building twice the forms and its pass after faster. The first
two readings went after the writer's syscalls and found nothing: the
rotation inside the burst is an eight-byte create in every rep
(`recycle_wal` off), and the WAL's thirty-one writes took under four
milliseconds in every traced burst. The writer's own CPU over the burst
beside its wall named a wait, and the hold counters named whose:
`hold_within`, the paced commit's wait for the thread to trail by at
most the share, 47 ms of the slow burst's 48 ms gap and nothing in the
clean ones. Two things coincided. The share is a hundredth of the
partitions' keys, and the drain the sweep's first point ends in is
answered before the merge of the pieces it leaves, so at the burst those
pieces still held keys no partition did, the share undercounted the
store by them, and a burst of exactly a hundredth crossed it by a batch
or two at its last commits; and the merge was running beside the burst
on the guest's four vCPUs, so the pass in flight the hold waited for was
long. The sweep's comment called its first point "the fully merged
store", and `sync` joins no merge -- `settle` does, as this file already
said of an experiment that assumed otherwise. The sweep settles at its
first point now (`Engine::settle`, `bench/CLAUDE.md`), the probe and the
`lagburst` and `lagcounters` lines carry the writer's CPU over the burst
and its holds, and the burst reads one shape, 22-48 ms in twenty-eight
bursts with holds of 0-9 ms; the pieces the background leaves below its
trigger still stand in some reps, and the share still undercounts by
them, so the hold at a burst of exactly the share is the few milliseconds
of a short pass and not the merge's. The rule: a premise about the
store's shape at a point is held to the count the counters print -- the
pieces standing -- before the point's cells are read; a cell as wide as
a threshold measures the threshold's other side whenever the threshold's
count moves, so it is read with that count beside it; and a burst slower
than its commits is read against the writer's CPU and its holds before a
syscall is suspected.

**A background job's I/O class, priced by the device's queue.** The
full rewrite's lag pass under a bound ran beside its merge chain, the
scans' faults waiting behind the merges' reads and writes, and the obvious
lever was the block layer's idle class for the merge thread, so the
readers' faults would be served first. Paired at ten million keys under
1.25 GiB the pass read three times slower under it: the merges, served
last, never drained inside the pass, 160 pieces stood at its end where
the default left 6, and every scan touched a page of every piece over its
range -- four times the major faults, and the tenths flat where the
default's fall tenfold as its chain drains. Under a bound a merge's I/O is
the readers' I/O deferred, and a class that puts the readers first hands
them the pieces. The rule: a background job that reduces what the readers
have to walk is priced by the pieces the readers meet, never by the device
queue it stands in; and a lever on a shared device is read against the
count of what the other side left undone before it is read against the
rate.

**A prefetch aimed at a device that was not waiting.** The merge chain
inside the full rewrite's lag pass reads each input through a mapping, one
kernel readahead window a wait behind the scans' faults, the shape whose
ask-ahead had halved the controller's build; the same ask ahead of the
merge's walk fired -- six hundred windows, two gigabytes asked, counted --
and the pass did not move by a tenth of a second in either rep, with the
device bytes inside it identical. A prefetch moves a wait earlier; it
moves no byte, and a device carrying 4.5 GB in eleven seconds beside the
scans is not waiting for anyone's request. The rule: before a prefetch is
built, read the device's bytes over the window beside the thread's waits
-- a walk whose device is saturated gains nothing from asking sooner, and
the lever on it is the bytes or when they are paid; and a lever that
worked on one walk is priced on the next by the counter that says it
fired, since a mechanism firing to no effect is a refutation and not a
bug.

**A thing kept for a reader, freed by whoever published next.** The tables
a publish replaced are kept for the writer's upkeep's look
(`State::replaced`), and a publish trimmed the list to the tables since the
look; the look itself, having read a table, dropped nothing. A store only
read after its writes publishes nothing for seconds, so a replaced memtable
of a hundred megabytes and the seal's snapshot copy of it, another hundred,
stood through a pass of scans under a bound that could not spare them,
named by the lag census (`mem_replaced`, `snap_tables`) in one arm of a
pairing and absent in the other, since the other's chain of landings had
happened to publish after the look. The list is behind a lock and the look
drops what it has read where the state stands (`tests/db.rs`). The rule:
whatever is kept for a reader is freed by that reader when it is done,
never by an event that may not come; and a retention that a pairing's
census shows in one arm alone is read as a timing the other arm escaped
before it is read as that arm's.

**A rule right for a build and wrong for an attach, and a reading dropped
at every landing of a chain.** The form files a merge's outputs are given
at their finish (`Options::zpages_build_at_finish`) stood on disk beside
every partition through a sweep of scans while the controller read raw,
in three reps of four, and the arm's pairing read as a loss; the trace
line, given the buy clause's inputs (`ZSelect::buy_state`), said
`suspended true` from the load's end on. The wasted-build budget suspends
building for a dwell of sixty seconds once more than half the last eight
builds were superseded, the load's merge chain had superseded the builds
ahead it ran beside, and the suspension refused an attach as it refuses
a build -- a rule for the builder's wasted CPU applied to a purchase that
wastes none. And where the attach was not refused the reading was never
there: a sampler's reading is of the partitions at a generation and is
dropped when one arrives at another, so under a chain of merges -- a
landing every half second, a sample longer than that -- no reading ever
reached a step, and forty-one samples ran through one pass of fifteen
seconds, each over every partition's pages on the thread the landings go
through. The suspension holds off builds and never an attach now, a trip
during an attach suspends the building of the rest without cancelling it,
and where files land under the store's own writing the reading is
refreshed against a count of them in place of a sample
(`Maint::z_sample_refresh`). The default flipped under the gate then found
what the arm had never met: the refresh stood in for every sample, and one
made from a sample that had declined -- taken over no partition, before
the drain's first landing -- priced a build at nothing, refused the
attach and stood at the generation, so fourteen tests never reached
COMPACT; the refresh hands a reading only where the files on disk cover
the keys, from a sample that did not decline, and elsewhere the selector
samples as before. The rule: a guard is tested against the kind
of purchase it was written for, and a reading that a changing input
invalidates is refreshed from the part of it that changed, or it never
stands for long enough to be read -- and a stand-in for a measurement
stands in only where it carries what the measurement would have carried;
and a controller's refusal is read off its clause's inputs on the trace
before its evidence is reasoned about, since four clauses looked satisfied
from the fields the line carried.

**A pick at the budget's edge, flipped by a partition in flight.** The
coding gear picked the raw-tail form wherever the model predicted it fit
the budget, and a pick holds for the builds after it while the budget
moves: a form that fits at the load's end is a form the mixes may not
hold, since the budget subtracts the process's memory at the sample and
fell by a quarter through the mixes at ten million keys under 1.25 GiB.
There the raw-tail form sat at 97-99% of the budget, and one pass of four
sampled 28 partitions with one in flight where the others sampled 29: the
pick flipped with the count, that pass built every form file again at the
larger coding and refaulted through every mix, C and E at 2.6-2.8 and
2.0-2.3 times the others', with the engine and the arm the same in all
four. The
cells had been read as bimodal, the shape the host's two speeds have,
until the table's `zpage_builds_raw_tails` read 29 in one arm and 0 in the
other and the trace's `zsample` line named the pick. The line is at the
share of the budget the re-code policy already leaves to the page cache's
other tenants (`zselect::FORM_FILL`), one constant for both; a fix for a
wrong answer and not an arm, since the shape before is the pick at the
edge. `tests/zpages.rs` builds the shape, a budget the raw-tail form fits
whole and fills past the share, and the selector's unit test holds the
two partition counts to one pick. The rule: a choice made once for a
store's life is made with the headroom its inputs will lose, never
against their value at the sample, since a line at the inputs' edge is
crossed by whatever moves them next -- here a partition in flight; a cell
that reads bimodal under one engine and one arm is two decisions and not
noise, and the counter that names the decision is read before the cells
are read as the host's; and two policies that bound one quantity bound it
at one line, named once.

**A fill that held the writer for forms nobody read, and a wait that was
faults in disguise.** The upkeep thread's pass settles the commit it was
lent and then fills the table: a form for every block an unsealed key
overlays that holds none, the builder ahead's work done on the pass where
no builder runs, as out of core. The writer's next scan takes the upkeep
back and waits for the pass in flight, and nothing in the fill asked
whether it was wanted: at ten million keys under 1.25 GiB E's scans waited
735 ms of 6.7 s on 832 passes whose settles took 19 ms and whose fills
built 143 forms each, 119,000 over the mix, for blocks the scans never
read -- the walks met the same forms with those builds and without them.
The pass's whole yield (`upkeep_yields`) had been priced as a trade and
left off, since it cut the settle and the scan then settled the residue
itself; only the pass's steps timed apart on the thread's clock
(`Db::pass_steps`) told the step the writer needed from the one it did
not, where a profile of the thread put its time in builds and decodes,
which is what a fill is made of, and said nothing about who waited for
them. The fill gives the writer its upkeep back at its next block now,
the settle whole and the pass published (`Options::upkeep_fill_yields`;
`tests/db.rs` holds the fill cut at a block for the writer and whole
without the option) -- and the pairing read level, the wait gone and the
writer's minor faults doubled: E is the only mix that scans, so it builds
its whole block cache inside its own window, 137,000 forms and 97 MB of
first touches that are reclaims of about 10 µs each under a group at its
limit, and the thread's builds had been taking them while the writer
waited, leaving the pages warm for the writer's own builds once the forms
were dropped. The trim thread trimmed nothing in either arm (`trims 0` in
the census) and an arm with no trim thread read the same faults: not a
churn, the cache's building itself. The yield stands; the building is the
lever. The rule: a wait for a worker is decomposed by the worker's steps
before it is priced as the worker's work, since a step the waiter needs
and a step it does not are one wait from the waiter's side; a yield is
placed per step, because the steps differ in what stopping them costs;
and a wait removed is read beside the waiter's own counts after the
removal -- faults, CPU -- since work a worker did for nobody can still
have been paying a bill the waiter inherits.

**A reading that waited for the period of the thing it fed.** The controller's sampler took 0.4 s at the crossing to fill thirty-two ranks of every partition in two codings and estimate a form whose files were on disk to be measured, and the reading it gave was bought only when the model was next recomputed with it, at the model's period of a second: the buy on the prediction compares the model's raw and compact predictions, the compact one has no value until the model has the form's size, and nothing recomputed the model when the size arrived. The install came 1.1 s after the crossing, two thirds of the row's divided C at ten million keys under 1.25 GiB, every read before it raw, and C read 0.68x of LMDB. The event-level trace had shown the sample land and the buy come with nothing between them; the step-level one (`zselect t=`, printed at every SUSPECT step with the census's ops and the rewriting flag) named both waits. A reading's landing resets the model's period now, and where the files on disk cover the keys the reading is made from the files in the tick that wants it (`Options::zpages_reading_from_files`). The rule: an input that arrives on its own clock recomputes whatever is a function of it, since a period alone leaves the function stale for the rest of the period; and a measurement that estimates a thing already built is replaced by reading the thing, since the estimate is paid while the thing stands there; and a wait inside a controller is read off a trace of every step, not of the events, because the step that declines is the one with nothing to print.

**A comparator's warmth is the passes before the cell.** The pairings that priced C and E out of core ran the load and the mixes alone, and read C 1.43x and E 1.10x of LMDB at ten million keys under 1.25 GiB; the banked row, whose read and scan passes and their threaded passes run before the mixes, read 0.74x and 0.60x, since those passes bring LMDB's B-tree in until it faults on half a percent of its reads and runs near its in-core rate. Our compact form is unmoved by the warm-up -- it faults on nothing in either shape -- so the ratio turned on the comparator's cache alone, and the cells became a race of instructions against a B-tree in core, which the form's decode loses. Under a gigabyte, where LMDB's file leaves core too, every cell is ours by 1.6x to 4.5x. The rule: a pairing against a comparator is run in the row's pass order, not in the passes the question names, because the comparator's cache is a function of everything that ran before the cell and the engine's own form may not be; and a cell lost against a comparator in core is read for the regime it is in before it is read as the engine's -- the bound under which both engines read from the device is where the second form is for, and a row's bound is checked against the comparator's fault rate, not against the file sizes it was drawn from.

**A policy that took its gate from the policy beside it, and read its heat in the units of two counters.** The hot-raw policy's turn (`Options::zpages_hot_raw`, the hot partitions served raw beside the form files of the rest) was written beside the re-code policy's and took its condition -- the builder idle and nothing to build -- which a re-code needs, since a re-code is a build, and a release does not. At the bounded rung the load's drain lands its last five partitions after the crossing and the controller builds their form files through the first mixes, so in a pairing of the mixes the policy never looked (`zpage_hot_looks` 0 in both arms) and the pairing read the instrument's cost alone, a tenth against the policy's arm in one rep: the shape a policy that costs and does nothing has. Then three readings of its heat were wrong before one was right. A served form file's heat was its decode *time*, and a decode's time holds the fault on a page not yet in, so under the uniform read pass the partitions whose files were coldest read hottest and two were released for it; the counts are the heat now, every count priced at the store's mean decode. The skew was the partition's share of the heat over its share of the keys, and the first window after the install is thin -- the files attached and warmed inside it -- so one partition in twenty-four read 1.3x its share on noise and was released; the skew is read against the median partition's reads a key now, over a window of at least thirty-two sampled events, which a uniform pass never crosses and the Zipfian's second to fourth partitions, at 1.2x their share of the keys but two to four times the median, do. And the window's count was taken as the form files keep it, sixteen times the samples, beside the raw partitions' count in samples, so a raw partition read sixteen times colder than a served one and the swap rule would have churned; the test's trace showed a heat sixteen times the arithmetic before any pairing ran on it. The rules: a gate copied from another mechanism is derived again from what this one needs, and a pairing of a policy is read after the counts that say it acted in its arm -- its looks and its moves -- since a policy that never acted prices nothing but its hooks; a measure of heat is a count and not a time wherever the time can hold a wait; a share read from a window is read with the window's size beside it, and against the median of its peers rather than a nominal share, since the peers carry the same noise; and two counters compared are brought to one unit first, the trace printing both so that a factor of sixteen is read off the arithmetic and not off a pairing.

**An instrument of the profile read as the engine's.** The row's E at
ten million keys under 1.25 GiB, the hot partitions raw and the rest
compact, put 73 of its thread's 962 samples in `clock_gettime` where
LMDB's E put 11, and the difference was read as the census window's two
stamps an operation, the clock at every counted operation's start and end
for the busy wall its fault price is drawn from. A lever was built for
them -- one operation in sixteen stamped once a window is a thousand
operations long (`Options::zpages_census_sample`), every operation of a
shorter window still, since a window that short is one of slow operations,
the faulting regime the price is for -- and paired in the row's shape
against the arm that stamps every one (`supdb-zbusyevery`) E read 0.98x in
both reps, under the pairing's resolution. The profile taken again
without the harness's counter variable put the thread's clock samples at
one of 973: the profiling script had set `SUPDB_MIX_COUNTERS`, which turns
the scan timers on for the supdb arm and for no comparator
(`bench/src/engines.rs`), ten clock reads a scan the row never pays, and
the census's stamps were the rest. This file already said that a cell
against a comparator is read from a pairing with the counters off; a
profile is the same measurement, and the symbols the comparator lacks are
the first place the instruments the environment turned on show. The rule:
a profile is taken under the row's environment and nothing more, and a
symbol the comparator does not have is checked against the instruments
before it is checked against the engine; and a lever built on a reading
is held to the reading after it is built -- here the clock samples --
before it is held to the rate.

**A run written for the store with nothing unsealed, measured on the one
with something.** The block walk's clean run joins every clean block after
the one it stands on into one walk of the partition, so that a scan of a
hundred over a drained store is one dispatch and not three; it took a
block as clean by the form in the table's slot, which a block no walk had
built does not have, and the canonical table's null forms only while that
table was complete. A drained store has every block built or every form
null and complete, and the run was priced there. The write mixes leave a
few unsealed keys in every partition, so no partition takes the bulk walk,
the table is incomplete at every commit, and the cold partitions' blocks
are ones no scan has visited: a scan there built one block, ran one block,
and dispatched into the form file again at the next. The run builds the
block it reaches now, where it has the snapshot, and takes it when it
comes out clean (`Options::scan_run_builds`; `supdb-norunbuilds` is the
run that stops), and a canonical table that is incomplete reads a null
form through the slot as the walk itself does; paired in the row's shape
E read 0.96x for the arm without it in both reps of one sitting and 3.5%
in a second with the counters on, under the resolution. The promotion
after a walk (`Options::promote_entries`) had credited every key a run
took to the run's last block and copied the first block's ranks into the
last block's slot, which the gate's promotion test caught once the run
extended over unvisited blocks: it credits each block of the run with
what the run took from it and judges each on its own ranks. The rule: a
shortcut is priced on the store the workload leaves, not the store the
pass before left, since a store with nothing unsealed is the one shape the
mixes never show; and bookkeeping written for one block is read again
when the thing it books becomes several.

**A cursor a block for a scan that crosses two, and a count that named it.**
A block's reads of its covered partition went through one cursor since
the per-run dispatch was priced (`Options::zpages_block_cursor`), and each
block's walk and each block's build placed its own, while a clean run
dispatched into the form afresh; a scan of fifty crosses 1.8 blocks, and
the distinct keys the write mixes leave are spread flatter than the draws
that made them, so two in three of a covered scan's blocks in E at ten
million keys under 1.25 GiB held an unsealed key, and the scan placed 2.5
cursors or dispatches, each a rank search over a cold fence, a group
opened again and a page header checked, 6.5 groups opened where the scan
spans 2.7. The clean run's extension, built on the reading that the
dispatches were the block boundaries', moved the dispatch count by nothing
(`zpage_scans` 66,228 against 65,946 in E) and the clean walks by a
quarter: the count said the boundaries were not the cause and the sparse
blocks' cursors were. One cursor is held across the blocks a scan walks in
a partition now, the clean runs and the sparse walks through it
(`Options::zpages_scan_cursor`; `supdb-blockcursors` is the arm), and a
cursor behind the rank a read asks for steps over a few records and is
placed afresh past sixteen (`CURSOR_STEP`), since each step across a group
decodes it. The first shape lent the scan's cursor to the builds as well
and read E 4% slower with the groups opened up 6%: a build moves the
cursor to its keys' cuts and the walk after it starts behind them, so the
cursor was placed afresh and decoded the group it held again, where a
build's own cursor leaves its groups in the memo for the walk. Builds
place their own, the walks share: E's dispatches 48,800 against 59,800
and its groups 171,000 against 178,000 in one rep with the counters on, E
1.06x there and 1.00x over a sitting of two whose untouched cells -- the
point-read mixes, the scan pass -- moved a tenth and more, so the count is
the verdict and the rate unresolved. The rule: a lever built on a reading
of a count is held to the count after it is built, before it is held to
the rate, since a rate that moves a little in the right direction can be
the lever's side effect and not its mechanism; a per-block structure is
read against the blocks a scan crosses, since a scan of fifty over blocks
of sixty-four is two blocks more often than one; and a cursor lent to a
step that moves it is lent at the cost of the step after, which starts
where the lender left off and not where the borrower did.

**A room modeled as a share, measured as a quarter of the limit unused.**
The hot-raw policy asked for a release where the form files and the raw
partitions together would stay within three quarters of the budget
(`HOT_RAW_FILL`), the rest a share for the page cache's other tenants, and
the budget itself the limit less the process's anonymous memory and a
margin: at ten million keys under 1.25 GiB it released three partitions and
read the room as full, while the group's `memory.stat` at E's start held
845 MB of page cache and 121 MB of anon under the 1,342 MB limit -- 270 to
370 MB unused through the mix -- and the other tenants measured a quarter
of the share reserved for them, the ordered indexes 25 MB resident, the
WAL 30, the rest 15 (`scripts/resident.py` over the store and
`memory.stat`, both at the mixes' phase marks). The release is asked into
the room the group measures now (`Options::zpages_hot_raw_slack`: the
limit less its anon and its cache, the margin, and a tenth of the budget
for the write mixes' growth; `supdb-zhotline` is the line), and the first
pairing of it read level: the room asked for a fourth partition at E's own
start, where the line had said full, and for nothing before it, because the
skew a candidate is rated on was the look's own hundred milliseconds and
the Zipfian's fourth to eighth partitions read forty to eighty sampled
events a look under the point-read mixes against a floor of a hundred and
twenty-eight, so nothing past the third was hot enough to ask for until
E's scans made it so, and E is half a second. The skew is read over the
last eight looks' samples under the option now (`HOT_RAW_WINDOW`), and the
pairing in the row's shape read six releases a rep against three -- the
first four by A's end, the fifth in D, the sixth at E's start -- E 1.05x
in both reps and 1.13x in a sitting with the counters on, D 1.06x, and the
point-read mixes both ways by where the releases fell (B 0.75x in one
sitting and 0.96x in the next, A 0.90x and 1.04x), since a release's
device read of 42 MB and its form's drop land on the mix it happens in. A
fourth part was built and taken out: the raw pages mapped into the page
tables at the release (`MADV_POPULATE_READ`), on the reading that ten
thousand pages were ten thousand minor faults on the mix that first read
them; the test written for it read seven faults over ninety-one pages
without it, the kernel's fault-around mapping sixteen a fault, so a release
costs its first mix a millisecond of faults and the lever had nothing to
save. The rules: a room modeled as a share of a budget is held against the
group's own accounting -- `memory.stat`'s cache beside its rss -- before a
policy is read as bound by room; a floor on a window's samples is read
against the rate the window's workload gives the candidates it is for,
since a floor drawn under one mix's rate silences the next's; and a lever
on faults is priced after the kernel's fault-around is counted, by the
faults a reader takes and never by the pages it touches.

**A view an open, for a cursor that opens seven groups a scan.** The
block path's cursor over a form file built the page's view at every group
it opened -- the header parsed and checked, the directory's keys walked --
and twice more where it was placed by rank, for a page it had the view of
already, and asked the memo for the group only after; a covered scan of a
hundred records in E at ten million keys under 1.25 GiB opens seven groups
over two or three pages, and the profile put the view's parse and the
header line's first touch at a tenth of a covered scan beside the decodes
at half. The cursor holds the view across the page's groups now and asks
the memo before any view is built (`Options::zpages_cursor_view`;
`supdb-zviewperopen` is the view an open): the views over the row's scans
fell by half (63,800 against 117,400) and E read 1.07x in both reps of a
sitting. The decodes themselves were half a covered scan at 0.74 µs for a
1.6 KB group, which is a quarter of the decoder's warm rate, and the
profile put more than half their samples on the decoder's copy of its
compressed input: the input's lines cold at every group. The cursor asks
for the next group's lines at a group's open now -- the next group of the
page by the directory, else the first kilobyte of the next entry's page --
and a group's walk is lead enough to bring them
(`Options::zpages_group_ahead`; `supdb-znogroupahead` asks for nothing):
E read 1.21x in both reps of a sitting, with the arm's hot-raw policy
releasing one partition more than the default's, which should have helped
the arm. The first shape of the ask fell to the next page's lines after a
group the memo had held, since that open had built no view, so the view is
built at the ask where it is missing and held for the groups after. The
rules: a structure a walk rebuilds at every step is counted per step
against the steps the walk takes -- a view per open against seven opens a
scan -- before the step's own cost is read as small; and a decode's cold
figure is read for its input's misses before its branches, since a
prefetch of the next group's lines, a dozen instructions behind a walk
that is lead enough, moved E by a fifth where the probe over hot pages
had read the whole page's lines as a tenth of a decode.

**A prefetch read as a stall of its own instructions, and a pace that lost
to the burst.** The block walk's prefetch of the next copy block, forty-eight
lines in one burst at a block's start, had been priced by taking it away (E
1.23x in core with it) and stood; the profile of E at ten million keys under
1.25 GiB put 8.7% of the thread in it, and the source lines put fifty-nine
of its seventy-three samples on the prefetch instructions themselves, which
read as a burst outrunning the core's fill buffers, each prefetch past them
waiting a memory round trip. Paced through the walk of the block before --
four lines of each of the next block's three buffers every sixteen entries,
the whole by the forty-eighth and the rest at the walk's end
(`Options::scan_prefetch_paced`) -- the walk read E 0.90x in both reps of
the row's shape, with the arm's own hot-raw policy releasing a partition
more, so the burst stands and the pace is the arm (`supdb-pacedprefetch`).
The samples on the prefetch instructions were the lines' arrival, not the
issuing: a prefetch that finds no fill buffer waits for one, and whether it
waits at the block's start or spread through the walk, the lines cost what
they cost, while the burst at the block's start has the whole walk of the
block as its lead where the pace's last step had sixteen entries. Two more
things the same profile named were bookkeeping with the wrong shape. The
sparse form's search for a key within a cut's run walked the run by key, and
the keys past a partition's end all share its last cut, so E's inserts,
eight thousand above every key in the store, each walked past every one
before it; the run is searched by binary search now, with one compare for a
key above the last. And the retired form files' sweep at an operation's end
walked the reader table's two hundred and sixty-five slots at every
operation for as long as a pin held the files -- the hot-raw policy's
releases through a mix -- where the forms' and snapshots' sweep beside it
had a retry guard; the files are under the same guard now. The rules: a
profile's samples on a prefetch instruction price the lines it waits for,
not the issuing, and a lever that moves where the waiting happens is held to
the pairing before the reading is believed; a search within a run keyed by
one quantity is a search, never a walk, since a workload exists that puts
every key in one run; and a sweep at every operation's end is held to the
guard its siblings have, since a pin that stands for a mix makes the sweep
run for the mix.

**A profile's largest symbol, read for its callers before it was read as
anyone's.** The flat profile of E at ten million keys under 1.25 GiB put
22% of the mixes thread in `memmove`, and the reading was the harness's
copy of every value into its sink, the same bytes the comparator copies at
12% of its thread, so the lever built on it was the copy block's values
prefetched whole ahead of the walk where the first kilobyte had been
(`Options::copy_prefetch_whole`), built before the callers were known. The
profile with call graphs put the sink at 3% of the thread and the copies in
the scan path's own block builds (`BuildCtx::materialize` 6.6% with its
copies, `deltas_for` 5.3%) and the covered path's group opens (15.6%, the
decode 11.2% of it); the counted run put the writer's builds at 15,396
sparse, 29,538 clean and 619 copies over 158,000 scans, and 144,349 group
decodes for about 17,000 covered scans, nine opens each, with the memo
answering 7%. Paired in the row's shape the whole prefetch read E 0.95x in
one sitting and 1.23x in the next, the scan pass 0.95x and 0.89x, the
releases falling differently in each: unresolved, the kilobyte stays and
`supdb-copyprefetchwhole` is the arm. The rule: a library symbol's share
is attributed by its callers before a lever is built on it -- `memmove`
has every copy in the process behind it and names none of them -- and a
profile read beside the comparator's counts the symbols the comparator
lacks before the ones both have.

**A seed the shortcut beside it threw away.** The covered scan's start
seek found its rank through the fence and one group, and the first block's
walk then placed a second cursor at that rank: a second fence search, a
view and a memo hit for the group the seek had just opened. Seeded -- the
seek through the form file's cursor, kept as the partition's reader's and
resumed by the walk (`Options::zpages_seek_seeds`) -- E read 1.20x in one
rep of two and the scan pass 0.82x in both, with the form's dispatches
doubled (`zpage_scans` 10.06 million against 5.05): a drained store's scans
take the one-walk shortcut ahead of the block loop, which dispatched afresh
and left the seeded cursor to be closed unused, so every scan of the
drained passes paid the seek's placement and a second. A test of a handle's
scan over a drained covered store reproduced it, two dispatches and a view
where the shape before had one and none; the shortcut goes through the
seeded cursor now. The gate then found the same leak in the other
direction: a first block held as a copy, the promotion's, walks from the
copy and never through the reader, so the seeded cursor was closed unused
there too and counted as a form walk under a scan that walked none. The
seek hands its cursor to the walk, which seeds the reader only where the
first block walks through it -- a sparse block, a clean run -- and lets it
go otherwise with its counts filed and no walk counted (`pwalk_discard`).
The scan pass read 1.25x for the seed in both reps of
the sitting after, with the form's dispatches one a scan in both arms and
the memo's hits down by the second placements' (368,000 against 5.4
million over a sitting), and 0.87x in one rep of a clean third whose E
read 0.89x for the seed in both -- a spread the remembered pin's sittings
also showed, 0.97x one way and then 1.10x the other for a mechanism that
touches no scan, so the count was read as the verdict and the seed
shipped on. A fourth sitting in the row's whole order, the scrambled mixes
after the mixes, read E 0.97x for it and the scan pass level: E against the
seed in three sittings of four. A count stands for a verdict only where the
mechanism strictly removes work, and the seed replaces one placement with
another, so it is off and `supdb-zseekseed` is the arm. The rule: a structure handed to a walk is checked at every dispatch that can
take the walk's place and at every form the walk can take, and a count
that moves the wrong way under a lever names the path the lever never
reached, before any rate is read; a cursor that opened a group and walked
nothing is the seek's and not a scan's, so its close counts no walk.

**A bar taken for the policy's limit, and room the kernel was using.** The
hot-raw policy released five partitions in one rep and eight in another
while its trace showed 55-115 MB of room unused and the sixth partition
held cold at 1.09x the median partition's reads a key against a bar of
1.5x, so the bar read as the binding constraint and the room as E's gain
to take: the covered scans, eight decodes each, were 11% of E's scans and a
third of its thread. The bar relaxed to the median where the candidate fit
the room (`Options::zpages_hot_skew_room`) released eighteen partitions a
rep, and E read 0.72x in both reps of two sittings, D 0.86x, F 0.88x, with
C 1.23x and 2.0x and B 1.28x: E took 0.037 major faults an operation
against 0.0014 and read 153 bytes an operation at the device against 5.7,
the group's cache at its limit with 765 MB of raw partitions in it, and the
policy made twenty-one back moves, five swaps and six fulls across the
mixes. The room `memory.stat` showed unused was room the kernel needed for
what the policy itself had asked in, and a release's costs -- a 42 MB
device read, the form's drop, the eviction behind both -- land in the mix
it happens in, where its gain accrues over the mixes after; C won because
the partitions released by chance during the uniform read passes were raw
when its Zipfian began, which no policy can arrange. Off;
`supdb-zhotskewroom` is the arm. The rule: a policy's bound that reads as
the binding constraint is relaxed in a pairing before it is called slack,
and a release is priced by the costs that land in its window -- the device
bytes, the faults, the moves -- beside the rep's release count, since the
same engine with more releases is a slower engine in a short enough
window; and a lever that trades one lost cell for another has not won
either.

**A sweep that walked the table at every end a pin held it, and the pin
remembered.** The retired form files' and snapshots' sweeps at an
operation's end walked every slot of the reader table, in sequentially
consistent loads, at every operation for as long as a pin held what they
would free -- the upkeep thread's passes through E, hundreds of them, each
pinning the state a landing had replaced -- 3% of the mixes thread, and the
retry guard moved only the guarded arm's share. The sweep remembers the
slot the last walk found pinned below every tag and the epoch it was
pinned at, and reads that one slot until it moves
(`Options::sweep_remembers_pin`, `Readers::oldest_for`): exact, since a
later pin is at a later epoch and a later retirement has a later tag, so
nothing retired since the walk can be freed while the slot still shows the
epoch, and the answer is the one that walk gave. Paired in the row's shape
the shortcut answered 27,000-50,000 sweeps a sitting with no walk where
the shape before walked every one, and the rates read E 0.97x for the walks
in one sitting and 1.10x in the next, with the mixes both ways: the count
is the verdict and the rate unresolved, as the scan's one cursor was read;
`supdb-sweepwalks` is the arm. The rule: a check that re-reads a whole set
to confirm a blocker it found last time reads the blocker first, and a
guard put on one path's sweep is checked on every path that sweeps.

**A memo that took every decode of a walk that reads each group once.**
The memo of decoded groups took every group a cursor decoded: a buffer
recycled thousands of decodes ago and cold, an `Arc` allocated for it, the
displaced entry's freed, and the group's lines written into a slot the
walk would read once. A walk's groups after its first decoded into the
cursor's own buffer, the memo asked and not written
(`Options::zpages_walk_owns_decodes`), owned 31 million of the row's
decodes and read the scan pass 1.02-1.05x in three reps of four, E 0.98x
and 1.05x, C both ways: unresolved, so the memo's path is the default and
`supdb-zwalkowns` is the arm; the walk's groups after the first were
fourteen of twenty-two until the page's first group, opened by the
cursor's load and not its next-group step, carried the flag too, which the
test found by count. The rule: a flag that marks one step's opens is
carried by every path the step takes to an open, and an option is held to
the count that says its mechanism fired before its rate is read.

**A policy priced on the mixes it was built for, and paid for in the mixes
after them.** The hot-raw policy's room (`Options::zpages_hot_raw_slack`)
was paired in the row's shape up to its unscrambled mixes, and every pairing
read E ahead. The row runs the scrambled mixes after those, and the first row
banked with the room read the scrambled E at 0.59x of LMDB at ten million
keys under 1.25 GiB where the row before had read 0.86x. Paired through the
whole order, the room read E 1.10x and the scrambled E 0.76x, with 0.077
major faults an operation against 0.021, from sixteen releases a sitting
against four: the partitions the unscrambled Zipfian's head had made raw
stayed raw where no partition is hot, their records holding cache the form
files needed. A way back for a raw partition read under the median was built
against it (`Options::zpages_hot_cold_back`), and its first reading was in
two units. A raw partition's scan counted one read where the same scan over a
form file counted the seven groups it opened, so a raw partition under the
scans read a seventh as hot as it was, went back in the middle of E and was
released again a look later. Counted in the groups a form walk would open
(`Seg::hot_scan`), the way back still made fifteen to nineteen moves a
sitting. It read E 0.73x in one sitting and level in the next, and the
scrambled E 1.42x and 1.12x, while the scrambled E's faults stayed at
0.06-0.08 an operation in every arm with the room, with the way back or
without it. So the way back is the arm (`supdb-zhotcoldback`), the units are
a fix, and the scrambled E's faults were read as the room's -- they were the
way back's own, as the entry on a move made in one step says. The rules: a
pairing runs the row's whole order, through every pass after the cell it was
built for, since a policy whose state outlives the mix it was made in pays in
the mixes after it. And a policy's way back is a move with the way out's
costs, the device read and the drop, so a policy that flaps is priced by its
moves before its rates.

**A budget drawn as a share of the limit, binding under a store that
fit.** The block cache's own budget was an eighth of the memory limit, set
so that the cache would not grow to the store's size at ten million keys
and have the process killed. At three million keys under 1.25 GiB the
store and a cache of its own size both fit, and the eighth bound anyway:
after the lag sweep's full rewrite the pass's working set was 388 MB of
forms against a budget of 160. A bound cache sheds what it builds, so the
scans built 11,000 blocks in a pass of 3,000 scans. The builder ahead
started again at every merge landing and posted forms the scans installed
and then shed, and the scanner's frees of the builder's chunks met the
builder's allocations at the allocator's arena locks: 4,500 voluntary
switches. The pass ran 430-560 ms against LMDB's 27. The phase timers
named the install and the builds, a pairing of one arm at a time against
the default named the budget (the arm with no budget read the pass 12.6x
faster, the arm with no builder 4x), and a profile cut to the pass named
the locks. The budget grows now, at each crossing, into the room the
partitions leave of the controller's page-cache budget
(`Options::scan_cache_auto_room`), and binds only once that room is gone.
Paired against the eighth, the pass read 6.7x and 9.1x in two sittings and
the sweep's weighted sum 2.0x. It never fired at ten million keys under
the same bound, where the partitions fill the page cache's budget, nor in
the row's other passes at three million, where the cache never crossed.
What is left of the pass is its first scan, which waits 10-50 ms for the
upkeep thread's last fill of the burst. The rules: a budget for one
structure is drawn from what the process's other tenants measure to need,
never as a fixed share of the limit, since a share binds below the
working set of a store that fits as surely as it holds back one that does
not. And a cache bound below its working set is slower than no cache, so
a bound is priced against the arm without the structure as well as the
arm without the bound.

**A loss read on the thread's CPU when the thread was waiting.** The
scrambled E mix at three million keys in core read 0.84x of LMDB in the
row, and its first decomposition, with the counters on, put 43 ms of its
235 ms of scans in the settle of the scattered writes the scrambled
write mixes before it had left, filed by an exact seek a key into
blocks E mostly never builds. A filing by the ordered index's samples
was built for that (`Options::settle_lazy_raw`) and read 1.06x and 1.02x
in two sittings, under what a pairing resolves. The profile of the mix
undivided then read the scanning thread's CPU at 1.84 s against LMDB's
1.88 in windows of 2.47 s and 2.29, with 2,310 samples of merge threads
beside it, and the run queue's wait at 11 ms: not the CPU and not a
core, but blocking. The mix timed by kind put the gap in the commits,
710 ms against LMDB's 244 for the same 249 commits, where the scans
were within 5%, and a trace of the syncs read the commits' fdatasync at
0.81 ms in the unscrambled E and spread to 10 ms and past in the
scrambled one, beside the merges' writer helpers syncing 8 MB every
20 ms. A merge sync every megabyte (`Options::merge_sync_ahead`) was the
obvious lever on a device cache the merges fill between syncs; it made
seven times the syncs and moved the commits nothing, and cost the lag
sweep a tenth. The seals' and merges' threads at the idle I/O class
(`supdb-bgidle`), on a device whose scheduler honours the classes
(`mq-deadline`), read the commits slower still, 863 ms against 724 and
756 against 673: what the commits wait on beside the merges is neither
the device cache's backlog nor the queue's order, and is open. The rules: a mix's loss is timed by kind before its
thread's profile is read, since a profile of a thread that waits shows
its work and not the wait; and a lever on a device's queue is held to
the latency it is for, the commits', not to the count of what it
changed.

**A move made in two steps one way and in one the other.** The hot-raw
policy's release asks the kernel for a partition's raw pages and flips only
once nine tenths of them are resident; its way back attached the form file at
the look that decided it. A form file is advised random from its attach, which
is right for the point reads it was advised for, so a file attached cold came
in a 4 KB fault at a time under the scans it served. Every mix that took a
major fault at ten million keys under 1.25 GiB had a back move in it, and every
mix with releases alone took none: the scrambled E made two or three backs as
its block cache took the budget under the fill share, and took 0.07-0.08 faults
an operation, about two form files' pages. Two notes before this one had put
those faults on the room's releases and on the raw partitions staying raw; the
phase each move fell in, printed beside the mixes' fault counts, said otherwise
in one run. The way back asks for the form file's pages now and attaches the
file a look later, the raw form serving meanwhile
(`Options::zpages_hot_back_ask`; `supdb-zbacknow` is the move in one step),
and the scrambled E read 1.72x and 1.21x in two sittings with no faults. The
rules: a move between two forms is made in the same steps both ways, the pages
asked ahead of the flip, since a mapping advised for one access pattern reads a
cold file at the worst rate it has; and an effect put on a policy is read
against the moves the policy made, mix by mix, before it is put on the
policy's choice of what to move.

**A sum that traded a cell won for a cell lost.** The merge chain run
through a burst was turned off on the lag sweep's weighted sum, the burst's
time and twice the undivided pass's, which read it 6% behind: the burst
slower by a quarter, the pass faster by less than twice that. The row reads
neither the sum nor the undivided pass. It divides the sweep's passes by ten,
so the pass after the full rewrite is the tenth of its scans where the
chain's drain sits, and compares each cell with its comparator: the pass at
0.15x of LMDB, the burst at 29x. Re-paired on the row's own division, the
chain read the pass 3.1x and 3.0x in two sittings and the burst 0.7x, twenty
times LMDB's still, and the sum level. The rules: an option is priced on the
cells the bar reads, each against its comparator, and a sum over two of them
is read beside them and never in place of them, since a cell won twenty-nine
times over has room to give that a cell lost at a seventh does not; and a
verdict is taken again when the division or the order it was read under is
not the row's.

**A structure kept for readers, reclaimed first because nobody had read
it.** The form files a merge's finish builds are kept in the page cache for
the scans to come, and the burst before the lag sweep's full-rewrite pass
took a third of them back out: a page written once and never read sits on
the kernel's inactive list, the first reclaimed, whatever the store meant
it for, and the burst's last seconds of writes -- pieces, merge outputs, the
log -- were newer. The pass then read eleven form files of twenty-nine cold,
a 4 KB fault at a time under an advice of random, 97% of its major faults,
while the first reading had put them on the pieces; the faults by file and
the cache's residency by kind at the pass's start (`scripts/resident.py`)
said where they were in one run each. The form files under nine tenths
resident are asked for whole when scans come back after a quiet stretch,
once no merge is in flight (`Options::zpages_rewarm_scans`), since asked
beside the chain's last merges they were pushed out again and refaulted
fivefold. The rules: under a bound, what the store writes for its readers
is asked for again when the readers come, because the kernel ranks a page
by its reads and not by its writer's intent; an ask for pages is made when
nothing is writing beside it, or it pays twice; and a test of a trigger
that counts scans counts the writer's, since a handle says so once a
commit.

**A shortcut taken where the structure being decoded agreed, beside a
structure in memory that knew.** A covered partition's settle files a
written key under its block with the cut left for the build, and asked the
form file which block: the fence and the page's directory name the group a
key falls in, and the answer was a block only where that group lay inside
one block of sixty-four records. A group straddles a block's edge often
enough that the rest of a burst's keys fell to the exact seek, which opens
the group and decodes it, so the upkeep thread's passes over the lag
sweep's one-percent burst at ten million keys under 1.25 GiB ran long
enough that the first scan after the burst waited 89-172 ms to take the
upkeep back, with the decoder and the page views a third of the scanning
thread. The ordered index's top level holds one sample a block, in memory,
and names the block of any key whose head ties no sample without reading
the form; asked first (`Options::settle_covered_samples`), the thread kept
up, the scan waited a millisecond at most and the pass read 1.46x and
1.72x in two sittings. The scan's own counters named the wait (`take` in
the lag counters) and the profile named the decode, where the pass's time
alone read as the settle's. The rules: a shortcut valid on a subset is
priced by how often its input falls outside the subset, since every miss
pays the full path; a question one structure answers from memory is asked
of it before a structure that has to be decoded to answer it; and a wait
for a worker is read as the worker's slowness before the waiter's own work
is profiled.

**A bound written for one kind of commit, left off its sibling.** The
pace that holds a buffered commit until the upkeep thread trails by at
most a share of the store was written for the commits that would
otherwise file their batch themselves, and a syncing commit, which lends
every batch so its thread files through the fsync, was given no bound at
all: the thread trailed by whatever the burst left it, and the first scan
after the lag sweep's full rewrite at three million keys either waited up
to 40 ms for the pass in flight or settled up to a hundred milliseconds of
the residue itself, three fifths of a pass LMDB ran in a third of the time.
The lag counters said which (`take`, `settle`), in every rep. Paced in core
(`Options::upkeep_pace_syncs_bp`) the pass read 2.4-3.3x in two sittings
and the bursts gave a seventh to a quarter of their rate and stayed ahead
of RocksDB's. Paced out of core it lost: there the thread's resolves are
major faults, and the hold held the writer to them, the one-percent burst
at a third of its rate and under RocksDB's. The rules: a bound on how far
a worker may trail is set on every path that hands it work, since the
reader after the burst meets the trail whichever path made it; and a bound
that holds a producer to a consumer is priced in each regime the
consumer's rate changes in, since a hold is only as cheap as what it waits
for.

**A trade priced while two other losses stood in its cells.** Two merges
in flight paid the full rewrite's merge debt sooner under a bound and
read as a trade: the full rewrite's pass ahead, the clean store's pass and
the one-percent pass behind in three reps of four. The losses were not the
merges': the clean store's pass met form files the burst had pushed cold,
and the one-percent pass's first scan settled through the form's decodes.
Each got a lever of its own, and the same pairing then read every lag pass
but the tenth ahead, 1.14-1.88x, in two sittings, and in core 1.13-1.80x.
The rule: an arm left as a trade is priced again once the effects it was
traded against are fixed, since the verdict was a reading of the cells,
and the cells had more in them than the arm.

**A seam that priced one of a path's two costs.** The reading made from
the form files on disk prices the form by timing a few of its pages'
decodes, and the test seam pins the decode (`ZInject::set_decode_cost`)
since the line to enter is a function of it; the same timing's faults
priced a fault at their own wait, and the seam's fault cost was read by the
census and not there. A store two percent past its budget, which the
attach test holds to waiting for reads, was bought on the prediction once
in a gate run and in none of thirty-three runs after it: the files' pages
had been dropped behind the builds, the timed decodes faulted, and under
the gate's load a stalled fault raised the price of a fault until the line
to enter fell under the model's saving. The reading prices its faults at
the seam's cost where one is set, publishes the price it carried
(`sample_t_f_ns`), and the test puts the files out of the cache and holds
the price to the seam's. The rule is the one the decode's seam was written
for, read across the path: every measured cost a test's premise depends
on goes through the seam, and a path that takes two costs from one timing
takes both from it.

## Standing limitations

These are the engine's, as opposed to refuted predictions. Each is a curve
in the suite's figures, at `full` scale where it says so:

- The lag pass after a full rewrite under a bound runs beside what is left
  of the merge chain that rewrites the store once. The chain runs through
  the burst out of core (`Options::merge_chains_writes`), so the pass meets
  the burst's last merges and a third of the pieces, and still trails LMDB,
  whose burst rewrote in place and took twenty times ours. The idle I/O
  class for the merges and their inputs asked ahead were priced on it and
  refuted (`docs/engine.md`); what is left is the drain at the burst's end,
  and the levers on it are the chain's pace through the burst and what a
  pass pays for each piece it meets.
- Out-of-core reads run at the speed of their misses. Once the file exceeds
  the memory that can cache it, every miss is a synchronous page fault and a
  rate is the faults an operation takes over the device's latency. The faults
  an operation takes are the engine's to cut, and it takes more than LMDB with
  readahead off (the `ooc` scale, `bench/DESIGN.md`): a lead, not a limit --
  see the third consequence under "The suite lives in bench/".
- The durable ordered load trailed LMDB and RocksDB until ordered ingest
  went straight into segments; the suite's rows since say where it stands,
  and shuffled arrival still inverts both. Quote the pair, never one: the
  `load` and `load-shuffled` figures.
- The index layout study found smaller and faster points on the frontier
  that the shipping layout does not occupy (`docs/index-theory.md`).
