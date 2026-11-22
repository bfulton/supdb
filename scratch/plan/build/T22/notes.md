# T22 cross-arm -- notes

Step: repair round 1 done. Finished copies: keep/check.sh, keep/CLAUDE.md, keep/source.rs
(keep/sums.txt; the tree matches them); keep/crypt.rs is src/crypt.rs untouched, which the
force-capture mutant was put back from. Nothing runs; build/tmp/T22 removed.
No compiled engine code changed in either round (scripts/check.sh, tests/source.rs, CLAUDE.md):
no callgrind rows, no stride group, no count.

Base bc9cdd84; HEAD a3b634df adds only scratch/ (checked: `git diff --stat bc9cdd84 HEAD`
names only scratch/). This change has no plan section; brief.txt is it.

Layout of this directory:
- runs.sh TAG N BIN [qemu args] -- [test args]: N runs of the binary straight under
  `qemu-aarch64-static -L /usr/aarch64-linux-gnu`, no cargo; runs/TAG/run<i>.log and a
  summary.log line a run (exit, count of "QEMU internal SIGSEGV", the `test result:` line),
  ended by "EXIT done". TMPDIR a run of its own under build/tmp/T22, removed after the run.
- supdb-base.bin: the group's test binary, built once with the case's three variables
  (`cargo test --no-run`, profile checked, target aarch64), sha256 f26d8b77...; the same hash
  as the closing run's (supdb-f73fa21bb2eb2ba6). list-base.txt: its --list, 530 tests.
- repro/: the reproducer with no engine in it, its controls, a native run and gdb's try.

## 1. The rate before (binary direct, parallel as the group runs it): runs/base/, before.log

| runs | dead (QEMU internal SIGSEGV, exit 139) | clean (530 passed) |
|---|---|---|
| 12 | 8 (runs 2-9, each within a second, 16-17 tests finished) | 4 (1, 10, 11, 12) |

This shell has RUST_BACKTRACE=1 in its environment (so did the closing run's, same container),
and the rate is much higher straight than the closing run's 2 in 5 through cargo, where the
deaths came ~80 s in. Both are the same mechanism (below): a panic with backtraces on.

## 2. What is in flight

`crypt::` alone, 30 runs: 8 dead (runs/crypt30). The dead runs' missing names always include
the first few crypt tests still running; one of them, `crypt::tests::a_cipher_that_panics_leaves_its_chunk_to_the_next_reader`,
panics on purpose inside `catch_unwind`.

qemu `-strace -D` (runs/cryptstrace, 20 runs, 11 dead -- the trace does not suppress it). qemu
8.2.2's -strace prints the process id, not the thread's, on every line, so threads are told
apart by interleaving (an entry with its result on a later line). Every dead trace ends inside
one of:
- `mmap(NULL,1588,...ANONYMOUS)` / `munmap(<that address>,1588)` in a tight loop (the crypt
  tests' kept mapping, made and dropped per open; every munmap is of the address the mmap just
  returned, the length the same: the program's own range), interleaved with
- `openat(AT_FDCWD,"/proc/self/maps",O_RDONLY|O_CLOEXEC)` from another thread, followed (when
  it lives) by statx(fd), lseek(SEEK_CUR), read(5369), read(32)=0: Rust std's
  `fs::read_to_string`, which is backtrace-rs's gimli symbolizer reading the process's
  mappings (`parse_running_mmaps`) to symbolize the panic's backtrace. Nothing in src/ opens
  /proc/self/maps (grep: no `maps` in src/).
  e.g. trace1.log's last lines: `mmap(NULL,1588,...)` with `openat(...,"/proc/self/maps",...)`
  interleaved, then `munmap(0x00004000217e1000,1588)` with no result: the munmap of the
  address that mmap returned, beside the open of /proc/self/maps.
  trace5.log (a clean run) shows the same window living: line 400
  `openat("/proc/self/maps")` beside `madvise(0x000040001c025000,1978368,MADV_DONTNEED)`
  (glibc's shrink_heap of a thread arena, after its read of /proc/sys/vm/overcommit_memory)
  and the 1588 mmap/munmap loop.

Counts: a crypt:: run with RUST_BACKTRACE=1 opens /proc/self/maps twice (glibc's
pthread_getattr_np for the main thread at start, single-threaded, and the panic's
symbolization); with RUST_BACKTRACE unset, once (runs/strace-nobt).

Rates that say which part matters (30 runs each, `crypt::`):
| set | RUST_BACKTRACE | dead |
|---|---|---|
| crypt:: | 1 | 8/30 (runs/crypt30) |
| crypt:: | unset | 0/30 (runs/crypt30nobt) |
| crypt:: --skip a_cipher_that_panics | 1 | 0/30 (runs/crypt30skip) |

## 3. Whose bug: QEMU's (cause i)

The guest's calls are legitimate: each munmap is of a range the same thread mapped the line
before, the madvise is glibc's on its own heap, and the open is of /proc/self/maps, which any
program may read at any time. qemu-user emulates /proc/self/maps itself (it fakes the file
from its own record of guest mappings) and the read races munmap/mmap from other threads.
A reproducer with no engine in it (repro/maps_race.c: three threads mmap 1588 bytes
anonymous, touch, munmap; one thread opens and reads /proc/self/maps 2000 times; built
`aarch64-linux-gnu-gcc -O2 -pthread`, run `qemu-aarch64-static -L /usr/aarch64-linux-gnu
./maps_race 3`): 10 of 10 dead with the same "QEMU internal SIGSEGV {code=MAPERR, addr=0x20}"
(repro/summary.log). Controls (repro/control.log): the maps reader with no mapper thread 10/10
clean; the same mappers with the reader on /proc/self/status (which qemu passes through)
10/10 clean. Native x86 build of the same source 5/5 clean (repro/native.log).
Why not (ii): no call in any dead trace is on a range the program had not mapped or had
already unmapped (each munmap pairs with the mmap before it at the same address and length;
the madvise is inside glibc's own arena), and the reproducer, whose every call is on its own
range, dies the same way; with the one reader of the emulated file removed (backtrace off)
the same tests in the same binary run clean.
QEMU's stack (repro/gdb.log): the static-pie binary is stripped; gdb gives addresses only
(fault in thread 6, a TCG vCPU thread, 9 frames, no names). The debug symbols (Ubuntu ddebs)
are not installed; not read.

## 4. The change

Not (a)/(b)/(c) of the brief: departure, with its reason. The defect is reached by the test
harness's environment, not by any test's own code: with RUST_BACKTRACE set (as in this shell,
and -- inferred, not verified here -- as dtolnay/rust-toolchain exports RUST_BACKTRACE=short
for the job's later steps), *every* panic the run makes, on purpose (catch_unwind and
should_panic tests) or a real failure, symbolizes its backtrace by reading /proc/self/maps
beside threads that map and unmap. Compiling the panicking tests out would lose paths (the
panic-safety of the cipher's chunk) and still leave a real failure to be turned into a
QEMU crash; one test at a time would cost 3.5x and still crash at a failing test's backtrace.
The group's case sets RUST_BACKTRACE=0 for its run, so the emulator never serves
/proc/self/maps after the first thread starts. No test is compiled out, skipped or loosened;
the emulated run's --list is the base's (nothing changed in src/).
What it loses: a failing test under emulation prints no backtrace (the native ARM leg and
the test group print theirs).


The diff: scripts/check.sh (the cross-arm case's `RUST_BACKTRACE=0 \` line and its comment),
CLAUDE.md (one paragraph under "Running the checks", the rule this taught). Finished copies:
keep/check.sh (sha256 40702f87...), keep/CLAUDE.md (keep/sums.txt).

## 5. The rate after (A)

The binary is unchanged (no src/ change; cargo runs the same supdb-f73fa21bb2eb2ba6), so
the after-rate is the same binary under the case's new environment.
- runs/after-bt0 (after-direct.log): 22 runs in a row with RUST_BACKTRACE=0, 22 clean, each
  "530 passed".
- runs/altwhole: 20 rounds, RUST_BACKTRACE=1 then =0 in turn, so both are read in the same
  windows (the deaths come in bursts in time, mutants.md): =1 7/20 dead (exit 139, the
  SIGSEGV), =0 0/20, each "530 passed".
- runs/group3: `sh scripts/check.sh cross-arm` three times with RUST_BACKTRACE=1 in the
  caller's environment: exit 0 each, "530 passed", "all checks passed: cross-arm".
Before, all runs with RUST_BACKTRACE=1, straight: 8/12 (runs/base) + 7/20 (altwhole) = 15/32.
After: 42 straight runs and 3 group runs clean. The chance of 42 clean at the measured
15/32 is (17/32)^42 = 3e-12; at the closing run's 2 in 5 through cargo it is 0.6^42 = 5e-10;
even at a rate of one in five, under every count here, 0.8^42 = 9e-5. Under one in a thousand.
The bursts make runs of one configuration not independent; the interleaved 20 alone, at the
7/20 measured beside them, give 0.65^20 = 2e-4.

## 6. Exactly what the emulated run lost (C)

Nothing. The emulated binary is the base's (same hash path, no src/ change), --list 530
names (list-base.txt), every group and direct run "530 passed". The native test group
(after.log, `sh scripts/check.sh test`, EXIT 0, "all checks passed: test"): the library's unit
tests "535 passed" (530 + the five `not(supdb_emulated)` tests there were before), 27
"test result: ok" lines, none failed. No test is compiled out by this change.

## 7. Callgrind rows

None: no compiled code changed (the diff is a shell script's case and CLAUDE.md). Not cause
(ii), so no stride group and no count.

## 8. D: groups by exit status
- test: after.log EXIT 0.
- lint: lint.log (EXIT line).
- lists with SUPDB_BASE=bc9cdd84: lists.log (EXIT line).

## Departures from the brief
- Not (a), (b) or (c): the trigger is the run's environment (a backtrace symbolized reads the
  emulated /proc/self/maps), not a test's code, so the fix is the variable in the group's case,
  no test lost. Compiling out the panicking tests would lose cover and leave any real test
  failure to become a QEMU crash under RUST_BACKTRACE; it is not done.
- The brief's (b) said the serial filter is the case's mutant; here the mutant is the line
  dropped (mutants.md), killed.
- QEMU's own stack not read: the static-pie binary is stripped (repro/gdb.log, addresses only).

## What it holds and what it cannot see
- Holds: the group fixes RUST_BACKTRACE=0 for its emulated run whatever the caller exports,
  so no panic symbolizes a backtrace under the emulator.
- Cannot see: any other reader of /proc/self/maps beside threads that map (none in src/; the
  one at startup is glibc's for the main thread, before any thread). A future test that reads
  /proc/self/maps itself, or a dependency that does, would bring the crash back under the
  emulator; the comment in check.sh names the mechanism. A failing test under emulation
  prints no backtrace now (the native legs print theirs).
- Not verified: that GitHub's runner exports RUST_BACKTRACE (dtolnay/rust-toolchain is
  remembered to export RUST_BACKTRACE=short; not checked here). With it unset the group
  never crashed here either (0/30 crypt:: unset, 0/20 interleaved), so either way the case
  now gives a verdict independent of it. Whether QEMU 8.2's /proc/self/maps code has an
  upstream fix: not looked up (nothing is fetched).

## Repair round 1

The reviewers' extra mutants (mutants.md, the round's table) found two holes, each now closed:
- `RUST_BACKTRACE=0` moved after `--lib` is a test-name filter: the group ran 0 tests and exited
  0 (runs/mut3-after-lib, "530 filtered out", "all checks passed"). The case now captures the
  run's output (as the fscrypt case does), prints it, and holds it to a line
  `test result: ok. N passed (N >= 1); 0 failed; ...; 0 filtered out`; the same mutant is red
  against it (runs/mut3-after-lib-new, exit 1, "cross-arm: the run did not run every test it
  compiled"). What it cannot see: a test compiled out by a cfg, or marked #[ignore], is not
  "filtered" -- those are the tree's choices and the native groups still run them.
- A lib unit test that formats `Backtrace::force_capture()` beside the crypt tests killed the
  group one run in twelve with the fix in place (runs/mut3-force-capture): the fix holds only
  while the library's own code reads no /proc/self/maps and makes no backtrace. New source gate
  in tests/source.rs: `maps_readers` reads every line of src/ (unit tests included, comments
  not) and refuses a `/proc/self/maps` literal, the word `Backtrace`, or a `RUST_BACKTRACE` /
  `RUST_LIB_BACKTRACE` literal; `the_library_reads_no_maps_and_makes_no_backtrace` holds the
  tree (green, 0 hits) and `a_maps_reader_or_a_backtrace_in_the_library_fails` holds each
  spelling red and a comment green. Red before: the force-capture mutant applied by script fails
  it natively, deterministically (runs/srcmut/force-capture.log, exit 101); green after
  (after-r1.log, source: 81 passed). Cannot see: a path built at run time, a dependency that
  reads the file (none of the lib's do today: not verified beyond the lib's own source).
- `RUST_BACKTRACE=` (empty), `=short`, and the line moved to the `test` case: each killed by the
  group, exit 1 with the QEMU SIGSEGV (runs/mut3-empty run 3, mut3-short run 2,
  mut3-moved-to-test run 1). So only `0` is a safe value (std: `0` off, `full` full, anything
  else, empty included, short); the case's line is the one place it is set.

Round's gates, by exit status:
- `sh scripts/check.sh cross-arm` three times with RUST_BACKTRACE=1 in the caller's environment
  (runs/group3r): exit 0 each, "530 passed; ... 0 filtered out", "all checks passed: cross-arm".
  The emulated binary is the one the after-rate was taken on (no src/ change), so the round adds
  3 clean group runs to the 42 straight and 3 group runs of section 5; the arithmetic there
  stands (0.65^20 = 2e-4 on the interleaved twenty alone, under one in a thousand).
- test (after-r1.log): EXIT 0, lib "535 passed", 27 "test result: ok", source 81 passed.
- lint (lint-r1.log): EXIT 0 (clippy, fmt, door, seal). fmt.sh --check clean after one reflow
  of the new test.
- lists with SUPDB_BASE=bc9cdd84 (lists-r1.log): EXIT 0.
- CLAUDE.md's paragraph extended by the round's rule (a fix resting on an input is held where
  the input can be undone; a verdict by exit status is held to the count of what ran), written
  after the test group started; no test reads CLAUDE.md.
