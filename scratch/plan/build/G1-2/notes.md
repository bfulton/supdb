# G1-2 door: notes

(Rounds 0-2 kept their notes in mutants.md and the logs beside it; this file
starts at repair round 3 and says where everything stands.)

## Round 3: step now
- 11:20 tree as round 2 left it saved: keep_r3/tree.tar, keep_r3/TREE_SHA256SUMS.
- Step: DONE (12:05). Tests 65/65 (after_r3.log), lint group with the door green
  (lint_group_r3.log), fmt applied (fmt_r3.log) and the finished copies re-kept
  after it (keep_r3f). Mutants: mutants.md, "Round 3". Nothing committed.

## The door's target directory
- target/door: 60M after a run (du -sh, 11:18). The red copy's own target
  is built and removed in each run. On this box (9.9G free) the lint group
  affords it; no change proposed.

## Round 3: what is where (11:40)
- Finished copies of the unit's files: keep_r3f/tree.tar (TREE_SHA256SUMS);
  keep_r3/ is the tree as round 2 left it (the "before" of every *before kind).
- before_r3.log: round 3's tests over round 2's config (the libc class test red).
- door_r3.log: run.sh with round 3's config and red.sh, EXIT=0.
- Mutants: mutants_r3.py -> mutants_r3.results (one verdict a line as it
  comes), mutants_r3.log; started in the background, log run_r3.log with an
  EXIT line. A restart: rerun the same command; done() skips verdicts already
  written, and restore_tree puts keep_r3f back (sha checked) after each.

## Round 3: what changed and why
- Blocking 1 (libc list called a class): the libc class is now mechanical.
  tests/source.rs `every_libc_call_that_takes_a_string_is_barred_or_said_to_touch_no_file`
  reads libc-<Cargo.lock's version> from $CARGO_HOME/registry/src/*/ (LIBC_FILES:
  unix/mod.rs, linux_like/{mod,linux_l4re_shared}.rs, linux/{mod,arch,arch/generic},
  gnu/{mod,b64,b64/x86_64,b64/aarch64}; LIBC_DIRS new/{common,glibc,linux_uapi}) and
  requires every `pub fn` with a `*const c_char` or `*mut c_char` argument to be in
  clippy.toml or in tests/data/door-libc.txt (name, reason; 68 pure string, env,
  kernel-name, interface and stream-buffer calls). Departure from the reviewer's
  pattern: arguments by type, not by name -- by name the scan missed `unlink(c: ..)`
  and `link`; by type it finds 243 calls. 110-odd names added to the config
  (the reviewer's 19, bind and connect by hand, and the NSS/zone/locale/syslog/tty/
  stdio-stream calls the type scan found); every one resolved (door_run's
  names-nothing check passed). Red: the test itself removes utime from the barred
  set and requires the error; before_r3.log is the test red over round 2's config
  (it panicked on its first assertion there, `unlink` unfound, which is what moved
  the pattern from names to types). The header now says which entries are a
  class held to the source and which are hand lists. red.sh has an 11th line,
  libc::utime.
- Blocking 2 (red input on another config path): run.sh names `conf` once and
  hands it to door_run and to red.sh, which copies "$1/clippy.toml"; X9a/X9b killed.
- Extra mutants' holes closed in tests/source.rs: foreign_blocks reads every
  `extern` not followed by `fn`/`crate` (macro-written blocks); door_allows counts a
  `#[$meta]` attribute and an allow written outside an attribute; compiled-out code
  bars a `use`/`extern crate` that renames, globs or takes out a barred function's
  name (use_hits); concat! outside the door modules fails; door_cfg's every name
  tested each way; door_texts held to every config entry.
- red.sh: lines in a list (entries, tab, line); phase 2 lints the copy under the
  config less the lines' entries and requires every line missing, by place and by
  count against the list, and a clean run (kills the line-check mutants X12a/c).
  Cost: one more library lint of the copy, ~8 s; the lint group's door step ran
  26 s (lint_group_r3.log).

## The door's target directory (measured 12:02)
- target/door: 60M (du -sh). The red copy builds its own target under
  target/door/red.XXXXXX and run.sh removes it. Affordable with 9.9G free; no change.

## Engine and costs
- No file under src/ changed this round (keep_r3 vs keep_r3f src sums identical), so
  the callgrind rows are not recounted; engine_changed false.

## Not verified / residual
- The libc test assumes cargo's registry layout ($CARGO_HOME/registry/src/<index>/
  libc-<ver>); verified here only. A vendored or offline CI layout would panic the
  test (it fails rather than skips).
- LIBC_FILES is a hand list of libc's files for glibc x86_64/aarch64; a libc release
  that moves declarations into another file passes until the test's own anchors
  (open, unlink, mkstemp, execveat, setmntent, utime, posix_spawn_file_actions_addopen,
  strlen, >200 calls) miss; a declaration moved to an unlisted file is not seen.
- Compiled-out code: a type alias (`type F = std::fs::File; F::open(..)`) and a path
  built by format! are not held by text. Not run as mutants.
- The arm64 leg: names in the config resolved on x86_64 here; the aarch64 files are
  scanned but clippy's names-nothing check has not been run on aarch64.

## Commit (12:20)
- Gate `sh scripts/check.sh build test lint` from a clean TMPDIR: gate_commit.log, EXIT 0, all groups green, door: ok. Committed fa7e9356. TMPDIR removed.
