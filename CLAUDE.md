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
| `src/db/zfile.rs` | the compact form's file on the engine's side: the builder that writes a partition's form file from the partition's own bytes, the attach that binds it to its segment by content, the cell a segment holds it in, the point read, the count, the scan walk and the cursor through it, behind `Options::zpages`, and the controller's platform side, the cache budget read from the cgroup and the test's seam for it; the block cache's reads of a covered partition go through it too (`Options::zpages_block_cache`); and the stride form (`Options::zpages_stride`; `zpages_stride_room` keeps it to the builds made while the store in it fits, `zpages_stride_convert` moves a served partition between it and the block form, and `zpages_stride_below`, an arm's, keeps it to the partitions under a key), a partition's values alone at their stride where its keys are one run of numbers -- the module's own docs, and `docs/engine.md` for where it wins and where it does not fit |
| `src/db/dirkey.rs` | a live store's directory under a key, encrypted by the kernel (`Encryption::Kernel`): the requests of Linux's file encryption, what `create` and `open` hold a directory to, and `lock`, with no cipher and no byte of a read path in it -- the module's own docs, and `scripts/fscrypt.sh` for a filesystem to test it on |
| `src/db/vault.rs` | a live store under a cipher the caller supplies (`Encryption::Cipher`): the door every file operation of the engine goes through, which is the standard library's call for a plain store; a file as a memory file while it is written and as its envelope in the directory after; the marker and the keys, the caller's wrapping the store's and the store's wrapping a key a file; the manifest's slot and the log's record sealed, with no cipher in the crate -- the module's own docs, `tests/cipher.rs`, and `ZFUZZ_CIPHER=1` for the differential fuzz |
| `src/db/pager.rs` | the cache a store under a cipher reads its files through: a file as address space its plaintext's length, a chunk of it decrypted where it is first asked for and a megabyte given back at a time once the cache is past its budget and no reader could hold it; the holds and rests of the threads that read with no slot of the reader table, the two lines past the budget at which the store's own work and then a caller's read wait, and the failure a read remembers, with no cipher in it and no reader of the format -- the module's own docs, and `tests/cipher.rs` for a store several times its cache |
| `src/db/zselect.rs` | the compact form's selector: `ZSelect::step`, a function of its state and an explicit `Input` (no clock, thread or file), the one definition of the transitions between FITS, SUSPECT, building, COMPACT and the trial, the replays that hold it, the model of a cache's faults over populations of pages (`model_populations`), and what the reads of a store in two forms cost with the moves between the forms that pay (`forms_cost`, `conversions`) -- the module's own docs |
| `src/format.rs` | the on-disk format's fixed quantities, owned by no writer |
| `src/block.rs`, `src/index.rs`, `src/flatindex.rs` | the format itself: blocks, extents, the flat key index -- `docs/index-theory.md` |
| `src/bytes.rs`, `src/blob.rs` | the read path over any byte source, one that lends whole, one that copies, and one that lends by the piece it is asked for (`Bytes::piecewise`, `flatindex::Pieces`); compiles for wasm |
| `src/crypt.rs` | a finished file published encrypted and read through the `Bytes` seam: the envelope, the hook a caller's cipher fits (`Aead`), `Encrypted` as a byte source over the stored object and `encrypt` as its writer, with no cipher in the crate; compiles for wasm and nothing in the module links it -- the module's own docs, `examples/crypt.rs` for the adapter, and `web/crypt.mjs` for the browser's half |
| `src/wasmapi.rs` | the C ABI the browser calls; hand-written because the module's size is budgeted |
| `web/` | the browser reader, its byte sources, the fetcher that decrypts, the Worker it runs in, and the size control -- `web/README.md` |
| `tests/` | the engine's contract, the read paths held to each other, the format's damage cases, a checkpoint held to the store it was made of, a crashed store's runs held to its log; `tests/common` holds what more than one file needs, and `tests/data` the envelope on record |
| `bench/` | the benchmark suite: its own cargo workspace, the arms, the runner, the gate and the figures -- `bench/DESIGN.md` |
| `tests/source.rs` | the gates over the tree's own text, over a lexer in `tests/lex/`: the wall between the forms and the encryption modules (`tests/data/couplings.txt`), what the door's clippy run cannot see (`tests/data/door.txt`, `door-libc.txt`), every public entry point of `Reader`, `Db` and `Blob` to a cost probe or a reason (`tests/data/cost-entries.txt`), the seal's tokens to the calls they are made for, and every workflow under `.github/workflows` to the groups of `scripts/check.sh` |
| `scripts/door/` | the door by symbol: a clippy run of its own over the library with the barred calls in `clippy.toml` (`run.sh`, in the `lint` group), and its red input (`red.sh`) |
| `scripts/cost.sh`, `cost-diff.py`, `cost-hot.txt`, `cost-red.sh` | what a store in the clear pays, counted: the probes built at a base and at the tree under callgrind and compared by function, and the count's red input -- the `cost` group |
| `scripts/ratchet.sh`, `ratchet.py`, `raises.txt`, `ratchet-red.sh`, `base.sh` | the lists that only go down, held across a change against the base `base.sh` takes from the event, a raise declared in `raises.txt` -- the `lists` group |
| `scripts/seal-red.sh`, `seal-red-red.sh` | the seal's red input: a copy of the crate in which each whole borrow the read path keeps compiles in its own module and fails abroad, and that check's own red input -- in the `lint` group |
| `experiments/ffi/` | what a C ABI and a Python binding cost a point read, borrowed against copied, beside LMDB's and RocksDB's C APIs; its own manifest and dated results, and not built by the gate -- its `README.md` |

One writer produces the format, `SegmentWriter`, with two callers -- `Db`
when it seals or compacts, and a caller with sorted write-once input -- and
three readers parse what it wrote: `Blob` over a mapped file, `Blob` over a
copying source, and `SparseBlob` over ranges. That is why `format.rs`
belongs to none of them.

Six modules carry a blanket allowance of the door's two lints
(`clippy::disallowed_methods`, `clippy::disallowed_types`), and they are
the door: `vault`, `bytes`, `pager`, `crypt`, `dirkey` and `blocks`, the
modules that open, map, advise and sync files and read the process's own
counters for everyone else. An allowance anywhere else is an item's, with
its reason, and `tests/data/door.txt` counts it. No other module carries a
blanket lint allowance; `block` and `flatindex` allow `dead_code` on the
wasm target only, where their writer halves have no caller. Nothing is
exempt from the format gate, and everything holds to `-D warnings`.

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

`sh scripts/check.sh` runs every group -- build, test, stride, lint, wasm, bench,
lists -- and CI calls the same script with the same names, so a green run
here is a green run there. tests/source.rs reads every file under
`.github/workflows`: a workflow that runs on a change's event (`ci.yml`,
and `apple-silicon.yml`, which runs the `machine` group where a pull
request changes the machine capture) is held to run steps that are a
group's call or set-up on its short list, one that checks no change (the
measurements, the rows' proposal) to triggers that are no change's -- by
hand, on a schedule, after a merge to main -- and one it does not list is
red, as is a group no job runs and a line of a set-up script that writes
the job's environment unlisted (`$GITHUB_ENV` is a way to set a check's
knobs from outside it). The first version read `ci.yml` alone, and review
found `apple-silicon.yml` checking pull requests by hand beside it: a
check whose scope is one member of a set passes the next member, so it
reads the whole set and fails a member it does not list. Two checks the
YAML used to make itself are groups now, `cross-arm` (the linker, the
qemu runner and the `--cfg` the job's environment carried) and the cost
job's base, which `scripts/base.sh` decides from the event's values for
every check that compares the tree with the commit before it. The lists
that only go down -- `tests/data/couplings.txt`, the door's two,
`tests/data/cost-entries.txt`'s reasons -- are held across a change by the
`lists` group against that base, since each list's test sees one tree: a
raise is declared in `scripts/raises.txt` and matched exactly, where it was
once a CODEOWNERS line, a check that ran on nobody's machine. The workflow's
test first read the `run:` lines and nothing else, and review found three
ways round it that it passed: an `if: false` on the step or the job, which
left the group counted as run; a `continue-on-error`, which reads a red
group green; and an `env:` that set the base (`SUPDB_BASE: HEAD`, the tree
against itself) or narrowed a group (`RATCHET_RED_STEPS`, `COST_PROBES`).
It reads every key of the workflow now, against lists of its own, and the
second review found the reader of keys keyed on the parent's name: a job
named `inputs` took any key as an input's, its `if: false` among them, and
its group counted as run; the triggers were read by their keys alone, so a
workflow that ran on no pull request read green; and the groups' `case`
ended at the first `esac` at any depth, so a group after a nested case was
read by nobody. A key is read by the whole path to it. The lists'
check had the same shape one level up: it held each list the tree's
registry named, and a change that moved a list from the registry to its
not-held table, with a raise beside it, read green; the registry is held to
the base's now. A check over one kind of line passes everything written in
another kind, and a check whose scope is written in the tree it checks is
held to the scope at the base. And the lists' check answered by its
base: the first raises a change declared (two cost entries for test seams
a merge of upstream brought) read green against the branch's tip before
the change, which is a push's before, and red against the main line, which
predates every list -- there a list is new, nothing of it is held, and the
declarations matched no raise. One raises.txt is read under both events,
so a line for a list the base does not have holds nothing and is said so
(`ratchet-red.sh` has the case, and the same line red against a base that
has the list). A check that takes a base is run against every base an
event can hand it before it is called green. Every group denies warnings, the wasm group since it was
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
timing run needs the machine to itself, and CI gives it a job of its own. The
`lint` group is the engine's alone: the suite's clippy and its format
check run in the `bench` group (`sh bench/scripts/check.sh lint` by
itself, a minute), and a commit that touched `bench/` alone went out
behind a green `lint` with a doubled `#[allow]` that clippy refuses.
The `cross-arm` group died in two runs of five with "QEMU internal
SIGSEGV" and no test failed, and the first reading put it on the
tests added since the last green run. It was the shell's
`RUST_BACKTRACE=1`: a test that panics on purpose has its backtrace
symbolized, the symbolizer reads `/proc/self/maps`, and qemu-user 8.2
fakes that file from its own record of the guest's mappings and crashes
reading it while another thread maps or unmaps -- a C program of four
threads and no engine dies the same way every run, and the same tests
with backtraces off never do. qemu's `-strace -D` named it in one
batch, the test binary run straight under the emulator with no cargo in
the loop (eight of twelve dead there against two of five through
cargo), and the group sets `RUST_BACKTRACE=0` for its run. An
instrument's crash is traced to the calls in flight before it is read
as any test's, and a variable of the caller's environment is one of the
inputs a group fixes for itself. Review then found two ways round the
variable that it passed: the library's own code reading the file or
making a backtrace -- a unit test that formats `Backtrace::force_capture()`
killed one run in twelve under the fix -- which tests/source.rs now
refuses by line (`the_library_reads_no_maps_and_makes_no_backtrace`); and
the variable moved after `--lib`, where cargo takes it for a test-name
filter and the group ran no test and exited 0, so the case holds its run
to `0 filtered out`. A fix that rests on an input is held where the input
can be undone, and a group's verdict by exit status is held to the count
of what it ran.

The forms and the encryption are kept apart by two gates, so that a change
to a form needs no change under the cipher and one that would be wrong
under it fails in the clear. The wall: `tests/source.rs`
(`the_forms_and_the_cipher_do_not_name_each_other`) counts the tokens by
which a form module names the encryption (`vault::kept`, `ATTN_PAGED`,
`piecewise`, the word `cipher` and their kin) and by which an
encryption module names a form, by file, token and region, against
`tests/data/couplings.txt`: a total above the file's is growth and red, one
below it is red until the file is lowered in the same change, and a
coupling moved between functions changes no total. Its tokens are a list,
and each spelling review found it passing is a test of its own -- a
renamed re-export at the crate root, an alias of the crate, a vault call
written as a path or broken across lines, a macro the encryption side
exports -- but a branch on the cipher in words the list does not have
passes until the list grows. The door: no file, mapping, advice, sync or
process read in the library outside the six modules above, by symbol --
clippy's barred calls over the library under the checked profile on Linux
(`scripts/door/run.sh`, config `scripts/door/clippy.toml`), red first by
`scripts/door/red.sh`, which writes a barred call of each family into a
copy of `zfile.rs` and requires each refused by its own entry. It
replaced a unit test that read `db.rs` for the calls by text, which never
read `zfile.rs`'s `File::open`, `File::options()` and `fs::read_to_string`:
a check by text over one file passes the next file and every alias, where
clippy resolves the path. What clippy cannot see the source test holds by
text against `tests/data/door.txt` -- an item's allow and its reason, a
`/proc` path, an `extern` block, code a `cfg` compiles out of the door's
build -- and the barred libc calls are held to the libc Cargo.lock
resolves, read out of cargo's registry
(`every_libc_call_that_takes_a_string_is_barred_or_said_to_touch_no_file`),
so a vendored or offline layout fails that test outright. Threads are not
barred yet: `std::thread::` is counted by file there, and grows red.

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
removed it. Spell that `TMPDIR` canonically: the tests that find a
store's mappings match `/proc/self/maps`, which names the resolved path,
by the store's directory as a prefix, so a `TMPDIR` written with a `..`
in it failed four zpages tests and the leak test every run, five of five
alone, with "one form file mapped: []". A pairing's arm is the other
way the allowance is found spent: `bench ab` at ten million keys exited
1 after the comparator's sweep with its own store left behind and
nothing in its table or on its stderr naming the cause, at zero bytes
free; the three gates of the day had left fourteen hundred test stores
in the private temp directories (1.3 GB), and the bench workspace's
`target` held 14 GB, 3.6 of it the checked profile's example binaries,
which the gate's bench group rebuilds and nothing reads. Read `df` of
the temp volume before a timing run, and delete the test stores and the
checked examples first.

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

A change to an engine default lands only once `sh scripts/check.sh`
has passed with the new default in place.

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
runs before anyone read the function list. Read a recursive function by
its self rows: callgrind's inclusive figure for a recursive quicksort
counts each level's children again, so the settle's sort of its pending
writes read a fifth of a settle's instructions at ten thousand keys, a
radix was built to replace it, and the settle came out 0.9M of 48M
instructions lighter, the two percent the self rows had said. And valgrind runs one thread
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
the trailer), and a sync handed to a helper is hidden only while nothing
needs the file before it ends: the log's pre-write moved to a helper
after the buying commit's sync was waited out whole by the next commit,
which a write mix makes a few hundred microseconds later, where inline
its zeros had ridden the commit's own flush (`Options::wal_ahead_beside`,
refuted); a write to the file under a concurrent fdatasync stalls
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
its branches, and the lever on those is a loop with none on the common
sequence: the one length byte read whether the token asks for it or not
and taken by a select, the run and the match moved a fixed sixty-four,
read a cold group in three quarters of the branching loop's time and a
learned one in more than its, and every decode the engine makes is a
cold one, so the scans read a fifth to a quarter faster with the same
decodes counted (`Options::zpages_flat_decode`). A loop priced on its hot
figure would have been refuted. And a decode removed is priced by what it cost: raw tails on
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

The threaded scan passes have a counters line of their own
(`SUPDB_SCAN_COUNTERS=1`, a `scancounters` line a pass, before the mixes
and after them, the mixes' line's fields): the scans a form file served,
the groups they decoded, the blocks the handles built and the keys left
unsealed, which is how the pass after the mixes was read as the same
decodes down another path. Its read before a pass takes the writer's
upkeep home, so the pass after it meets a store nothing is filing into,
and its rates are not a row's. `SUPDB_SCAN_MIXED_FOLD=1` folds the store
before the scans after the mixes, a probe: the same files with nothing
overlaid, for telling what those scans pay for the path from what they
pay for the store.

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
epoch and keeps the memory until it leaves. A segment has one of the
same kind: its ordered index reads its heads through a pointer the
segment work moves between the index's mapping and a copy of them in
the process's own memory (`Options::heads_owned`, `Maint::z_heads_look`),
the copy whole before the pointer names it, and a copy given up is
retired under a fresh epoch and freed only past every reader pinned
before it, by the look that gave it up; every thread that seeks an index
holds a pin, which is what the rule rests on, so a seek from a thread
that pins nothing is a seek of freed memory. Everyone
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
And the share is the first scan's to pay: a paced batch is held now
until the thread has filed it (`Options::upkeep_pace_holds`;
`supdb-ingestpacelends` lends it within the share), since the trail is
a linear trade -- each write of it saves the burst its filing at every
batch and costs the first scan after the burst its filing once -- that a
sum over the burst and the pass reads lending's way at any depth, while
the cells read it the other way: at a hundred thousand keys the share is
one batch, the first scan after the full rewrite paid the pass in flight
and the batch after it, and the pass read 0.9x of `lmdb-nosync` with the
burst at 2x; held, 1.2-1.3x and 1.5-1.6x.
`supdb-ingestlendbursts` is the arm that lends every batch and
`supdb-ingestnopace` the one that files every batch. A syncing commit
lends every batch, since its thread files through the fsync, and in core
it is paced too, within a quarter of a percent of the store's keys
(`Options::upkeep_pace_syncs_bp`; `supdb-nopacesyncs` lends unpaced):
unbounded, the thread's trail at the end of a full rewrite was the first
scan's to wait out or settle, the pass's largest part. Out of core it is
not paced, since there the thread's resolves are major faults and the
hold paced the writer by the device; out of core is the compact form
installed, not the controller's census, which a full rewrite in core
can enter (`Options::upkeep_pace_syncs_census`).
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

**The envelope is around the file, and nothing inside the format knows
it.** `supdb::crypt` wraps a finished file: a header in the first page,
then the plaintext in chunks, each encrypted on its own under the caller's
cipher, read back as a `Bytes`. That is why it cost the readers no change,
and it is the constraint. The crate ships no cipher -- the dev-dependency
on `ring` is the tests' and the examples', a caller like any other -- and
the wasm module links none: the browser decrypts at the fetch with
WebCrypto (`web/crypt.mjs`, a wrapper round the fetcher every source
takes), so nothing of `crypt` is in the module (a build with its names
kept has no symbol of it; the size moves by tens of bytes with what LTO
inlines elsewhere, which is not the module growing), the plans stay
plaintext ranges, and what the cache keeps in OPFS is plaintext. The layout is
therefore written down twice, in Rust and in JavaScript, which is what
this file says not to do with a superblock constant;
`tests/data/crypt-v1.bin` is what holds the two to one answer.
`tests/crypt.rs` requires `encrypt_with_id` to write exactly those bytes,
and `web/crypt.test.mjs`, which the wasm group runs under Node
(`scripts/deps.sh` readies it), requires `crypt.mjs` to read them, so a
layout change once this has merged is a new version and a new file beside
that one, never that file written again. An `Encrypted` verifies its first
chunk at open, so a header is never believed before a tag has covered it,
`len()` included, and a wrong key fails there; every tag covers the header
and every nonce is the object's id and the chunk's index, which is what
refuses a chunk moved, borrowed from another object or cut off the end; the
zeroes between the header and the first chunk are under no tag and are
checked to be zeroes. A nonce repeated under one key gives both objects
away, so an id is drawn per envelope and `encrypt_with_id` is for tests.

Every choice in it that is not the cipher's was made on
`cargo run --release --example crypt_cost`, which reads one segment plain
and through each kind of reader in one process, and the first version,
pushed before that probe existed, was wrong in each of them. It decrypted
at every read, with the thread's last chunk remembered, on the reading
that a `Blob` copies its index once at open and a lookup touches the
source rarely: a point read of a run in a block read the source at every
call and paid two chunks' decryption for it, thirteen times the plain
read for a short run and near eighty for a run of a value or two at the
smallest chunk, and worse at every larger one. A reader keeps what it
decrypts now (`ReadOptions::keep_bytes`, unbounded by default): a chunk
goes once into an anonymous mapping the plaintext's length and is lent
from it after (`slice_at`), so `Blob::zero_copy()` is true over an
envelope and a warm read costs the plain read's time; the bound of zero
is the reader for an object larger than memory, and what is kept is
anonymous and the kernel's to reclaim never. Its test cipher was a
portable AES-GCM at a sixth of the rate `ring` reaches on the same
core through the wide AES instructions, and the first touch of a chunk
is the cipher's rate and little else: the adapter an example shows is
the one a caller copies, so it is the fast one. Its chunks were whole
pages of plaintext with the tag after, 4112 stored bytes, so every chunk
lay across two pages of the file and a cold read faulted two where the
plain file faults one; a stored chunk is the power of two now and the
plaintext in it sixteen short, and from cold with random advice a read
through an envelope takes the plain read's faults at the default chunk
and two to two and a half times them at 16 KiB and up, which is what the
default was chosen on. That left a chunk found by a divide by 4080 at
every call, which is a multiply and a shift instead (`Per`): built on
the divider's latency as this file has it and not priced here. A lent
section is asked for again at every lookup (`Sec::get` calls `slice_at`
each time), so a borrow that walked its chunks' states would cost what
it saves: a reader with every chunk kept answers on one
load, and before that the longest ranges lent are remembered whole
(`Kept::known`), the longest kept when a fifth comes, since a run of
values must not put the key section out. The browser's wrapper asked for
the header alone before its first fetch, a round trip an open did not
have; the header rides in the first request now, sized for the whole
chunks of the default chunk -- sized first by the bytes wanted and their
tags, it fell short of the last chunk's end and asked twice anyway. And
the writer made a write a chunk; it hands its sink a megabyte at a time,
for the folios the page cache makes of it. With a fast cipher what is
left of a first touch is the fresh page the plaintext is kept in, which
costs more than decrypting into it: a long range's pages asked for in
one call before its chunks are filled took a quarter off the open of a
`Blob` over an envelope and is the default (`ReadOptions::populate`,
never past what the reader has room to keep), and huge pages under the
plaintext added four tenths to the open and to the first pass of point
reads, the kernel zeroing two megabytes for a chunk of four kilobytes
(`ReadOptions::huge_pages`, refuted and off). The rule: a second way to read is priced beside the
first, in one process, on every operation the first is measured on,
before its default is chosen; and the order of the costs is read off the
probe before any of them is tuned -- the reader's shape was up to
eighty times, the cipher six, and the layout two by its count of pages.

The suite has the arms the probe could not be: `supdb-segment`, a
write-once segment read through `Blob`, beside `supdb-segment-enc` and
`supdb-segment-encbounded`, the same file as an envelope through the
reader that keeps and the one that keeps nothing, and all three again
with every value in a block (`bench/DESIGN.md`). They found two things
the probe had not. The reader that keeps nothing remembered one
decrypted chunk a thread, and a value in a block is read as the piece of
the block it lies in, which lies across two chunks more often than in
one: a walk of a block's values asked for the same two chunks turn about
and decrypted both at every value, a scan at a twenty-sixth of the
keeping reader's rate, where the probe's scan had read 1.6x. A thread
remembers eight now, each in the place its index names, and the same
pairing reads the scan at a quarter and the point read at a quarter,
which is its two chunks a read. And the first envelope arm wrote its
plain file beside the envelope, the segment on the device twice, at half
the plain arm's load: the plain file goes to memory now (`/dev/shm`),
which is also the only way the plaintext is never on the device, and the
load still reads six tenths, the segment written and then encrypted in
two passes with the reader's open decrypting the index after
(`ROADMAP.md`, an envelope written in one pass). Through the keeping
reader the suite's own passes read level with the plain file where the
values are inline, and the first pass of point reads over values in
blocks a tenth lower with its p99 three times, each chunk's one
decryption falling in that pass. The pairing of the two readers at a
million keys was stopped at forty minutes of ninety: a threaded scan
pass is `size` scans a thread, so an arm four times slower there is
priced at a hundred thousand.

**A live store under a key is the kernel's, and the engine's part is to
refuse.** `Encryption::Kernel` does not encrypt anything: `Db::create` hands
the kernel the key and sets Linux's file-encryption policy on the store's
empty directory before it writes into it, `Db::open` holds the
directory's policy to the key it was given, and from there every file
the store makes is ciphertext on the device and plaintext in the page
cache (`src/db/dirkey.rs`). That is the whole reason it is done this
way: a store reads by lending from its files' mappings, the page cache
is what holds those bytes, and the kernel is the one place a file can be
decrypted into it. Encryption the engine did itself would put every byte
a store reads into memory of its own, as the envelope's reader does for
one finished file, and take the advice, the residency counts and the
out-of-core accounting with it. What the engine owes is the refusals,
and each is a test: a store given a key where the directory cannot be
encrypted writes nothing; a key given to a store in the clear is refused
before a byte is read, since the store would go on in the clear under a
caller who believes otherwise; a directory under another key is refused;
and an encrypted directory whose key the kernel does not hold is refused
by name when opened with none, because its files' names are encrypted
too and it would open as a store of nothing. The key is not kept in the
options past the call, and a store does not give it up at its close:
another store or another process may be open under it, and a key given
up under them is their next file refused.

The tests are in two halves because a test cannot make a filesystem. The
refusals run wherever the tests do. The rest wants a directory on a
filesystem with the feature and the file that filesystem is in, to
search what the device holds for the store's keys, values and file
names: `scripts/fscrypt.sh up` makes one in a file behind a loop device,
`scripts/check.sh fscrypt` runs the ignored tests of `tests/dirkey.rs`
on it and asks for each by name, since a run of no tests is green, and
CI has a job that does both as the runner's own user. Three things it
taught. A key is given up through a descriptor, and `lock` asked through
the store's own directory, which is a file under the key held open for
the asking: the kernel answered that files were busy every time, and the
filesystem's root is the descriptor to ask through. A mutant that
skipped the encryption wrote a store in the clear onto the test
filesystem, and every run after it found that store's keys on the
device, in blocks a removed file leaves behind: the device remembers, so
a red run of that test is followed by `fscrypt.sh down` and `up`, and
the search has a control, a file written in the clear beside the store
that it has to find. The same test searched the device for the store's
file names and went red in the gate after a pairing had put a plain
store on the filesystem: every store's files have the same names, so a
name on the device says nothing about this store, and the names are
held by the locked store's listing, which is this store's alone. And the pairing: `supdb-dirkey` against `supdb`,
both on that filesystem, read level on every read, scan and read mix in
both sittings, and at half on every write that syncs, the buffered load
at a quarter. A bare write and `fdatasync` in an encrypted directory and
a plain one, no engine in it, read the same 2.4x, and `/proc/crypto`
named the reason: `xts(ecb(aes-fixed-time))`, a kernel built without the
AES instructions (`CRYPTO_AES_NI_INTEL` is not set in this guest's), a
software cipher at a hundred megabytes a second where a kernel that
names `xts-aes-aesni` runs gigabytes. So the write cells of that pairing
are this kernel's and nobody's, the read cells are the design's, and the
price on a kernel with the instructions is not known (`ROADMAP.md`). The
rule: under the kernel's encryption a write rate is read after
`/proc/crypto` has named the cipher's driver and after a floor with no
engine in it has been taken in the same two directories, since the
cipher is the kernel's and its speed is a property of the build.

**A live store under the caller's cipher reads its files by the piece
through a cache of its own, and what the engine owes is the door and the
asking.** `Encryption::Cipher` is the
second way a store is kept under a key, for a caller who brings the cipher
(`src/db/vault.rs`). It was asked for first and built second: the caller's
cipher was the plan from the question that started this, the envelope gave
it to a finished file, and the live store was then handed to the kernel
because that kept the read path, without anyone being told the plan had
changed. The two are not one feature done twice. The kernel's keeps the
page cache and wants a filesystem that can encrypt; this one goes wherever
Linux does, authenticates every byte, leaves files that are ciphertext
wherever they are copied, and reads through a cache of plaintext of a
size the caller sets.
They are one option with three values (`Options::encryption`:
`Encryption::None`, `Kernel`, `Cipher`) and were two for a day: a key
beside a cipher, each an `Option`, with a test on an encrypting filesystem
that a store under both read as itself. It did, and it was the wrong
thing to have made possible. A store under both hands the kernel bytes
that are ciphertext already, so every byte it writes and every byte an
open reads is encrypted twice for a device that learns nothing more from
the second, and the two switches read as two layers to be stacked where
they are two answers to one question. The choice is an enum, a store kept
one way is refused when it is opened the other (`tests/dirkey.rs` holds
both refusals, the cipher's inside a directory the kernel encrypts
already, which is the filesystem's business and stays possible), and the
test that composed them is gone. The refusal's own first test passed with
the refusal deleted: the kernel's key opened the directory, the open went
on, and failed further in at a manifest it could not parse, an error all
the same and for no reason anyone would recognise. The test asks for the
refusal's words and a directory left as it was. The rule: two options
that answer one question are one option; a test that two things compose
is first asked whether anyone should want them to; and a refusal is
tested by what it says, since a store that fails later for another reason
is refused by accident.

The first shape of it held the store in memory, and that was decided by
building it. A store reads by lending from mappings and the cipher is in
the process, so the plaintext is the process's to hold: the first shape
made each file a memory file (`memfd_create`), written and mapped by the
code that writes and maps a plain store's, with its envelope written once
it was finished, because a memory file is a mapping like any other and
nothing on a read path had to know. The price was written here afterwards
as a property of the design -- the store has to fit in memory, an open
decrypts all of it, a descriptor a file -- under the words "the
constraint and not an accident". It was neither. Nobody who asks for a
store under a cipher means the stores that fit, and a limit of that size
was the first thing to say and was said last, in a README, two commits
and a pairing after it was built. The rule: a feature that holds for some
stores is a different feature from the one asked for, and which of the
two is being built is said before it is built; and a constraint a note
calls the design's is checked for being the cheapest thing to have
written.

What it takes instead is the readers asking for bytes before they read
them. `Blob` borrowed its key section whole at every lookup and
`flatindex` addressed into the borrow, which over a mapped file is the
kernel's paging and over anything else is the section made present whole.
A source now says that it lends by the piece (`Bytes::piecewise`); `Blob`
holds such a source's sections by their place and borrows none of them
(`Blob::pieces`); and `flatindex`'s readers take any `Sect`, the slice, over which they are the
code they were, or `Pieces`, each read of which is a `slice_at` of the
range it wants. The record parsers are not generic and there is still one
of each: a section by the piece hands them a window that holds the
record, the piece the offset lies in and then more, which is sound
because a parse that succeeds has read only bytes it asked the window
for. `tests/blob.rs` holds the readers to it with a source that lends a
copy of exactly the range asked for, and counts what it was asked for,
since a reader that asked for its sections whole would agree on every
answer too. The engine names one reader type as before: a paged mapping
is a `MmapBytes` whose lendable length is zero, so its every `slice_at`
fails the bounds check a mapped file's passes and goes round by the pager
(`slice_slow`), and nothing in `db.rs` knows which it holds.

The plain store's price was counted, and for a commit it was called
small where it had to be nothing. The first count read twenty-two
instructions a point read more, `Blob::flat` out of line where it had
been inlined; then six in eight hundred and fifty, written here as the
feature's price with their attribution -- four for a test of whether the
store has a cache at the top of `read_all`, three for a third kind of
`Sec` -- beside eighty-eight a scan, a count and a test a key of every
merge, a counter raised and lowered at every write, and a call a block
of every builder's look. Six is under what any pairing resolves, which
is how it was let stand, and it was every store without a cipher paying
for one. A store in the clear pays nothing an operation now, and the
count is what holds that: `bench/examples/read_only.rs supdb 100000 1
n-none` and `lag0_probe.rs supdb 100000 1` under callgrind with the
toggles the profiling section names, `work_probe.rs 100000 whole` for a
writer's whole round and `merge_walk.rs 100000 4 1` for a merge, each
built at `origin/main` from this tree's example sources and at the tree,
and read by function (`callgrind_annotate`, the functions' own
instructions side by side), never by the total alone. They read under
the main line's: the point read seventeen instructions of 848, the scan
fifty-five a scan of a hundred entries, the round 1.3%, the merge 4.4%.
Those four were the count, and a store's reads through its compact form
were not in it: a scan through a form was 370 instructions over the main
line while this paragraph said nothing was, until the form itself went
under a cipher and was counted (the compact form, below).
`sh scripts/cost.sh BASE` is the count made one command: the probes (the
point read and the scan, each in the raw form and through the compact
form, the scan past `SCAN_REST` in both, the durable and the buffered
ordered load whole and their commits under a toggle, a writer's round, a
merge, the segment work on the writer's thread, a segment written through
`SegmentWriter` and `Blob::scan_at` through the public API) built at BASE
from this tree's example sources and at the tree, each under callgrind,
every function's own instructions side by side, and a failure where a
hot function gains past its slack (`scripts/cost-diff.py`, below);
`sh scripts/cost-red.sh` is its red input, and `sh scripts/check.sh cost`
runs the two. The first version of the list named
its functions as paths, `supdb::db::Reader::read_all`, matched as
substrings of callgrind's names, which print a method as
`<supdb::db::Reader>::read_all_in::<G>::{closure#5}`: no line named an
engine function, the only matches were the probes' own functions inside
other functions' generic arguments, and the point read could have gained
anything. Its first whole run against upstream said so by printing
`HOT GAINED` for `read_exts_inline::<coded_record_to<supdb_range::{closure#0}>>`
in a probe that never scans. The comparator reads canonical names now
(generic arguments dropped, `<T>::f` read as `T::f`), fails a probe in
which no line names a function, and names the lines a whole run never
reached; the red input's first step holds each of those against names
written as callgrind writes them. A gate's list is held to the names the
instrument prints, by a fixture in the instrument's own spelling, before
the list is trusted to name anything. Against itself the count is exact
for every listed function in a probe with a toggle, while libc's and the
allocator's counts move between two runs of one build (the point read's
`memcmp` by most of an instruction a read) and so does the controller's
look at its budget, which runs on a clock (`!` in the list takes it out).
And a list is also what a gate does not hold: the list was the only thing
that could fail, and the review found the scan probe running
`Reader::install_ahead` and `make_table` once a scan, on no line, so a load
and a branch in either, or a step moved out of a listed function into a
helper of its own, read green. A toggle collects the operation on its own
thread and nothing else, and against itself every function of the engine,
the suite and the probe it collects has read the same to the instruction,
but for a few that wait on another thread (the segment writer's look at its
helper, +16 over the seal probe's run once and +63 over a commit probe's),
so in a probe with a toggle all of them are hot, listed or not. Where an instrument is exact,
the gate holds everything it measures; a list is for where it is not.
The second review found the slack was the same hole. It was a twentieth
of an instruction an operation, and an operation of the seal, the merge
and the loads is a key: a load and a branch at every 16 KiB piece of a
key section is a hundredth an operation and read green, in a probe whose
every function read the same to the instruction against itself. A slack
an operation is a slack a key, and the owner's rule names the block and
the step too. Under a toggle the slack is 64 instructions over the
probe's whole run, a function, whatever its operations, for every
function built from source -- the engine's, the suite's, the probe's and
the standard library's code compiled into the binary, a `#[no_mangle]`
helper's with them -- and libc's and the allocator's, which move with the
addresses they are handed, fail past two instructions an operation and a
twentieth of their own count both. With no toggle (the loads, the round,
the merge) every function of the engine is hot, listed or not and new
ones above all, at a hundredth of an instruction an operation: the self
checks put the rest of their noise at under a two-hundredth. The functions
that move by more were `~` lines, out of those probes, on the reading that
each was exact under a toggle; the third review listed the toggled probes'
functions and found three of them in none -- the writer's sweep at an
operation's end, the reader table's look, and every function of the
segment work, which `~supdb::db::Maint::` took out whole on evidence for
two of its hundred and forty methods -- so a step a landing or a key
written into a landing, publish or retirement read green in every probe
that ran it. A `~` line carries a bound now, three times the most two to
four self-checks moved its function, and names its function, not its type
(`Maint::collect` and the three others measured past a two-hundredth);
and the segment work has a toggled probe of its own (`land`: the segment
work and the upkeep inline, the seal landed before the scan that follows
it, the store's platform readings through its seam), whose every function
reads the same against itself but the seal's wait, bounded. A `~` line
with no bound is refused. The rule: a function taken out of one probe is
named, with the probe that holds it, and an exclusion with no bound and no
holder is a function the gate does not measure. The slacks as numbers:
under a toggle 64 instructions a function over the run; with none, a
hundredth an operation, which is 1,000 over the loads' 100,000 keys, 3,000
over the round's 300,000 operations and 2,000 over the merge's 200,000. So
a fixed cost of up to that much once a store, in its open or its close,
reads green there -- the owner's rule is a key, an entry, a block or a
step, and a store's open is none of them -- and a cost of under ten
instructions a commit of the ordered load's hundred, or a few a 16 KiB
piece of its stream, is seen by the commit probes (`commit_durable`,
`commit_buffered`: the toggle on the suite's `write_batch`, the writer's
appends and its commit) at 64 over the run; a cost a segment by the seal
probe, four segments of a quarter of its keys, at 16 a segment.
Counted against the upstream head this branch merged, every probe read
under it but the scan, eight instructions over, and no function of the
scan's source had changed: two of upstream's (`forms_off_scan`,
`scan_seek_seeded`) were inlined in upstream's build and called out of
line in ours, the scan having grown with the cipher's work, so each
call paid its frame for a test that answers no in a store in core. Each
keeps its first test inline now and the rest out of line, and the scan
reads fifty-two under. Two readings beside it were nothing: `scan_seek`
fifteen over by its own count, which was upstream's out-of-line `flat`
inlined into it, thirty-five a call gone; and `memset` 1.2 a key over in
both loads, which was the reader table zeroed once a store by one `rep
stosb`, that callgrind counts a byte an instruction. A move by function
is read beside its callers' and callees' before it is read as a cost,
and a function whose count moved with no change to its source is first
asked whether the compiler stopped inlining it into a caller that grew.
What it took, each a shape:

The question rides a test the operation already makes. Every read looks
at two bytes of its handle at its start and its end, for the compact
form's census; the cipher's bit is in both (`ATTN_PAGED`, set where the
handle is made), so a store under none makes the tests it made, and what
a read under one does there -- the wait for a cache far past its budget,
a snapshot's word, the failure its answer is replaced by (`paged_begin`,
`end_slow`) -- is behind them and out of line. The end's look is made
with the answer in hand (`Entered::finish`), and `leave` with no look in
it is where the read came out ahead. Whether an operation is the
writer's own is its depth (`WRITER_OP`), where it was a counter; a
merge's rests are a type (`MergeRest`, `NoRest` for a plain store, which
compiles to no step counted); the forms' word for "nobody has asked"
is another under a cipher (`REST_HERE`), and the builder ahead's stop
word has a third value (`AHEAD_REST`), so the one compare each look
made sends a store under a cipher to its rest and a plain store on.

The reader's kind is found in the refusal. `Sec` is two kinds again: a
section of a source that lends by the piece is held by its place, as a
lent one is, and `Bytes::lend_at` refuses it whole -- for the mapped
file, by the bound check it always made, against a lendable length of
zero -- so `Blob::flat` is the function it was and the reader by the
piece is where it answers `None`. A scan's walk refuses such a source
before it walks (`Blob::scan_whole`), and the engine takes the walk by
the piece up in the branch a failed walk takes anyway (`scan_either`);
`Blob::scan_at`, the caller's, asks the kind only of a type whose values
can be of it in a caller's hands (`Bytes::ASK_PIECES`).

Naming the second walk inside the first cost the first. `return
self.scan_pieces(..)` on a branch no plain scan takes was three to four
instructions an entry of the fixed loop, the callback handed on by
value, by `&mut` or inside a closure alike; an arm by the piece moved
behind a closure in every reader (`by_piece(move || ..)`) cost the walk
of the unsealed keys two an entry and the point read seven. What the
compiler does with a callback that has a second taker is not something
a branch's coldness undoes, and the count of each shape was the only
way to know: six shapes of `scan_at` were built and counted before one
read level.

And a count that moved by one in a function nobody touched was worth
the look. `emit_unsealed`, whose source and whose callees' sources were
the main line's, read one to two instructions an entry over it in every
build, block placement and a spill, moving with each unrelated change.
The function held a `RefCell` borrow of the snapshot's stale list from
its first line to its last, across the caller's callback: taken for the
one look it is for, the walk of the unsealed keys is sixteen
instructions an entry under the main line, and a callback that reads
through the handle no longer runs under a borrow of the handle's state.

A bound restated in a generic caller ahead of the override that made it
did not fold either. Sealing `Bytes::lend_at` was first tried with the
method taken off the trait: a lendable length on `Bytes` (`self.lend` for
a mapping) and the lend a private function of `blob` that refused past
it and then called `slice_at`, whose own check against the same field
was to be the same compare. It read `Blob::scan_whole` six instructions
a scan over the commit before and the public `Blob::scan_at` probe 127 a
scan, with `key_at` and `copy_block` near one a key in the landing:
the zero-sized token on the method as it was read level everywhere. A
check that two inlined functions make of one quantity is counted before
it is called one check.

And a count is chosen among the shapes that hold a rule, never in place
of one. `MmapBytes::lend_whole_map`, the successor of the public
`as_slice`, went in crate-private with no token, on the reading that a
token a reader would raise the wall's count of the seam's paths out: any
module of the crate could then write the whole borrow that answers the
empty slice for a paged mapping, which is the hazard the seal is for, and
review read it off the signature. It takes the ordered index's token or
the stride form's, each made in its own module (`lend_seal`, the trait
`bytes::whole_map_readers::Reader` implemented in the seam alone), with
the raise declared. A seal is as wide as its token's constructor and
holds nothing the compiler cannot see: a derive or a `Default` on the
token, a function of the reader's module that hands one out, one made by
unsafe code, or a new caller inside the reader's own module all compile,
and a red line abroad tries only the routes it names. tests/source.rs
holds each token's module to its one text and every token to the call it
is made for (`the_seals_tokens_are_made_only_where_they_are_spent`), and
its first version read both as text: review wrote the route this
paragraph names in spellings rustfmt keeps and the check read none of --
a function of the ordered index returning `<lend::Token>::new()`, spent
abroad by type -- and a renamed import, a call through a function value
and the names handed to macros as identifiers beside them, each compiling
with no unsafe. The second version read the words by their position (a
method after `.`, a module before `::`) and, inside a macro's tokens,
refused `lent` and `lend` wherever they stood, with an exception list
keyed by a line's text; and the stage's closing critic found it red on
ordinary code -- `debug_assert!(to.lent <= n)` in the engine, any
`.base(x)` of another type, a listed signature rustfmt split -- since
`lent`, `lend` and `base` are common words here. The seal's words are
names nothing else in the tree uses now (`lend_at`, `lend_whole_map`,
`base_address`, `lend_seal`, the tokens' re-exports, `BaseAddressToken`,
`whole_map_readers`), so each is read as an identifier wherever it
stands, a macro's tokens included, with no rule for macros: every line of
src that names one is a call at a site listed by file and scope with its
token made inside its own parentheses, or a declaration at a site listed
the same way and standing in a `use`, a listed impl's header, a call's
signature or a token's module held whole; a `use` of one binds a
re-export's name and nothing else, and each listed scope is one function
of its file (`the_seal_leaves_ordinary_code_alone` holds the ordinary
code and the rewrapped lines; the two `a_seals_token_*` tests hold each
route). The first version of that held a declaration by its scope alone,
and review found the scope of a top-level line is the whole top level: a
macro handed the token's name there (`hand!(OrdLendToken);`) handed a
token out, gave it an impl or gave the readers' trait to another type,
where the exact text before it had refused every such line, and a second
`fn file_facts` in a nested module of zfile took a listed scope's name.
The rule: a check over source that holds a name holds it by every
spelling the language gives it, and a name it holds is one the tree uses
for nothing else, since a check over a common word is a check over
everyone's code, and an exception list keyed by text is red at the next
rustfmt; and a site keyed by scope admits whatever that scope can hold,
so a scope as wide as the top level is keyed by the kind of item too.
The second review found the call's token read from the word's first row
of the table, which every row shared, so a row of another token would
have held every call of the word to it: a value a table's row carries is
read from the row that matched, and a case with two rows that differ
holds it. What it still cannot read is a name a macro builds from pieces
(`concat_idents!` is unstable, `paste` is no dependency), code from
outside a file, which the wall refuses at a count of zero, and a closure:
a call inside a closure of a listed scope is that scope's, as the lexer
has it, so a listed reader that returns the closure, or a function value
of it, hands the borrow out, and what a listed scope returns is read in
review (a nested `fn` is a scope of its own, and refused).

The same shape twice more, each read at the
instruction (`--dump-instr=yes`, each instruction's count beside the
disassembly, `addr2line -i` for whose inlined code it is) and by
neither function's source. A caller's function that `Blob::seek` was
inlined into carried `seek`'s arm by the piece beside the slice's, and
the public `scan_at`'s loop in the same function read two register
copies an entry more; `seek`'s arm out of line put the loop back to the
instruction. And the settle that inlines the ordered index's planned
seek read thirty-seven instructions a key over the main line with its
own source unchanged, once `head` bounded its read by a call that
returns (`head_absent`, the read by the piece) where it had asserted and
the top level a mapping read by the piece builds at its first seek was
reached by another (`top_late`). The first fix read the cost as spills
in the search's loop, held the heads as one slice (`heads_held`), took
seventeen of the thirty-seven, and was called the compiler's for the
rest; the count by instruction, made only when a review asked for it,
put the loop at twelve instructions a probe on both sides all along and
the excess around it: the query's head and its position in the top
level kept on the stack across the two calls, reloaded at each compare,
in a plan stored and copied through memory. Both arms out of line, each
a function the plain path does not name (`plan_seek_late`,
`seek_planned_by_piece`), took the rest to eight a key, and those eight
are still there, the plan stored and copied through the stack: the
function reads nine a key under the main line because the stride's
probes read unchecked over the slice the plan cuts, bounded once at the
slice, two instructions a probe the main line pays, so the growth is
outweighed inside the function and not removed. A cold call that
returns is a call the caller's registers are saved across, and a panic
is not. The rules: a cold path that returns into a hot path is priced in
the caller, by its instructions, before it is called free for being
cold; where a function's count moved with its source unchanged, its
inlined callees are diffed by instruction before its own lines are, and
before a fix is built on where the cost is thought to be; and a remainder
is not called the compiler's until the instruction listing says which
values it keeps where.

What a plain store is still asked is where something is made or waited
for, never in an operation's path a key, an entry, a block or a step: a
handle made (`paged_bits`), a snapshot taken or released, a file
operation (the `vault::` door's counter), a wait on the device or
another thread (`unlent`, `unlend`), a merge's start and its end
(`vault::hold`, `check`), a fill's start (`look_mask`). Each is a load
and a branch beside a syscall, a thread's park or a pass. One more is
the compiler's and was found counting an ordered load whole: a store's
creation zeroes its reader table in one call, a hundred kilobytes, where
the main line's build of the same source (`Readers::new`) wrote it a
slot at a time, once a store.

Seventeen mutants of the new paths, four of which outlived the tests
there were: a fill that looks every sixty-four blocks under a cipher,
which only a store of wide values could tell from one that looks at
each; the forms' word left the plain one after the forms were given up;
the writer's own fill not resting, which no test ran, since the
builders' test filed its burst on the store's thread; and a read that
does not wait for a cache far past its budget, which had no test since
the wait was written. Each has one now, and a snapshot under a cipher
has its first: nothing had read through one, and four of the seventeen
were its. The last of those tests hung under its own mutant where it
should have failed, a reader parked in a callback and the failure path
holding the end that lets it go, so the test moves that end into the
scope it can fail in. And the tests that hold a cache to its budget run
one at a time (`one_at_a_time`): four of them at once on four cores
were each other's stalls, and the larger store's peak read 3% over its
bound in one run of the file, with nothing of the engine's changed.

The rules: a cost with the option off is nothing or it is a cost, and
"under what a pairing resolves" is not nothing -- it is counted, by
function, against the commit before the option existed; a question a
path must ask is put where the path already asks one; a branch that
names a second callee for a callback is priced on the first callee's
loop; and a function whose count moved with no change to its source is
read for what it keeps live before the move is called the compiler's;
and a test that parks a thread lets it go on its own failure, or a red
run of it is a hung one.
The same count under the cipher read 450 more than its twin in the
clear and then 230: a page-size `sysconf` on every read, inside the
test of whether the cache is crowded, and a divide by a field where the
divisor is a constant.

The cache (`src/db/pager.rs`) is a file as a shared mapping of a memory
file its plaintext's length, a state a chunk, and the envelope the chunks
are decrypted from. A chunk is a stored page less its tag, so two chunks
share a page of plaintext and what is given back is a unit, the least
run of chunks that is whole pages, 256 of them. What makes giving back
safe is the reader table. A reader holds what it was lent for the length
of its operation and nothing says where, so the evictor marks a unit's
chunks, moves the table's epoch on, and frees once no reader is pinned
before the move (`PagerGrace`, `Readers::oldest_lent`), which is the rule
a retired state is freed by; a reader that meets a marked chunk takes the
unit back, or waits for its pages to be gone and decrypts again.
Addresses do not move, so a range asked for again after a rest is the
same bytes in the same place: a walk that rests keeps its borrows by
asking for them again, and copies nothing. Under debug assertions a read
with nothing pinned panics (`Paged::ensure`), which is how the threads
that read with no slot were found, and they hold the cache explicitly
(`pager::Hold`): a merge over its inputs, the open over its segments.

A pin is longer than a read, and every place that was true held the
cache past its budget. Each was found the same way, by the count of what
the evictor waited for (`PagerCounts::longest_wait_us`) and, for the
length of the hunt, a backtrace kept at each pin and printed for the slot
being waited on; none was found by reading, and two readings that came
first were wrong. In the order they fell: a snapshot's handle is pinned
for its life, so it says at each read what it holds from (`Slot::page`);
a scan of no limit is one read, so it is walked a thousand keys at a
time over the one pinned state, with a rest between (`scan_rested`); a
merge kept every output in memory until its last, so each is given its
envelope where it is finished (`Emitter::leave`); the writer's flush and
settle are pinned for as long as the merges they wait for run, and the
segment work for as long as it joins a thread or syncs a manifest, so a
wait says it holds nothing (`Reader::unlent`, `unlend`); the landing of a
merge compares a key a block of the old partition and the new, to carry
the block cache's copies across (`Db::same_blocks`), which for records of
a hundred bytes is most of the pages of both, read on the segment work's
one pin -- the store read into a cache an eighth of its size in fifty
milliseconds, at every merge -- so it rests as it goes and answers no
once the cache is crowded; the block cache's builders read every block a
burst dirtied under one pin, and gave their forms up there
(`Reader::abandon_asked`) until the first pairing out of the cache showed
what that cost, below; and merges read their inputs in order at the
cipher's rate, hundreds of megabytes a second, so whatever stalled the
evictor for a few milliseconds, they had filled the cache by then, and a
reader past the crowding line waits for the evictor, briefly and with an
end (`Pager::breathe`). The rule: under a cache the engine fills itself,
a pin is priced by what anyone reads while it stands, since the evictor
waits for the oldest; a wait is the one place code not written for it can
say it holds nothing; and the instrument is a count of the longest wait
with the name of who was waited for, built before the first guess.

A chunk that fails its tag is found where it is read, not at the open,
and the readers' answers are `Option`s in which bytes that did not read
and a key that is not there look alike: a search treats a record that
will not decode as "not less", and a lookup a slot that will not read as
absent. So the pager remembers the first failure and every read of the
store answers with it from then on (`Pager::check`), asked at the read's
end. A merge asks before it names its outputs (`vault::check`): one that
read no values for a key writes the key without them and deletes its
inputs. `tests/cipher.rs` holds the property restated -- a byte changed
anywhere is a store refused at its open, a store that refuses to be read,
or a state the store acknowledged -- and requires each of the outcomes to
occur.

Four things building it, each a shape. A memory file had two views. The
envelope's naming and the file's hand-over to the cache took the table's
lock one after the other, and a reader that mapped the file between them
made its view and took the memory file from every entry, so the
hand-over found no entry over the file, made a second view for nobody,
and the evictor, giving back a megabyte of that view in the instant
before it was dropped, punched the memory file under the first: a
segment's hash slots read as zeroes, its key as absent, a delete in it
unseen, one seed in two hundred of the fuzz under a cache of sixteen
kilobytes, with every file well-formed. The two are one critical section
(`Vault::settled`), and a second view of one memory file panics under
debug assertions. A reader waited for the evictor while holding what the
evictor waited for: it moved a chunk to busy, found the unit's pages
going, and waited for them with the chunk still busy, where the evictor
waits for no chunk to be busy before it lets them go; found reading the
two loops side by side, since the fuzz had not met it. The reader table
was handed to the cache after the store's threads were started, so a
thread that read in between read under a pin nobody waited for; it is
handed over where the table is made. And the evictor's margin was eight
megabytes at least, which under a budget of two was every pass giving
back everything: a floor written for a budget of gigabytes and tested
under one of megabytes is two different constants.

What holds it: the pager's unit tests under a stand-in cipher (a range is
the plaintext and nothing else is decrypted, nothing goes under a hold, a
rest gives way and the range asked again is the same bytes, threads read
the plaintext while the cache turns over, a changed chunk poisons, a file
adopted stays until it has its envelope);
`a_store_larger_than_its_cache_reads_as_itself_within_it`, a store nine
times its cache through a load, a scan of the whole of it, four reader
threads beside a writer that flushes, and a reopen, with the cache's peak
held to its budget and a fixed slack throughout, the open held to a
handful of chunks a file, and the process to no descriptor for a file;
`the_block_caches_builders_build_through_a_cache_the_store_does_not_fit`,
a burst over every block filed by the store's own thread under a cache a
sixth of the store, held to two mutants;
and the differential fuzz under a cache of sixteen kilobytes
(`fuzz_range_under_a_cipher_that_does_not_fit_its_cache` in the gate,
`ZFUZZ_CIPHER=1 ZFUZZ_CIPHERCACHE=16384` by hand, with `ZFUZZ_CACHE=all
ZFUZZ_OPTVARY=1` for the block cache's paths), which asserts that the
cache turned over, since a run under a small cache that gave nothing
back is a run of the other test.

The engine reaches its files through `vault::` (`create`, `open`, `map`,
`rename`, `hard_link`, `remove_file`, `names`, `len`, `exists`,
`sync_dir`), each the standard library's call after one load of a counter
where no store under a cipher is open in the process. A store under a
cipher is given a directory that cannot exist, its marker file's path with
a name after it, so a file operation the door is not in front of fails
with "not a directory" and writes nothing; and the library is linted for
the calls the door stands for, by symbol (`scripts/door/run.sh`, under
"Running the checks"), since the first only fires where a test under a
cipher reaches the call. A name nobody classed
is kept in memory: the log and the manifest alone pass to the directory
under their own names, and the engine seals what it writes into those
itself, a record a commit and a slot a publish. A new kind of file is
therefore safe before anyone has thought about it, and lost at the close
until someone has.

A file being written is a memory file, and an fsync of a memory file is
of nothing, so every durability the engine
takes from an fsync of a file that is not the log or the manifest is one a
store under a cipher gets another way, and there were three. A segment
before the manifest that names it: the manifest's bytes are made by
`manifest_image`, which writes and syncs the envelope of every segment it
names before the slot that names them exists in any form; a seal and a
merge write theirs earlier, on their own threads, where a plain store pays
the fsync (`vault::persist`), by name, and pass over a name that has gone,
since a promotion links a piece under another name and unlinks the first
while the seal's thread is on its way there, and the manifest that names
the file writes it whoever did not. Ordered ingest's run, a segment synced
at every commit: its commits go to the log as well, sealed and synced
there (`Db::log_run`, below), and its segment gets its envelope where the
run closes. And the ordered index, which a plain store leaves unsynced and
composes again where it is missing, as a store under a cipher does. A
fourth fsync of a kept file is a durability that is not there, and the
list is the thing to check a change against.

The recycled log was off there too, for as long as it was written down
that a file written through before its first record could not be a
sealed one. It can (`Options::recycle_wal`): the spare passes to the
directory as the log does, a recycled file's header is a key drawn for
its new life, and what lies behind the header -- the last life's records
under the last life's key, or the zeroes the file was made with -- fails
its tag, which is where a sealed log ends. One state is new, the crash
at a rotation: the log's name on the last life's header, the name being
the journal's and the header's bytes the page cache's. A plain log's old
header reads there as a log with nothing in it; a sealed one's is a key
wrapped for another file, which an open refused as not this store's log.
The first version synced the header into the spare before the rename, an
order no test could hold and a store that would not open if a device
ever lied about it. A store that recycles reads that header as a log
with nothing in it now (`Wal::replay`'s `recycles`), and one that does
not still refuses it, since only a changed byte makes it there;
`tests/cipher.rs` builds the state by renaming the spare, and
`ZFUZZ_RECYCLE=1` runs the fuzz with recycled logs. The rule: a state a
crash can leave is one the open reads, built in a test, before it is
one an fsync's order is trusted to prevent; and "cannot work under a
cipher" is a claim about a mechanism, to be checked against the
mechanism before a feature is switched off on it.

Ordered ingest was off there for the same kind of reason, and is on. A
plain run is its own log: a segment open for append, a marker and an
fdatasync of that file a commit, the WAL never written. Under a cipher
the run's file is a memory file until it closes, so a commit synced
there is synced nowhere, and the first answer was to switch the run off
(`under_cipher`, the function that fixed a store's options under a
cipher, gone with the last of them) and send an ordered load through the
log and a hash table to a seal's sort. The run does not need its file to be its log. It
is kept whole -- the ordered table, the records laid out once as the
segment takes them, the close that composes a trailer where a seal sorts
-- and each commit's batch goes to the log as well, sealed, written and
synced as any commit's is (`Db::log_run`). The run's segment is named at
the log's sequence, so the manifest that names it covers its frames and
a replay skips them; a crash before that replays them into a table, as
any commit's. The temp file a plain open recovers a run from is not in
the directory: nothing of a run under a cipher is, until its envelope.
`bench/scripts/pair.sh supdb-cipher supdb-cipherlog 300000 --passes
load` prices it against the shape before, the arm with
`direct_ingest` off: the ordered load 1.23x of it in both sittings and
1.4-1.56x buffered (`supdb-ingestcipher supdb-ingestcipherlog`), the
shuffled load level.

Three things building it. The table reads its committed records through
a mapping of the run's file (`RunMap`), and the cache gives a kept
file's pages back by the megabyte once the file has its envelope, out
of the memory that mapping reads. A file its writer has mapped is held
(`vault::open_mapped`, `Mapped`): the envelope is written where the run
closes, and the cache's view of the file is owed it
(`MappedFile::owed`) until the last such mapping is dropped, which is
the table's drop -- the publish after the run's landing, since the
state keeps a replaced table for the upkeep's next look
(`State::replaced`). Until then the run's plaintext is memory the cache
counts and cannot give back, a seal's worth, as a seal's output in
flight is. The first version let the vault leave the file at its
envelope, as a merge's finished piece is left, and the reads after the
close decrypted what had just been written. The second: the envelope of
a run's segment is written on the run's own thread (`close_run`), where
a plain run pays its fsync; with that line deleted every test passed,
since the manifest writes the envelope of any segment nobody has
(`manifest_image`), on the segment work's thread inside the landing, so
the vault counts the envelopes a manifest's call had to write
(`CipherCounts::late`) and a test holds it at none. The third: the log
rotates under a run as under any commit, and with the rotation deleted
every test passed too, the log one file of the run's size; the test
holds the log's file under a run to twice the rotation's. Ten mutants
in all, those two the ones that outlived the first tests.

What a plain run pays for it is counted and is under nothing: the bit
that says a run's commits are logged is in the byte the table already
carried for `direct_table_refs` (`MemTable::after`, `RUN_REFS`,
`RUN_LOGGED`), read once at a commit's end where the one flag was, and
the counters of logged runs are bumped where the log is written, never
where a run opens. The first fold of the test left two instructions a
commit, a spill of the byte; the commit had been cloning the table's
and the store's `Arc`s for the borrow checker, two locked increments
and two decrements a commit, and borrowed they put `Db::commit_direct`
a thousand instructions a commit of a thousand keys under the main
line's (`bench/examples/load_only.rs supdb 100000 1` under callgrind,
built at `origin/main` and at the tree, by function).

What the run costs under a cipher is the double write and the envelope
at the close, and it is not small: `pair.sh supdb-cipher
supdb-cipherbase 300000 --passes load` reads the ordered load at 0.68x
of the plain run in both sittings and the buffered pair at 0.52-0.57x,
the shuffled loads at 0.80-0.84x, and the device's bytes per byte
stored at 2.5 against 1.3. With `SUPDB_LOAD_PHASES=1
SUPDB_DRAIN_STEPS=1` before the arms: the commits 20-50 ms longer over
three hundred, a write and an fdatasync of the log where the plain run
syncs its own file; and the drain 77-94 ms against 26-35, of which the
envelope is 43-55 between the readable landing and the fsync -- forty
megabytes sealed and written in 33 ms and synced in 20, by `strace -f
-tt -T -y` over `load_only supdb-cipher 300000 1`, where the plain
run's bytes went to the device commit by commit -- and 14 more are one
`unlink`: the log file the run filled and rotated out of, retired on
the segment work's thread by the manifest publish a drain's promotion
makes, before the drain is answered. A plain ordered load has no such
file, and its drain's promotion retires two temp names that are links.
By the code any drain whose promotion or merge publishes while a closed
log file waits retires it there (`Maint::publish`, the owed slot
superseded); it has been looked for in no plain drain. Both are open in
`ROADMAP.md`: an envelope sealed as the file is written makes a run its
own log again, and a log retired after the answer takes the inode off
the drain. From this commit the twin in the clear keeps its
run, as `supdb` does, so the ordered-load cell of a pairing against the
twin prices the cipher's run against the plain one, where before it
priced two loads through the log. The rules: a feature switched off
under a mode is first asked which part of it the mode breaks, and that
part is given another way before the rest is given up -- the run lost
its fsync, not its table or its close; the new shape is priced against
the old one under the mode before it is priced against the plain
store, since the second pairing says what is left and only the first
says whether the work was worth doing; and a mutant that survives
because another mechanism covers for the deleted one is held by
counting which mechanism did the work.

The compact form was the last thing off there, and the reason written
down for it was about the code and was read as being about the form:
"built from a partition's records borrowed whole and attached by its
bytes in the page cache, neither of which a file read by the piece
offers". Neither was the form's. The reader already read a form page by
asking its mapping for it (`ZFile::page_bytes`, a `slice_at`) and copied
the resident part to the heap at its attach, so over a paged mapping it
is the reader it was; and the builder took its records through a trait
(`zpages::Images`) that had a second implementation for the tests. The
form builds, seals and reads under a cipher now and `under_cipher` is
gone; whether the controller should choose it there is another question,
which a pairing answered no (the end of this section). What it took:

The builder reads by the piece. `PieceImages` is the directory's word for
a rank and then a window that holds the record (`flatindex::Pieces::image`),
each asked of the store's cache, on a thread with no slot of the reader
table and so under a hold (`pager::Hold`). What the build does about the
cache its segment is read through is a type (`zfile::Walked`), as a
merge's rests are: `Mapped` is the plain build's code, the page cache's
advice by the mapping's address, and `Held` rests at each page written,
where nothing the walk was lent is read again, so a partition several
times the cache is built within it. The same advice is asked there of
the kernel's cache of the envelope, below. The attach and the sampler
take a hold for their own reads the same way.

A form file is an envelope beside its segment's, under the segment's
token and key: `x-<token>.z`, a third kind after the segment and its
ordered index (`vault::Kind`). No manifest names it, as none names a
plain store's: it is a function of its segment, the store learns of it
where it is first probed or attached (`vault::companion`), the open's
sweep leaves it while its segment lives, and it goes with its segment
whether or not this open ever knew it (`Vault::remove`). It is written
under a temp name and renamed over its own, where a segment's envelope
is written in place: a segment's token is drawn for its one envelope,
and a form file is written again for the same segment -- in the other
coding, or after a page failed a check -- while a reader holds the view
over the first, whose file a write in place cuts to nothing under it. A
second envelope under one file's key meets no nonce of the first, each
envelope's nonces standing behind an id drawn for it.

The controller needed nothing measured anew and one thing forwarded. The
group's budget, its refaults and a thread's major faults are, under a
cipher, the kernel's cache of the envelopes, which is where a store that
does not fit memory waits; so the model, the census and the sampler's
price of a fault read what they read. What did nothing under a cipher was
every advice the engine gives the page cache about a mapped file, since a
paged mapping lends nothing to advise: the build's ask ahead and its drop
behind, the drop of a partition's raw pages once its form serves, the
swap at the crossing, the residency a build ahead waits for, and the
read path's own asks out of core. A paged mapping answers them for the
file behind it now (`bytes::Paging::resident`, `forget`, `willneed`,
through `MmapBytes::resident_at`, `advise_dontneed_range`,
`advise_willneed` and `advise_dontneed`), so every caller of those got
them and no call site names a cipher. An envelope is mapped for reads far
apart, so without the ask ahead a build read its segment a chunk a device
round trip. And a form file built ahead of the controller's buy is kept
cold: its plaintext is not taken into the store's cache, where it would
stand in the raw form's place, and its envelope's pages are written back
and dropped (`Vault::persist_form`'s `cold`).

One thing the forwarding found is the kernel's and holds for a plain
store too: the page cache keeps a file in folios the size of the writes
that made them, and a drop of a range takes only the folios wholly
inside it. The pager's first test of a whole file's drop read a quarter
of four megabytes still resident, the first megabyte, because the range
began a page in, past the envelope's header; a whole file is dropped
whole now, and a partial drop leaves its edges, which the crossing's
test under a cipher allows for and nothing has looked at for a plain
store's drops of a record region.

Every test of it passed at its first run, the fuzz's among them, so none
was believed until its mutant failed it. Twenty-four mutants; four
outlived the first tests. An envelope written in place passed everything,
there being no test in which a form file was written twice: the vault's
unit test writes one, takes a view of it, writes another under the name
and reads the first through the view. The build's check of whether its
reads had read could be deleted, since a record that does not read is an
image of no bytes, which the build already refuses as a record with no
key: the check went. The footprint read from a section held whole, which
is zero bytes under a cipher and so a model that says every store fits,
passed until a test drove the controller there (`tests/zpages.rs`, the
tests with `cipher` in their names). And the controller's probes for a
form file an earlier open left passed without the store learning of the
file as it asked, since the attach learns of it too; what the probe
alone decides is the swap at the crossing, which the test now waits for.
One mutant, an attach that holds nothing, did not fail a test and hung
one: its debug assertion fired on the segment work's thread, and the
writer waits in `land_seal` for a landing a dead thread will not make
(`ROADMAP.md`). The runner stops a test binary after seven minutes and
reads that as the mutant's death, and removes the stores a mutant's
tests left, after a hung one's filled the disk under the run and took
its verdicts with it.

A chunk of a form file that fails its tag was a read of the store that
failed, and the store refused every read after it, where a plain store
takes a damaged form file out of service and reads the partition raw.
The pager poisons the store for every file but a form file, which the
store can make again (`pager::Paged::regenerable`): there the failed
chunk is a range the file does not lend, and the form's readers already
take a file out of service at a page that does not lend. The test holds
both ways a chunk can fail -- at the attach, which reads the file's
chunks and rejects it, and in service, re-decrypted after the cache gave
it back -- to the store read whole and never refused
(`a_changed_byte_in_a_form_file_takes_the_form_out_of_service_and_not_the_store`).
Its in-service half first damaged "the first form file" of a hash map,
which in one run in six was a file nothing serves, read by nobody
through eleven thousand rounds: it damages every one now. And one regime is
not priced: a store that fits memory and not its own cache of plaintext
decrypts two chunks for a raw point read and one, and a decode, through
the form; the controller reads the kernel's cache and sees none of it.

Counting the plain store against the main line for this found what the
count before it had missed. "A store in the clear pays nothing an
operation" was held by a point read and a scan of the raw form, a
writer's round and a merge, and a scan through the compact form was none
of them: it read 370 instructions a scan of a hundred entries over the
main line, from the commit that gave the readers a source that lends by
the piece (`eb377fe4`), four commits before the claim. The walk through
a form reads a record's values with `Blob::read_exts`, whose read of a
run in a block is written into its caller; that reader had grown a
second way to fetch a block, and though no record of a form file's
partition has a run in a block, the walk's loop kept a register less for
it, two spills an entry. Out of line for the form's reads
(`Blob::read_exts_inline`), the scan through a form counts 986
instructions under the main line's and the point read through one nine;
kept out of line for every caller, the raw scan read 150 over. The same
count read the point read through a form 1.8 over at first, one helper
(`ZFile::view_of`) left a call where the main line's build had it in the
read. The probes are `bench/examples/read_only.rs` and `lag0_probe.rs`
with `supdb-nocachezpages` beside `supdb`, the toggles as above, built
at `origin/main` from this tree's example sources and at the tree.

And then it was priced, last, and lost. `bench/scripts/pair.sh
supdb-cipherauto supdb-cipher 5000000 --bound 671088640 --passes read
--reps 2 --ops-div 10` (run with the arms named `supdb-cipher` and
`supdb-ciphernozpages`, at `b4d555bc`): five million keys under 640 MiB,
the controller buying the form against the same store with the form off.
The controller bought, nineteen form files, and 470,000 of the pass's
500,000 reads went through them at 0.60 and 0.75x of the store without
in the two sittings, 0.69-0.81 major faults a read against 0.49. The
builds were still reading the device inside that pass (a fault 75-80 µs
against 30), so it was run again whole, five million reads a sitting
(`--reps 1 --ops-div 1`): the faults level at 30 µs, every read through
the form, 0.60 faults a read against 0.43, 0.73x. A form page is four
kibibytes and an envelope's chunk is sixteen bytes less, its tag stored
beside it, so every form page lies across two chunks and a cold read
through the form takes two pages of ciphertext from the device; the raw
read it replaces takes two as well, a hash slot's and a record's, but
the hash slots are an eighth of the bytes and stay resident, so it takes
about one. "ZPAGE 4096 against a chunk's 4080: a form page is two
chunks" was a line in the plan for this work, written before any of it,
and was not priced until the form was built, tested, mutated, gated and
pushed with the controller on. So the controller leaves a store under a
cipher in the raw form as shipped (`Options::zpages_under_cipher`, off;
`cipher_controller`), `ZPages::Always` builds the form there, and the
arms with the controller on are `supdb-cipherauto` and
`supdb-ingestcipherauto`. What it wants is an envelope whose chunks are
whole pages, the tags kept apart (`ROADMAP.md`). The controller did not
leave the form in five million reads either, its model reading the raw
form at 0.29 faults a read and the form at none while the census read
0.60 through the form: under a cipher the model's budget is of two
caches in series, the store's and the kernel's, and nothing has held its
prediction to a measurement there.

The rules: a feature that "cannot work" under a mode is a claim about
the code that does the work today, and it is checked at each place that
code touches what the mode changes -- here a mapping opened by a path, a
section borrowed whole, and four file calls -- before the feature is
given up; an advice forwarded through the type every caller already
holds reaches the callers nobody listed, where a branch at each call
site reaches the ones somebody remembered; tests that pass at their
first run are run against the mutants before anything is said of them;
and "pays nothing" is said of the operations that were counted, by
name, with the ones that were not -- every read path a store has, each
form of it, is a row of the count, and a path left out is a path the
claim does not cover. And a mechanism made to work under a mode is
priced under the mode before its default is chosen there, on the cell it
exists for: this one existed for reads out of core, and the first
out-of-core pairing was the last thing run. A line in a plan that says
why a thing may not pay is an experiment owed before the thing is built
to completion, since everything built after it is built on its answer.

A plain store's partition is named by its fences, sixteen bytes of two
keys, so the names cannot go to the directory either: an envelope is named
by a token drawn for it, the names live in the sealed manifest beside the
tokens, and an envelope neither slot names is swept at the open, as a
segment no manifest names is.

The keys were decided on a count. The first sketch sealed everything under
the caller's cipher, with an envelope's id as its nonces' prefix: eight
random bytes against a key that seals an envelope a seal and a merge, and
the chance of two prefixes alike passes the 2^-32 a cipher of this kind is
held to at sixty-five thousand files, days of a busy store. So the
caller's cipher wraps one key, the store's own, in the marker, once at the
creation and once at each open, which is also what lets the caller's key
live somewhere slow, and lets it be rotated by writing a hundred bytes
(`cipher_rewrap`); the store's key wraps a key a file, in the manifest for
a segment and in the header for a log file; and a file's bytes are under
its own key, where a nonce is a chunk's index or a record's count behind a
prefix drawn at the open, so a log reopened after a crash never meets a
nonce its torn tail used. What is authenticated is every byte of a segment,
an index, a manifest and a log record; what is not is the store taken back
to an earlier state of itself, the directory put back whole or the log cut
at a record's end, since nothing in a directory can say a newer one
existed. `tests/cipher.rs` holds that as a property: a byte changed
anywhere is a store refused or a store that opens as a state it
acknowledged, the last or one before it, and never anything else.

The fuzz is the test that matters most (`ZFUZZ_CIPHER=1`, and
`fuzz_range_under_a_cipher` in the gate): `z` under a cipher and `r` in the
clear over the same operations, against the model. A crash there is a
`drop`, and a plain store's dropped files are whole in the page cache
where a store under a cipher has lost every memory file it had, so the
order a crash is survived by -- the log kept until a manifest names a
durable segment -- is held by that run more strictly than by the plain
one.

Four things building it, each a shape. The rewrite of the engine's file
calls was mechanical, and one of them was not a file: `File::open(&self.dir)`,
a handle on the directory for the pending manifest's helper to sync later,
became `vault::open(&self.dir)`, which under a cipher opens the marker
file, and the directory's fsync would have been a file's, in an arm no
test runs under a cipher. It was found reading the list of opens the
rewrite left and has a call of its own now (`vault::open_dir`); a
mechanical rewrite is read at every site for what the path names. The
first test's control, the same store in the clear, was to find a fence in
a file's name and found none: every key shared its first sixteen bytes,
the fence's length, so the plain store was one partition named with no
fence, which this file already says of keys with a long common prefix; a
control is given something to find, and asserts it found it. The test of
a log cut at every byte passed its first run with every cut opening as
the whole store, and the assertion that the cuts fall in several states
is what said so: a store dropped is not a store crashed with its tail in
the log, since the drop joins the seals in flight and lands them, so the
fixture ends in commits small enough to stay under a seal. And the first
envelopes were written with nothing about the memory files' pages or the
device thought of, which is the paragraph below.

Forty mutants of it, and seven outlived the first tests, in the three
ways this file already names. Two were checks behind a tag: a record of
no bytes refused before its tag was looked at, and a log's name checked
before its wrapped key, each of which the tag refuses anyway -- the first
was taken out, since a check nothing can reach is a line nobody can hold,
and the second is the kind byte's reader and stays. Two were checks no
input could reach: a slot whose files are not whole files and a slot that
lists more files than its manifest names, both behind the slot's own tag,
held now by a slot sealed under the store's key in the test and by the
count's check moved to where a test can hand it a count
(`Vault::adopt_all`). And three were tests that did not look: a replay
that answered the log's header as its length cut the log to nothing at
the open, and a store that crashes twice in a row, the second time before
it has written anything, is the test that reads that; an ordered index
adopted where the directory held none; and the compact form left on,
which the door had made harmless -- its builds failed at "not a
directory" and the store went on -- so the test counts the builds begun.

What the first shape cost was read off pairings against the same store
in the clear,
`supdb-cipherbase`, which ran with the options a cipher fixed and no
cipher (`bench/scripts/pair.sh supdb-cipher supdb-cipherbase 300000`, and
at 100000), and against `supdb` for what the mode cost with what it
forwent (`pair.sh supdb-cipher supdb 300000`). The twin was an arm for as
long as a cipher fixed an option, `0ea02c5f` the last commit with it:
a cipher fixes none now, and the pair of `supdb-cipher` is `supdb`
itself (`bench/DESIGN.md`). The first
pairing, at a hundred thousand keys, read three things in both sittings:
the loads at 0.77-0.89x, the scan pass after the shuffled load at 0.8x,
and nothing else. The scan pass was faults. `SUPDB_LAG_COUNTERS=1` read 220
on the scanning thread against a plain store's 40 over the same fourteen
megabytes, since a memory file's pages are four kilobytes and a first
touch maps sixteen of them where the plain file's megabyte folios map one
a fault; whoever opens a kept file maps every page of it now
(`MADV_POPULATE_READ` in `vault::map`), the pass takes fewer faults than
the plain store's, and the cell reads level. The loads were the drain, and
the drain's stamps (`SUPDB_DRAIN_STEPS=1`) said where: from the seal's
readable landing to its fsync, 5 ms in the clear and 25 under the cipher,
for a segment of fifteen megabytes whose cipher is three of them. A plain
segment's bytes go to the device while the segment is built -- the writer
hints each piece and a helper syncs behind it -- so its fsync finds little
left to write; an envelope is written after the segment is finished,
whole, because the envelope's header is every chunk's associated data and
holds the file's length, so nothing can be sealed before the length is
known. Two levers were tried on the write itself. Past the page cache
(`O_DIRECT`, a megabyte a write), on the reading that the ciphertext's
fresh pages were the cost -- `cargo run --release --example crypt_cost`
reads the envelope's writer at 1.3 GB/s where the cipher alone runs 7.9
-- the step grew from 25 ms to 35, each piece waiting for the device where
the cached pieces go to it together: refuted and taken out. Each piece
hinted to the device behind the cipher (`sync_file_range`, `vault::Ahead`)
took it to 21 and stays. What is left is the format's, and is the first
item under this one in `ROADMAP.md`: at three hundred thousand keys the
ordered and the shuffled load read 0.83-0.86x of the same store in the
clear in both sittings, the buffered arms' 0.76-0.84x (`pair.sh
supdb-ingestcipher supdb-ingestcipherbase 300000`), the full-rewrite
burst a few percent behind in one sitting, and the scans and the mixes
level; against `supdb` the shuffled load read the same and the ordered
load 0.6x, which was ordered ingest's run forgone and not the cipher (the
run under a cipher, above). One
cell is not explained: the first single-threaded pass of point reads read
0.9x at a hundred thousand keys in five sittings of six, and at three
hundred thousand 0.95x in both sittings of the buffered pair and
unresolved in the durable one, with the threaded passes after it level;
with megabyte folios allowed for shared memory
(`/sys/kernel/mm/transparent_hugepage/hugepages-1024kB/shmem_enabled`) it
read 0.93x and unresolved, and a merge that lands later under the cipher
would do the same to a pass of fifty milliseconds. An open decrypted the
store and a store held a descriptor a file for the file's life, two a
segment; both went with that shape.

What the second shape costs was read the same way, and in core it costs
more than the first, which is the trade: `pair.sh supdb-cipher
supdb-cipherbase 300000` read, across its two sittings, the loads where
they were, 0.84-0.92x; the single-threaded pass of point reads at 0.92x
with the threaded passes unresolved; the scan passes at 0.89-0.94x and
0.80-0.90x threaded; the lag sweep's scans at 0.84-0.86x over the drained
store, 0.65-0.80x after the one- and the ten-percent burst and 0.80-0.82x
after the full rewrite; the threaded scan mix at 0.78-0.87x; E at
0.83-0.90x; and the other mixes level or unresolved. The count named
the point read's share: 229 instructions over its twin's 841
(`bench/examples/read_only.rs supdb-cipher 100000 1 n-none` beside
`supdb-cipherbase`, the toggle as above), spread over the asks -- each a
call through the source's pointer and a load of a chunk's state -- the
window a record is parsed in, the look at whether the cache is crowded
and the look at whether a read failed, with no one of them a fifth of
it. It is 339 over a twin of 818 since the plain store was made to pay
nothing: what a read under a cipher does at its start and its end is
two calls out of line where it was one wrapper, and its first ask is
for the section whole, refused with an error made and dropped
(`not_lent`), ninety-five instructions of it in all. Two cheaper shapes
of each were counted and each cost the plain read three or four
instructions -- the end's look answered as a verdict and the answer
replaced in line, and `Blob::flat` asking through an `Option` -- so
they stay as they are, and the read under a cipher is an open item in
`ROADMAP.md`. The scans were at 0.25-0.45x of the twin before they were
given a window of their own: a scan by the piece asked for each record's
key, its extents and its values apiece, and it asks for sixty-four
records' span at once now and walks the window with the loop a slice is
walked by (`Blob::scan_pieces`, `walk_fixed!`). The window's loop had to
be a function out of line: written into `scan_at` beside the slice's, it
cost the plain scan five instructions an entry, by what the optimizer
then left out of line, and named there as a function to call it cost
three to four, which is why the plain walk refuses the source and the
engine takes the other up (the plain store's price, above). What reads
by the rank still asks by the record -- the
sparse forms' walks, the merge path, the cursor -- and that is the
bursts' cells and E, and an open item in `ROADMAP.md`.

Out of its cache it was priced against itself: `pair.sh supdb-cipher64
supdb-cipher 1000000 --reps 4`, a million keys under a cache of 64 MiB
against the same store under a cache that holds it. Point reads 0.17x on
one thread and 0.11-0.13x on two and three, the scan passes 0.26x and
0.15-0.20x threaded, the lag sweep's scans 0.23-0.39x before the full
rewrite and 0.11-0.19x after it, the mixes that read 0.23-0.66x and A and
F 0.75-0.92x, the ordered load level and the shuffled within a tenth and
unresolved in four pairs a sitting. A miss is a chunk copied out of the
envelope's mapping, which the kernel's page cache holds, and decrypted in
place into a fresh page of the memory file, with a megabyte given back
behind it: four microseconds, of which the
kernel's fault on the fresh page is over half, the copy a fifth and the
cipher a tenth (`perf record -e cpu-clock --call-graph dwarf` over `cargo
run --release --example cipher_store 1000000 64`, the samples under
`Paged::slow` by their leaf). That is a fault a page where a plain store
out of memory pays a device's answer, and a store larger than memory pays
both; a run of chunks populated and decrypted as one is the open item.
The budget is of the files' plaintext. A file being written is in memory
whole until it has its envelope, a seal's output or a merge's piece,
which the cache counts and cannot give back; and the block cache held
more than the 64 MiB at that pairing's end (`cache_bytes_at_end`), as it
does beside a plain store, under its own bound (`scan_cache_bytes`).
`cargo run --release --example cipher_store` prints an open's time and
the cache's counts through a pass of reads and a scan, for a cache of any
size.

The first pairing out of the cache read the pass after the full rewrite
at a twentieth, 1.3 s for a pass of 45-60 ms in the cache, and six things
were under it, each found by a count and none by reading. The suite
carries the cache's counts for that (`SUPDB_LAG_COUNTERS=1`, the
`lagcounterscipher` line under `lagcounters`), beside the scanning
thread's CPU and sleeps (`SUPDB_SCAN_OPS=1`, `lagops`) and the device's
bytes (`lagdev`).

The store dropped its own ciphertext. `lagdev` read 53 MB from the device
in that pass and nothing in the passes before it, and the scanning thread
slept once a scan: an envelope whose plaintext the cache took whole was
dropped from the page cache where it was written (`POSIX_FADV_DONTNEED`),
which was right for as long as a store held its plaintext for life and
wrong from the day the cache gave any of it back. The kernel's cache of
the ciphertext is what a miss is read from, and the kernel's to reclaim.
The rule: what one shape made free is priced again when the shape goes;
the drop came through the redesign unread, because nothing failed.

The builders gave up. Over the rewriting burst the upkeep thread built
eighty blocks where its twin in the cache built fifteen thousand, so the
pass's scans built twelve thousand themselves, 136,000 first touches on
the scanning thread against 25-33,000 at the points before: a builder
that found the cache crowded gave its forms up, having no way to let the
cache give back under its one pin. A builder rests between two blocks now
(`Reader::build_rest`), where every local it holds is a copy, a position
or a table. The argument is by the loop's top at each site and by who
runs the loop -- the upkeep thread, the builder ahead, and the writer
inside one of its own operations (`WriterOp`), never a read that may be
nested in a scan's callback, which would rest the scan that lent the
callback its bytes -- and what holds the argument is the fuzz under a
cache of sixteen kilobytes with the block cache on, where a borrow read
past a rest is read out of pages already given back.

The evictor waited eighty milliseconds for one reader
(`PagerCounts::longest_wait_us`), and the instrument that named the
reader was built for the hunt and is not in the tree: a thread id a slot,
written at the pin; the evictor printing the name of the thread it had
waited eight milliseconds for (`/proc/self/task/<tid>/comm`) and running
`gdb -p <pid> -batch -ex 'thread apply all bt 16'` on its own process at
that moment; and a stamp at every pin and rest that printed the stack
where a stretch of eight milliseconds without one ended. gdb's first
capture had the writer in `Wal::sync` inside `commit`, pinned, and the
upkeep thread and a merge asleep in `breathe` behind it: a durable
store's every commit held the cache shut for the length of its fdatasync,
which reads nothing of the store's files (`Db::sync_wal` under
`Reader::unlend`). The stamps split the rest three ways: the fill's
sixty-four blocks between two looks, a look a block under a cipher now;
waits on another thread; and stretches of eight to fifteen milliseconds
in appends that read no file and take a fifth of one, which is a thread
descheduled with its pin, on four cores with seven threads to run.
Nothing annotates a timeslice away, so the wait for a cache past its
budget has two lines: the store's own work waits at the first
(`Pager::crowded`), a merge and a builder at their rests, and a caller's
read at the second, half the budget past it (`swamped`). A read waited at
the first before, twenty milliseconds for an overshoot of megabytes.

Every fill of a cache at its budget took the evictor's lock to tell it
so, and with three threads filling the scanning thread slept on that lock
two thousand times a pass (`lagops`, `vcsw`, where its wall was its CPU
plus a tenth): a flag the evictor clears before it looks, and the lock
for the first to set it (`Pager::nudge`).

And two that the suite did not show and the builders' test did. A file
with no envelope yet is counted in what the cache holds and cannot be
given back, so a seal's output in flight read as a cache past every line
for as long as its sync took, and each merge beside it slept a breath at
every rest: the lines are drawn against what could be given back
(`Pager::could_give`). And `settle` waited for the upkeep thread's pass
pinned: the pass filled the cache, the evictor waited for the settle's
pin, and the pass waited a breath for the evictor and gave its forms up,
three waits in a ring that a timeout ended, every time (`join_upkeep`,
`hold_within`, `hold_for_upkeep`, each wait under `unlent`).

With the six the pass after the full rewrite reads 0.11-0.19x where it
read a twentieth, 140-510 ms in the probe's reps for 1.3 s, in two shapes
by whether the upkeep thread had built the burst's blocks when the pass
began, and what is left of it is the misses under the blocks the scans
still build; E went from 0.53-0.57x to 0.66x and the shuffled load from
0.76-0.80x to within a tenth. The cells that are a miss an operation did
not move, as they should not.

The builders' test
(`the_block_caches_builders_build_through_a_cache_the_store_does_not_fit`)
took five shapes to say anything. With commits of a thousand over the
store's small files nothing crowded, and the old code passed it. A burst
that rewrote every key built its blocks without reading one, since a
rewritten key's record is masked: the burst adds a value to every key
instead. A seal followed the burst and ended the pass (`seal_max_pct`,
`seal_idle`, off for it). Then the assertion failed on the old code at
some eight hundred blocks of nineteen hundred, which was read as the test
telling the two apart, and it failed on the fixed code at the same count:
the settle's wait above. It is held to two mutants now, the look without
the rest and the settle's wait left pinned. A fifth was mine: a script
that sliced the test file between two markers found the second marker
before the first, replaced the empty string, and wrote the insertion
between every two characters of the file. The rules: a test is run
against the fix before a mutant's failure is read as anything, since a
red mutant and a red fix at one count are a bug the test found and the
fix did not have; and an edit by markers asserts the markers' order.

One seed of the fuzz outside the gate's range failed on the main line as
it did here ("a copy's identity entry disagrees with its search", a plain
store with the upkeep inline). It was found by running ranges the gate
does not, and attributed by building the main line's head and running the
seed there before reading it as this change's; it is the third coming
of the entry "a rank kept across the rewrite that moved it", below.

And one the hunt did not find, because the store read right in every
run of it: a chunk lent out of a unit marked twice. One seed in three
hundred of the fuzz under a cache of sixteen kilobytes, four seeds at a
time on four cores, ended in a merge that wrote its keys out of order, a
key outside its piece's fence, or a value that was another's; the same
seed alone, a hundred and sixty times, in nothing; and the gate's range
never. It was there on the commit before the day's work, which is the
first thing that was run. A page the cache gives back reads as zeroes,
so the read that went wrong was nowhere near where it showed, and the
instrument was to make it a fault: `SUPDB_PAGER_GUARD`, under debug
assertions, makes pages given back unreadable until they are filled
again (`pager::guard_on`). The first run under it faulted in a point
read's load of a hash slot, a few instructions after the ask that made
the slot present; a probe at the ask's end said the range was readable
there; and the reader table printed from the fault had the reader pinned
at the epoch as it stood, so the evictor had been right not to wait for
it. The reader had taken a chunk on its state's word. A reader that
finds its chunk dying takes the unit back and then says the chunk is
lent, two words and two steps, and a thread that lost its core between
them stayed off it while the evictor ended the pass the rescue was from
and marked the unit again in its next, the chunk dying still: said to
be lent then, the chunk stood in a unit about to be given back, and the
reader after it, pinned after that mark, read it as it went. The reader
that says so looks at the unit again now, and where it is marked puts
the chunk back and takes the unit back from that mark too
(`Paged::slow`); `a_chunk_is_not_lent_out_of_a_unit_marked_again_under_its_rescue`
walks the interleaving by hand. `Paged::free` had met the state and
explained it: it emptied chunks that were dying "or revived by a reader
that took the unit back from an earlier mark, was waited out by this
one, and has gone", which is the bug described as a case. It asserts
now that no chunk of a unit given back is lent. The rules: a state met
where the design says there is none is asserted, never given a
sentence; a transition of two steps over two words is looked at again
after the second; a failure that needs the machine busy is run busy,
several seeds a core, before a range is called clean; and wrong bytes
are made a fault before they are chased as wrong answers.

The rules of the hunt: a wait under a pin is a pin, and so is a
timeslice; a wait is drawn against what its remedy can move, and at
different lines for work that can wait and a read that cannot; a count
that includes what cannot be given back fires the wait for nothing; and
the instrument that names who is waited for is built before the first
guess, as it was the first time, and was taken out both times.

The rules: a second way to hold the same bytes is priced on the first
operation of every pass as well as on its rate, since a mapping's first
touch is where two kinds of memory differ; a write slower than its cipher
is stamped step by step before a lever is chosen, and fresh pages were the
wrong lever here as they were for the buffered load's sync; and a mode
that fixes options is given a twin arm with the same options and no mode,
or its pairing prices the options.

The first tests of the browser's half changed every byte of the record
and required a refusal, and seven of fourteen mutants of `crypt.mjs`
survived them: a header with a wrong magic, version or shift fails every
tag as well, so the three checks could each be deleted, and two checks of
the object's length each caught what the other was for. A check behind a
tag is held by asking the parser alone or by an input made under the
key, as a check behind a checksum is held by a forged one, and of two
checks on one quantity the test has to reach each where the other
cannot.

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
before writing it. A magic that moves moves the store's format version
with it (`STORE_FORMAT`, in every manifest's body behind the names, which
a reader from before it never reads), and `STORE_FORMAT_OLDEST` says
whether the release still reads the version before; a unit test holds the
version to the files' magics, so a magic moved alone fails it. Version 2
is the run's place in the sequence: no segment's magic moved, since a
reader of segments reads them as it did, but a reader of the log from
before takes a run's numbers for records missing and the WAL's new kind
for no WAL at all, so the manifest refuses it first, and a version-1 store
is rewritten as version 2 at its first open, before anything of the new
kind is written into it.

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
frame fails its CRC and replay stops before it. A write the device refuses
is put back before the commit answers it -- the file cut to its last good
length, the batch's commit frame taken off what is pending and its
sequence number given back -- so the next commit writes the batch again
under its own frame and the log holds no byte a replay would misread; a
sync that fails is kept and never retried, and every commit, sync and
flush answers it until the store is reopened, so nothing after the failure
is acknowledged over frames the device may not hold (`Db::lost`), and the
open finds a prefix of what was acknowledged before it. Seal is write to a temp name,
rename into place, publish to readers, fsync, then the manifest -- one of
two slot files written in place and fdatasynced, the newest valid slot by
its publish sequence being the store at open, so a torn publish is the
slot an open ignores; a publish never writes the slot that holds the
newest manifest known durable, the last whose fdatasync returned, so the
slot it does not write is one the device holds whole, and the open
fdatasyncs both slots as it found them, the one it takes and the other,
before it removes a file or writes over either (`Options::manifest_slots`,
`SlotChoice`; the shape before it wrote
a temp name, renamed it over the old manifest and fsynced the directory,
`supdb-ingestmanifestrename`, and there a seal's names are made durable
by that publish's directory fsync, on a filesystem that puts a rename
on the device no sooner than the renames before it; an open under that
shape fsyncs the directory, and any pending segment it takes from a
store that ran under the slots, before it removes anything, since the
manifest it read may be a rename a kill left in the page cache, and then
removes the slot file a life under the slots left, the manifest it read
written as `manifest` first where that file held it, so that an open
under the slots after it makes the file again, fsyncing the directory
before it makes it and after, before it removes anything). The open removes nothing -- a segment, an
envelope, a log file, a spare -- and writes no slot until the slot it
takes is checked against the directory, and refuses with every file as
it found it where that slot cannot be the store
(`Db::open_refusals`, `Db::unnamed_refusals`): a name it lists that is
not there; no slot readable beside `par-` or `pcs-` files, or under a
cipher beside envelopes, which only a store with a manifest writes; and,
in the clear, a segment it does not name, newer than every segment it
names and within what it covers -- a merge's output or a run's -- whose
last key nothing it names and no run's temp name holds, read by that key
alone and only in the segments whose fences take it in. Under a cipher an envelope the slot taken does not name has its
key in no slot the open can read, so it is not held to anything: the
open sweeps it, and a fall back past a damaged slot that is not refused
answers without the writes only that slot named (`ROADMAP.md`, both). A
crash leaves an unnamed segment only from a job whose publish never
became durable, so nothing was retired behind it and every window opens
as before; damage that makes the open pass over a durable slot leaves
one whose log files and temp names may be gone, and a seal's segment
past what the slot covers is swept as the window's even then, since
under a buffered policy a torn publish of its landing leaves the same
files as damage past a log that retired. A seal that fails at any step -- its
landing, its directory's fsync there, the fsync or the envelope of its
data before its readable landing or after it, a closing run's presync,
and under the rename shape the publish that names it --
stops the seal queue for the life of the open (`Shared::seal_wedged`): no
seal lands and the segment work writes no manifest from its first look
at the queue after the failure, the writer's presync's included
(`Maint::seals_stopped`), so nothing names its segments durable, covers
its sequence or retires the log that holds its writes, and the fsync is
never tried again; readers go on reading what was published, every
`flush` and `settle` answers the failure until the store is reopened,
and so does every call that has to land a seal for room -- with the
segment work inline every `seal` and every commit at the threshold, the
commit's batch in the log first -- a `sync` answers nothing of it since
what it promises is in the log, until a write is dropped for want of
room behind the stopped queue -- a write that ends an ordered run, or the
first write after a `sync` handed the live table to a seal, each of
which freezes a table -- and from that write on every commit, `sync` and
`flush` answers it; and the open replays the log and sweeps what no slot
names.
Under `manifest_pending` the slot the seal's readable landing wrote
before the failure stays, naming the segments pending and covering their
sequence, and the open's rule for a pending name is what reads it. A
run's close whose records the writer presyncs lands durable only at the
writer's own ask, after the presync (`SealJob::awaits_writer`). A flush's drain is answered once its
seal's fsyncs are done and the slot is written, and the slot's fdatasync
follows on the segment work's thread with the WAL's and temp names'
retirement behind it (`Options::flush_waits_manifest` is the shape that
waited, `supdb-waitmanifest`): the round trip was a tenth of a
ten-thousand-key drain, and a crash inside it is a torn newest slot, the
case the slots were made for, since any publish before that fdatasync
writes the same slot again and never the other, giving the owed sync
up before its write begins, so a write that fails leaves nothing to
name the slot durable. A buffered store's flush commits what is
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
that syncs and rotates at every seal instead). An open takes nothing it
replays as durable, since a kill leaves the page cache whole: the older
files it applied frames from go on the loose list, the live file's
frames count unsynced, and a header the open writes over a file it found
without one says loose where an older file is, so the first sync by
policy or `sync` after the open syncs the older files oldest first and
then the live one, and the first flush covers them by its segment
instead and is answered behind its slot's fdatasync, as a buffered
drain is; an open with nothing replayed, or whose cut of a torn tail
synced the live file, owes nothing. A log file whose first sector is
zeroes -- a log no sync ever covered, a page a power cut lost -- is a
log with nothing in it, newest or older, under either kind, and no frame
behind it is replayed: a sync covers the header with the frames, so none
was acknowledged unless the device changed bytes it had synced; the gap rules then read the file after an older one,
the log's end at a loose boundary and a record lost at a chained one,
since a chained header says the file before it was synced whole and its
zeroes are damage. A zero header over a first sector that holds anything
else is damage and refused. Readers wait for no
fsync and the manifest does, and no manifest is written while a seal's
segments are published and unsynced -- it would name a segment that may be
torn and cover a sequence the WAL still has to hold -- so a merge that
finishes in that window lands after the seal (`Maint::collect` holds it,
`publish` asserts it). The arm that writes the seal's own slot in that
window, naming its segments pending (`Options::manifest_pending`,
`supdb-manifestpending`), trades the order for a rule at the open --
a pending name counts only once the segment reads back whole, else the
slot before is the store, and the open that takes it fsyncs the segment,
its ordered index under `ord_durable` and the directory before it writes
the slot naming it durable, since a segment read back after a kill was
read out of the page cache -- and retires the WAL only behind a slot that
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
count of zero is one whose landings the state shows. A store is held by one open at a time, under an exclusive `flock` of
its `LOCK` taken before the open reads its directory -- an open sweeps the
segments its manifest does not name, which beside a live store are the
seals and merges in flight -- and given up last at the close; a test that
emulates a crash calls `Db::crash`, which gives the lock up as a dead
process's is given up, since a `mem::forget` keeps it and the reopen is
refused as a second open. A crash between any
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
store's greatest -- every key a segment holds or a frame of the log names,
a delete's included, since recovery replays the log's frames as newer than
the piece it rebuilds from a run's file -- goes to an ordered memtable and,
at each commit, to a segment open for append: the batch's records, then a
commit marker carrying their CRC, length and count, then one fdatasync on
that file where the policy is due; a commit under `EveryN` that is not
writes the batch's whole pieces and holds the rest with the writer, and
a `sync` writes what the run holds and syncs the file, after the log
beside it -- an open run's only: a run in the clear closed by any write
it cannot take (a key at or below its greatest, an empty or overlong
key, a value that does not inline, a `put`, a `delete`) or by any seal
(`Db::seal`, or a commit's at the threshold) goes to its seal with what
the policy left unsynced, a `sync` before that seal lands does not cover
it, and a `flush` does. A run's first due commit syncs the log before
the run's file wherever the log owed a sync when the run opened -- the
frames of a table frozen before it, a file a rotation left loose, frames
an open replayed -- noted once at the run's open and asked in the due
branch (`Direct::log_owed`), since nothing writes the log while a run is
open in the clear; under a cipher the run's due commit syncs the log
behind its own frames anyway; the sync of a batch the run's file owes,
which a commit with nothing new for the run, a `sync` or a `flush` makes
whatever the policy (`Db::run_again`), stands in for a due one and syncs
the log first the same way. That orders the log behind the syncs the
policy makes and no others: the helper thread behind the run's writer
fdatasyncs the run's file every `sync_ahead` under any policy, the
kernel writes its pages back, and a run that closes before its first due
commit has its records synced by the flush that closes it (the presync)
and its whole file by its seal's fsync, neither waiting for the durable
landing of a seal before it -- so before the run's first due commit a
power cut may keep run commits nobody acknowledged and lose the log's
older frames (`ROADMAP.md`). The note is of the log alone: a run closed
into a seal still in flight owes its unsynced commits to that seal's
landing, and a later run's due commit syncs nothing of its file, so a
power cut after it may keep the later run and lose the earlier one. A
log sync that fails at the due commit -- kept where it is an
fdatasync's, returned as is where a loose file would not open or the
live header would not chain -- leaves the batch owed by the run's file,
so the next call with nothing new writes and syncs it, the log first.
The WAL never
sees them (`direct_ingest: false` is the arm that still sends them there).
Recovery walks the temp file's record stream to the last marker whose three
quantities agree with the records before it and makes what precedes it a
piece where it lies: the file cut back to that marker and finished as the
run's close would have finished it (`SegmentWriter::resume_run`), the walk
standing in for the table as the keys' source; what follows is a batch
nobody was told was durable. The run's
writer writes at the offset it counts, never at the file's cursor, so a
write the device refuses part way leaves bytes past what counts and the
next write lands over them; the batch is the writer's whole by then, so
the failure is held for the commit that syncs it, and the batch is owed
until a sync covers it -- no commit with nothing new, `sync` or `flush`
answers `Ok` before one has. A failed fdatasync of the run's file, the
commit's or the helper's behind the writer, is kept as the log's is, and
the run's close refuses to finish the file, since its fsync would be the
retry; a run closed while it owes a batch writes the batch into its
segment, durable with the segment once it lands. The CRC is
there because a crash can land the page a marker is on and not a page of its
batch, and a record whose bytes came back zero still parses. A run stands in
the store's sequence: its records take one number each, as a WAL record
does, from the number its file names in its superblock page (`run_note`),
so the frames and the runs written after it stand past it by exactly what it
holds, and the manifest's covered sequence is the one watermark for both.
The open walks the log from that watermark -- frames in their files, and at
each gap in them and at their end the run that starts there
(`Db::fill_runs`) -- and takes a run only where the walk reaches its start:
one that starts past the walk's end is commits a machine's crash kept
without the ones before them, and is removed with the frames past that
point -- unless its note says it began with everything before it behind
a sync (`RUN_NOTE_CHAINED`, a run begun where `Db::owes_sync` finds
nothing a sync would have to do), where what is missing before it was
promised: a record lost, and the open refuses, having asked every run
left before it removes one. A gap no run fills is the log's end where a header says something
before the file was left unsynced -- `WAL_LOOSE`, and `WAL_LOOSE_RUN`, the
kind a run that closes unsynced leaves on the file the next frame goes to
-- and a record lost anywhere else. A run the open takes becomes a piece,
and until the manifest covers its numbers an empty file named by the range
(`cov-`) says the gap is held. Every commit writes its batch to the run's
file, synced or not, as a commit writes the WAL; a sync by policy syncs
whatever was left unsynced before it, the loose files and the runs that
closed loose, ahead of the live log or the run in progress, and `sync`
syncs the run in progress too. The first sync that acknowledges a commit
of a run fsyncs the directory once, after the run's file, for the temp
name the file is under (`Db::sync_run`, `Db::dir_barrier`): a file's sync
does not promise the entry that names it, and a run whose name a crash
took is every commit of it gone. One barrier a run, none where a
rotation's barrier for its new log file had the name in the directory
already, and none in a store whose commits never sync, whose landings'
directory fsyncs carry it. And none where the run opened on a spare
(`Options::run_spare`): the store makes the next run's temp name empty
before a directory fsync it makes anyway -- the creation's, the open's
last, a run's close ahead of the landing its seal fsyncs the directory
at -- and counts its directory's fsyncs begun and returned
(`vault::dir_ticket`, `dir_synced_after`), so a run on a spare an fsync begun
after it has returned from owes its name nothing; a spare no fsync has
reached yet pays the barrier. An open keeps the empty temp name it finds
as its own spare and removes the rest only once it has taken the store,
so a refused open leaves them; an empty temp name is a run that
acknowledged nothing. A barrier that fails is never tried again:
each name it was for gets a second entry, a hard link under its name and
`.guard` (`vault::guard`), and the directory is synced again, a new
operation after the failure that the new fsync covers; the guard stands
until the file goes -- every removal and rename through the door unlinks
it first -- and an open that finds a guard without its file gives the file
its name back before anything reads the name. Where the guard cannot be
made either, the failure is kept (`Db::lost`): reads go on and every write
answers it, since a directory that cannot make a new entry durable cannot
make the next name the store writes durable either. The close is
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
the file from the arena, the whole write pieces it completes as the buffer
path's pieces reached it and the rest behind them, so the staged copy and
the piece buffer's pages are gone from the writer's faults and the copies
are the arm
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

**A file a store names never changes.** A segment, its ordered index
and its form file are each written under a temp name and renamed or
linked into place, and nothing writes one after that: no patch in place,
no writable mapping. `Db::checkpoint` rests on it. A checkpoint is hard
links to the files the manifest names, with a manifest of its own, so a
byte changed in place under a name is changed in every checkpoint that
shares the file, with nothing raised in either store, and a file rebuilt
in place -- truncated and written again -- is torn under whoever has it
mapped from the other one. A change that needs other bytes under a name
writes a new file and renames it over the old, which leaves the other
store's link on the old inode: an ordered index an open finds torn is
composed again that way (`write_ord`), which is why a checkpoint may
share one that was never synced. The checkpoint is made by the segment
work, the one side that retires a segment, so no name goes while it is
being linked; and it is built under a temp name and renamed, because an
open of a directory with segments and no manifest takes every segment
it finds for the store.

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

It came back as a mark: the open set the live log's synced mark to its
length, on the reading that replay read the frames back from the
device, and after a process kill it had read them from the page cache.
A `sync` after the reopen found nothing unsynced and answered `Ok`, the
first flush was answered before its slot's fdatasync with the log
covering nothing, and a power cut then took what both had acknowledged;
the older files replay read were not on the loose list either, so the
first sync would have chained the live header over a file a power cut
could still cut, and the open after that cut refused a gap it read as a
record lost. Every test reopened a store and read it, and none synced
after a reopen and cut the power. The rule: what an open rebuilds from a
log is as durable as the device has been told, not as the read found
it, and the open hands the live path the same marks the writes that
made it would have left -- unsynced, and loose where a sync is owed.
It came back at the manifest: the open took the newest valid slot as
the store and synced none, and a slot a drain had written with its
fdatasync owed is in the page cache alone after a kill; the reopened
store's first publish wrote over the other slot, the one the device
held, and a power cut tearing that write left no slot that named the
store. The open fdatasyncs the slot it takes now, and the other beside
it ("A slot chosen by its sequence", below), before it unlinks anything
a slot does not name. And it came back at the pending arm:
the open took a pending name whose segment read back whole, wrote a
slot naming it durable that covered the log frames holding its writes,
and fsynced nothing, so a power cut after the reopen lost the segment's
unsynced pages and the log was skipped past them; the open fsyncs a
pending segment, its ordered index where those are kept durable, and
the directory before the slot that names it durable. The corollary: a
check that reads a file back proves the page cache, and only an fsync
makes what it read durable. The first tests of that order read the
open's record of what it had fsynced and tore what the record lacked,
and the record said nothing of where the slot's write fell among the
fsyncs, so an open that synced the directory, or every file after the
first, after the slot passed them all; the slot's write is in the same
record now, and the test holds each fsync to a place before it. A record
the open keeps is the open's word, and an open that skipped the
directory's fsync and kept the word passed that too: the directory's
fsync is read off the calls themselves (`Db::trace_dir_io`). Each file's
was read off the kernel's page flags, which only root may read, so an
open that skipped a file's fsync and kept the word passed every run
without the privilege, the gate's among them; each file's fsync goes
through a call that records it once it returns (`vault::sync_file`),
and the page flags are a second word where they can be had. Where they
cannot, the page cache answers for a slot file without them: a drop of
a file's clean pages (`POSIX_FADV_DONTNEED`) leaves a page no sync has
written, which `mincore` then finds, against a control written and
synced by nobody that has to read as unwritten
(`os_page_kept_unwritten`); a call that recorded a slot's sync it never
made was caught by the page flags alone until then. The
corollary: an order is tested by one
record that holds both of its ends, since a record of each end apart
holds that each happened and not which came first; and the record is
made by the call, never beside it by the code under test.

**A sweep made before the choice it was for.** The open listed the
segments, removed every one its chosen slot did not name, and only
then found out whether that slot could be the store. With the newest
slot damaged it fell back to the slot before, swept the segments only
the damaged slot named -- a flushed run whose temp name had retired
behind it, the only copy of its records -- and answered `Ok` with every
flushed key absent; with the slot before naming inputs a merge had
unlinked, it swept the merge's output and then refused at a segment it
could not find, in words that named a missing file and not a damaged
slot; and with both slots emptied beside partitions it took the scan
kept for stores from before manifests, which takes `seg-` names alone,
and removed every partition. Every test of the slots tore a publish
still in flight, whose log still held its writes. The open removes and
writes nothing now until the slot it takes is checked against the
directory, and a segment the slot does not name is held to what
retired behind it: a run's segment, or a merge's, is swept only where a
run's temp name or the slot's own segments hold its last key. The log
was a third holder in the first version, and a mutant that took it away
survived every test: nothing a crash leaves puts a run's or a merge's
keys in the log alone, so the arm only let damage through. Both ends of
the segment were read, and a mutant that read either end alone survived
too: in every state one damaged slot leaves, the two ends are held
alike, and the last is the one a run's records show at, since a run's
keys sit above every key before it. And the docs promised every key
of the segment read where two were. The first version held a seal's
segment past what the slot covers to the log replaying it as well, and
a buffered store's torn publish leaves exactly that -- the segment
synced, the log's frames for it cut by the power cut -- which the open
must sweep; told apart from damage past a retired log by nothing in the
files, it is a gap and not a check. And the check ran only where the
open had passed over a slot, until a mutant that ran it at every open
survived every test: every state a crash leaves passes it, so the gate
was a second condition nothing could reach. The rule: a step that removes what
a decision rules out runs after the decision is made and checked, never
as part of making it; what it removes is held to the evidence of what
retired behind it, and only where every crash state leaves different
evidence; a refusal is tested by its own words, since a refusal
further in, after the damage is done, carries others; and the docs of
a check name what it reads, never what it stands for.

**A check added to the open, priced by nobody.** Stage 0 put on every
open an fdatasync of each manifest slot through a second open of the
file the slot read had just closed, a stat of every segment the slot
names beside the directory listing that already said which names were
there, two more listings of the same directory, the log replayed ahead
of a refusal that needed nothing of it, and, after a crash, every
segment the slot names mapped to look up one key whose partition the
names' fences already said. Each was argued to be free by its unit and
timed by none, since no cell of the suite opens a store: the suite's
stores are made, never reopened. A probe of the suite's own store,
opened a few hundred times with the shapes alternating over the same
files, priced it: the two fdatasyncs are a quarter of an open of a small
store and nothing at a large one, and the second opens, the stats and
the listings a step under what a pairing resolves at every size. The
open reads its slots once and syncs them through the descriptors it read
with, answers "is this name there" from its one sorted listing in the
clear (under a cipher the listing is of the names a slot gave the store,
so each envelope is still looked for), makes its checks before the
replay, and looks an unnamed segment's key up in the segments whose
fences take it in (`Db::name_takes`); the open's own reads are in the
process's trace (`vault::note`), which is what its tests count. The
first test of the slots' fold counted the read's own record and so held
it only against the shape it replaced, whose second open had made one
too: a second open by path for the sync made no record and passed. The
slot read and its fdatasync record which descriptor each went through
(`vault::note_file`, the number and the open file description's offset,
which a second open of the file does not share), and the test holds the
two to one. The rule: a step added to a path no cell times is priced by
a probe of that path before it lands, and every read it makes is first
asked whether the path already holds the answer -- a descriptor, a
listing, the fences in a name; and a fold that removes a call is held by
the identity of what the remaining call used, never by a count of a
record the removed call would have had to make.

**A slot chosen by its sequence, beside a sync left owed.** The
manifest's two slots were written by the publish sequence's parity, on
the reading that the slot before a publish is always durable; the
drain's slot then had its fdatasync deferred past the answer, and a
merge's or a promotion's publish in the same drain, or a second seal's,
wrote the next sequence into the other slot -- the newest durable one --
while the first was still owed, and dropped the owed sync. A power cut
tearing that write left the device the older slot, whose retirements
had run, and the open refused the store over acknowledged commits, or
both slots torn. Every test of the slots tore the newest slot by hand
on a store at rest, where the slot before it was durable. Each publish
now writes the slot that does not hold the newest manifest known
durable, and a slot is known durable when an fdatasync covering its
last write has returned (`SlotChoice`), so two publishes with no sync
between them write one slot twice; the tests watch every slot write
(`Db::watch_slot_writes`) and build what a cut during it leaves from
which writes a sync covered. The rule: of two copies kept so that one
stands while the other is written, the one written is chosen by which
is known durable, never by a counter, and "known" is a sync that
returned, never one issued or owed; and a deferral of a sync is checked
against every step that can follow it before the sync runs. It came
back on the failure path of the same window: the publish after an owed
slot writes that slot's file, and a write that failed part way returned
before the publish gave the owed entry up, so the owed fdatasync ran
after it, returned over the torn bytes, named the slot durable and
retired the log behind it, and the next publish went into the only
valid slot. A sync of a descriptor covers whatever the file holds when
it runs, not the write it was owed for, so a write into an owed slot
gives the owed entry up before it begins (`Maint::write_slot`), and a
test tears that write (`Db::tear_next_slot_write`); another fails a
slot's fdatasync (`Db::fail_next_slot_sync`), since a slot named durable
where its sync was issued, not where it returned, is invisible to every
test whose syncs succeed. The corollary: a
record of what a pending sync will make durable is dropped the moment
anything else writes the bytes it covers, on the path where the write
fails as on the one where it succeeds. And the open's own sync of the
slot it takes was held only by a test that reads the kernel's page
flags, which only root may, so in the gate it ran nothing, and the
order it keeps -- the slot durable before the sweep unlinks what the
other slot names -- was called unobservable on the reading that no
unsynced slot names less than the one before it, which a kill between
a merge's slot write and its fdatasync makes false. A seam fails that
sync (`Options::fail_open_slot_sync`) and the open is held to leaving
the directory as it found it. The corollary: a check whose only test
needs a privilege the gate does not have is a check nothing runs, and a
claim that an order is unobservable is checked against the kill between
every write and its sync before it is written down. That seam failed
the open's sync whichever file the open had opened, so an open that
synced the other slot passed it and every test but the root one; it
names the inode it failed for now, and the test holds that to the slot
taken. And the upgrade's order -- the newest slot written first, the
one the store was read from after it -- was held by nothing, both
writes being synced and no test cutting between them, until a seam tore
the open's first slot write (`Options::tear_open_slot_write`). The
corollary: a seam that fails an operation names what the operation was
on, since a mutant that aims the operation at the wrong object fails
through the seam as well. And the owed entry carried its slot and its
sequence for a test's record, while the state that owes it already says
both: a word stored on a drain's path that only a seam reads is a cost
the seam was to be free of. The docs then said the open syncs its slot
before it removes any file, and under a cipher the vault's sweep of
envelopes neither slot names ran before the sync; the order was safe and
the sentence false, and the test made an exception for the mode instead
of the code keeping the sentence. The sweep runs after the sync now and
the test holds both modes to a directory left as found. And the slot
landed by the open's own writes -- a store's creation, an open with no
manifest, a run recovered from its temp name -- and by an inline
publish was held by no test, since every test's first publish came
after a flush the test had settled and synced: a test tears the first
publishes after each of those. The corollary: a sentence of a contract
is checked against every mode before an exception is written into the
test that holds it, and a choice seeded at several places is held at
each place, by the first write after it. And the open synced the slot it
took and left the other to the kernel's writeback. A publish into the
slot whose fdatasync was owed, the next publish into the same slot -- the
other holding the newest manifest known durable -- and a kill part way
through that write leave the page cache holding the slot torn and the
device free to hold the owed publish in it whole, its writeback having
run before the torn write dirtied the page again; the reopen passed over
the torn slot, took the other, swept the owed publish's segment and
acknowledged commits, and a power cut before anything wrote the torn
image back handed the next open the owed record as the newest slot, which
refused the store over a segment that open's sweep had removed. A model
of the two slots found it, and every test had built the device's slots
from what a sync covered and the cache's from what the cache held,
never a slot torn in the cache over one whole beneath it. The open
fdatasyncs the other slot too, whatever it read as, where the file is
there -- a newest slot passed over for pending segments included, which
the open writes over itself but only after its sweep, so a kill between
the two leaves the same device -- and a seam fails that sync (`Options::fail_open_other_slot_sync`);
both syncs are read off the calls that made them (`vault::sync_slot`,
`Db::trace_dir_io`), which holds the open's own sync without root; the
seam's failure stands in for the fdatasync's result inside that call, so
a record made before the sync returned, or a sync run and failed after,
is a record of a failure, which the test refuses. The
corollary: of two copies kept so one stands while the other is written,
an open that picks one settles what the device holds of the other before
it acts on the choice, since a copy read as torn from the cache may be
whole on the device.

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
gate before pricing it as the default. It came back as a mode
beside an arm: the open's check that a pending slot's segments read
back whole walked each with the whole-section scan, which a segment
read by the piece refuses, so under a cipher every pending slot read as
torn and the open took the slot before it, whose log the pending
landing had retired; no test had run `manifest_pending` under a cipher
and cut the power. An arm's crash test runs under every mode the arm
can be set beside. And it came back as the arm itself: the
manifest's rename shape (`manifest_slots` off) was held by reading
alone while three fixes to the slots were built and tested beside it,
and the first crash states built under it found three holes the reading
had named and nobody had made. A publish whose directory fsync failed
after its rename counted the seal out with the queue going, so the next
landing's fsync named the seal's segments durable over the entries the
failed one was for and retired the log; an open of a manifest a kill had
left renamed and unsynced swept what it stopped naming, and a power cut
could keep the sweep and lose the rename; and an open of a store that
had run under `manifest_pending` took its pending names, read out of the
page cache, without the fsyncs the slots' open makes. A fourth was
found after those were called closed, in the crossing back: a store that
ran under the slots, then the rename shape, killed between a rename and
its directory fsync and opened under the slots again found both slot
files, the first life's `manifest.alt` among them, so it made neither,
fsynced no directory, and swept what the renamed manifest stopped
naming; the tests of the crossing had stores born under the rename
shape, which have no second file. The rename open takes the file away
now, before it removes anything else and answering a removal that
fails, and the slots open makes it and fsyncs the directory before it
removes anything, the cipher's sweep of envelopes included, which ran
before that fsync. A fifth was in that fix: the slots open fsynced the
directory after it made the file, so an open killed between the two, or
whose fsync failed, left both files, and the open after it made nothing,
fsynced nothing and swept; the test of the failed fsync reopened and
asserted only that the reopen succeeded. The directory is fsynced before
the file is made as well now. The corollary: an arm kept for pricing
is a shape a store can be opened in, so every crash state built for the
default is built for the arm, and across a change from one to the other
and back, with what each life leaves for the next, before a fix to the
default is called done; and a marker that says a step is done -- here a
file whose presence says the crossing is made -- is made only once the
step is durable, and its test runs the open after the one that failed. And a seam a test needs on such an arm's path
rides the word that path already loads (`vault::sync_dir_as`), never a
load of its own on every publish.

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
scan signal, a handle bumps once per commit and not once per scan. And
a third time as a count kept on the thing counted: a form file counted
the groups its walks decoded into their own buffers in one word of the
file's, which was once a scan while such decodes were an arm's and
became once a group when a walk's first touch of every group was made
one (`Options::zpages_scan_admit`). Every single-threaded cell of that
pairing read ahead and the threaded scan pass read 0.90-0.96x in both
orders, two and three handles storing to one line a group; with the
count on the handle's memo, beside its hits and misses, the same pass
read level. A statistic of a shared structure is kept by whoever does
the counting and summed where it is read, and a counter is priced again
when a change moves the path it sits on from the rare case to every
operation.

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

**An order that held by fence.** The pieces sorted by fence and then by
sequence, which is an age order wherever the pieces that meet share a
fence -- every piece aligned -- and wherever a piece cut at fences a merge
replaced meets only pieces cut finer, which every merge over given ranges
cuts. A first partitioning planned against a store with none joined a merge
in flight that was a first partitioning too, and went on with no fences
over the store the join had partitioned, cutting it again from every key at
fences that refine nothing. A seal frozen while it ran was cut at the old
fences and a seal after its landing at the new, a new range opened below
the old piece's fence, and the newer piece over a key sorted ahead of the
older: a read took the older piece's tombstone for the newest source and
answered the value before the last put, the merge that folded the two
wrote it, and a store kept from the differential fuzz's failure read it
so at every open. The doc that said reads were ready for a piece over ranges it was not
cut at was right about the ranges and silent on the order. Once a piece is
unaligned the pieces sort by sequence alone (`sort_segs`), every sort holds
each piece behind every older piece it meets under a debug assertion, and
`tests/fences.rs` builds the re-cut with the old piece sealed beside it
(`Db::hold_merge_start`). The rule: an order a rule rests on is the order
of the quantity the rule is about, and where a cheaper key gives the same
order only under a property of the history -- that every re-cut refines --
the order is taken from the quantity wherever the property can fail, and
asserted where it is made.

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
shape a path without them. It came again where the compact form's walk
was given the raw walk's fast path: the check that a fixed run's length
is a whole number of its values divided by the count, a sixteen-bit
divide with its remainder tested at once, for a count that is one in
the common record -- a twentieth of the threaded scan pass over the
compact form at ten million keys under 1.25 GiB, every sample on the
test after the divide (`perf annotate` of the scan threads' window, cut
by `SUPDB_LAG_MARK`'s phase marks). A fast path copied from another walk
is read for its divides again, since its guards came with it.

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

It came back as a skip of a promise. `sync` makes everything written
durable, and synced the log only when the live table was hashed, on the
reading that an ordered run is its own log and the log holds nothing of
it; and it did nothing for the run, on the reading that a run's commit
syncs its file. Both were true under `Always` and neither under
`EveryN`, the policy whose stores are the ones that call `sync`: a run's
commit that is not due writes its batch's whole pieces and holds the
tail, so a `sync` answered `Ok` lost to a process kill every key of a
run under a megabyte; a run opens over a log that still holds the frames
of a table frozen before it, unsynced; and under a cipher the run's
commits are frames of that log. `sync` asks the log what it owes whatever
the table, and the run by the count of commits no sync has covered,
which the log's sync starts over, so the run is asked first. The
corollary: a call whose contract is durability is gated on what is
unsynced, never on which structure is live, since the policy that
leaves things unsynced is the one whose callers rely on the call. The
fix covered the open run and its first docs promised every commit: a
run closed into a seal in flight holds its unsynced commits on the seal
thread's way to its segment, beyond anything the writer can sync without
waiting for that thread, and the review read the gap off the doc before
any test had gone near it. The restated docs then named two of the ways
a run closes, a key below its greatest and `Db::seal`, and a run closes
at every write it cannot take -- a `put`, a `delete`, a key or a value
the run cannot hold -- and at the seal a commit makes at the threshold,
which is the common one; a reader of the docs was pointed away from the
store's own seal. The same docs still promised that `EveryN` loses a
tail and that the policy's next sync covers the frames an open replayed,
over a window the unit had itself reproduced and filed: a run's due
commit syncs the run's file and leaves the log's older frames unsynced. A
promise is written for the states the fix reached, the states it did not
reach are named beside it until they are built, and a list of the ways
into a state is read off the call sites that reach it, never off the
cases a test happened to build. The due commit's window, once built,
held the same shape from the other side: the policy's sync of the run
was written as "the sync", and a sync by policy promises everything
before it, so the commit that pays it pays whatever it covers -- the log
an earlier table left owing as well as the run. It reached no cell of
the suite, whose ordered loads open their runs over a log that owes
nothing and whose buffered arm's policy is never due, so a test of the
due commit beside a frozen table, a loose file and a reopen under
`Always` is what holds it, and one beside a clean log that it syncs
nothing more. Its first docs then said that a crash keeping the run's
synced commits keeps every commit before them, and took the exception
out of the policy's headline: the review read the file's other syncs off
the writer -- the helper thread that fdatasyncs a run every `sync_ahead`
of file under any policy, the only sync a buffered store's run ever
gets before its close, and the kernel's writeback -- and every test's run
was a few hundred bytes, under the helper's stride. A test with a small
stride and the helper's count waited on reached it at once. The
corollary: a sync a policy issues is priced as the promise it makes,
every file the promise covers, and where the files it covers are known
when the run begins, the note is taken there and read where the sync is
due; and a promise about what a crash keeps is read against every way
the bytes reach the device -- each thread that syncs the file and the
kernel's writeback -- not only the syncs the fix ordered. The second
review found two more, read off the code as the first had been. A
commit of nothing after a run's piece write was refused writes the
batch again and syncs it whatever the policy, standing in for the due
commit the failure spent, and synced the run alone: a power cut after
its `Ok` kept the run's batch and lost every older frame of the log,
the defect the unit was for, on the one path the note was not read at;
it reads it now. And a run that closes before any commit of it is due
is synced twice on the way to its seal, by the flush's presync and by
the seal's own fsync, both before the seal ahead of it is durable,
which the docs' list of the ways had left out because it named the ways
an open run's bytes move. The corollary of the corollary: a sync that
stands in for one the policy would have made reads every note the
policy's sync reads, and a list of the ways bytes reach the device is
read off every function that syncs the file, the close's among them,
never off the state the run happens to be in when the list is written.
The third review read two more. A log sync that failed at the due commit
returned before the run's sync, which was right, and answered an error
`sync_wal` does not keep where a loose file would not open: the batch,
marked committed, was then unsynced and owed by nothing -- no write had
failed -- so under `Always` a commit of nothing after it answered `Ok`
with nothing synced, a state no path had reached before the unit put a
second sync ahead of the run's; the failure marks the batch owed now, a
line on the error path. And the sentence the unit added, that a crash
keeping a run's synced commit keeps every commit before it, was true of
the log's commits and not of a run closed earlier into a seal in flight,
whose file the later run's note says nothing of. The rule: a step put
ahead of another on a path inherits the failure states the other's
failure used to leave, so each error it can return is read for what it
leaves owed; and a promise of "every commit before it" is read against
every log a commit can be in, the runs' files as well as the WAL.

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
And as a copy: a test of a power cut after a rotation copied the live
store file by file as its crash, with a seal its commits had handed in
flight, on the bet that the seal would not land inside the copy; under
the whole binary's load it did, between the listing and the manifest's
copy, and the copy's slot named a segment the copy did not hold -- no
state a crash leaves, read as the open refusing one. A copy of a live
store is a crash only while nothing lands, so it holds the landing
across the copy, and lets it go before anything that can fail. Three
mutants the test was listed as killing were killed by the race and not
by the test. The hold was each test's to remember, and most of the
binary's tests copied a live store: the race came back in a delete's
crash test, whose copy met a merge's landing and was read once as a
mutant's kill. Every copy of a live store goes through one helper now,
which pauses the segment work across the copy
(`Db::pause_segment_work`, `copy_live` in `tests/durable.rs`): every
landing, slot write and retirement is that thread's, so the pause holds
them all and leaves a test no hold to forget, and a copy of a store
nothing holds open, or one a hold of the test's already keeps from
landing, says so at its call (`copy_closed`, `copy_held`). The pause's
mutant is the whole binary run four at a time with the pause taken out.
And as a clock: a test that held a seal's fsyncs on a timer asserted
that the flush published the segment within the first half of it, and
a busy box took longer; the thread that sees the publish lifts the hold
now. And as a read at an answer: a flush answers a stopped queue without
waiting for the seal job of the run it closed, whose presync runs on a
thread of its own, and the test read the presync's record at the answer.
And as a builder: the test that a publish's builder has its forms
installed by the next commit under the upkeep thread passed, in most
runs, because the commit's fill built the blocks -- the builder had
started at the seal's freeze, its forms were dropped as another
generation's once the landing published, and the builder for the landed
state started at the commit after the hand-over, installed only where
its post beat the thread's pass, which it lost one run in sixty. The
engine's counters printed at the assertion (`forms_events`) told the
three shapes apart, and `settle` now starts and joins the builder for
the state that stands under that arm. And as an fsync: the test that a
syncing commit with the pace's model off
(`Options::upkeep_pace_syncs_model`) returns with its batch unfiled bet
that the upkeep thread's pass would outlast the commit's fdatasync,
and on a host whose fsync outlasted the pass every batch was filed at
the return, two runs of the whole binary in five; it holds the thread
before its pass now (`hold_upkeep_pass`), and a commit that waited would
never return, which its watchdog says. And as a step: two controller
tests read the state between the builder's first job and its second by
the counts after a tick, on the bet that the second waited for the
controller's next step, a tenth of a second on. With the builder's turn
at a job's end made the default (`Options::zpages_drive_at_job_end`)
the second job followed the first in milliseconds and both tests failed
every run, the gate's first under that default among them; each of the
three defaults turned off alone named the one. The test of the
conversion's order holds the first job before its rename and the second
at its start now, under either turn, and fails on a tick that starts a
conversion ahead of a partition's own build; the test whose order is
read off the warms taken in a job a step pins the turn at the step. And
as a spare: the test of a recycled log under a cipher wrote sixteen
rounds and asserted that one of their rotations had taken the spare,
which the segment work offers behind a seal's durable landing; where
that thread ran behind the writer every rotation made its file, three
runs of forty-eight under eight-way load, once in a unit's gate whose
diff the binary could not see. It rotates, flushes and settles, so that a
retired log stands as the spare, and holds the next rotation's log to
that file's inode now. And as work left for a step: the cipher test of
form files built within a cache the store does not fit read the cache's
evictions after its fill and again after the form's settle, on the bet
that the builds fall between the two. A store under `ZPages::Always`
builds as its partitions land (`Options::zpages_eager`), a third of the
builds had run by the first reading in every run printed, and in one run
of a gate's stride group all of them had: "the builds gave nothing back"
over a cache that had given back 690 units. With the test's thread made
four seconds late to its reading it was red three runs of three. It runs
twice now, with the builds left to the settle, where the count is exact
and a build before the reading is itself a failure, and with the builds
begun at landing, held to having been made. The
rule: a test that
needs a state holds it, and a bet on timing loses the first time the
thing it bets against gets faster; a mutant's kill is read again once a
race under the killing test is found, since the race kills mutants too;
and a test that passes is read for the path that passed it, since a
fallback that does the same work passes it too.

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
phase, not by repeating the run. It came again as two signals of one
failure: the segment work stops the seal queue and then, its look done,
leaves the failure for the writer's next commit, and tests that waited for
the stop's flag (or for the seal counted out) and then committed a batch
read the commit's answer as the failure's -- in about one run of three
of a mutant pass once the stop moved ahead of the count. A test that
waits on one signal and reads another waits for the one it reads: here a
commit with nothing staged, until one answers. Two more helpers of that
file had the bet and were left when the first was fixed: one went red in
a whole run a day later, at a seal the failure fell to. The window is a
few microseconds, so the helpers were found by making it wide, a spin of
a set width between the stop's flag and the failure's leaving, put into
the engine for the hunt and taken out after: at a millisecond the two
were red in twenty-four runs of twenty-five, and the whole binary run
under the same width named no third. A fix of one test for a race is
followed by the race made wide under every test of its binary.

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
It came back at the log, where the path did check and then forgot: a
failed fdatasync of the log was the commit's answer, and the next
commit's fdatasync of the same descriptor, which Linux answers with
success over pages it marked clean after failing their writeback, was
the next commit's `Ok`; the frames behind the hole were acknowledged and
replay stopped before them. A loose file's sync and the directory's fsync
a new log file's name needs did the same, the second by design, a
barrier "retried by the syncs after it until it succeeds". The writer
keeps the first such error now and answers it to every commit, sync and
flush until the store is reopened, and the check rides the one each of
them made already: the kept error refills `pending_err` each time it is
taken. The corollary: a sync's error is not the call's alone either --
it is a fact about what the device holds, so it is kept for every later
call that would promise the same bytes, and a retry is never the remedy.
It came back a third time at the direct run's file, twice. The helper
thread that syncs a segment behind its writer keeps its fdatasync's error
for the finish, and its doc said why -- the writer's own fsync, on the
same description, "would then report nothing" -- while the run's commits
are the writer's own fsyncs and never the finish: a failure the helper
met was answered by no commit, and the commit after it was acknowledged
over the hole. The writer reads the helper's error where it already
tells the helper it has synced (`synced_all`), so a writer with no helper
pays nothing. And the run's own failed fdatasync was answered once and
forgotten as the log's had been, and with the kept error in place a write
below the store's greatest still closed the run, whose finish fsyncs the
same file and whose landing names it: a retry with a manifest behind it.
The finish refuses a file whose sync failed, and asks nothing to know
it: the lost sync and a held write are bits of the byte the writer's
test seams already asked beside every write and every sync, so a write
of a file whose sync failed is refused in the write's own cold path, a
finish always has its trailer to write, and the first draft's checks of
two fields of their own, one at the top of every run commit's sync and
one at every segment's finish, were a load and a branch each that no
check of the path made before. The corollary: a kept error
is checked at every path that would sync the same bytes again, the
background ones included, and not only at the calls that answer the
caller.
That check still read the helper's word after the helper had acted on
it: the helper synced through a duplicate of the writer's descriptor,
one open file description between them, so a failed fdatasync of the
helper's took the device's error for both, and a commit whose own
fdatasync returned in the instant before the helper stored its error
read the helper's word as clean and was acknowledged over the hole. A
model of two syncs in flight on one description named the order, and no
test had built it, since the seam failed the helper's sync by count and
not by description. The helper opens the file again for itself now
(`/proc/self/fd`, one open where it is spawned), so the kernel reports
the error to the writer's description as well, whoever syncs first: on
this kernel a probe over a loop device on a full tmpfs read the writer's
fdatasync `Ok` after the duplicate's failed one and the error after its
own description's. The seam says which description met the error
(`HelperFault`, `kcmp`) and holds the helper between its syscall and its
store, which is the window. And the finish's join of the helper, which
takes its error before a seal's or a merge's segment is renamed into
place, was held by no test, every test of a failed helper sync being a
run's; one each now. Those two reach the failure whether the helper
syncs before the finish asks it to stop or at the stop, and a mutant
that took the sync at the stop away passed them in five runs of six:
the stop is held by a test whose helper begins only after it
(`HelperFault::start_after_stop`), since a path reached by timing is a
path a test bets on. And the helper names the file by the writer's
descriptor's number (`/proc/self/fd/<n>`), so a test builds that number
at two, three and four digits, none of them a palindrome, since a path
built from a number's digits is wrong in a way only some numbers show.
Where the open is refused the writer goes on with no helper and keeps
the checksum row it would have handed over, and a fallback that dropped
the row passed every test, whose stride spawned the helper before the
key section's first whole piece, so there was no row to drop: the test
runs at a stride that leaves pieces in it as well. The corollary: where
two threads sync one file and either can answer a caller, each syncs
through its own open file description, since the kernel's error is a
description's to consume and a word stored after the syscall is a word
read too early by the other; and a hand-over is tested where there is
something to hand over.
It came back a fourth time at the presync itself, whose error the flush
answered: the presync runs on the writer's thread after the seal job is
handed, the job's fsync of the same description reports nothing of what
the presync met, and the segment work landed the job durable whenever its
thread returned -- before the writer had its answer, by timing, so the
manifest named the segment and the next flush answered `Ok`. A failed
presync stops the seal queue now, and a job whose records the writer
syncs lands durable only at the writer's own ask, which follows the
presync (`SealJob::awaits_writer`): a flag the segment work reads where
it already asks whether the job has returned, and no wait anyone made
before. The corollary: an error answered to the caller is not therefore
seen by the thread that acts on the same bytes; whoever names the bytes
durable is ordered after whoever learns they are not. The order was
landed unpriced, as a serialisation of the landing behind the presync
and a hop to the writer's ask, and priced after against the shape that
landed the job whenever its thread returned: the drain's stamps put the
presync's return before the seal thread's fsync of the trailer in all
but a drain whose writer lost its core between the two, so the ask is
in before the job's thread returns and the landing waits for the thread,
as it did; no rate moved and the file system calls were the same, call for
call. A reorder is priced by which side of the race each of its ends
falls on, read off a stamp at each, before the hop it adds is called a
cost or called free.

**A failure after the publish, left for the next publish to name.** A
seal lands in two phases, its segments published for readers at the
first and named by a manifest only once its thread has returned with
their fsyncs paid, and a seal that failed before its readable landing
stopped the queue for the life of the open. One that failed after it --
at its data's fsync, its envelope under a cipher, its directory's fsync
at the landing itself, a run's presync joined into the job -- did not,
by design: its comment said the segments "stay live with their sequence
covered, so the next landing's manifest names them". The next landing
was a merge's, a later seal's or the pending arm's close, and its slot
named durable a segment no fsync had covered and covered the sequence of
the log that held its writes, which the open then skipped: a power cut
left a store that refused to open over acknowledged writes the log still
had. Every test of the failure path failed the landing, never the fsync
after it. The queue stops for every failure now and the fsync is never
tried again (`Shared::wedge`), and a stopped queue is held as the window
between a seal's phases is held, for good (`Maint::seal_between_phases`):
a merge's, a tier's and a promotion's landing wait in that window
already, the writer's asks end at `join_seal`, a seal's own landings --
`publish_unsynced` and the pending slot -- are not reached once no seal
lands, the close asks the queue before it publishes, and `Maint::publish`
asserts the window shut. The first repair put a refusal in
`Maint::publish` itself, a load beside every manifest's write, and a
load in every `sync`; the window was a check every manifest path made
already, and a `sync` promises nothing of a seal's writes. Every `flush`
answers the failure. The window was then held for a stop the segment
work raised itself, and a stop the writer raised -- its run's presync,
failed after the job was handed -- was held only by the job waiting at
the front, landed, for the writer's word: a presync that failed before
the job's readable landing left it unlanded for good, and a merge in
flight landed its slot after the store had answered the failure, naming
nothing of the job but writing the manifest the docs said nobody would.
The review read it off the docs; a test with the job held before its
finish and a merge at its landing hold made it. The segment work takes
the stop into its own hold where it already loads the flag, at its look
at the queue, which every path of it to a manifest makes first. The
rule: a failure that leaves a thing published
and not durable is a state, not an error to report and pass: it is held
where the paths that would make the thing durable already wait for it
to be durable, and a path that bypasses that wait is the bug; and a test
of a failure path fails every step of it, the ones after the publish
above all, since those are the steps a publish has already made look
done; and a stop either side can raise is held by the flag both sides
read, never by the shape of whatever job happened to be at the front
when one of them raised it.

**A refusal that came after the take.** A direct run's close took the
run from the writer and then waited for room in the frozen list, and a
refusal there -- the seal queue stopped, the list full behind it --
dropped the run's writer with its ordered table still live, so the write
that ended the run went on into the ordered table and panicked in its
order check, and a `Db::seal` of the run left a table no run backed. The
close waits for room before it takes anything now, so a refusal leaves
the run open and its table live; and the write a run cannot take, which
cannot fail, is dropped only with the refusal kept (`Db::lost`), since a
write dropped beside a later commit answered `Ok` would break the order
the commits promise. The first version of the drop took any error the
wait met with the list full for a refusal, and a merge's error the
segment work had left for the next commit is one the wait meets while
the list is full behind seals that will land: backpressure read as a
stopped queue, a write dropped and the store answering every call
until it was reopened. The write is dropped only where room will not
come -- the queue stopped -- and otherwise the error is the next
commit's and the wait goes on. A second arm, no seal left in flight to
land, was in the condition for a while and no test could reach it once a
failed seal stopped the queue before it was counted out, since a frozen
table is always a seal in flight or a stopped queue's; it was taken out,
and with it the same arm of the writer's two waits for room
(`land_seal`, `take_fresh_mem`), which read the seal count for nothing
else. The write after a
`sync` handed the live table to a seal had the same shape from before:
the table's replacement failed, the write was dropped and the next commit
answered it once, and the commits after that answered `Ok` over it; it is
dropped and kept on the same terms now (`Db::writes_go`), and behind a
stopped queue the handed table is frozen where there is room, since
nothing lands to retire it either way. The docs then said a `sync`
answers nothing of a stopped queue and named the run's dropped write as
the one exception, while the hand-off's write is dropped and kept on the
same terms and from it every `sync` answers too -- which the fix's own
test asserted beside the doc that denied it. The rule: a step that
can be refused is asked before anything it would consume is taken, so a
refusal leaves the state as it found it; a call that cannot fail and
cannot be done is dropped only where nothing after it is acknowledged;
and "cannot be done" is read off the state that makes it so, never off
an error that the state merely happened to be beside; and a contract's
exceptions are read off every site that keeps the refusal (`keep_lost`),
not off the site the doc was written beside.

**A failure that left the buffer it failed on for the next call to
build on.** The log's write staged a commit's frames, wrote them, and
cleared them; a write the device refused returned before the clear and
before the cursor moved, and left both as they were mid-failure: the
batch and its commit frame still pending, and whatever part of the write
had reached the file sitting past the log's length. The next commit
appended its own batch and closed it with a frame whose CRC covered
everything pending, the failed batch's commit frame included, so replay
rejected it and every commit after the failure was lost; a part written
shifted every frame after it; and a rotation after either made the open
refuse the store with a sequence gap the engine had written itself.
Nothing raised until the open. The failure path now puts back what the
call changed -- the file cut to its last good length, the commit frame
off the staged batch and its sequence number given back, so the batch
joins the next commit's under one frame with no gap -- and a file that
cannot be cut back is kept as a sync's error is. Two batches under two
frames was the other shape, and it costs a position kept on every
commit; this one costs nothing until a write fails. The rule: a call that
fails part way through changing a buffer and a cursor puts both back to
where the call found them before it answers, since the next call builds
on whatever it leaves; and the shape of the put-back is chosen on the
failure path, where its cost is. Two things the first version of the
put-back missed, each found in review. Its tests made every store with
`Db::create`, whose log is written at its cursor, so the seek did the
work and a put-back without the cut passed them, the retry's bytes
being always longer than the stray ones; a reopened store's log is
opened for append, where a write lands at the file's end whatever the
cursor says and only the cut keeps it from landing behind the stray
bytes, and the tests run the cuts on a reopened store now. And under a
cipher an ordered run's commit goes to the log as well (`Db::log_run`),
after the run has counted the batch its own: a commit or a `sync` with
nothing new for the run answered `Ok` at the run's "nothing to commit"
and never wrote the frames the failed write had left staged in the log,
so the batch was visible and on no device. That early return asks the
log too now. The batch is still visible from the failed commit on, as a
run's batch is before its sync, since the run marks its table before
its log write; the first wording of the crate docs promised readers
nothing before the next commit, which held for the log's path alone,
and they name the run's exception now; the second promised the batch
durable at whichever call wrote it again, which held under `Always`
alone, since a commit under `EveryN` writes without syncing, and they
say durable as that call's policy makes it now. The corollaries: a put-back of a file is tested on every
way the file is opened, since the same calls do different work on each;
and a "nothing to do" answered from one counter is wrong wherever a
failure can leave the work in another buffer that counter does not see;
and a contract written for a failure is read against every path that
reaches it, since the paths order their marks and writes differently.
It came back at the direct run, whose file is its own log and whose
writer had the same shape twice. A piece the device refused at a commit's
marker returned before the commit counted the batch, with the writer's
keys, its marker and its CRC already past it, so the next commit handed
the writer the same keys again and the run refused every commit after it
as keys out of order, never naming the device; and a write at the sync
that failed, or failed part way, left the batch counted and the file's
cursor past what the writer counted, so a commit with nothing new and a
`sync` answered `Ok` at the run's "nothing to commit" with no write at
all, and the commit after a part written landed past the stray bytes and
was lost to the recovery walk. The writer writes at the offset it counts
now (`pwritev_all`), holds a failure it meets between two syncs with
every byte for the next sync to write and answer (`AlignedWriter::defer`),
and says it owes the file in a bit of the same byte its seams and its
held and lost states share, set where the write fails and cleared on the
sync's cold path once a sync has covered it, so that early return asks
one compare it did not make before and the exact count
(`SegmentWriter::owes`) only behind it: the first draft asked the count
itself, two loads where the path had none; and a short write, which no
seam had made, is one now, since a loop that moves the offset on by the
count it got is a line only a short count reaches. A failure met inside
a sync that then wrote and synced every byte it held is paid by that
sync, never answered to the next one, which would answer it without
writing its own batch. The corollary: a failure in the middle of a call
whose earlier steps another component has already taken is completed
forward, never answered from where it stood, since the half that was
taken cannot be handed back; and a writer that counts its own offset
writes at it, so that the cursor is never a second count. A run that
closes while it owes a batch -- a key below its greatest, before any
commit synced the batch again -- writes the batch into its segment, and
the close is the batch's sync; a test closes such a run, held and owed,
under both policies, every arm and a cipher, since the first tests
closed only runs whose owed batch a sync had already written. The
writer that stays stuck has a mutant that is not, and it does not fail
the test of a kept sync: the writer meets the full piece it holds at
every turn of its write, writes nothing and loops, and only a runner's
timeout said so. The test runs its writer on a thread of its own with
an end, and fails where it hung. The corollary: a mutant killed by a
hang is a test with no end of its own, which is given one before the
kill is counted.

**A hand-off that left the old run's end with its new owner while the
giver went on.** A run of ordered ingest goes to its file a write piece
at a time, and a run that closed was handed to its seal with its last
piece still in the writer's buffer, for the seal's thread to write with
the trailer. Under a policy that syncs at every commit the buffer is
empty at every commit. Under one that does not, the writer went on into
the next run, and that run's first whole piece reached its own file
before the seal's thread had written the old run's end: a process killed
between the two left the first run's file short of its last commits and
the second holding later ones, and the open recovered both --
acknowledged batches gone from the middle of the history, batch 290 of a
load there and 287 to 289 not, where `SyncPolicy`'s own words are that an
unsynced tail is lost whole and never served in part. Nothing raised and
every file was well-formed; `tests/kill.rs` killed a durable writer and
no other. The suite's recovery workload found it in its first whole row,
at the one rung whose load crosses the seal threshold just before its
end (`bench/CLAUDE.md`, a guarantee matched on one kind of crash), and
the engine's test holds the seal's thread so that the window is as wide
as the kill needs. The writer wrote a closing run's records before the
hand-off first (`Db::close_run`), which left a process's death the open
run's end to take: up to a megabyte of acknowledged commits, two to eight
batches of the suite's load, where a commit to the WAL and the
comparators' unsynced commits lose none. Every commit writes its batch
now (`Db::commit_direct`), priced in the tree that measured by an arm
that wrote at every commit beside one that did not: nothing resolved in
thirty-six pairs at ten, a hundred and three hundred thousand keys, the
reads and the scans level, where the stream's second shape, written at
every commit, had read every pass over the segment behind. Two builds
alternated, each against its comparator in one process, put the price
on the buffered ordered load: three to four percent at ten thousand
keys in two sittings, where the load is ten commits and each is now a
write, and nothing at a hundred or three hundred thousand, with the
mixes level; and the load's device bytes rise by a page for
every sync that lands inside a run -- the writer helper's and the
drain's -- since the file now ends mid-page where it ended on a write
piece, and the page a sync catches half full is written again when the
next commit fills it. The durable arm's file has always ended so. The
rule: where work is handed to another
thread and the giver goes on, whatever orders the giver's later effects
behind its earlier ones is finished before the hand-off, never left with
the receiver; and a guarantee stated of one path, here the WAL's, is
tested on every path that bypasses it. What a machine's crash could
still take is the entry after this one.

**A history in several files, ordered by nothing a crash keeps.** A run
of ordered ingest kept its records in a file of its own and took no
sequence numbers, so nothing on disk said where it stood: the open took
every run it found, each to its own last marker, and replayed the log
beside them. A process's death leaves every file whole, and with the
hand-off above fixed the order no longer mattered to it. A machine's
crash under a policy that does not sync every commit keeps whichever
pages the kernel had written, and those are not the oldest: a later run's
megabytes written back while an earlier run's last pages sat dirty, or
frames in the log behind a run whose file had not reached the directory,
came back as commits served without the ones before them. Three more of
the family stood beside it, each read off the code while the fix was
being placed and none with a test. `sync` over a run in progress synced
nothing, since it synced the log and the log holds none of a run's
records. A sync by policy under `EveryN` synced the file it was writing
and started its count over, leaving frames, or a run, written before it
and never synced behind an acknowledgement that covered them. And the
open made a recovered run durable, as a piece the manifest named, in
front of log frames a killed writer had left in the page cache alone. A
run's records take sequence numbers now and its file says where it
starts; the open walks the frames and the runs as one history from the
manifest's watermark and ends it at the first loss (the ordered ingest
paragraph under the crash discipline has the whole of it); every sync
that acknowledges covers every file written before it; and the open
syncs the log before it names a run's piece. `tests/runlog.rs` builds
each state a lost page leaves, by stopping a writer with its seals held
and cutting a file back or taking it away, since no test can pull the
plug. A directory barrier for a run's temp name went in beside the fix,
on the engine's own word that a file's sync does not promise the entry
that names it, came out on its price, and went back in. The price was
real and small: the durable ordered load at ten thousand keys a
thirtieth slower in the before-and-after pairing, `strace -T` putting
the barrier at a tenth to a quarter of a millisecond once a run, on a
load of ten commits -- a flush of the device with nothing to write,
since the file's own sync has forced the journal commit that carries the
entry -- and nothing that resolves from a hundred thousand keys. Priced
again as it went back, the two builds alternated eight times, each
against its comparator, it read a tenth of a millisecond on a run's first
commit in every pair (`SUPDB_LOAD_PHASES`, the `first` column), the
commits after it level, and the load's total slower in three pairs of
eight; the rate against the comparator leaned a twentieth, and so did
the comparator's own load between the two builds, code the change cannot
reach. What it
had been traded for was the wording of the guarantee: without it a
commit acknowledged as durable rested on what ext4 and XFS do with a new
file's first sync, which POSIX does not promise, and a name that went
missing was refused at the open only where log frames followed the run.
The rules: a history kept in more than one file is ordered by a
number every file carries and one durable watermark, so that what stands
behind a loss is found by arithmetic and not by which files happen to be
there; a sync that acknowledges is a sync of everything written before
it, wherever it went, and its test is a count of what it left unsynced on
every path that writes; a recovery that makes something durable makes
durable what stands before it first; a crash a test cannot cause is
built, since the kill tests had killed processes for a year and a
process's death cannot tell `EveryN` from `Always`; a barrier added
beside a fix is a change of its own, priced on the smallest rung, where
one of anything is a share of the pass; and the price is then weighed
against what the barrier promises and not against the pass, since a
thirtieth of the smallest rung had bought back a durable commit that
depended on the filesystem under it, which no row measures and a reader
of the guarantee takes for granted.

**A flag set on the word of a comment, found by the next thing to trust
it.** A reopened log counted the frames it had replayed as synced, under
a comment that said they were "read back from the device". A killed
writer leaves its unsynced frames in the page cache, and the open reads
them from there: a `sync` after the reopen found nothing to sync and
returned over them, and the log's older files were on no list at all, so
the first sync by policy after the reopen marked the live file's header
chained over files it had not synced. Nothing found it for as long as
the flag stood, since no test reopened a killed store and then asked
what a sync had left.
What found it was a new reader of the same predicate: a run of ordered
ingest now says in its note whether everything before it was synced as
it began (`RUN_NOTE_CHAINED`), the open refuses a gap before a run that
says so, and a run that said so falsely is a store that will not open
after a crash it should have survived -- so every state that reaches the
question was read for whether its answer was true, and two were not: the
reopen, and a drain that failed, whose closed run was unsynced and on no
list. The replayed frames count as unsynced now and the older files are
the first sync's; a failed drain's run is loose as any run that closed
unsynced is (`Db::drain_run`); and the question has one definition
(`Db::owes_sync`), asked by the syncs and by the run. The rules: read
back is not synced, and a flag that says "durable" is set by a sync this
process issued or by nothing; a predicate a new mechanism leans on is
audited at every state that reaches it, the recovery's and the failure's
first, since the old callers only ever asked it to decide a sync that a
wrong answer made cheaper and never one that a wrong answer made fatal;
and where a wrong "yes" refuses a store, the tests build the states
where the answer is "no" and hold the open to opening.

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
option's name. It came back as a quantity the writes keep rather than a
resource they hand out: the store's greatest key, which a key has to be
above to open a direct run, was raised by `put` and by `append` and not
by `delete`, so a key deleted above the greatest and then appended opened
a run while its tombstone sat in the log past what the manifest covered.
The live store read the append, the run's table being newer than the
frozen tombstone; a crash before the tombstone's seal was durable
rebuilt the run as a piece and replayed the log over it, and the
acknowledged append read as deleted, at that open and every one after.
The delete raises the greatest key as the other writes do, and
`tests/durable.rs` builds the crash in each window of the seal, from a
delete first, second and alone in its batch, from a transaction, from a
delete the seal commits with no `commit`, from a delete that ends a run
and from a tombstone the open replayed (`Db::max_key_of` counts the
memtable's tombstones). A raise is a raise: one taken as an assignment
lowers the greatest at a delete below it, and the run that opens there
spans partitions and is refused by the flush's merge; a greatest
recomputed at an open without the segments does the same. The
corollary: a quantity that guards an order between two logs is kept by
every write that reaches either log, where the write is made and not at
whichever call makes it durable, and the test of each write is the
crash, since the live store reads right whichever way the guard went.

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
it just as well. And it came back turned round, as a kill the instrument
made. The flush tests after a reopen put back, at their power cut, the
slots a drain had left owed their fdatasync, counted by the hold that
keeps that fdatasync waiting, and the count was of slots written
unsynced: with the merges on, the drain joins a merge whose synced
publish supersedes the slot before the answer, so a drain that deferred
its slot over a log that owed frames was answered over a slot the device
held, and the test put that slot back and failed the drain anyway. The
mutant was dead for the instrument's reason, in the one store shape the
tests had, where nothing it changed could be lost. The count is of slots still
owed now, the flush tests run with the merges off as well, where the
drain's own slot is the only one, and a test holds the instrument both
ways. The corollary: a mutant's kill is read for whether the state the
test built is one the mutant makes, since a second mechanism that does
the first one's work makes the instrument wrong in both directions.

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
the allocation count has (`tests/leak.rs`). It came back as a record:
the calls the open's tests read off (`Db::trace_dir_io`) were kept
process-wide, the record stayed on once any test had turned it on, and
it grew with every recorded call of every test in the binary while each
reader filtered it by its own directory. It is kept by the directory a
test asks for now, and handed back and let go when that test asks it
off. The corollary: a seam that records is scoped to what the test
reading it owns, and let go by that test.

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

It came a third time as a flag, found twice on the two lines that were
merged, each by a seed of its own. A copy whose entries are its block's
records says so (`CachedBlock::identity`), and a patch writes the key's
entry at its cut less the block's first rank where it does, without a
search; the copies are what that carry keeps, on the reading that their
key range is the same on both sides, which is true of their keys and not
of their ranks. On one line a key written into such a block and left to
the backlog -- a batch no scan's commit filed -- was sealed and folded in
by the merge, a record of the new partition at the rank it landed at,
and the patch that filed it after the rewrite took the entry its rank
named, another key's, and in release wrote the run there: seed 1179 of
the differential fuzz, which stopped finding it the day the commits near
reads began filing their own batches (`Options::forms_settle_every`), so
the seed went green while the bug stood, and only a rebuild at the
commit that recorded it and a bisect that named a default and not a fix
said so. On the other a delete and an insert in one block kept the
boundaries and the count, and when their writes were lent to the upkeep
thread and still unsettled at the freezes, the settle filed them after
the rebase, through the tables the freezes replaced, and the inserted
key's patch wrote its value over another key's: the checked profile's
assertion beside the patch in fourteen runs of eight hundred of seed
1050 under the cache and the option draws. Both lines then fixed it, in
two shapes, and the merge kept one. A copy holds its claim of the
partition its build read (`identity_of`, the blob id beside the flag).
A walk's start takes the entry by its rank only where the claim is of
the partition in hand (`CachedBlock::identity_for`), though no state was
found that walks such a copy before the insert that moved its ranks is
filed and clears the flag. A patch under a claim of another partition
takes the entry its rank names only where one key compare proves that
entry is the key in hand (`key_eq`, `Reader::splice_run`), since a copy
holds each key once and in order, and searches by key otherwise: the
search at every patch into a carried copy was the land probe's cost
(`scripts/cost.sh`), and the compare is the check that makes the rank
sound for that one patch: against the upstream head merged, the land
probe's `CachedBlock::lower_bound` and `Reader::splice_run` read 280
instructions under it an operation of the probe, a key rewritten. The hint ahead of the patch
(`copy_entry_at`) takes the claim of the partition in hand alone, as
upstream's did, and the walk's and the form's (`prefetch_block`,
`by_cut`) take any claim; each only aims a prefetch. `tests/settle.rs` builds both states: the fold, the drop that
keeps the block boundaries and the unfiled write, and the upkeep thread
held before its pass while the seals and the merge land. The rules: a
claim about a structure's relation to another object names the object,
like the cache derived from it, and a carry is checked against every
claim the carried structure makes about the object it was derived from
-- its positions, and the flag that says its entries are the object's
own -- since a claim kept in a boolean is a position no search will ever
re-take; and a seed that goes green after a change in what it draws or
what the store defaults to is a seed that stopped looking, so it is run
where it failed, and the bisect from there names whether a fix or a
steer made it pass.

**Two lines' tests merged by text, one store between them.** A merge of
upstream that conflicted in three files and merged the rest clean put two
tests in `tests/settle.rs`, one from each line, that named their stores
alike (`scratch("copy-identity-rewrite")`, one directory a process): run
as the binary runs them, four at a time, they removed and wrote each
other's stores, and the binary was red in fifteen runs of fifteen at the
create, the settle or the assertions, while each test alone was green. In
the same merge a function upstream wrote read the ordered index's top
level as a field, which this line had made a method that reads the level
by the piece under a cipher (`OrdIndex::top`): there the text did not
compile, and the field it named would have been an index of no samples
under a cipher (`a_heads_bracket_by_the_piece_is_the_mapped_ones`). The
rules: a merge that is clean by text is read for the names both lines
coined -- a test store's, a field's -- since two lines that each named a
thing once can name it twice; and a test binary is run whole after a
merge before any test of it is read alone.

**A note in one cell, copied whole to bump a counter.** After the same
merge the scan probe read `Reader::scan` eleven instructions a scan over
upstream by function, and the first reading called six of them the
compiler's: spills around upstream's `note_read`, which upstream's own
default made run past its first test at every scan, in a scan this line
had made larger. The count by instruction against upstream's build
(`--dump-instr`, each instruction laid to its line by the text of that
line in each tree) said where they were: the read's note was one
`Cell<ModeNote>` of six fields, taken whole at every read and put back
whole at its end, seventeen instructions of `Cell::get` and fifteen of
`Cell::set` a scan in upstream's build too, for a read that bumps one
counter and looks at one flag. A cell a field, `Reader::scan` reads
twenty-one instructions a scan under upstream by function, and the point
read probe's total thirty-eight a read under upstream's. The same count found
`cut_raw`'s gain, called codegen beside it, in the heads' search inlined
into it: a probe through `head()`, whose bound test for the read by the
piece calls a function that returns, against upstream's assertion; the
heads in memory searched as the slice they are, bounded once, read under
upstream. The rules: a gain called the compiler's is first counted by
instruction against the side it gains over, line by line, since what
looks like register pressure in a function that grew is as often a step
both sides pay; and a `Cell` of a struct is a copy of the struct at every
`get` and every `set`, so a structure a hot path touches a field of is a
cell a field.

**Two counts compared across a worker's race, and a steer that asked and
was never answered.** The same merge's whole suite met two reds in tests
upstream wrote, each once and each green alone. One compared two stores'
counts of the boundaries a bounds walk read, the option on against off,
and read the store with the option off at twice the other: a backtrace a
walk, printed under a scratch variable, put every store's count at two
walks -- the landing's of the piece and the upkeep thread's of the same
keys as an unsealed snapshot, filled after the burst's commit -- and the
red store at one, its seal having frozen the burst before the lent fill
ran. Held before its pass (`hold_upkeep_pass`) in one store it was red
in every run, here and on upstream's head. The other was the stride
group's conversion steer, red where no seed moved a partition out of the
stride form: its asks are taken only at the segment work's turn with the
builder free and nothing due, each replaced by the next, and beside the
seals' builds a seed's asks were all replaced untaken -- thirteen runs of
twenty-four six at a time, sixteen on upstream's head. The first test
fills on the writer's thread now and counts from after the fill, since
whether a commit fills at all is the fills' room's, a reading of the
group's refaults: counted from before the commit it was green in every
run here and red in every run where one store's commit was made not to
fill. The second's first repair asked the served partitions by turns
until one left the stride form, at most sixty-four asks; with the asks
counted it took one in each of a hundred and sixty seeds, the first
partition a stride file every time, so a mutant that always asked the
first and one that allowed a single ask both passed, and the turns and
the bound were a check nothing reached. It asks the first partition
three times now, each ask settled, and holds the moves to one each way
at the least, which also holds the move back into the stride form that
most seeds of the range had never made. The rules: a test that holds two
stores' counts equal counts what the step under test makes, from just
before it, with every worker that adds to the count made inline or held,
and every gate on a platform reading left before the count; a steer
whose asks a worker may replace untaken drives its asks to answers the
test can name before the run's count is read, and a loop that retries
until something happens is counted for the turns it took before its
bound is believed; and a red met once is found by printing who did each
counted thing, not by running it again until it does not come. The
second test's repair had the first rule's hole in it: its three asks were
counted from a reading taken behind a plain `settle`, which joins no form
build and takes no ask (`zpages_settle` does both), so where the loop's
last ask was still untaken with the builder free, the segment work's turn
behind that settle started it, it landed inside the first settled ask,
and three asks read as four moves, "(1, 3)", once in a gate's stride
group and in none of fifty-three runs after. Two schedules were forced on
a guess before anything was printed -- a conversion's build made long,
the thread's own turns made rare -- and neither made one red in fifty
runs; a trace of each ask taken and each move counted, beside the test's
reading, showed a build in flight and an ask pending at the reading in
its first run, and the state built from that (the builder settled, one
ask stored, then the plain settle) was red in every seed of every run and
green with the count begun behind the form's own settle. A count's
baseline is read behind the join of every worker that moves it, by the
call that joins that worker and not by a call with a like name; and the
schedule to force is read off a trace of the state at the reading, since
a guess at it costs a build and a sweep each time it is wrong.

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

**A header that needed no sync, read as if it had had one.** A new log
file's header is written and left to the next sync, which covers it
with the frames, and the open read a file shorter than its header as a
log with nothing in it on the reading that a power cut tears a file at
its end. A power cut can also keep the size and lose the page: a store
just created, a file a rotation began, the log of a durable ordered
load whose commits all synced the run's own file, and a buffered
store's log, which a flush acknowledges past, could each come back with
a first sector of zeroes where the header was, and every open after refused the
store as "not a supdb WAL" with its acknowledged writes in segments and
runs beside it, at every open, since every open reads the log again.
The test that a fresh store opens after a lost header was never
written, because the file a test copies is the page cache's, which
never loses a page. The rule is the fresh-pages one turned to the
device: what a power cut leaves of a file no sync covered is any part
of it, the first page included, so an open reads every such state --
here a first sector of zeroes as a log with nothing in it, and only
that, since a header zeroed over frames is not a lost page -- and the
fix is at the read, where it costs an open a compare, never a sync at
the write, where it would cost every rotation one.

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
at one line, named once. It came again from the other input. A reading
taken while the lag sweep's store at ten million keys was still being
loaded, fourteen partitions of the twenty-four it would hold, picked the
raw tails at 601 MB under a fill share of 940; the store doubled, every
output's form file was built raw-tailed at its finish, and a form the size
of the raw records was installed and served through the sweep, its passes
two to three times the other reps'. The count that named it was the groups
decoded, zero through the whole sweep, and the trace's reading line was
nineteen seconds into a load of thirty-six. A raw-tail pick is taken back
where the records have grown past the fill share since it
(`Maint::z_repick`), and a file on record in the refused coding covers
nothing. The corollary: a decision held for a store's life is taken again
wherever an input it is a function of is read again, and a pick made over
a store still being written is a pick over a store that does not exist
yet. And the take-back was a second cost of the first: the arm whose
reading came in the load built a sixth more form files and gave back up to
twenty-four of them. A pick for readers waits for a reader now
(`Maint::z_pick_raw_ok`). The corollary's corollary: a decision that has
to be repaired when its input moves is not made before the input exists,
since the repair costs what the early decision bought.

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

**A regime read off a state the regulated work can enter.** The pace that
holds a syncing commit behind the upkeep thread is for a store in core, and
it read "in core" as the controller's gate at the raw form. The controller
leaves the raw form for its census on the group's refault rate, and a full
rewrite at three million keys under 1.25 GiB raises it itself: with its
block cache grown to 439 MB of the bound and its merges running, the rate
went from 39 a second to 13,000 between two steps, the trace put the census
four seconds into a burst of eight, the pace stopped there with the store
still read from memory, and the first scan after the burst settled the rest of the
burst's trail, 40-130 ms of a pass LMDB ran in 28. The lag-only pairings
that priced the gate met the same census and a smaller residue, and the
regime the gate was added for, ten million keys, had the compact form's
gate from its load on, so the two gates had never been told apart. The pace
stops at the compact form's gate now. The rule: a switch keyed on a
controller's state is keyed on the state that means what the switch needs
-- the form installed is the controller's verdict, the census a question it
is still asking -- and a state the switched work can itself bring about is
read off the trace of the run before it is read as the regime. The seal's
floor, the asks ahead for pieces, blocks and heads, the builder ahead and
the chain through writes read the census as out of core the same way, and
each was priced where the gate was the compact form's. Beside it: the write
mixes of that pairing, which run the same code in both arms, read 0.8x for
one arm in two sittings, and timed by kind the gap was the commits'
fdatasyncs, 0.41-0.78 ms a commit from process to process in both arms; a
cell an option cannot reach is read by its kinds and its counters before
its rate is read as anyone's.

**A cache that measured its benefit where the benefit was another handle's.**
The block cache out of core repays a block's build only where a later scan
walks the block again, and the lag sweep's uniform passes meet each block
about once, so a writer whose scans' start blocks do not recur now gives its
forms up and takes the merge path, 2.2-2.6x on the full rewrite's pass at ten
million keys under 1.25 GiB. The first version ran the measure on every
handle, each over its own starts, and the threaded scans beside the mixes
read 0.85x: the writer publishes its forms for the handles, and three
threads reading the same blocks through them is a reuse none of their own
starts shows, so each handle gave up what the others were paying for. The
regime is the writer's, and a live handle takes the forms back at the
writer's next commit, since the writer may make no scan while the handles
read. The rule: a measure of a shared structure's reuse is taken where the
structure is shared, or confined to the one consumer whose measure is whole;
and a mode that turns a producer off is undone by a consumer's arrival at a
point the producer passes whether or not it reads. Beside it: a test of the
switch that wrote nothing where the scans after it read passed with the
switch's drop removed, and the fuzz's stores, a few blocks each, never left
the forms on under a real measure, so a seam flips the regime every few
scans there (`scan_forms_off_flip`, `ZFUZZ_FORMSOFF`); a mechanism a test's
input cannot drive is driven by a seam, and the seam is checked to have
fired by its own count.

**A fixed cost a pass, paid at the batches' rate.** The upkeep thread's
fill walks every block of every partition whenever its tables are
incomplete, a millisecond a pass at three million keys, and under the
syncing pace a burst's passes come a few batches apart, so the walk was a
sixth of the thread over the full rewrite and the writer was held for a
quarter of the burst; the one-percent pass's first scan then waited for the
pass in flight, the cell that lost to LMDB. A smaller pace share, the
obvious lever on the scan's wait, made every batch a pass and paid the walk
at every batch. The fill is left for a later pass while the writer has
posted a newer commit now. The rule: a per-pass cost is priced at the
rate passes come, and a regulator that shortens the passes multiplies it;
work whose product the next pass rebuilds anyway is done by the pass that
finds nothing waiting behind it.

**A footprint priced on the stores the suite makes.** The WAL's recycling
pre-wrote the live file and a spare whole at create, 64 MiB written and
synced, and kept 32 MiB at rest in every store, which no pairing sees: the
suite makes a store a pass, and a pairing prices the passes. The tests make
hundreds, and the gate with that shape as the default would have left tens
of gigabytes on an allowance of eighteen. The pre-write follows the tail
now, a mebibyte ahead, flushed by the commit that reaches it (ahead of
which commits, the megabyte's own shape below says). And the test
written for the recycled file's stale frames passed under the whole-file
pre-write for the wrong reason: its keys ascended, went to a segment
directly and never touched the log, and its assertion that the live file
was longer than its header was satisfied by the zeros; with the pre-write
following the tail it failed, and its keys are permuted now. Six more
took the file's length for the log's tail, tearing zeros where they meant
to tear a batch, and read the tail from the store now. The rules: a
default is priced on every store the library makes, the smallest
included, at create and at rest as well as in the passes; and an
assertion that a structure exists is held to the structure's content,
since a fallback that makes something of the same size satisfies its
length.

**Advice taken for an order, and a wait for work nobody was doing.** The
lag sweep's clean-store pass at ten million keys under 1.25 GiB read two
shapes, its scans taking a seventh of LMDB's major faults in one rep and
four fifths in two, and the row's own per-rep quantities split them --
the scans' major faults, the bytes other threads read, the device's time
a read -- before any probe; twelve sweeps then failed to reproduce it at
its worst, and the one line that separated the mild cases was the
sampler's fault price on the trace. A shuffled load's last merge drops its outputs' form files as
it builds them, its landing takes the store past the selector's line, and
the crossing asks all of them back a second before the first scan. Read
as a race with the read-in, that got two fixes its pairings refuted. Files
kept at their build were gone by the landing, the rule this file already
had for a page written once and never read, and were left dirty besides.
A settle that waited for the read found no read: the residency it polled
stood at half the files and did not move, because `POSIX_FADV_WILLNEED`
is advice and a group at its limit reads what it has pages for and drops
the rest without a word. The settle asks again where its looks stall,
with room made first, and the slow shape went from two sweeps of seven to
none (`Options::zpages_settle_joins_asks`). The rules: an ask of the
kernel is held to the kernel's count of what it did (`mincore`), at the
point that depends on it, and made again where it fell short, since
advice declined raises nothing; a wait is written against the state it
waits for and prints that state, since the first version of the wait
would have read as a fix that did nothing until its trace line said
`360.9 -> 360.9`; and a mechanism read off timing -- a read "still
running" -- is confirmed by a count of the thing itself before a fix is
built on it, twice here it was not.

**A megabyte bought at the first commit, and three policies built before
the device was priced.** The WAL's pre-write went in as a mebibyte of
zeros ahead of every commit, priced on the write mixes at three and ten
million keys, and the quick row a day later read ycsb-B at ten thousand
keys at half of LMDB's: B there is one commit in a pass of a millisecond
and a half, and it wrote and flushed the megabyte the mixes after it
rode. Paired against the appended log, B read 0.32-0.40x, the shuffled
load 0.73-0.83x and the full rewrite's burst 0.76-0.86x, with A and F as
far ahead as before: the megabyte's price had moved from the cells that
used it to whichever commit came first, and onto every commit of a
thousand writes, which gains nothing by it. Two fixes were then built on
a reading of that and paired -- a reach as long as the file, and steps
doubling from sixty-four kibibytes under a size gate -- and each moved
the megabyte between cells, B to A and F, the first commit to the first
after a load. A probe of the primitives (`bench/examples/walsync.rs`)
then said in a minute what four sittings had circled: zeros cost what
frames do, a megabyte two and a half milliseconds, and an extension a
fifth to two thirds of one beside; a pre-written commit saves a tenth of
a millisecond; so the purchase pays where a megabyte holds sixty
commits, loses by a third and more where it holds eight, and at ten
thousand keys, where the mixes write two thirds of a megabyte in forty
commits, no schedule comes out ahead of the appended log and each only
chooses the cell that pays.
And the size gate first written sat at sixteen kibibytes, under a guess
that the mixes' commits were thirteen kilobytes; an `strace` of the
writes read 16.8. A commit over thirty-two kibibytes appends now and
ends the run, and a run of small commits pre-writes a reach once it has
written one itself (`Options::wal_ahead_small_runs`). The rules: a cost
bought ahead is bought on the evidence of the run it is for, since a
store's first operation may be its only one -- the smallest write set
again, under a rule this file had for it already, and the default went
in without the quick row; a lever is priced on every size of the
operation it touches, a commit of one write, of a hundred and of a
thousand, and the sizes are read off the syscalls before a threshold is
drawn between them; a lever that moves a fixed cost between cells is
read on the cells' sum at the smallest rung, where a schedule that only
relocates it shows as one cell up and another down in every pairing; and
when two policies in a row trade the same cost, the primitive is priced
before a third is built. Beside it: the first pairings of the last shape
under a bound read the passes after two of the bursts a fifth slower in
six pairs of six, a mechanism was written for it -- faster syncs leaving
the upkeep thread a longer trail -- and the counters of the next sitting
read the trail at nothing in both arms and the passes in two shapes
under either, which a third sitting put on the builder ahead's install.
A difference in a pass of twenty milliseconds is read off its counters
before it is given a cause, and a cell that reads bimodal within one arm
is not a verdict between two.

**A shed the same pass undid.** The forms the commits' fills build at
write time are listed, and shed unwalked when the group's refaults move,
to give the page cache back what they took. That was priced where the
cache's budget binds, and under a budget that binds a block with no form
is left to the scan that wants it. At three million keys under 1.25 GiB
the budget grows into the room the partitions leave and never binds, and
there the commit's own fill builds a form for every overlaid block that
holds none: the pass that shed forty thousand forms built each again in
its next step, from the sources and sized for an overlay as large as the
block, a hundred thousand builds a burst where the store has forty-seven
thousand blocks, a hundred megabytes more in use and a second on the
burst, in the sweeps where the refaults moved inside the burst and not
in the others. The cell it lost was the scan pass after, and five
readings went elsewhere first -- the allocator's free bytes, the copies'
capacity, the page cache's tenants, the builder ahead, the first scan's
wait -- each true of the slow sweeps and none their cause. What named it
was a count every log already carried: the blocks the upkeep built over
the burst, fifty thousand in the fast sweeps and a hundred thousand in
the slow, and then the events line of the points before it, which split
the same sweeps at the one-percent burst by who had built their forms.
There is no shed while the fill stands to undo it
(`Options::forms_settle_sheds_unbound`), and the list the shed reads,
which only a shed had emptied, is pruned
(`Reader::prune_settle_built`): in memory, where the room never goes, it
had grown by an entry a build for the store's life. The rules: a
mechanism that gives work back is checked against whatever takes the same
work up in the regime it runs in, since a shed beside an unconditional
fill is two builds; when a cell reads two shapes, the count that
separates them is looked for among the counters already printed, at every
point of the sweep and not only the slow one, before a mechanism is
reasoned from the slow shape's symptoms; and a list something appends to
is read for who empties it where the emptying event never comes.

**A form dropped for a position that could be asked for again.** The
look that carries a table across the merge that rewrites its partition
over the same blocks kept the block's copies and dropped its sparse
forms, because a delta's cut is a rank in the old records -- and in the
same loop resolved again the cuts of the keys filed since the snapshot,
by the seek the settle uses. The dropped blocks went back to the fill,
two thousand a burst, and the ones a late landing left with few keys
since stood sparse at the scan pass, where a sparse walk reads the
partition's records between its deltas: under a bound those were records
the merge had just written and nothing had read, the first pages
reclaimed, a major fault a walk and the pass's whole wait. The look
resolves each delta's cut again and keeps the form now
(`Options::rebase_recuts_sparse`). The checked profile's comparison of a
conversion against a build from the sources had assumed the drop twice:
that a dense form's block is dense in the sources, and that the two
copies agree entry for entry. A kept form is dense with deltas the merge
folded in, and holds an entry of no values for a key the merge dropped,
which the sources have no key for at all; forty seeds of a hundred and
sixty failed on those two assertions with every answer right, and the
check is held now to a copy forced from the sources and to what the two
emit. The rules: where one loop re-derives a position for some of a
structure's entries and drops the rest for holding the same kind of
position, the drop is a choice and is priced as one; and an assertion
that compares a derived thing with a rebuild encodes how the derived
thing was made, so a change to what may be kept is read against every
such check before a fuzz failure is read as a wrong answer.

**A loop tuned under one regulator's batches, run under another's.** The
settle's loops were tuned while the buffered pace set a pass's size: a
hundredth of the store, fourteen thousand keys at three million, a few of
them to a block and two hundred ranks apart. There the resolve's gallop
from the last answer reads heads the seek before left warm, the next
block's form is asked for whole because several keys land in it, and a
seek from the index's top level, priced against the gallop, read level
and was not kept. The syncing pace is a quarter of a percent, and the
thread under it takes what a few commits left: over the full rewrite at
three million keys a pass held nineteen hundred keys, sixteen hundred
ranks apart, and the counters the sweep already printed said the rest,
2.62 million block visits for 2.70 million keys. Every visit was one
key's. The gallop over that distance was a dozen probes and a binary
search of ten more, each waiting on the one before, a sixth of the thread;
the whole-form ask was forty-eight lines for a patch that reads one entry
and writes one run, two of whose lines it never asked for; the form's
header, the slot that names it and the block's list of created keys were
each a line nothing had touched since the block's last visit. Five asks
ahead took the settle's cost a key down by two fifths
(`docs/engine.md`), each sized by what a pass holds now: the stride a
seek will search, named from the top level a few keys before the seek;
the header by the slot's pointer; the patch's own lines by the key's cut;
the value with its chunk; the table's lines further ahead still. The
rules: an ask ahead is sized by the batch's density in the structure it
is for, so when a regulator changes what a pass holds, every loop tuned
under the old size is profiled again; a refutation at one density is a
refutation at that density, and the lever comes back when the density
moves by an order; and the count that says which regime a loop is in --
here the keys against the blocks they were grouped into -- is read off
the counters before the profile is.

**A call saved and a wait bought.** Out of core a settle asks the kernel
for the heads its resolves will seek, and each ask first reads its own
page's residency, a `mincore` a key: a tenth of the upkeep thread at
three million keys under 1.25 GiB, where the store sits at the
controller's census with nearly all of its ordered indexes in memory. One
look at a partition's whole index, standing for a hundred milliseconds,
took that tenth off the thread's CPU, and the burst read one to three
percent longer in both orders with the writer held a quarter to a half
longer: under the bound the indexes' pages leave the cache all through
the burst, and three to five thousand of the thread's seeks faulted on a
page gone since its partition's look where six hundred did with an ask a
key (`perf record -a -e major-faults -c 1 -d`, the faults laid to their
files by address). The thread's CPU had been the measure and its wall was
the price. The same record named what the calls had been standing in for:
the scan after the burst took eleven to twenty major faults on those
pages in one pass of three, in its start seek, and those were the passes
that ran two and three times the others. The rules: a lever on a thread
that paces another is priced by that thread's wall, the holds and the
burst, before its CPU; a cache of an answer the kernel can change is as
stale as the kernel is fast, so the rate the answer changes at is counted
before the cache is built; and a call a key that guards against a fault
is a sign the pages should not be the kind that leave.

**A count keyed on a generation a landing moves.** The engine reads
whether scans are around from the writes since the last one, two stores'
worth being far from reads, and kept the count twice: the commit's
hand-over by its batch (`upkeep_since_scan`), which paces a burst only
while scans are around, and the pass by the log's growth
(`Reader::settle_due`), which files only then. Each took "the whole log
where the generation has changed since", which was a new memtable while
the writer was the one publisher. Since the segment work has a thread of
its own, a landing publishes a new generation over the live table it
leaves alone, and every landing added that table's whole log to both
counts again. Over the lag sweep's full rewrite at three million keys, a
seal's two landings and a merge's for every hundred commits, the counts
passed two stores' worth with one written: for the burst's last one to
twelve percent the hand-over stopped pacing and the pass stopped filing,
and the first scan after the burst read up to ten milliseconds of log and
settled up to a hundred and twenty, a pass of a hundred milliseconds at
the median where the others took sixteen. It was a third of the bursts
on one host, thirty of ninety, and none of thirty on the host before it,
where the same count had ended just under the line, and the pairings of
that day read it as the host's until two counters every sweep prints
were read rep by rep: the commits held, 2,363 to 2,668 of 2,700 in every
stalled burst and 2,700 in every other, and the first scan's `sync` and
`settle` phases. Both counts are keyed on
the live table now (`State::lives`), as the lag tick's had been since the
same mistake was found there. The rule is the one "a pass that reported
done for a turn it skipped" gave, a check that a thing is the one named
compares the quantity the work depends on and not a counter other things
also move, with three corollaries: when a second publisher is given the
pointer, every counter keyed on what only the first used to move is read
again, not only the one that failed; a regulator that stops regulating
raises nothing, so the count of the operations it held is printed beside
the count it should have held and read per rep; and a cell that moves
between hosts with the engine unchanged is read for a threshold the
engine sits at before it is read as the host's.

**Whatever the raw form serves now, taken for what it is left to
serve.** The ordered index's heads are eight bytes a key that every seek
of a raw partition reads, and under a bound their mapping's pages leave
the cache between two seeks however often they are read; held in the
process's own memory they are not the cache's to take, and the bound
counts them either way (`Options::heads_owned`, the entry before this
one's last rule). The first version copied the heads of every partition
the raw form was serving at the look, and at three million keys, where
the store sits at the controller's census through a full rewrite, it
took a quarter off the upkeep thread's settle and the stalls of a device
wait out of the pass after. One run of the same lever at ten
million keys read its counters before its rates: ninety-two copies made
over a lag sweep and ninety-two given back. A partition a merge lands is
served raw until the controller attaches the file the merge built for
it, a tick later, and at the load's end every partition is served raw
for the second between the crossing and the install; copied in that
second, the copies read every index file from the device beside the
first scans, a pass of five hundred milliseconds against four hundred.
The second version waited for a controller's step that had bought
nothing, and the crossing still made eight to twenty, since the
controller sits a moment on a purchase its model has already predicted
while the reads' faults make the price. A copy is made now for a
partition the controller is leaving raw by its own account: under the
form's gate one it has declined, and at the census every one while its
model says the raw form fits (`Maint::z_heads_look`, `heads_left_raw`).
The rules: a state read off what serves at this instant is a state read
between two steps of whoever decides what serves, so the decider's own
account is asked -- its decline, its model -- and not the cell it has
not filled yet; a lever built at one rung is run once at the other for
its counters before any pairing of it, where a copy made and given back
ninety-two times is one line of one run; and a memory economy that
copies is priced by the reads the copy makes, on the thread and in the
window it makes them.

**A decode as long as its container for a read that wanted one record,
into a cache that took whatever it was shown.** C at ten million keys
under 1.25 GiB was one of the four cells the row at `7b90eb8` read at or
under LMDB's, and its point read through the block-coded form decoded a
group whole at every miss of the memo of decoded groups, into a buffer of
the memo's, which the group then held a slot for. A divided C is 167,000
reads; 71,000 of them decoded a group, and most of those groups were read
once in the mix, a Zipfian's tail being most of its distinct keys, so each
such read wrote eighteen hundred bytes to answer for a hundred and thirty
and turned a group the head would read again out of one of the memo's four
thousand slots. The profile named the decoder only after it was cut twice:
the mix's window between the harness's marks was two fifths `pow`, the key
generator's zeta sum over ten million terms, which the harness computes
inside the marks and outside its clock, and of the engine's samples left
the decoder was 35-39%. Three things were built, each an option priced by
taking it away. A read's first touch of a group decodes it only as far as
the walk for its key reads, into the read's own buffer, three fifths of
the group's bytes at the mean, and the memo takes a group at its second
touch while a tag of the first stands (`Options::zpages_point_part`,
`zlz4::decompress_more`, `zfile::Memo::admit`): the read pass read
1.2-1.4x on one thread and to 1.7x on three, and B, C, D and the scrambled
B and C ahead in both orders. A walk's first touch of a group is its
cursor's own decode by the same rule (`Options::zpages_scan_admit`): the
threaded scans beside the mixes read 1.1-1.2x in both orders, with the
threaded scan pass level. And a table of where the keys read through the
memo lie, asked ahead of the filter and the fence
(`Options::zpages_point_keys`), answered half of C's reads and a quarter
of the scrambled C's: C read 1.02x to 1.19x for it in four sittings of
four and the scrambled C ahead in three, and D, which reads the latest
inserts and no key twice through the form, 0.95-0.98x in all four, the
price of a probe of a megabyte the core's cache does not hold. The table
was turned off between its second sitting and its fourth. The first two
read the six scrambled mixes four to six percent lower by their geometric
mean, and the scrambled D's count, a hit in two hundred and seventy reads,
was written down as the scrambled mixes'; the next two read that mean
three and eleven percent higher, every pairing of the lever was run again
for a default that then moved back, and the counts by mix had said from
the first counted run which cells the table reaches. One cell moved with
nothing to move it. The scrambled E mix, which makes no point read, read
0.87x, 0.92x, 1.00x and 0.90x for the partial decode over four sittings,
and a counted rep of the two arms read its scans' groups, memo hits,
misses and admissions the same per scan to two figures, the main thread's
faults the same, and the hot-raw policy's releases the same in every run;
E beside it read 1.13x and 0.81x in both pairings, the pair's first arm
ahead, E's time being the price of its block cache's fresh pages, which a
pair's second place pays more for. The lean is left as read, a tenth with
every count against a cause. The partial decode is licensed by the page's
first decode, which walks every group whole: a decode that stops at a
record does not see what the block's end would have said, and the test
that first asserted it did was the one wrong. The rules: a cache admits on
evidence of a second reference where what it is shown is mostly seen once,
since what a first touch displaces is the price of taking it; a decoder
that runs to the end of its container is asked how far its reader reads;
an option's verdict is read cell by cell where its mechanism counts, never
as a mean over cells it reaches differently, and a count read in one mix
is not its family's; a cell that moves under an option is held to a count
of the option's mechanism in that cell before it is read for or against,
since two sittings can agree about the pair's order or about nothing; and
a profile cut to a phase by the harness's marks is cut again to the
engine's symbols before a share of it is read, since the marks bracket the
harness's own setup.

**What a reader dropped in place, a publish in flight brought back.** A
look that passes a table drops it from `State::replaced` where the state
stands, and the list is copied into the next state by whoever publishes
it. A publish reads where the look stood, copies the list and swaps its
state in, three steps on a thread that is not the look's, and a look that
fell between the first and the third stored its new position and cleared
the list of a state the swap then replaced: the table it had passed, and
the seal's snapshot copy it holds, stood in the new state until the next
look, whenever that came. The test written for the drop found it, once in
a gate and once in fifteen runs under eight-way load, never alone, after
passing for a week on a wait for the wrong event: it waited for a landing
to be counted, the count that moved was the load's table's in every run,
and the table it froze landed later, under the look, in the runs that
failed. A trace of the store's four counts at each step read the same to
the last step in the runs that passed and the runs that failed, which is
what said the wait was not waiting for what came next. Every publisher's
swap is one function now, which trims the list of the state it swapped in
by where the look stands after the swap (`Db::swap_state`), and the look
trims whichever state is current after it has stored where it stands
(`Reader::note_over`); the swap and the load after it, the store and the
load after it, are sequentially consistent, so one of the two sees the
other. Two seams leave each order standing (`Db::hold_landing_swap`,
`Db::look_after_a_landing`), and the first test waits for the table it
froze. A third seam, for the writer's own freeze under the upkeep thread's
look, was built and taken out: the writer holds its upkeep through a
commit's freeze, the look it was to race could not run, and a second copy
of the trim that nothing could reach became the one function instead. The
rules: a drop made in place on a structure others copy forward is made
again by whoever copies, after the copy is published, and by the dropper
on whatever was published since, both where neither can see the other
first; a wait in a test names its event by something only that event
moves, since a count another event also moves is moved by it in every run;
and a check two callers need is one function, so that the test of one
caller holds it for both.

**A reading kept past the workload it was of, and a gate that read a counter across a gap.**
The writer gives its block forms up where its scans' start blocks do not recur
and takes them back where they do (`Options::scan_forms_off_unreused`), and it
decides at a scan: between two scans the last reading stands, and while it
stands the commits file nothing. In the row's order the uniform scan pass gave
the forms up and the writer's next scan was E's first, five mixes later: B, A,
F and D committed a hundred and eighty thousand writes into no forms, and E
took the forms back at its 512th scan and built its block cache inside its own
window -- fifteen thousand sparse forms, twenty thousand more from the builder
ahead and seven thousand first touches on the scanning thread, in half a
second. That was the one cell the row at `e4021f7` lost at ten million keys,
0.94x of LMDB's and two shapes by rep. The regime's own pairing had read E
level against the arm that keeps its forms, and a later note had named what E
pays, "the price of its block cache's fresh pages", without asking why E was
the mix paying it. The reading is stale now after a stretch of a mix's small
commits with no scan between them (`Options::scan_forms_off_stale_writes`; a
burst's batches, the lag sweep's, leave it standing, since filing a burst is
the writer's to pay), and that moved E in three counted reps of four. In the
fourth the write mixes still filed nothing, and a count of the commits that
file nothing by the clause that refused them (`Db::settle_refusals`: the forms
off, far from a scan, no room, nothing due) put every one on the fills' room.
The room closes for a second when the memory group's refaults grow, and it
read growth as any growth since its last reading, however old: the first
commit to ask after the read passes compared the counter with a reading
twenty-six seconds old, the read pass's twenty-four thousand refaults, and
closed the room for B and A; and three pages refaulted in a hundred
milliseconds closed it for F and D, with three hundred megabytes of room by
the gate's own byte count on the same line of the trace. The room reads the
refaults against the line the controller's pressure term is drawn at, over a
reading no older than its quiet (`Options::forms_settle_room_pressure`,
`refaults_moved`). With both, the write mixes file at every commit on the
upkeep thread in every rep, E builds no sparse form and takes four hundred
first touches, and paired against the forms kept off until a scan E read
1.56x and 1.24x with the arms listed either way, ahead in every pair of eight. The rules: a regime entered on a measurement of one kind of operation
is looked at again when the workload stops making that operation, not only
when the operation comes back to be measured, since the cost of the stale
reading falls on whatever comes first after the stretch; a difference of two
readings of a counter is a rate over the time between them, so a reading
after a gap is a baseline and says nothing of now; a gate on a platform's
counter is drawn where the counter's owner draws it, one line named once,
since any growth at all is the background of a group at its limit; a lever
behind two gates is read with the refusals counted by gate, since with one
gate open it reads as a lever that works in some reps; and a note that names
what a cell pays is half a finding until it says why that cell is the one
paying.

**A decode halved for a read whose cost was not its bytes, at the price of
the room another cell lives in.** C's time at ten million keys under 1.25 GiB
is three quarters first touches of tail groups, each a partial decode, and the
obvious lever was the group cut: a kibibyte of records a group where the form
ships two (`Options::zpages_group_bytes`, `supdb-zgroup1k`). The arm decoded
0.60 of the bytes on its first touches, as designed, and C read 0.97x: a first
touch is the page's header and directory, the payload's first lines and the
tables beside them, misses a group's size does not change, and the bytes the
decoder then runs over are the cheap part. And the cut costs the form a
twentieth more of its bytes, which at this bound is the room the hot-raw
policy releases into: five partitions released a rep where the default
releases eight, E 0.75x in four pairs of four and the threaded scans 0.89x,
for the scrambled point-read mixes a seventh ahead. The rules: a read's cold
cost is counted in the lines it touches before a lever is built on the bytes
it decodes, and the arm that changes only the bytes is the test; and a format
parameter that trades size for speed under a bound is priced on the cells the
size pays in, which are whichever tenants of the page cache the bound was
already squeezing, not on the cell the speed was for.

**A structure dropped by a generation that counts what it was not derived
from.** A handle's scan snapshot is the memtables' unsealed keys, sorted,
and the look at the log dropped it at every new generation, which a merge's
landing moves as surely as a freeze does. The writer's own look carried it
across such a landing with its tables (`rebase_tables`); the look without
tables -- every reader handle, and the writer on the merge paths, which is
where it scans once it has given its forms up -- dropped it, and the scan
after sorted every unsealed key again: 388,000 of them at the pass after the
lag sweep's full rewrite at ten million keys under 1.25 GiB, 50-60 ms a
build, fourteen or fifteen builds in a pass where the merge chain landed
inside it. The snapshot stands across a publish that leaves the memtables
now (`Options::snapshot_stands`), trimmed of a landed table's base. The
rule is the one "a count keyed on a generation a landing moves" gave, for
a cache: a derived structure is invalidated by the identity of what it was
derived from, and a generation that every publish moves invalidates it at
publishes that changed nothing it holds.

**A question answered from the pages a step had just let go.** A merge was
made to drop behind it the raw records of an input partition its form file
serves, which nothing reads once the form serves it, so the pages it read
back would not stand as the cache's newest through the landing
(`Options::merge_drops_covered`). The burst ran faster with it and the pass
after it ten times slower, 124 pieces standing where 26-31 had: its merges
took as long, and a third as many started, because of each pair the
second landed half a second after its job had ended. A CPU profile of the
thread every landing goes through named `Db::same_blocks`, the landing's
question whether a rewritten partition cuts the same blocks, which read the
old partition's key every sixty-fourth rank through its records -- the
pages the merge had just dropped, read back a page at a time while the next
merge waited to be planned. Without the drop the merge had left them warm,
and nobody had priced the question at all. It reads the ordered indexes
now, whose top level holds a block's first key in memory
(`Options::same_blocks_from_index`), and the drop, repriced with the
question answered there, bought nothing and is off. The rules: a step that
gives pages back is priced at every later reader of those pages, on every
thread, and the thread that serialises the landings is one of them; a
question one structure answers from memory is asked of it before one that
has to be paged in; and a phase that runs faster under a lever is read for
the work it pushed out of itself, since a burst whose merges did not start
is a burst with less beside it.

**A gate drawn at the largest burst, read at a smaller one.** The rewarm
asks for the form files the cache has dropped when scans come back after a
stretch without one, and waited for a second of quiet and for every merge
in flight to land, both drawn at the full rewrite, whose merges hold every
partition and pushed out what a rewarm beside them brought in. Under a
gibibyte at ten million keys the ten-percent burst lasts 1.0-1.1 s and
leaves a chain of merges running through the pass after it, so the rewarm
fired at that pass in two reps of eight, and the pass read the burst's new
form files a 4 KB fault at a time -- nine thousand faults, every one in the
page view's first read of a file written once and read by nobody, the
first pages a reclaim takes -- and trailed RocksDB, whose compactions' files
stayed in its cache. A rewarm beside the merges, for the partitions they do
not hold, after a quarter second (`Options::zpages_rewarm_beside_merges`,
`Options::zpages_rewarm_quiet_ms`) read the pass 1.3-1.7x in every sitting
with a third of the faults; under 1.25 GiB it found no cold file to ask
for. The faults by file were read with `perf record -e major-faults -c 1
-d` cut to the pass's lag marks and the process's maps. The rules: a gate
drawn at one case is checked at the cases between it and the next, since a
threshold a burst crosses by a few milliseconds fires in some reps and not
others; and a pairing in which the mechanism never fired is the noise
floor, read before any other -- the same pair under 1.25 GiB, no rewarm in
either arm, moved its cells 10-20% each way between sittings.

**Two levers priced as one, and the half that was the cost.** That rewarm
shipped as two changes paired together, the quarter second and the asks
beside the merges, and the next 1.25 GiB row read the pass after the full
rewrite at 0.54x of LMDB where the row before had read 1.81x. The row
before was another host's; on this host class the cell had read 0.7-1.1M
entries a second in every row since the chain, so the drop was the host's
and the loss was a cell to win. The faults by file put the pass's major
faults on form files in their page view's first read and on the burst's
last pieces, and the rewarm's trace, given each asked file's residency, the
room and whether merges ran, split the sweeps on one column: every pass
whose rewarm asked beside the chain took 7,200-10,900 faults under 1.25
GiB, every one whose rewarm found no merge 74-3,300. An asked file is read
in by evicting what the kernel ranks last, and after a burst that is the
burst's own pieces, written once and read next by the pass. Two levers
were built on the way and taken out by their counts: skipping the files of
partitions a merge was due to rewrite never fired, since the asked
partitions held one or two pieces under a trigger of four; and bounding
the asks by the room the group measured asked the moment room opened, and
the merges' writes took the room back -- 7,300 faults in such a sweep, as
many as the asks it replaced. The ten-percent pass the asks were for found
no merge running at its rewarm in every sweep since, so the quarter second
was its gain, and the rewarm waits for the merges again
(`Options::zpages_rewarm_beside_merges` off; `supdb-zrewarmbeside`). The
rules: a lever made of two changes is priced by taking each away; a cell
that drops between two rows is read against its own history on the row's
host class before it is read as the engine's; and a measured room is a
reading of a moment, so an ask that the next writer will take the room
from is not made into it.

**A key hashed by every source it was looked up in.** A point read
consults the partition, each level-0 piece over its range, the frozen
tables and the live one, and each hashed the key itself, the same FNV a
byte and a multiply at a time, and only once the source before it had
been read: the piece's filter probe, its fence and the hash in front of
it were a fifth of the scrambled D mix's thread at a hundred and three
hundred thousand keys, the whole of the gap to LMDB, because the
scrambled write mixes leave a piece below the merge trigger and the
unscrambled ones leave none. The read takes the index's FNV word once
and every source's hash from it, and asks for each piece's filter block
before it reads the partition (`Options::read_hash_once`,
`Options::read_lines_ahead`). The first shape of the asks also asked for
the partition's first slot on every read, an ask the lookup followed at
once: callgrind over the read pass counted it at 44 instructions a read,
and the pairing read the pass 0.94x for it while the mixes with a piece
gained a fifth. The rules: a quantity every source derives from the key
is derived once by the caller; an ask ahead is made only where there is
work between it and the load it is for, and its instructions are counted
on the path that has none; and a mix family that leaves a different
store shape -- a piece standing, a frozen table -- is read for which
sources its operations consult before its cells are compared with the
family that leaves none.

**A form given up for a cost that had since gone, and the costs that
scaled with it when it came back.** The last block of the last partition
collects every key inserted past the store's greatest, and it had been a
copy once and was made a merge of its sources (the wide form) because the
copy was rebuilt after every inserting commit, 12 ms a rebuild at three
million keys. A patch has spliced into a copy since, so the reason was gone
and the rule stayed: the scrambled E's scans that started among the inserts
merged a window of every source over the block, 50,000 instructions a walk
and 385 walks a mix at a hundred thousand keys, about a tenth of the mix's instructions.
Copied once its walks had paid for the build and kept past the wide bound,
the block read slower at three hundred thousand keys before it read faster,
because two costs no copy had paid scaled with its size: the patch summed
every entry's run to ask whether the copy had bloated, and the walk into it
asked for its whole entry and key buffers ahead, a few hundred kilobytes of
lines a walk. A running count of dead bytes and the copy's first lines took
both away (`Options::forms_copy_hot_wide`, `CachedBlock::dead`). The same
work found the window's merge taking a scan's limit from every source and
merging them all for the limit it read (`Options::scan_wide_capped`), and the
tombstone search run for a key one source held (`Options::emit_lone_unsearched`).
The rules: a structure retired for a cost is looked at again when the cost
goes, since the rule outlives its reason; a form allowed past a bound its
code was written under is read for every per-operation step that is linear
in its size, since none of them was ever priced at that size; and a test of
a form that stands across writes writes the way the workload does -- an
ascending run of inserts into an empty table opens a direct run, whose close
drops every block table, which read as the form being dropped until a
backtrace on its drop named `close_direct`.

**A second pass that asked again what the first had found.** A point
read over a store with tombstones runs a pass newest source first for the
newest tombstone and then reads the values oldest first, and the second
pass asked every source again: the live table's probe, each frozen
table's, each piece's filter and, where the filter passed, its index. A
`put` is a delete and an append, so every store an update mix has reached
has tombstones, and every point read after the first write mix paid the
second probe; the scrambled D read 0.93x of LMDB in the row's order with a
seventh of its thread in the pieces' filters, and the read pass, over a
store with none, never showed it. The pass keeps what it found of each
source for the reads after it (`Options::read_probes_once`): callgrind
over three hundred thousand keys with two pieces of puts and puts in the
live table counted 1,492 instructions a read against 1,658, and the read
pass with no tombstones level. The rule: a pass over sources a pass
before it probed takes that pass's answers, since a probe asked twice is
paid twice; and a cost that appears only once a store has a property the
workloads give it after their first write is priced in that store, not in
the clean one the read pass meets.

**A recovery that made again what it had been left, and that nobody had
timed.** The open made a piece of a run a killed writer left by reading
the run's file whole, copying every key and every value out of it into
vectors of their own, feeding each back through a segment writer into a
new file, syncing that and removing the old one, and then opened the
piece and walked its records again for a Bloom. The log beside it was
read whole into a buffer before a frame of it was parsed, and the store's
greatest key was found by a walk of the table the replay had just filled.
Nothing was wrong, and the open of a store of three hundred thousand keys
took a fifth of a second: the suite had opened no store until the
recovery workloads, their first rows read supdb's open of an ordered
load level with RocksDB's replay of its log and two hundred times
LMDB's, which recovers nothing, and were read as what a recovery costs,
and the profile of one open was taken only when somebody asked what
there was to do while a pull request waited (`bench/examples/reopen.rs`; the counts are
in `docs/engine.md`, crash injection). A third of the ordered open's
instructions were the allocator's and most of the rest the second
writing of bytes the file already held in the form a segment keeps them.
The open finishes the run's file where it lies now, by the finish its
close would have run (`SegmentWriter::resume_run`), over a walk that
keeps offsets and copies nothing; the log is parsed through a mapping,
and the greatest key kept as the records go by. A recovery that edits
the file it recovers has crash windows the file never had, since an open
cut short leaves the writer's file with part of an index, or whole, or
linked, or named: `tests/runlog.rs` builds each, and holds the finished
piece to the segment a clean close of the same commits makes, byte for
byte. One line written beside the change was a hazard: a removal of
whatever stood under the piece's name before the link, against a stale
name an earlier open might have left. A mutant without it passed every
test, and reading for a state that reaches it found the open's sweep of
unnamed files ahead of it -- so the only file that could stand there was
one the manifest names, and the removal would have unlinked it. The
rules: a recovery path is a path, profiled when a workload first takes
it and never excused as rare or as the design's price; in one, look
first for work that reproduces what is already on disk in the form the
format keeps; a recovery that writes into what it recovers is tested at
each of its own crash windows, built from the states before and after
it; and a defensive step that no test can reach is read for what it
would do in the state that could reach it, before it is kept as
harmless.

**A turn left for the next one, which had no reason to come.** The
upkeep thread's fill is left for a later pass while the writer has a
newer commit posted (`Options::upkeep_fill_skips_posted`), on the reading
that the pass for that commit would make it. A pass files and fills only
where it is due, and it is due by what its own commit brought -- a backlog
past the bound, a burst's batch, a scan since -- so the commit posted
behind a pass could bring one that did nothing: a commit with nothing
staged, or a mix's hundred writes under the bound. The fill was left by
the first pass and taken up by no other until some later pass was due of
itself. `a_load_nobody_reads_files_no_forms` ends its writes with a commit
of nothing; where that commit was posted while the pass for the last batch
was in flight, the settle after it returned with no form built, and the
arm that files far from reads read as filing nothing: once on a hosted
runner, in none of three hundred runs of the test alone under load and
none of twenty on one CPU. Three reconstructions through the seams
reproduced nothing, each a guess at the schedule: the fills' room closed,
the thread held before each pass, a held pass under the wrong commit's
name. What reproduced it was the test's own store in a loop beside the
whole test binary's load, seven trials of a hundred and fifty, and what
read it was the counters the assertion had been given before the hunt:
four to seven fills left, no fresh build, the last pass refused as
nothing due. A fill left is owed now (`FormsState::fill_owed`) and the
thread's next pass is due by the debt, none of four hundred trials under
the same load; the test holds the pass and posts the empty commit behind
it (`hold_upkeep_pinned`), near reads in the store as it ships and far
from them under the arm, and fails without either half of the fix. Across
the suite's mixes and lag sweep at ten, a hundred and three hundred
thousand keys under both policies, a fill was left in 2,519 passes of
9,038 and no pass was due by the debt alone (`Db::fill_owed_due`, `owed`
on the counters line; `docs/engine.md` has the command): every commit
behind a deferral there brings a pass due of itself, so no cell can have
moved and none was priced. The rule: work a step leaves to the next step
is recorded as owed where it is left and makes the next step due, since
whether a next step runs is decided by its own inputs and never by what
was left for it; a flake no run of the test alone reproduces is run
beside the load it failed under before its schedule is guessed at, and
its assertion is given the counters first, since a failure that comes
once is read from whatever it printed; and a change to when work is done
is held to a count of how often it fires in the suite's own passes before
it is priced, since a mechanism that fires in none of them leaves a
pairing nothing to read.

**A form priced at the one bound it fits, and an arm that kept its
sibling's setting.** The stride form -- a partition's values alone at
their stride, read by arithmetic on the key -- was built for the C mix at
ten million keys under 1.25 GiB, and its forced arm read C there at three
times LMDB's with no fault. Its bytes are the payload's, a gigabyte at
that store between the block form's 0.72 and the raw form's 1.45, so it
fits beside a lean process under that bound and on neither side of it:
under 1 GiB it took 0.085 faults a read and lost five of the six
scrambled mixes to the default, at three million keys, where the raw form fits, it traded
its point reads for the scans, E and the loads, and the budgets at which
it passes the fill share the coding gear keeps while the raw form does
not fit are 1.18 to 1.45 GB, which the suite's bounds lie under. That
arithmetic needs the three sizes and nothing else, and it was done after
the form was built, reviewed six times and gated. And its first pairing
beside the block cache read E at 0.6x of LMDB with 0.028 faults a read in
the first read pass, which was the arm's and not the form's:
`supdb-cache64tofitzstride` is its block sibling's policy with one field
moved, and the sibling keeps the raw records in the page cache
(`zdropraw` off), so the stride files stood beside the form they replace.
The rules: a second form's size is set against the budgets on either side
of the cell it is built for -- the bound below it, the rung where the raw
form fits -- before the form is built, since a form that wins at one
bound has a band and the band is arithmetic; a probe that shows what a
cell allows is a ceiling for that cell and says nothing of its
neighbours; and an arm made of a sibling's policy inherits what the
sibling pins, so the options a pairing's question rests on are read off
the arm before its cells are. It came again with the stride form's
sparse variant, built for keys that are no run of numbers: a hundred and
eight bytes a key where the block form's are eighty-five, a gibibyte at
ten million keys, priced for two days at 1.25 GiB and 1 GiB, where the
store in it fits only while the process is at the sixty megabytes it
holds after a load and not at the three hundred and forty it holds by
the scrambled mixes. Three rules for asking the form were built and
paired at those two bounds, each a different way of losing less, before
one traced rep at 1.5 GiB, run to show the form where it fits, read the
store whole in it ahead of the shipping engine in nineteen cells of
twenty-two (`docs/engine.md`, keys that are no run of numbers). The
corollary: the arithmetic of a form's band is done with the process at
the most it holds through a row, read off the group's own samples
(`scripts/groupmem.py`), and the first pairing of a form is at a bound
inside the band, whether or not the roster has one there.

**A fit read off a file's size, and an estimator built before the counts
at hand were held to the answer.** The stride form was written off under
1 GiB as a form that does not fit, its gigabyte against a fill share of
the budget, and paired form against form in the row's order
(`docs/engine.md`, which form a workload calls for) it read C at four
times the block form there and the scans at twice, with no fault until
the write mixes had grown the process. Its faults were the same to three
figures in both orders, and a model of the page cache with one unknown,
the bytes the pass left to the form, solved to a capacity that rose 75 MB
through the read passes and fell a hundred through the mixes, every kind
of pass in a stretch agreeing on it (`bench/examples/formfit.rs`). So a
form fits or does not by the pass, a line drawn at a share of a budget is
crossed both ways inside one row, and the store that reads fastest at
such a bound is in two forms. How much of each the model then got wrong:
priced at the capacity the stride arm's faults implied, a store half to
three quarters stride was to read ahead everywhere, and built
(`Options::zpages_stride_below`, the arms `...zstridedrop2m` to `...7m`)
it faulted through every pass after the write mixes. The one unknown was
three things a census of the page cache by file counted in one run: the
process, a hundred megabytes larger after the mixes than before; the
ordered index's heads, which the block form's scans keep in the cache
and the stride form's never touch; and, over the read passes, no limit
at all, the group standing a sixth under its bound while the form files
were read back after the load. With the room counted so, three tenths
of the keys in the stride form fit, and that store read ahead of the
block form or level with it in sixteen cells of twenty in both orders,
C at two and a half times. The
estimator went the way the form had: a sketch of reuse distances that
every form would read at its own bytes a key was built first, read the
forms 8% to 50% off because a granule of ranks is no form's page, and was
taken out for two things already there, a miniature of the cache itself
and Che's approximation over the reads the store counts a partition
(`zselect::model_populations`), neither of which put a store that faults
on the side of one that fits in any line of the probe. The rules: a fit is
priced as faults an operation in the pass that would take them, never as
a size against a share; a model of faults is held to the measured ones by
solving for the capacity pass by pass and asking whether it moves like one
quantity; an unknown solved from one arm's faults is that arm's, so
before it prices another store what it is made of is counted, by file
and by owner, at the pass it is to be used in; and before an estimator
is built, what the store's existing counts already predict is held to
the exact answer first.

**A form priced in the arm that forces it, without the lifecycle the
controller gives one.** The stride form's forced arm read the lag sweep
at ten million keys under 1.25 GiB with its one-percent burst at a sixth
of the default's rate, the pass after the ten-percent burst at 0.6 major
faults a scan and the pass after the full rewrite at half the default's,
and the first reading was fit again: a gigabyte of stride files beside
the burst's pieces. The counts said otherwise for two of the three. At
the ten-percent pass the group stood 180 MiB under its limit while the
scans refaulted the form 4 KiB at a time: the burst had pushed the pages
out and nothing asked them back, since the rewarm runs only once the
selector has installed a form (`Maint::z_rewarm_look`) and a forced arm
installs none. The burst's writer was waiting and not working, with no
hold, behind form files built just before it and never synced, since a
build syncs only a file it drops (`BuildPlan::sync`). Only the third was
the form's. `supdb-zstride` is the default with every build asking for
the stride form, and under the controller the first two went. What was
left is a fit the selector's model cannot see: its budget is the limit
less the process and a margin, and after a full rewrite 400 MiB of the
page cache is the burst's pieces, the indexes, the log and the merges'
own traffic, so it predicted no fault for a form the pass found a
quarter short of its pages (`docs/engine.md`, which form a workload
calls for). The rules: a mechanism measured in an arm that forces it is
measured without whatever the controller does around it, so before a
forced arm's loss is read as the mechanism's the gates on `installs` and
on the gate's state are listed and the arm that leaves the controller in
charge is built; a fit is read against the group's own count at the pass
before it is reasoned from sizes -- room unused beside refaults is an
ask not made, and no room is a fit; and a budget is named for what it
subtracts, since one that leaves out a tenant says yes on that tenant's
behalf.

**A rule taken build by build, of a store whose size its builds do not
know.** The stride form asked for only where the store in it fits the
room (`Options::zpages_stride_room`) was right at ten million keys under
1.25 GiB, where the store whole in the form fits, and at 1 GiB it built
24 of 29 partitions in the form, each while the store so far fit: the
store that stood at the load's end did not, and from the write mixes on,
the process a hundred megabytes larger, it read the scans beside the
mixes at 0.3-0.5x of the default at 0.3 major faults a scan
(`docs/engine.md`, which form a workload calls for). It is the raw-tail
pick's shape, read a third time: a choice that holds for a store's life,
made over a store still being written and a process at its smallest. A
rule at the build can only be the load's half of the choice; the other
half is a way to take the choice back once the store's size and the
process's are known, and the room that half is judged in is read with
the process at the most it has lately held (`Maint::z_budget_grown`),
since the budget read after a load is the largest the store will see.

**A mutant that survived because the defect was already there.** The
test that a merge's outputs keep the form of the partition they rewrite
passed with the key ranges taken out of the plan, every output asked for
the stride form. The test was not weak: the plan read the ranges from
the partitions a form file serves, a merge's outputs are served by
nothing until a tick attaches their files, and a store's merges are
planned one at the landing of the one before -- so after the first merge
the plan found no range at all and built every later output in the block
form, with the ranges or without them. The independent review named the
same defect from the code the same hour. The plan takes an output no
form file serves yet for the form its range was last asked in
(`Maint::z_stride_spans`), and the test rewrites the store twice with no
tick between. The rule: a surviving mutant is read as a question about
the code under test before it is read as a hole in the test, since a
defect that makes two behaviours alike hides the mutant that tells them
apart; and a plan made at a landing reads the state the landing leaves,
which is not the state a tick later.

**A second form kept beside the first, under the name of neither.** The
conversion between the block and the stride forms first kept the file it
replaced, unserved, so that the way back would be an exchange and not a
build. A build renames its file over the partition's name, so the kept
file was an inode with no name, and after an exchange the file under the
name was the kept one: the rewarm, a settle's wait for the asked files
and a reopen each reach a form file by its path, and each would have
taken the kept file for the served one -- read it in, waited on it,
attached it. The review found it by listing every use of the name; no
test could have, since every answer stays right. The kept file is gone
and each way is a build. The rule: a file reached by its name in one
place and by a handle in another is one file only while the two agree,
so whatever can make them differ is checked against every use of the
name; and a cheaper move is priced after the plain one has been
measured, not built beside it.

**A model's edge that reads as its opposite.** `model_populations`
answers every touch a fault where the cache it is given is no bytes,
whatever the populations are, so two stores compared there differ only
by their decodes, and the conversion read a move into the larger form
as its decodes saved for nothing, in a room that was not there -- the
room it judges a move in is a share of the budget less what stands
beside the forms, and a burst's pieces alone can take it to nothing.
Nothing moves in where the room is nothing now. The rule: a model used
to compare two states is checked where its answer stops depending on
the states, since there the comparison is decided by whatever term is
left.

**A gap in a trace that printed only what was on offer.** The
conversion's trace printed a line for each move a look offered and none
for a look that offered nothing, so two stretches of two and three
seconds between one build's end and the next one's start read as
stretches with no look. The look's guard, which holds the next look off
a hundred times what the last one took, was the one mechanism that
could hold a look off for seconds; a look of thirty milliseconds was
supposed for it, the look was re-timed without the build it starts, and
the commit said the thirty milliseconds as found (`c24b7cd`). The trace
given a line a look read a look at half a millisecond and 2.4 at the
most, and looks every 0.3 s through each stretch with no move on offer:
the store fitted its room as the model had it until the next pass's
threads grew the process. The rule: a trace that prints on events
cannot tell nothing happening from nothing to print, so the turn itself
gets a line before a gap in it is read; and a duration nobody timed is
not written into a commit as measured, the message being the one place
a guess stands as a finding for good.

**An effect put on a window it was not in.** B at ten million keys
under 1 GiB read 0.76-0.78x for the conversion arm in four pairs of
six. The conversions were the arm's difference there, each one syncs
its file and B commits, and the note said so: a write mix in the
window, its commits beside the conversions' syncs. No pass's edges were
on the conversions' clock. Stamped there (`SUPDB_LAG_MARK` beside
`SUPDB_ZPAGES_TRACE`, the store's `zepoch` line giving the controller's
zero on the marks' clock), the last conversion was in at 35 s and B
began at 42. What B read behind by was the hot-raw policy, which stood
down wherever the stride form was asked for, so that the arm served no
partition raw where the default serves its hottest -- and the arm
whose load is built in the block form, with no conversion in its run,
had read B the same 0.8x two paragraphs away, which was the count to
hold the reading to before it was written. The rule: a cell is put on a
mechanism once the pass's edges and the mechanism's events are read on
one clock, and not before; and where two arms that share a cause and
not the suspected mechanism read the cell alike, the cause is the
shared one.

**A read-back named for all the files, taken by two.** The one-thread
read pass after the load at ten million keys under a bound was known as
the read-back of the form files: built ahead under the selector's line,
so dropped as they are written, and asked back at the crossing a
fraction of a second before the reads begin. A rewarm at the install
had been built on that reading, found every file on disk resident and
moved nothing (`Options::zpages_rewarm_at_install`). Every major fault
of the pass laid to its file (`perf record -e major-faults
-c 1 -d -k CLOCK_MONOTONIC` cut to the pass's marks, each address
through the process's maps) put 22,000 of the default's 26,400 and
30,300 of the stride arm's 40,100 in two form files and none in the
other 24: the two builds ahead in flight at the crossing, not on disk
for its ask, taken in after the install with nothing of them in the
cache and read a page a fault under the random advice. With those asked
at their collect (`Options::zpages_ask_dropped`) four fifths of what
was left was one partition's raw records, the tail partition the cache
had let go, which no build ahead takes and which waits third for the
builder (`Options::zpages_cross_asks_raw`). The pass was two shapes all
along, 0.004 and 0.027 faults a read, by whether a build happened to be
in flight at the crossing, and had been written down as the pass's own
churn. The rule: faults are laid to files before a mechanism is named
for them, since a fault count says how many and a kind of file says
nothing of which; an ask covers what is on disk when it is made, so
whatever lands after it is asked for where it lands; and a cell that
reads two shapes with every count the same is two events, one of which
nobody has counted yet.

**An exchange priced with an actual against an estimate.** The
conversion prices a partition's other form from what it has: a block
file's own bytes for a partition in the block form, and for one in the
stride form its records at the ratio the block files served come to.
Where no move pays alone it prices an exchange, the hottest block
partition in and the coldest stride one out. A store that had moved one
partition out, and fitted, was offered a second move a period later and
made it: the exchange set the moved partition's actual block size
against the estimate of another's, the estimate was the shorter, and
at the fill the model read the shortfall as room made, thirty-four
milliseconds a second of faults saved by swapping two partitions read
alike. A test that held one move
to be all a store needed found it; the pairings had not, since under 1
GiB every stride file has to go anyway. The exchange is priced with the
cold partition's block form in the share the hot one's is of its own
stride form, both of whose sizes are known. The rule: two quantities
compared are measured the same way or the comparison is of the
instruments, and a decision that turns on a difference is checked at
the difference the estimate's error can make.

**A rent dropped by every look that did not offer the move.** A move's
rent was kept in a table the look rebuilt from the moves it offered, so
a move offered at one look and not at the next began again from
nothing. The moves out of a store over its room are priced one after
another with the ones before them made, and between partitions read
alike the order is the noise's: a move the pricing had put behind two
others for one look, when two were all the room wanted, lost the 0.6 s
of rent it had been paid and was started 0.7 s later, the store over
its room meanwhile. Kept across looks instead, a rent that only grows would have
bought moves on noise: a swap that pays a little in half the looks and
loses a little in the other half would come to its price in time. A
rent is a balance now, grown by what the move would have saved over a
period at the period's own reads and cut by what it would have lost
where the partition is the better as it is, never under nothing
(`zselect::conversions_priced`, `Maint::z_convert_look`). The rule: an
accumulator that is reset by the absence of its own input measures the
input's continuity and not its sum; and one that is kept has to take
the losses with the gains, or it sums the noise.

**A job a step.** The controller takes a finished build or warm in and
starts the next at its step, and it steps once a census interval, a
tenth of a second. Nobody had counted what that makes of a chain: at
ten million keys under 1 GiB the warms of 29 form files the cache
already held took two and a half seconds, a job a step, and each of 24
conversions was taken in 20-90 ms after it ended, a second and a half
of nine and a half. The builder's thread wakes the segment work's as it
ends, and the turn it wakes into did nothing with it
(`Options::zpages_drive_at_job_end`). The rule: a worker polled at a
period is priced by the period times the jobs in its longest chain, and
a wake that the woken side does not act on is a wake nobody made.

**Both orders agreed, about a cell the options could not reach.** Three
options that act at the load's end -- a dropped form file asked back, the
crossing's ask for raw pages, the builder's turn at a job's end -- read
C at ten million keys under 1.25 GiB at 1.60x and 1.52x of the default,
five pairs of six, with the arms listed both ways. Nothing in any of
them runs in C. Each priced alone read C both ways, 0.80x and 1.27x,
0.69x and 1.56x, whichever arm was listed second ahead, and the default
beside them read both rates itself: C there is one cell in two shapes,
by whether the Zipfian's head partition is raw as its reads begin, and
the count that names the shape was on every table (the reads the
point-key table answered, 2,500 or 85,000 through the mixes). What
decided the shape in the traced pair was a threshold at the value it is
compared with: the fill's quiet look asks for a partition where its raw
bytes fit the room less a reserve, 43 MB and 94 at that store, and the
room as the threaded scans end is 130 to 136. The rule: two sittings in
both orders are a verdict on an option only where the option's own
mechanism is counted in the cell; where it cannot be, the option is
split into its parts and each priced alone before the cell is written
down as its gain, since a cell in two shapes agrees with any option in
both orders one time in eight; and a cell found on a threshold is
recorded as the threshold's, with the two values beside each other.

**A drop that met a page still dirty.** A served partition's raw file
and its ordered index are dropped from the page cache as the form is
installed over them (`Options::zpages_drop_raw`), and the ordered index
is written unsynced by design: `POSIX_FADV_DONTNEED` over a dirty page
starts its writeback and leaves the page, so the indexes of a load's last
landings, 14 to 21 MB at ten million keys, stood in the cache with
nothing to read them until a reclaim came. Under the block form the read
pass runs at the group's limit and the reclaim comes at once; in the
stride form the store is under its limit through every pass before the
scrambled mixes, the pages stood, and the hot-raw policy's room, the
group's memory unused, which counts a cached page whatever is in it, was
short of the Zipfian's head partition by less than the idle indexes
held. The arm of a form that reads faster made no release where the
default made its one, and A, B and the scrambled D read 0.8x for it in
eleven pairs of twelve. `scripts/resident.py` by file at each pass's
start named it, 20 MB of ordered indexes against 6. The segment work
notes each drop and advises the index's file again a second later
(`Options::zpages_drop_again`, off). The rules: an advice to drop is held
to the kernel's count after it (`mincore`), as an ask to read is, and a
file written unsynced is dropped once its writeback is through or twice;
a room read off a group's unused memory is short by whatever idles in its
cache, so the cache is laid to files by residency before a policy is read
as bound by room; and a leak one arm shows because the other arm's
reclaim hides it is both arms' -- the default paid nothing for it only
by being at its limit already.

**A decision made once a rep, read at the median of three.** The stride
arm under 1 GiB was paired with the default three reps a sitting first:
the hot-raw policy's releases read one a rep in both arms at the median
of both sittings, A level and B the arm's, and the default's flip was
drafted in the tree -- the options' defaults, their docs, the bench's
policy -- while a six-rep sitting ran. Six reps read no release at the
arm's median in either sitting, and A, B and the scrambled D behind in
eleven pairs of twelve. The release is one decision a rep at a threshold
that sits at its own value, and the arms' releases differed in one pair
of the six the three-rep sittings held and in seven of the twelve after
them: a median over such reps is the side the coin fell on, and the
table's own column of pairs that differed had said a coin was there. The
rules: where a cell follows a decision made once a rep, the reps are
counted by the decision before the median of any rate is read, and the
cell is read as two shapes with the share of each; a count that reads
level at the median is read in its pairs column too; and a default is not
drafted in the tree while its last pairing runs, since a draft taken back
out is a second edit of the files the fix that follows has to touch. It came again
as the pass a decision falls in. F under 1 GiB read 0.90x to 0.94x for
the stride arm in four sittings of six reps, behind in both orders of two
pairings, and was written down as the arm's. A sitting traced and timed
by kind laid each rep's F to the mix the hot-raw policy sent its raw
head partition back to the form in, which is whichever mix the group's
slack runs out in: a rep whose way back fell in F read it at 565 to 642
ms and one whose way back fell in D or E at 510 to 593, in either arm
and but for one rep at 726 on its commits, and the arm's fell in F in
three reps of six where the default's fell there in one. In the same shape the arm was not behind. The corollary: a
decision every rep makes once still splits a cell in two when the pass
it is made in varies, so the reps are counted by the pass it fell in.

**A wait on the first counter of a turn, and a fix for it that the race
made wide refuted.** The tests step the controller by moving its injected
clock and waiting for the selector's step count to move (`advance_tick`),
and a tick steps the selector first and its policies look after: the
hot-raw policy's, the conversion's. The test of the ordered index's second
drop read, after one such wait, whether the hot-raw policy had asked for a
partition, between that tick's step and its look: one run in ten beside a
build, none of ten alone. It surfaced in a run of the whole suite under
another option's switch and read as that option's for the quarter of an
hour it took to run the test ten times each way. The first fix waited for
the look's own count at the one step whose result was read, and passed
twenty runs of twenty beside the build. Held to the race made wide, sixty
milliseconds of sleep put before the look, it failed every run and the
test without it passed every run: the tick before's look, late, ran after
the test had moved the budget and heated the partition, and asked a tick
early, which the unfixed test read as the ask it was waiting for and the
fixed one waited past, into the release. Every tick's wait has to cover
the tick, not the one whose counts are read. A tick counts its end now
(`ZCounts::tick_ends`), behind its builder's turn and its looks, and the
tests' wait is on that; the test passes with the sleep in. The rules: a
wait covers everything of the turn that the test will read, and
everything of it that reads what the test changes next, so it names the
counter the turn moves last; a fix for a race is held to the race made
wide before it is believed, since a fix that narrows the window passes
every run that the unfixed test passes; and a failure met under a switch
is run without the switch, as many times, before it is the switch's.

**A lever built for the largest peak known, before a count said whose
memory stayed.** At ten million keys under 1 GiB the allocator held 57-68
MB free and resident through the mixes, in arenas of one free chunk each,
and a group at its limit faulted on form files for the want of it. A
merge's thread peaks at 84 MB in the allocator, by a probe that counts by
thread, so the arenas were read as the jobs': the jobs' large arrays were
made mappings of the engine's own (`Options::jobs_mapped`), tested,
mutated and gated, and a traced rep with them read the same free bytes at
the same mixes. The diagnostic queued to tell the allocator's two
behaviours apart had been cancelled to compile the lever sooner. The next
reading took the arenas' sizes, 7.5 to 8.3 MB beside a memo budgeted at
eight, for the readers' memos, and the memos went into slabs
(`Options::zpages_memo_slab`), also tested, mutated and gated; the traced
rep with those read more free bytes, not fewer, and its pairing read a
cell behind in both orders for a policy that had stopped firing. What
answered the question was three counts no reading of sizes replaces, each
an hour or less to make: the allocator on its own (`bench/examples/arena.rs`:
a dead arena keeps all a thread held under the trim threshold and nothing
over it, and `malloc_trim` returns none of it), every allocation by its
thread in the suite's own shape (`alloc-census`: the readers 17 to 24 MB,
the seals 14 to 16, the closes 8), and the arm that already made the
allocator return everything, paired for the ceiling (`supdb-mmapthr`: the
read pass after the load a third faster, two write mixes a twelfth
slower, the rest level). The arenas were several owners', what stood
varied threefold between reps of one arm by which thread took whose arena
next, a smaller reader left more behind than a larger one because the
larger freed a seal's arena past the threshold, and the whole of it was
worth less than either lever had been built to win. The rules: memory
that stays is laid to its owner by a count by thread, in the suite's
shape and the phase it stays in, before a lever is built on the largest
peak anyone has measured; a quantity that varies between reps of one arm
is read over several before one rep attributes it; the ceiling of a
family of levers is priced by the cheapest arm that reaches it before any
member is built, and here that arm was in the tree; a diagnostic queued
to decide between two mechanisms is run before either is built; and a
budget is held to what its structure costs wherever one is found counting
what the structure holds -- the memo's was the second.

**A tenant of the bound that no census counted, charged by how a page
came into the cache.** The stride arm's scrambled D mix under 1 GiB read
0.77x of the default in twelve pairs of twelve, and three levers were
built on readings of what its mixes were short of -- the ordered
indexes' second drop, the jobs' arrays, the readers' memos -- each read
off the process's bytes by owner or the cache's by file. A rep counted
by mix (`SUPDB_MIX_COUNTERS`, `SUPDB_MIX_KINDS`) then read the mix in
two shapes with every counter level, and by that mix both arms' stores
were in one form, the same bytes in the same files. What split them was
the group's own `memory.stat` at each pass's start: cache and rss came
12 MB short of the group's usage in the default and 23 in the arm. The
rest is kernel memory, which a group is charged for and bounded on with
everything else (`memory.kmem.usage_in_bytes`), and `/proc/slabinfo`
named it: 111,000 buffer heads, 104 bytes a block of every page the
cache held because the process had written it. A page read back from
the device carries none, so the store's own form files, dropped as they
are built and asked back, cost nothing there, and a conversion's file,
written whole into the cache and kept, cost a fortieth of its size for
as long as it served: 11 MB at that store, and the group at its limit a
mix early. A conversion's file goes the way the others do under an
option now (`Options::zpages_convert_drops_output`;
`scripts/groupmem.py` is the sampler). The rule: a bound is held against
the group's own usage, and whatever of it the census by owner and the
residency by file leave unexplained is counted, the kernel's accounts
being a file's read away, before a lever is built on the part already
laid to an owner; a page's price under a bound depends on how it came
into the cache, written or read, which no count of residency shows; and
a cell that reads behind with every counter level is short of something
no counter counts, so the next instrument is the accounting a level
down and not another counter of the same kind.

**A default gated green over a roster the gate never opens.** The
stride form's options became the engine's on pairings at every bounded
rung and a green `sh scripts/check.sh`, and the quick run that followed
stopped at its first rung in two seconds: the harness refuses a stride
arm whose store holds no stride file after the ordered load, asked it of
any arm whose options say the form, and `supdb-nozpages`, the arm with
the compact form off, carried the engine's new option and no form file
of any kind. The refusal had been right for as long as only an arm that
set the stride form had it; the controller's arms were exempt by name
and the form-off arm by nobody. The gate's bench group opens the dozen
arms its tests name, and the fifteen controller tests the flip broke
were found by running the tests before the gate; the roster's arms are
opened by the quick group alone, which is not in the gate's default set
because it is a timing run. The refusal holds a forced arm now, and its
test has the form-off arm. The rule: a default is gated on every place
an arm that inherits it is opened, and a check keyed on an option's
value is read again when the option becomes everyone's, since "the arm
that asked for it" and "every arm that pins nothing" stop being the
same set; before a default's quick row, the whole roster is opened at
the smallest rung with one rep, a ten-second run that says what
twenty-five minutes would have said at their start.

**An ask left for later, by the step that was the later.** The sparse
stride form's ask is left by a build made before any reader has come
(`zfile::StrideNo::Deferred`), for a conversion to make once reads come.
The conversion's build took its plan from the rule a landing's build takes,
whether a reader has come, and the rule read the census, which counts the
reads the raw form serves: a store whose form was bought by attach on the
model's word is read through its form files with no census read ever
counted. So the conversion built the block form again and left the ask
again, the look after it found a partition with its ask left and started
the next, and a test's minute made thirteen thousand builds in one run of
three, the other two having had a counted read by chance, until the
selector left the form for the builds it had wasted. A conversion's plan
takes the sparse form whatever the rule says now, and the rule counts a
read a form file has served (`Maint::z_form_read`, `z_sparse_now`). The
rule: the step that answers a deferred question is given the answer and
not the question's gate, since a gate that can say "later" to the step
that is the later is a loop; and a signal that the store has been read is
taken from every path a read can come by, the path the mechanism itself
installs first.

**A price waived, and the evidence the price was.** A partition whose
ask of the sparse stride form a build had left was moved in by the
conversion at no rent wherever the model said the move paid, on the
reading that the build would have paid none. The rent is not only what a
move costs: it is how long the reads have to agree with the model before
a build is spent on its word, and it is what makes a move priced a little
either way on a second's touches come to nothing. With it waived, the
first looks of a read pass at ten million random ids under 1 GiB -- the
block form itself over its fill, the touches a second old and uneven by
the sample -- moved five and six partitions in, and every one was moved
back through the passes after, a build each way, the threaded reads a
twentieth behind and the scrambled D a sixth to a quarter; and under
1.25 GiB a partition the hot-raw policy had sent back to its form a look
before was moved in inside E for a tenth of a millisecond a second, E
0.8x in six pairs of six, which a traced rep's `zconvert` lines beside
its `zhot` lines named in one reading. The move is free now where a
question that needs no evidence says yes, the room's: the forms' bytes
with the move's own fit the fill, sizes alone; past that it pays the
rent. The rule: a price set to nothing takes with it whatever else the
price was doing, so what it guarded against is named first and given a
guard of its own; and a decision excused from evidence is made on a
question that needs none.

**A loss read as the keys', that every form shows and a faster form hid.**
The threaded scans on the store the mixes leave trailed LMDB over random
ids, 0.86x to 0.91x at ten million keys under 1.25 GiB, and were written
down as what ids cost: the block form's decode where numbered keys have
the stride form's arithmetic. Two readings of rows already banked said
otherwise before any profile was cut. The rows from before the stride
form read the same cells 0.78x to 0.95x over numbered keys, in the block
form. And in every row the scans after the mixes are a fifth to a quarter
slower than the scan pass before them whatever serves them, numbered keys
in the stride form among them, 103 million entries a second and then 79,
where twice LMDB's rate leaves the cell won; LMDB's own two passes read
alike. Counted (`SUPDB_SCAN_COUNTERS`, a `scancounters` line a threaded
scan pass), the two passes decode the same groups a scan and differ in
the path: a few unsealed keys in a partition take every scan of it off
the bulk walk and through the block loop and the form file's cursor, and
a random id's insert falls in every partition. A sparse form built to win
those cells, at a bound where it fits, trailed LMDB in them by the same
fifth. The rule: before a cell lost over a new input is put on the input,
the same cell is read in the rows before it under the old input and the
form the new one is served by, and beside the pass before it in its own
row; a step between two passes that every form shows has one cause above
the forms; and a cell a form wins twice over is read for what it loses
with the lead gone, since the next input is the one that takes the lead
away.

**A form priced for a week before its walk was profiled once.** The
sparse stride form was priced by its bytes, by where it fits and by three
rules for asking it, at two bounds where it does not fit; shown at last
where it does, its scans read level with LMDB's where the whole-run
form's read at twice, and that was not asked about. The first profile of
its scan pass put a quarter of the threads in rendering each record's key
from its number, a byte a digit, and a ninth more in copying the rendered
key over the cursor's: sixteen stores of a byte and then a load of
sixteen, the value built a field at a time and handed back whole, again.
A whole run's walk adds one to its key in place and a sparse form's
cannot, so its step had been written as a render and a copy and never
looked at. The first fix made the digits a word at a time and copied the
field's out of a buffer of sixteen at a run-time length: for a field of
fifteen digits, two stores of eight bytes and a load of fifteen across
them, the same stall at a third its size. What named it was the samples
inside the library's copy split by their offset, each size's path having
instructions of its own, read against the copy's disassembly: the
sixteen-byte path's samples gone and the eight-to-fifteen's arrived. The
field is stored from two registers now (`zpages::put_digits`,
`Options::zpages_stride_step_direct`). The rule: a form is profiled where
it serves before it is priced anywhere, the profile being cheaper than
the first pairing; a copy of a run-time length out of a buffer just
written is a load nothing forwards whatever wrote the buffer, so what is
made in registers is stored from them; and samples in the library's copy
are split by offset into the sizes copied before any caller is suspected.

**Two causes named for one stall off a flat profile, and the probe that
told them apart run second.** The values' copies out of a stride file's
mapping took two fifths more of the threads in the scans after the mixes
than in the scan pass, the same bytes in the same order. The first
reading was the pages: the mixes' merges rewrite every partition, the
files those scans read were written late, and this file's own note on
memory placement made a slower page plausible. The second was the core's
prefetch losing the values' stream between the runs a block-by-block walk
reads a partition in, and a lever was built on it: an ask for the head of
each run's lines, an option with its arms, tests and fuzz draw. Counted,
it fired at every run, moved the copies' samples by a fiftieth and cost
the walk more; its pairing read the scans within a tenth either way
beside cells it cannot reach leaning a sixth; it was taken out. The first
reading was then answered by ten lines of harness: the store folded
before those scans (`SUPDB_SCAN_MIXED_FOLD`), the same late files with
nothing overlaid, read at the scan pass's rate. So it is the path, and
which part of the path the copies wait on is still not known. The rule:
where a stall has two candidate causes, the probe that holds one of them
fixed is run before a lever for either is built, and it is usually the
cheaper of the two to write; a lever whose count says it fired and whose
target did not move is taken out, not kept as an arm of something that
does nothing; and what is left unexplained is written down as unexplained.

**A fix given to the walk a drained store takes, and not to the walk a
written store takes.** The bulk walk was made to step the fence to its
next entry (`Options::zpages_bulk_steps`) on a profile of the threaded
scan pass that put a sixth of the walk in a binary search of the fence's
ranks at every page. The cursor, which walks every partition with an
unsealed key over it -- every scan after a write mix -- kept the search,
and was found by the same symbol in a profile of the pass after the
mixes, a ninth of the cursor's own time (`Options::zpages_cursor_steps`).
Its pairing then resolved nothing: counted, it stepped ten million times
a rep, and its cells read three to seven in a hundred lower, as did the
scan pass and the load, which it cannot reach, the arm's hot-raw policy
having released less in every pair. The rule is this file's own, that a
path given the fix for a shape is the occasion to check every path that
shares it, with the walks' corollary: the scan pass on a drained store is
the one shape the mixes never show, so a fix to the bulk walk is looked
for in the cursor the same day. And a lever worth three in a hundred of
one pass stands on its count and its profile, not on a pairing of three
reps under a controller that makes a decision a rep, where the decision
splits the cell before the lever moves it.

**A ceiling priced beside a form that fit, and not priced again when the
form stopped fitting.** The memory the readers' arenas keep after their
threads end was priced once, over numbered keys at ten million under
1 GiB: the allocator's threshold as the ceiling, worth little, and the
slab that gives a handle's memo back (`Options::zpages_memo_slab`), which
read no fewer free bytes; both were left off. Over random ids at the same
bound the row's one cell lost to LMDB, the scrambled E mix at 0.68x and
0.76x, took 0.40 major faults an operation, and the same three steps that
note gives found it in an afternoon, each an arm already in the tree: the
census by owner, 88 MB freed and resident through four mixes and 177 by
the last, beside a block cache of 25; the ceiling by the allocator's
threshold, the scrambled mixes 1.05x to 1.45x; and the owners' arms one
at a time, the slab alone reading those mixes 1.19x to 1.85x and the
scans after the mixes 1.30x to 1.87x in both orders. Random ids are read
through the block form in every partition, so every reader's memo is
full, and their block form is 856 MiB where numbered keys' is under 700:
the form misses its fit there by about what the arenas hold. The rule: a
lever written off as worth little is written off at the fit it was priced
at, and is priced again whenever an input moves the store across a fit
-- a shape of key, a value's size, a bound -- since memory nothing is
short of at one size is the whole shortfall at the next; and a cell lost
by faults under a bound is read against the process's bytes by owner
before any form is blamed.

**A path the suite's keys took for free, and every other key paid a
record for.** A landing near reads walks each of its pieces against the
block boundaries of the partition under it, for the tables that will
read the bounds, and takes each boundary's key from the ordered index's
top level where a head is the whole of a key (`OrdIndex::whole_key_at`).
That is every key the suite numbers, sixteen digits behind a common
prefix. An id runs on past its head, so over ids each boundary was a
record: 156,000 a landing at ten million keys, a cold line each in core,
and under a bound, where the block form serves and the raw records are
out of the cache, a major fault each on the one thread every landing
goes through. The lag sweep over ids under 1 GiB was the first run to
meet it, and it lost no cell there, it was killed: the landing of the
ten-percent burst's seal took 76,000 faults in five seconds and had not
published, three tables stood frozen behind it with their snapshot
copies, and the full rewrite's burst took the process from 450 MB to the
limit in under three seconds. Under 1.25 GiB the same sweep survives
with its writer waiting five seconds for landings. The map of where ids
trail LMDB had been drawn over the row's passes and left the sweep out,
and a pairing queued for another option was what ran it. Three readings
found it in an hour, in this order: the counters of the last point that
printed, no piece standing and a frozen table with its copy in the
census where numbered keys read twenty-four pieces, which said a landing
was missing and not that memory had an owner to find; a sample of every
thread's state twenty times a second (`/proc/<pid>/task/*/stat` and
`stack`), which had the seal's thread done inside the burst and the
landing's thread in a page fault from then to the kill; and every major
fault by its instruction (`perf record -a -e major-faults -c 1 -d`),
which named `Maint::rank_before_publish`. And the bounds were for
nobody: the sweep's scans had the writer give its block forms up for the
merge path, and the landing asked whether the block cache was on by
option, not whether a table stood to read them. A walk places a boundary
by its head now where the head is not its key, between the two byte
strings the head brackets the key by, and reads the record only where a
key of the walked source falls inside (`Options::bounds_by_heads`,
`OrdIndex::head_bracket`), the same bounds to the entry; and a landing
leaves its bounds while the writer keeps no forms and no handle is live
(`Options::land_bounds_read`). The rules: a fast path keyed on a
property of the suite's keys -- a head that is the whole key, one
length, a run of numbers -- is a slow path the suite has never run, so
such sites are listed (`whole_key_at`, `uniform`) and a second key shape
is run over every pass, the sweep with the row, before the map of it is
called drawn; a process killed under a bound is read from the last
counters it printed and then from its threads' states before its memory
is summed by owner, since what piles up behind a stalled thread has an
owner's name on it and is not the cause; derived work is gated on its
reader, never on the option that makes a reader possible; and a stall
on the writer's path is somebody's pace, so its removal is read on the
phase after it -- the pass after the full rewrite read a seventh slower
under 1.25 GiB with the stall gone, its burst a quarter faster, the
merges having had five seconds less to drain. The same walk was pacing
a second thread. The snapshot's bounds take the same boundary keys, on
the upkeep thread, and over ids each was a form file's group decoded:
under the default that thread made six or seven passes over the whole
of ycsb-A at ten million keys under 1 GiB, fifty milliseconds each and
nearly all of it that walk. With the heads placing the boundaries it
kept up and made one pass a commit, 736 of them, filing every commit's
writes alone as `forms_settle_every` says it should and building twice
the forms the batched passes had (18,774 against 8,437-9,508), and A's
commits ran a quarter slower beside it (`SUPDB_MIX_KINDS`: 299-319 ms
against 228-252), A at 0.84-0.85x of the default in four pairs of four
and still twice LMDB's. Numbered keys, whose bounds were always free,
have always run that regime; the slow walk had batched ids out of it by
accident. The corollary: a thread's pass schedule is a function of how
long its passes take, so a change to a pass's cost is read on the
passes counted and the work a pass does, and a regime that holds only
because something was slow is measured with the slowness gone before it
is called the regime's cost.

## Standing limitations

These are the engine's, as opposed to refuted predictions. Each is a curve
in the suite's figures, at `full` scale where it says so:

- The lag pass after a full rewrite under a bound pays for the burst's
  merge debt twice: as a merge writing beside it, in the reps where the
  chain has not drained -- the pass's slow shape at ten million keys under
  1.25 GiB, which a third merge in the write-only stretch retires for ids
  (`Options::merge_parallel_quiet`) -- and as the working set the burst
  leaves, which under 1 GiB is about 1.2 GB against the bound: form files
  that do not fit, the pieces, and the memtables' unsealed keys, whose sort
  is the pass's first scan. There the pass over ids reads 0.6x to 0.9x of
  LMDB and the pass over numbered keys 1.1x to 1.3x (`docs/engine.md`,
  paired at the head), a third of it user time, a quarter the kernel's
  fault handling and the rest device waits, and the rewarm trades its
  faults for device reads one for one (`docs/engine.md`, keys that are no
  run of numbers). The levers left are the bytes of that working set -- the form,
  the pieces standing, the unsealed keys -- and the merge path's user time
  over them; the idle I/O class for the merges and their inputs asked ahead
  were priced on it and refuted.
- The frozen tables are held to a count (`FROZEN_CAP`, three) and not to
  bytes. A table freezes at the seal's threshold, 109 MB of memtable at
  ten million keys, and near reads its seal makes a snapshot copy of it,
  123 MB more: three behind a landing that does not come, with the live
  table past them, are a gigabyte, and under a bound of that the group
  kills the process before the writer's wait at a full list holds it.
  The one landing known to stall so was the bounds walk over keys whose
  heads are not whole keys, which the heads place now
  (`Options::bounds_by_heads`, `land_bounds_read`). The lever for any
  other is the writer's wait drawn from what the bound can spare, and is
  not built.
- Scans on a store with unsealed keys spread through it pay for the path.
  A few unsealed keys in a partition take every scan of it off the bulk
  walk and through the block loop and the form file's cursor, a fifth to
  a quarter slower than the same scans on the store folded
  (`SUPDB_SCAN_MIXED_FOLD`), whatever form serves. Over the suite's
  numbered keys the inserts land above every key and the stride form's
  lead covers the rest; over keys that fall anywhere it is the cell lost
  to LMDB wherever LMDB's file is in memory -- the threaded scans after
  the mixes at ten million random ids, 0.86x to 0.91x under 1.25 GiB in
  the block form and 0.89x to 0.92x under 1.5 GiB in the sparse stride
  form (`docs/engine.md`, keys that are no run of numbers). The levers:
  the block loop's three loads a block for the published form, each
  behind the last; what the values' copies wait on in the cursor's walk,
  two fifths more than in the bulk walk over the same bytes, which is not
  known; and folding the keys in once a store is only read, which
  rewrites every partition where the inserts fell in every one.
- The memos of decoded groups keep their memory after their readers
  end, in arenas the allocator does not return, and where the block form
  misses its fit by that much the store faults for it: ten million random
  ids under 1 GiB, where the scrambled E mix read 0.68x and 0.76x of LMDB
  while the bounds walk still read a record a boundary, and reads 1.06x
  and 1.22x with the heads placing them (`Options::bounds_by_heads`). The
  memos in slabs (`Options::zpages_memo_slab`) return it, the scrambled
  mixes 1.19x to 1.85x there before the heads, and are off: over numbered
  keys at the same bound the slab has nothing to return, and the hot-raw
  policy's one release, asked into memory that stands unused at a look,
  is never asked under it. What is owed before the default: that
  policy's room drawn from something the slab does not move, the slab
  priced again beside the heads, six reps a sitting over numbered keys
  at both ten-million bounds, the quick row and the gate.
- Keys the stride form does not hold are served by the block form
  wherever the sparse stride form does not fit, a hundred and eight bytes
  a key against eighty-five, and the block form's scans are a decode a
  group against a B-tree in memory: the three-thread scan pass at ten
  million random ids under 1.25 GiB is 0.94x of LMDB's, and was for
  numbered keys before the stride form took them.
- The controller reads its budget where a load ends, the process at its
  smallest. A store whose raw form fits that budget by less than the
  process then grows -- ten million random ids under 1.5 GiB, by a
  hundredth -- is served raw until its reads' faults have paid for the
  form, four cells of a divided row, and a form built ahead for it is
  dropped as it is written and read back under the first reads. The lever
  is a budget that counts what the engine's own structures are set to
  hold, and is not built.
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
