# Check of conform/notes.md (stage G2, the conform pass)

Export read: `scratchpad/plan/tree` (682b2c09). Nothing was built or run. My scripts are in
`scratchpad/plan/conform/check/`:

- `alt.py ROOT REGEX`: for every test that `classes.tsv` puts in `runs`, it prints the matches of
  REGEX in the test's body and in every helper it reaches. Reach is by name, through the same file
  and tests/common, the same over-approximation classify.py uses.
- `why_all.txt`: `why.py` run over db, zpages, fences, settle, durable and leak.
- `methods.txt`: the `db.`/`r.`/`h.`... method calls the `runs` tests reach.
- `fields_never_in_tests.txt`: the `Options` fields that no file under tests/ names.

Verdict: **stands with corrections.** Tier 1 now and the crash-state API deferred to S3 is still
the right call. But the run set holds tests that fail for reasons of mode, not of reader. Two
counts are wrong. Three costs and breaks are missed: the release lint compile, the unpinned writer
under the pager, and the opt-out's API shape. The "bytes" class is not all crash or damage work.

---

## 1. Counts rerun

These agree with the notes. Each was rerun with the command shown.

- Tests per file, `grep -c '#\[test\]' tests/*.rs`: 639. The per-file figures match the table.
- `Db::create(` sites: 415. `Db::open(` sites: 292 (`grep -o ... | wc -l`, per file). The
  constructors are only `Db::create` (src/db.rs:28345) and `Db::open` (src/db.rs:28680). A
  search of tests for `Db::[a-z_]*(` finds no other constructor; the only other associated fns
  called are `mapped_bytes`, `trim_supported`, `resident_beyond_use`, `trim_now` and
  `mapped_resident_bytes`.
- Classes, from `cut -f1 classes.tsv | sort | uniq -c`: 58 no-store, 83 sets-cipher, 167 bytes,
  87 observe, 13 mapping, 231 runs.
- Crash helpers, from `crash.py ... copy_store,power_cut,power_cut_logs,ss_copy_with_lost_pages,os_copy,zero_prefix`:
  125 tests, 37 of them set a cipher. That leaves 88 plain crash tests among durable's 104
  opt-outs.
- `Options` fields: 220. The fuzz sets 38 (`o.F =` or `F:` in tests/fuzz.rs).
- `one_at_a_time()`: 9 call sites. `let cache: u64 = 2 << 20`: 7 sites (tests/cipher.rs:375,
  529, 648, 772, 1391, 1623, 1779).
- `DEFAULT_SEEDS = 4` is at tests/fuzz.rs:105. `pick_cfg` is at tests/fuzz.rs:266, not 265.
  `with_cipher` is at 416.
- 5 `runs` tests reach `Watchdog::arm`: db.rs:13454, 13538, 13595, 13726 and 15032. 61 `runs`
  tests carry `from_secs(` or `elapsed() <`/`>`. `std::process::exit` appears only at
  tests/db.rs:13328 (`watchdog_exit`).
- Line numbers checked against the export and correct: vault.rs 397 `key_id`, 453
  `refuse_marked`, 562 `passes`, 640 `default_cache`, 667 `locate`, 803/820 `Vault::create`/`open`,
  2050 `names`, 2205 `map`; pager.rs 1312 `ensure`, 1570/1576 `guard_on`, 319-321 `hard`/`swamp`;
  db.rs 28356/28691 the two match arms, 12259 `cipher_controller`, 3996 `ZPages::Auto`, 4065
  `scan_block_cache: true`, 4178 `zpages_under_cipher: false`.

### Wrong counts

- **Unit tests in src/ are 404, not 474.** `grep -c '#\[test\]' src/*.rs src/db/*.rs | awk -F: '{s+=$2} END{print s}'`
  gives 404. The per-file figures the notes quote are right (zselect 151, zpages 59, zfile 46,
  crypt 24, flatindex 21, ordindex 19); the total is not. No `macro_rules!` in src/ emits a
  `#[test]`. The src macros are `walk_fixed`, `emit_record`, `indexed` (blob.rs) and `on`, `any`
  (wasmapi.rs).
- **The 8 re-create cases are all safe; none is left "not verified".** I read the five the notes
  left:
  - db.rs:6452 uses `dir("stream-copies")`, `dir("stream-stream")`, `dir("stream-torn-tail")` and
    `dir("stream-torn-marker")`: four directories.
  - db.rs:7584 calls `let d = dir(name)` inside the loop.
  - db.rs:16521 rebinds `d`, and its last create is in `d2` (16549, 16557, 16600).
  - zpages.rs:6034 calls `remove_dir_all` and then rebinds `scratch(...)` (6066, 6078).
  - durable.rs:3356 calls `remove_dir_all(&d)` at 3416 before the create at 3425.

  `Db::create` calls `create_dir_all`, and every `dir()`/`scratch()` helper removes and recreates
  its directory (tests/db.rs:17, tests/zpages.rs:39, settle.rs:16, fences.rs:16). So
  `Vault::create`'s non-empty refusal is reached by no `runs` test.
- **Double open.** I searched each test for two live `Db` bindings over one directory variable
  with no `drop`/`close` between them (a python scan over db, zpages, settle, fences and durable).
  It found none. The one test that holds two stores open at once, zpages.rs:9950, uses two
  directories.

## 2. Searches done another way

- **Method syntax on paths.** I counted `.exists()`, `.is_file()`, `.metadata()`, `set_len(`,
  `.read_dir()`, `.try_exists(` and `*_at(` across all tests. No `runs` test reaches any of them:
  `alt.py` with `\.exists\(\)|\.is_file\(\)|\.metadata\(\)|set_len\(|_at\(` hits only
  `blocked_store_at(`, which is a helper name.
- **Trait imports.** The only `use ... fs` lines in tests are
  `std::os::unix::fs::{MetadataExt,FileExt}`. Every use of them sits beside a `std::fs::` call
  that the scout's regex already caught.
- **Other ways to touch a file.** `libc::` in tests is posix_fadvise, mincore and madvise
  (zpages.rs) and syncfs (dirkey.rs). `Command::new` and `memmap2` appear nowhere in tests.
- **Stores made on another thread.** For every `Db::create(`/`Db::open(` line I walked back to
  enclosing lines with less indent and looked for `spawn`. I found none. The notes' worry that a
  thread-local opt-out misses stores made on another thread does not arise today.
- **Outside callers.** `examples/cipher_store.rs`, `bench/src/engines.rs`,
  `bench/examples/{work_probe,zsize,merge_walk}.rs` and `experiments/ffi/capi/src/lib.rs` call
  `Db::create`/`open`. Each builds in release, so the debug-only substitution never reaches them.
  The bench's examples under callgrind are release too.

## 3. Sampled sites (17), with the verdict on each

| site | notes' class | verdict |
|---|---|---|
| db.rs:180 `killed_before_first_seal_opens_from_the_wal_alone` | runs | right: drop and reopen, no file access |
| db.rs:201 `uncommitted_tail_is_lost_whole...` | runs | right |
| db.rs:406 `oracle_in` (helper) | cleared by hand | right: `read_dir` only inside the panic message (459) |
| **db.rs:6315 `an_ordered_load_goes_straight_into_segments`** | runs | **wrong: fails under conform.** It asserts `db.wal_durable().2 <= 8` (db.rs:6343-6347, 6363). Under a cipher the log header is `WAL_SEALED_LEN = WAL_MAGIC.len() + vault::WRAPPED` (src/db.rs:4271), and each run commit is logged as well (`Db::log_run`, src/db.rs:30170). |
| **db.rs:1005 `an_unpinned_writer_is_refused_beside_another_publisher`** | runs | **wrong, inferred.** It runs `oracle_in` with `writer_pins: false` (db.rs:1030-1036). Under a cipher the writer's reads take no slot (`pin_slot.filter(\|_\| self.opts.writer_pins)`, src/db.rs:18934, 19006, 19109). `Paged::ensure` then `debug_assert!`s a pin (pager.rs:1316-1320). `check_options` (src/db.rs:12277-12296) refuses nothing about a cipher. Expect a panic at the writer's first read of a sealed segment. Not run. |
| **zpages.rs:6034 `auto_degrades_where_it_cannot_run...`** | runs | **wrong, inferred.** The "auto-nopin" arm sets `writer_pins = false` and runs an ordered load and `check` (zpages.rs:6041-6058). Same panic as db.rs:1005. Not run. |
| **durable.rs:5126 `a_sync_with_only_the_run_owing_syncs_nothing_of_the_log`** | runs | **wrong: fails under conform.** It asserts that no log fdatasync happens across a sync of a run (5144-5149). Under a cipher, run commits go to the log (`log_run` ... `sync_wal`, src/db.rs:30192). |
| zpages.rs:395 `the_oracle_holds_with_zpages_forced_on` | observe (opt-out) | **opt-out not needed.** Its only file reference is `assert!(form_files(&dr).is_empty())` on the store under Off (zpages.rs:620). Under conform that is vacuously true. It is the compact-form oracle, one of the tests conform most needs. |
| zpages.rs:2146 `scans_agree_with_the_model_over_every_window...` | bytes | **not crash or damage work.** It reads form-file bytes only to take scan start keys from `fence_keys` (zpages.rs:3448-3452, in `windows_over_with`). Under conform `form_files` returns [] and the test runs with fewer windows. A fence-key accessor would let it run whole. |
| zpages.rs:700, 2037, 2242, 2884, 4128, 4285, 5487 | bytes | same as 2146: `form_files` + `fence_keys` used only as input (why_all.txt, "helpers=['fence_keys','form_files']" and no direct fs line) |
| settle.rs:109 (and settle 259, 411, 535, 618) | observe | right to opt out today. `partition_fences` is used to pick keys and counts (settle.rs:137, 158, 287, 436, 570, 649), and the names do not exist under a cipher. |
| fences.rs:343 | observe | right: asserts `seg-`/`par-` name prefixes |
| fences.rs:479 | bytes | right: compares two stores' segment files byte for byte |
| db.rs:6799 `a_value_the_record_cannot_hold...` | observe | right: `d.join("direct-00000000.tmp").exists()`. Under a cipher the run is a memfd, so a name accessor would not serve this test either. |
| db.rs:2898 | observe | right: `dir_bytes` sums file lengths (db.rs:2780) |
| leak.rs:394 | mapping | right |
| zpages.rs:1391 | runs | right: creates only. Its refusals come from `check_options` before any file is written. |
| checksums_off.rs:45 | runs | right |

## 4. What was missed

### 4a. `runs` tests that fail or mean something else under a cipher with no reader at fault

`suspects.py` searched only for advice, residency, budget and `ZInject` words. It did not search
for the engine's own seams whose meaning changes under a cipher.

Search: `alt.py ROOT '\b(trace_wal_syncs|wal_syncs_traced|run_synced_bytes|run_written_and_held|fail_next_run_[a-z_]*|short_next_run_write|run_remaps|stream_recycled|wal_durable|ord_rebuilt|cipher_counts|wal_unsynced|run_ino|fail_next_[a-z_]*|watch_slot_writes|tear_[a-z_]*|fail_open_slot_sync|wal_bytes|wal_len)\b'`.
It finds **21** `runs` tests (`check/modeseams.txt`): db.rs 1961, 2091, 2201, 6315, 15861, and
durable.rs 1178, 1472, 1488, 1505, 1773, 2250, 2277, 2707, 4953, 5062, 5099, 5126, 5310, 5342,
5440, 7140.

- **16 of durable.rs's 17 `runs` tests are in this set.** These seams answer differently under a
  cipher:
  - the run file is a memfd (`vault::open_mapped`);
  - its commits go to the log;
  - the log header is sealed.

  Two of the 21 are confirmed failures by reading: db.rs:6315 and durable.rs:5126. The rest are
  not verified; each needs a read or a run.
- **Add the two unpinned-writer tests** (db.rs:1005 and zpages.rs:6034; search
  `writer_pins:\s*false|writer_pins = false`). They are the only `runs` tests that set it.

**Corrected run set.** Of 231, at least 4 fail for mode reasons (confirmed or inferred, as above),
and up to 19 more are at risk. Clean `runs` is between 208 and 227. durable.rs's real
contribution is 0 to 1 tests, not 17.

An engine finding, separate from conform: `check_options` accepts `writer_pins: false` with
`Encryption::Cipher`. In release that writer reads with no pin, so the evictor does not wait for
it. That is unsound. In debug it panics. Conform will surface this. The fix is a refusal in
`check_options` (the `writer_pins`/`publish_in_background` rule at src/db.rs:12278 is the model),
not an opt-out.

### 4b. The opt-out API breaks the release lint unless it exists in release

The lint group is `cargo clippy --release --all-targets -- -D warnings` (scripts/check.sh). It
compiles the integration tests in release, where `debug_assertions` is off.

If the library's opt-out (a thread-local switch) and the identity cipher exist only under
`#[cfg(debug_assertions)]`, then a test-side `common::plain_files_only` that calls it does not
compile in that group. Each call site and the helper would need the same cfg. Otherwise the
library function has to exist in release as an empty `#[doc(hidden)]` fn, the shape `guard_on`
has (a `const fn ... false` when the cfg is off, pager.rs:1576).

This is inferred from the documented behaviour of `cfg`; nothing was compiled. The "nothing in
release" claim holds for the engine's open path; the test-facing API needs the release stub.

The opt-out should also be an RAII guard that resets on drop, not a bare thread-local set. libtest
runs each test on a thread of its own when threads are supported, so a leak today would be only
within one test. That is from memory of libtest, not verified here.

### 4c. The "bytes" class is not all crash and damage

- **15 of zpages.rs's 35 "bytes" tests read form-file bytes only to derive scan starts:** the 7
  listed in the table above, plus the windows tests 2146, 2152, 2160, 2171, 2179, 2197, 2215 and
  2233 (through `windows_over`/`windows_over_with`). That is 8 windows tests + 7 = 15. The notes
  do not separate them from the crash and damage tests.
- **30 zpages "observe" tests reach no file but through `form_files`** (why_all.txt). Some only
  assert a count, and some pass vacuously under conform.
- `partition_fences` users (settle 5, fences 6+) need the fences, not the names.

So the better accessor is not `Db::file_names` but two engine-state reads that do not depend on
the vault's names:

- a hidden `Db::partition_fences()` from the live `State`;
- `Db::form_fence_keys()`, or an equivalent, from the attached form files.

These serve plain and cipher alike. The notes' claim that the 167 "bytes" tests are "the ones S3
changes" is true for durable's 88 plain crash tests and db.rs's crash windows. It is not true for
these 15. With the accessors those 15 run under conform now.

### 4d. Vacuous passes are not counted

A test whose file lookup comes back empty under conform can pass with its check skipped (examples
above). The notes count tests as opted out or run, but not as "runs but its file-derived checks are
empty". The pass needs a guard against this. One way: `form_files`/`seg_names`/`partition_fences`
in tests/common panic when the store is under conform, rather than returning [].

### 4e. Process-wide side effects of conform on the tests that opt out

While any conform store is open, `VAULTS > 0` (vault.rs:655). Every door call of every store in
the process, the opted-out plain ones included, then takes the global `REGISTRY` mutex and scans
it linearly (`under`, vault.rs:658-663). The opted-out tests then run under a timing the plain gate
never sees. That matters for the 61 time-bounded tests. It does not change their answers
(inferred).

### 4f. Options coverage, a tighter input for G-options

35 of the 220 `Options` fields are named in no file under tests/ at all, comments included. The
list is `check/fields_never_in_tests.txt`; the python is in this session's command log (field
regex `\s*pub\s+([a-z_0-9]+)\s*:` at struct depth 1, then a `\bF\b` search over the concatenated
`tests/*.rs`). The 182 that the fuzz does not draw is the wider set. The 35 are the ones no test
exercises by name.

## 5. Attack on the recommendation

- The release cost of the substitution stays zero: the cfg removes it from `Db::create`/`open`, and
  the outside callers build in release. **But** see 4b: the test-facing opt-out must exist in
  release, or the lint group fails to compile.
- The checked-profile cost per open stands: one `OnceLock` load, plus the marker read under
  conform. The opt-out's thread-local read at `Db::create` is a second load per create. It is not
  per operation, so it is not a finding on the hot path.
- The plain store's hot path is untouched. `ATTN_PAGED` is set only where `shared.pager` is
  `Some` (`pager: held.as_ref().map(...)`, src/db.rs:28434).
- **Tests that can no longer run, beyond the notes' list:**
  - the mode-seam tests (4a), at least 2 confirmed;
  - the two unpinned-writer tests, which panic until `check_options` refuses the combination.

  These need opt-outs or engine fixes before the first conform run can be green. The notes' "about
  349 opt-out lines" becomes about 349 + 4 confirmed + up to 19 more.
- The fuzz is safe. Its "cache turned over" assertion (tests/fuzz.rs:1322) is gated on
  `under_cipher()`, which conform does not change.
- Watchdogs: the count of 5 stands.

## 6. Not verified

- Whether the 19 at-risk mode-seam tests in 4a pass under a cipher. That needs a read of each seam
  under a cipher, or a run.
- That an unpinned writer actually reaches `Paged::ensure` in db.rs:1005 and zpages.rs:6034. I
  inferred it from `pin_slot.filter(...writer_pins)` and the `debug_assert!`.
- `paths.py`'s reach counts (193, 167, 190, 110, 29) were not recomputed.
- Whether `Vault::names` gives the logical `par-`/`zpg-` names of files still in memfds; I did not
  read `Vault::names` (vault.rs:1131).
- libtest's thread-per-test behaviour under `--test-threads=1` (from memory).
- The clippy `--release --all-targets` break in 4b is inferred from cfg semantics; nothing was
  compiled.
