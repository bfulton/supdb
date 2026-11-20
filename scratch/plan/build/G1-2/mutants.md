# G1-2 door: mutants

Round 1: mutants.py -> mutants.results, mutants.log. Its verdict column reads
"KILLED" as "the command exited non-zero", right for the test mutants (D-Q,
killer a cargo test) and for F; for the gate-script mutants run on a red
input (A, B, C, G, M, R) it is inverted, so those were read by hand below and
run again in round 2 with the semantics written into the harness
(mutants2.py -> mutants2.results, mutants2.log; controls carry no mutant).

| mutant | killed by | note |
|---|---|---|
| A run.sh's config guard gone, config missing (plan) | not killed: red.sh fails anyway | the guard was a check nothing reached: taken out. Round 2 A0 (control): config missing -> run.sh fails (red.sh finds no diagnostic) |
| B both guards gone, config missing | not killed: red.sh's diagnostics check | red.sh's guard taken out too |
| C red status swallowed, `\| tail -1` (plan) | round 2: run.sh on the reduced config (no madvise, no MmapOptions::new) exits 0 under the mutant, non-zero without (C0) | KILLED |
| D ratchet counts only clippy::disallowed_ (plan) | a_new_allow_of_the_door_fails_in_each_spelling | KILLED |
| E literal rule does not skip test code (plan) | a_proc_path_outside_the_door_fails_and_a_tests_does_not | KILLED |
| F red copy not removed, trap gone (plan) | run.sh's empty-directory check (round 1 and 2) | KILLED |
| G red.sh's exit-status check gone | not killed: an `error: use of a disallowed` line is a non-zero exit under -D warnings | taken out; the comment says why |
| H thread paths never counted | the_door_holds_what_clippy_cannot | KILLED |
| I inner attributes not read | a_new_allow_of_the_door_fails_in_each_spelling (module allow) | KILLED |
| J foreign block's ABI string not skipped | a_foreign_block_outside_wasmapi_fails | KILLED |
| K `use std::thread::{self, ..}` read as a function | a_thread_call_added_fails_and_a_use_of_a_thread_fn_fails | KILLED |
| L ordindex check gone | the_ordered_index_opens_through_the_door | KILLED |
| M red.sh requires only the first line | round 2: red.sh on the reduced config exits 0 under the mutant | KILLED |
| N a count of zero accepted | the_door_table_is_read_as_it_says | KILLED |
| O a shrink passes | an_allow_moved_or_taken_away_fails_until_the_list_follows | KILLED |
| P an outer allow keyed by its own line's scope | the_door_holds_what_clippy_cannot | KILLED |
| Q an allow inside cfg_attr not read | a_new_allow_of_the_door_fails_in_each_spelling | KILLED |
| R0 (round 1, control) an entry naming nothing | FAILED as a control: run.sh passed | found the stale-cache hole (below); fixed by the clean; round 2 R0 control OK |
| R the names-nothing check gone | round 2: run.sh on a config with `libc::no_such_fn_g12` exits 0 under the mutant | KILLED |
| S the clean gone | round 2: run without the config, then with a bogus entry: the second passes under the mutant | KILLED |
| U the door run's exit status ignored | round 2: config with `std::fs::File::sync_data` added (19 sites) -> run.sh exits 0 under the mutant | KILLED |

Not a survivor: none left. Taken out as unreachable: run.sh's and red.sh's
config guards, red.sh's exit-status check.

The stale cache (found by R0): clippy puts its config in the build's
dependency file only when it found one, so a door run made with the config
absent left a cached result that answered for the config after it came back,
edits included ("Finished in 0.01s" over a config naming a function that does
not exist). run.sh cleans the library's door build before every run (about 7 s).


## Round 1 (repair)

mutants_r1.py -> mutants_r1.results. Kinds: test (the real tree edited, the source binary
run, KILLED = a named test FAILED; a mutant that does not compile is INVALID, which the
first pass's X1/X2 were, prepended above `//!`, and were rerun appended), script (a copy of
the repo, `sh scripts/door/run.sh`, KILLED = non-zero exit), before (the copy carrying round
0's scripts and config). Final pass at 10:25 after the last script change.

| mutant | killed by | note |
|---|---|---|
| X1 `use std::thread as t;` + `t::spawn` | the_door_holds_what_clippy_cannot ("renames std::thread") | was a survivor before |
| X2 glob `use std::thread::*` + spawn, sleep | the_door_holds ("globs std::thread") | |
| X3 pager.rs `pub(crate) use std::thread as pth` + call in zfile | the_door_holds (pager.rs: renames) | |
| X5 allow written by a macro (`clippy::$l`) | the_door_holds (allow ... the door grew) | |
| X11 block `{ use std::thread as t; t::spawn }` | the_door_holds | |
| X12 `use std::thread::Builder` + `Builder::new().spawn` | the_door_holds ("takes Builder out") | |
| X9a type moved into the methods table | each_table_of_the_doors_config_names_its_kind | clippy itself is silent on this one (tuple-struct ctor) |
| X9b function moved into the types table | each_table_of_the_doors_config_names_its_kind | |
| X9c function in the types table, run.sh | door_run (clippy's "expected a type" points into clippy.toml) | before: B4 hole |
| X4a `.cargo/config.toml` `-Aclippy::..` + std::fs::read | red.sh (no diagnostic on its eight lines) | before: B1 hole |
| X13 same with `["-A", "clippy::.."]` | red.sh | |
| X4b RUSTFLAGS in the environment + std::fs::read | red.sh | |
| X6 barred call in a macro defined in vault.rs, expanded in zfile.rs | the main run (clippy flags the expansion) | not a hole |
| X7 libc::pread64 | the main run | before: B3 hole |
| X14a libc::unlinkat, X14b libc::renameat | the main run | before: B2 hole |
| X1c std::fs::DirBuilder, X1d libc::sync | the main run | |
| X8a CLIPPY_CONF_DIR dropped from the door command | red.sh (all eight lines) | one command now (lint.sh) |
| X8b `--release` for `--profile checked` | red.sh (the debug_assertions line, 8th) | |
| X10 pattern reworded | red.sh ("door_run did not fail on an entry that names nothing") | before: B5 hole |
| R1 an entry naming nothing in the main config | door_run | |
| R2b/R2c door_run's config check gone (with/without a bogus entry) | red.sh | |
| R2d door_run returns clippy's status on a config diagnostic | red.sh's status-2 check | survived the second pass; the status check added |
| R3 red.sh's bogus entry not inserted | red.sh | |
| R4b red.sh's message check gone + pattern reworded + bogus entry | red.sh's status-2 check | |
| R6b libc::unlink entry removed alone | red.sh (7th line) | |
| M1 silences without the metavariable clause | a_new_allow_of_the_door_fails_in_each_spelling | |
| M2 rename not read | a_thread_call_added_fails_and_a_use_of_a_thread_fn_fails | |
| M3 glob not read | a_thread_call... | |
| M4 THREAD_TYPES holds Builder | a_thread_call... | |
| M5 `self as t` not read | a_thread_call... | |
| M6 the kind compare always equal | each_table... | |
| M7 door_table takes reasons | each_table... | |
| M8 thread_uses on non-door files only | a_thread_call... (the pager re-export case) | |

Pair controls (expected to pass; they show a red element is load-bearing, each paired with
the break it exists for, whose single form is killed above): R5 red's debug_assertions line
gone + `--release` (single: X8b killed by that line alone); R6 red's unlink line gone +
the unlink entry gone (single: R6b); R7 the red copy back outside the repository +
`.cargo` rustflags (single: X4a). C0 and B0: controls, both pass clean.

Survivors: none.

## Round 2 (repair)

mutants_r2.py -> mutants_r2.results (final; one verdict a line), mutants_r2.log. Kinds: test
(real tree from keep_r2, the source binary; KILLED = a named test FAILED, INVALID = did not
compile), tbefore (the same input under round 1's tests/source.rs and door.txt), door (a repo
copy, door_run: the lint and its config check), dbefore (the copy with round 1's
clippy.toml), script (a copy, the whole run.sh with its red step). before_r2.log collects the
before rows.

| mutant | round 1 | round 2: killed by |
|---|---|---|
| E1 `#[cfg(not(clippy))] remove_file` in Db::seal | hole (tests pass; the door run passes too, E1c) | a_cfg_only_the_doors_run_sets_fails... ("names `clippy`") and the_door_holds |
| E2 `#[cfg(not(debug_assertions))] Path::exists` in Reader::convert_walked | hole (E2c: door passes) | code_the_door_does_not_compile... / the_door_holds (`.exists(` in compiled-out code) |
| E3 `#[cfg(not(linux))] fn door_probe` reading a file | hole | the_door_holds (`fs::read` in compiled-out code) |
| E4 / E11 `libc::remove` in drop_written | hole | the door (disallowed `libc::remove`) |
| E5 / E14 `#[allow(deprecated)] fs::soft_link` | hole | the door (`std::fs::soft_link`; allow(deprecated) silences nothing of the door's) |
| E6 `Command::new("rm")` | hole | the door (`std::process::Command::new`, and the type) |
| E7 `UnixListener::bind` | hole | the door |
| E8 `libc::lchown` + `libc::setxattr` (E8x setxattr alone) | hole | the door (each) |
| E9 `mod door_helper;` in vault.rs, its file calls remove_file, called from db.rs | caught by G1-1's wall only because db.rs names `vault::door_helper` (a coupling); the door run passes (E9c) | a_door_module_brings_in_no_file... ("takes its allow with it") and the wall |
| E10 `#[allow(clippy::DISALLOWED_METHODS)]` | already caught: rustc does not honour the name (unknown lint, an error under -D warnings, and the call is still flagged; probe_r2) | the door |
| E12a `libc::open_by_handle_at` / E12b `libc::write` on a door fd | hole (E12c) | the door |
| E13 `std::thread::spawn` in crypt.rs code | hole (door modules exempt from the count) | the_door_holds ("thread src/crypt.rs *: 2 > 1") |
| E15a MmapMut from a door fn, `.flush()` in db.rs | hole (E15c) | the door (`memmap2::MmapMut::flush`) |
| E15b same with `.advise(Advice::DontNeed)` | -- | INVALID: `Advice::DontNeed` does not exist in memmap2 0.9 (it is `UncheckedAdvice`); rerun as E15d |
| E15d `.advise(Advice::Sequential)` | hole (E15e) | the door (`memmap2::MmapMut::advise`) |

Mine, on the lines this round wrote:

| mutant | killed by |
|---|---|
| O1 door_cfg answers every key=value true | a_cfg_predicate_is_read_as_the_doors_build_reads_it (+ others) |
| O2 attr_target_end without the `else` continuation | a_cfg_predicate... (the `else` shape) and code_the_door... (the `else { fs::write }` line) |
| O3 compiled_out scans no text | code_the_door_does_not_compile_is_held_by_its_text |
| O4 files_compiled_out finds none | code_the_door... (wasmapi.rs, src/door_probe.rs) |
| O5 door_children ignores include! | a_door_module_brings_in_no_file... |
| O6 a cfg attribute ends at its predicate, not its `]` (every region empty) | code_the_door... -- this was the first draft's own bug, found by that test before the mutant |
| O7 door modules' threads not counted | the_door_holds (door.txt lists crypt.rs and pager.rs) and a_door_module... |
| O8 a predicate naming clippy passes | a_cfg_only_the_doors_run_sets... |
| O9 a cfg_attr applying a cfg passes | a_cfg_only_the_doors_run_sets... |
| O10 the `libc::remove` entry gone | red.sh ("no disallowed diagnostic on src/db/zfile.rs line 7080", its 9th line) |
| O11 both Command entries gone | red.sh (its 10th line, 7081) |
| O12 door_texts without Path's method texts | code_the_door... (`.exists(` among the texts) |
| O13 cfgout not a kind of door.txt | the_door_holds (door.txt's cfgout lines refused) |
| O14 door modules' compiled-out code scanned and counted | the_door_holds (counts the door modules' twins) |
| O15 an undecided predicate read as compiled in | code_the_door... (`cfg(somebodys_cfg)`) |

Survivors: none. E1c, E2c, E9c (the door run passing the cfg and module inputs) are the
door's own blind spots, held by the source test, which is the repair.

Harness note: restoring the tree with `tar xf` puts back the files' archived mtimes, older
than the last mutant's build, so cargo answered the next plain run with the last mutant's
binaries (the source test failed as O15 does until the files were touched). Every verdict
in between was a fresh build, since each mutant's edit is newer; the finished run after was
taken after `touch` of every unit file (after_r2.log, lint_group_r2.log).

## Round 3 (repair)

mutants_r3.py and mutants_r3b.py (the X5r/X8c reruns) -> mutants_r3.results, mutants_r3.log.
Kinds as round 2, base keep_r3f (the finished copies), "before" = keep_r3 (the tree round 2
left): tbefore (round 2's tests/source.rs and door.txt), dbefore (round 2's clippy.toml),
sbefore (round 2's clippy.toml and scripts/door/*.sh). For script kills the cause is red.sh's
`door red:` line, read from the log (the results' column shows the first diagnostic line).

The reviewers' extra mutants:

| mutant | round 2 | round 3: killed by |
|---|---|---|
| X1 `libc::utime` in drop_written | hole (X1b) | the door (`libc::utime`) |
| X2a-d `mknodat`, `execveat`, `dlopen`, `lutimes` | hole each | the door, each by name |
| X3 `libc::bind` of a sockaddr_un | hole | the door (`libc::bind`; `connect` beside it) |
| X4 `ffi!` writing `extern $a { fn unlink }` in db.rs, unlink called | hole (X4b) | a_foreign_block_outside_wasmapi_fails / the_door_holds (every `extern` not followed by `fn` or `crate`). X4c: the door run passes it -- the source test holds it |
| X5 `quiet!` with `#[$a]`, `allow(clippy::disallowed_methods)` handed in | X5/X5b INVALID as first written (macro used before its definition; harness error); X5r' hole | X5r: a_new_allow_of_the_door_fails_in_each_spelling. X5r'': the door run passes (clippy honours the written allow) -- the source test holds it |
| X6 `cfg(not(debug_assertions))` `{ use std::fs as f; f::remove_file }` | hole (X6b) | code_the_door_does_not_compile_is_held_by_its_text ("a use that renames"); X6c the door passes (compiled out) |
| X7 `use std::fs::*; remove_file` in with_idle_io's not-Linux twin | hole (X7b) | code_the_door... ("a use that globs a module") |
| X8 concat! replacing an existing "/proc/self/status" | caught before (X8b: the literal count drops below the list) | the concat! rule |
| X8c a new concat! /proc path in drop_written | hole (X8c') | a_proc_path_outside_the_door_fails ("put together by concat!") |
| X9a/X9b run.sh's config dir `$PWD/scripts`, `$PWD` | hole (X9a', X9b': run.sh prints door: ok) | red.sh, handed the same directory: "no clippy.toml in .../scripts" |
| X10a/b/c door_cfg panic=>abort, pointer width 32, doc true | hole (X10a') | a_cfg_predicate_is_read_as_the_doors_build_reads_it (every name door_cfg decides, each way) |
| X11 red.sh's `grep ^allow ... drop_written` block deleted | -- | taken out as unreachable, with the attribute-above check beside it: an allow there, in the source or door.txt, either silences a red line (red fails on it) or fails the source test; red.sh's header says so |
| X12a red.sh's line check skips the last line (`-lt`) | the loop was a literal `1..10` | red.sh phase 2 ("without their entries the lines read as missing: 7072..7081, not 7072..7082 (the list's 11)") |
| X12b X12a + the utime entry gone (the reviewer's pairing with O11's shape) | -- | red.sh ("an entry the lines are listed with is not in the config") |
| X12c red.sh counts one line fewer (`added-1`) | -- | red.sh phase 2 (the count against the list itself) |
| X12d O11 again (Command entries gone) | killed | red.sh ("no disallowed diagnostic on ... line 7081") |
| X13 door_texts without the types loop + `Option<memmap2::Mmap>` in compiled-out code | -- | code_the_door_does_not_compile_is_held_by_its_text (every config entry's text among the texts) |

Mine, on the lines this round wrote:

| mutant | killed by |
|---|---|
| O16 foreign_blocks reads the next word at any offset | a_foreign_block_outside_wasmapi_fails |
| O16b `fn` not a function's (only `crate` excused) | every test (src/db.rs's `unsafe extern "C" fn` type is then a block) and a_foreign_block... |
| O17 a metavariable attribute `#[$a]` not read | a_new_allow_of_the_door_fails_in_each_spelling (the definition alone) |
| O18 an allow outside an attribute not read | a_new_allow... (the invocation alone) |
| O19 use_hits: glob not read | code_the_door... |
| O20 use_hits: rename not read | code_the_door... |
| O21 use_hits: a barred function taken out not read | code_the_door... |
| O22 the concat! rule gone | a_proc_path_outside_the_door_fails... |
| O23 the libc scan without `*mut c_char` | every_libc_call_that_takes_a_string_is_barred_or_said_to_touch_no_file (mkstemp) |
| O24 a door-libc.txt entry barred as well passes | every_libc_call... |
| O25 LIBC_FILES without linux_l4re_shared.rs | every_libc_call... (setmntent) |
| O26 a call neither barred nor excused passes | every_libc_call... (the utime case) |
| O27 the config's utime entry gone | red.sh ("no disallowed diagnostic on ... line 7082", the 11th) |
| O28 run.sh hands red.sh no config | red.sh ("usage: red.sh CONFDIR") |
| O30 red phase 2 takes no plain entries out | red.sh phase 2 |
| R-O10, R-X8a, R-X8b, R-R3, R-R2d (earlier rounds' gate mutants, red.sh rewritten) | red.sh: line 7080 / all eleven lines / line 7079 (debug_assertions) / "did not fail on an entry that names nothing" / "answered 101, not 2" |

Pair control, expected to pass: O29, red.sh copying `$root/scripts/door/clippy.toml` instead of
CONFDIR together with run.sh's config dir `$PWD/scripts` -- the reviewer's hole rebuilt from
two faults, each killed alone (X9a; red.sh's CONFDIR alone equals the root's path when run.sh is
right, so it is equivalent there). It survives, which shows red.sh's argument is what closes
the hole.

Survivors: none but the door-run rows X4c, X5r'', X6c, which the source test holds (the door's
own blind spots: a declaration of the crate's own, an allow a macro writes, compiled-out code)
and O29 (a pair control). Not a mutant: deleting red.sh's phase 2, which is the test of
phase 1's line check (it is what kills X12a and X12c), not a check of the tree.
