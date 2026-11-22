# T22 mutants

The change is one line of scripts/check.sh's cross-arm case (`RUST_BACKTRACE=0 \` before the
`cargo test` of the emulated run) and its comment; nothing compiled changed. Each mutant is
applied by mutants.sh (a python exact-string replace of check.sh, asserting one match), the
group run by group.sh with RUST_BACKTRACE=1 in the caller's environment (as this shell has
it) until its first red by exit status (at most 8, then 16), and check.sh put back from
keep/check.sh and checked by its sha256 (40702f87...) after each: results in
mutants-results-1.log and mutants-results.log, runs in runs/mut-*/ and runs/mut2-*/.

| mutant | killed by |
|---|---|
| the brief's: the case's `RUST_BACKTRACE=0` line dropped (the run takes the caller's RUST_BACKTRACE=1) | runs/mut-drop-line run 1: exit 101, "QEMU internal SIGSEGV {code=MAPERR, addr=0x20}", no test result line |
| `RUST_BACKTRACE=1` in place of 0 | runs/mut-value-one run 1: exit 101, the same SIGSEGV |
| `RUST_LIB_BACKTRACE=0` in place of `RUST_BACKTRACE=0` (the wrong variable: std's panic hook reads RUST_BACKTRACE) | survived 8 runs in the first batch (runs/mut-wrong-var, 8 clean); killed at run 1 of a second batch (runs/mut2-wrong-var: exit 101, the SIGSEGV). Interleaved straight runs say it is no fix at all: runs/alt, crypt:: 20 rounds of four modes in turn: RUST_BACKTRACE=1 5/20 dead, RUST_BACKTRACE=1 RUST_LIB_BACKTRACE=0 7/20, RUST_BACKTRACE=0 0/20, unset 0/20; and runs/strace-libbt0: the second open of /proc/self/maps is there under it, three of three. |

What the first batch of the third taught: the deaths come in bursts in time (base runs 2-9 one
after another, then 10-12 clean; this mutant 8 clean then dead at once), so a rate read from
consecutive runs of one configuration is not independent run to run. The after-rate is
therefore also taken interleaved with the red configuration (runs/altwhole), not only in a
row (runs/after-bt0).

No survivor stands.

## Repair round 1: the reviewers' mutants, and the two checks they asked for

Runner: mutants3.sh (exact-string edits, one match each asserted, the group run by group.sh with
RUST_BACKTRACE=1 in the caller's environment until its first red, at most 16; the file put back
from keep/ and checked by sha256 after each): results mutants3-old.log and mutants3.log, runs in
runs/mut3-*/. Source-gate mutants: srcmut.sh -> srcmut.log, runs/srcmut/*.log (`cargo test
--profile checked --test source -- maps`).

| mutant | killed by |
|---|---|
| `RUST_BACKTRACE=0` moved after `--lib` (a test-name filter), against the round's starting check.sh | SURVIVED (runs/mut3-after-lib: exit 0, "0 passed; ... 530 filtered out", "all checks passed"). A check by exit status alone cannot see it. The case now holds the run's `test result:` line to `ok. N passed` with N >= 1 and `0 filtered out` |
| the same, against the new case | runs/mut3-after-lib-new run 1: exit 1, "cross-arm: the run did not run every test it compiled" |
| `RUST_BACKTRACE=` (empty): std reads any value but `0` (and `full`) as short | runs/mut3-empty: 2 clean, run 3 exit 1 with the QEMU SIGSEGV. Killed, so empty is not a safe value |
| `RUST_BACKTRACE=short` | runs/mut3-short: run 1 clean, run 2 exit 1, the SIGSEGV |
| the `RUST_BACKTRACE=0 \` line taken off cross-arm and put on the `test` case | runs/mut3-moved-to-test run 1: exit 1, the SIGSEGV |
| a lib unit test in src/crypt.rs (beside the crypt tests) that calls `Backtrace::force_capture()` and formats it, with the fix in place | by the group: runs/mut3-force-capture, 11 clean then run 12 exit 1 with the SIGSEGV -- a flaky red, which is the hole the reviewer named. Now killed deterministically on the native target by the new source gate: runs/srcmut/force-capture.log, `the_library_reads_no_maps_and_makes_no_backtrace` FAILED (exit 101) |
| source gate: the `/proc/self/maps` check made `if false` | srcmut no-maps: `a_maps_reader_or_a_backtrace_in_the_library_fails` FAILED |
| source gate: the `RUST_LIB_BACKTRACE` half dropped | srcmut no-lib: the fixture test FAILED |
| source gate: the `RUST_BACKTRACE` half made `false` | srcmut no-rust-bt: the fixture test FAILED |
| source gate: the word `Backtrace` misspelt | srcmut no-word: the fixture test FAILED |
| source gate: comments read as code | srcmut comments-read: the fixture test FAILED (its comment case) |
| source gate: unit-test lines skipped (`if l.test { continue }`) | srcmut tests-skipped: the fixture test FAILED (the fixtures are in a `#[cfg(test)]` module, the code the emulated run runs) |

Two checks taken out before their mutants, as unreachable: the gate's `.rs` filter (src/ holds no
other file) and its lower-case `backtrace` word (a capture names `Backtrace`, an alias's `use`
included, so the lower-case word never decided a verdict the upper-case one did not).

No survivor stands.
