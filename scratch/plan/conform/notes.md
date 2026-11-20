# Stage G2, the conform pass: is it buildable on today's vault and pager

Export read: `scratchpad/plan/tree` (commit 682b2c09). Paths below are relative to that root.
Nothing was built, run or timed. Counts come from the commands and scripts named here, all in
`scratchpad/plan/conform/`:

- `classify.py ROOT FILE... [--detail] [--helpers]`: classifies every `#[test]` fn of a file.
  Output: `detail.txt` (one line a test, a tag `o e f n m b l`), `counts.json`, `classes.tsv`
  (one line a test with its class).
- `why.py ROOT FILE`: for each opted-out test, the helpers and the lines that put it there.
- `paths.py`, `paths2.py`: which read paths and time bounds the tests that run under conform reach.
- `suspects.py`: tests that run under conform but name advice, residency, budgets or `ZInject`.
- `crash.py ROOT FILE NAMES`: tests that reach given crash-copy helpers.
- `recreate.py`: tests that call `Db::create` twice on one variable name.
- `options_not_drawn.txt`: the 182 `Options` fields `tests/fuzz.rs` never sets.

## The answer in short

- Tier 1 is buildable on today's vault. The substitution is two match arms, in `Db::create` and
  `Db::open`. It needs an identity `Aead` that lives in library code (none does today), a reserved
  key id, an opt-out list, and an eviction mode in the pager that ignores recency.
- Of 639 integration tests, 581 open a store. 83 of those set a cipher themselves. Of the other
  498, 267 must opt out (54%) and 231 run under conform. Most opt-outs are `tests/durable.rs` (104),
  `tests/zpages.rs` (102) and `tests/db.rs` (46).
- The opt-outs split three ways:
  - 167 read or write a store file's bytes (crash states, damage, byte comparisons);
  - 87 only observe the store's file names or lengths;
  - 13 read mappings or residency (`/proc/self/maps`, `mincore`).
- The 87 "observe" opt-outs could run under conform if one hidden accessor returned the logical
  file names. `vault::names` already gives those names under a cipher (vault.rs:2050).
- Recommendation: build tier 1 now, the name accessor, and the fuzz half of tier 2. Do not build a
  crash-state test API on today's envelope format: the 167 byte-level tests are the ones the page
  layer (S3) changes. Item 7 gives the reasons.

---

## 1. How a store under `Encryption::Cipher` is made today

### The path from `Options::encryption` to the pager

- **`Options::encryption`** (src/db.rs:869, field of `Options`) has three values: `None`,
  `Kernel(DirKey)` and `Cipher(vault::StoreCipher)` (`enum Encryption`, src/db.rs:826-859). Its
  default is `None` (src/db.rs:3957, `impl Default for Options`).
- **`Db::create`** (src/db.rs:28345):
  - It calls `check_options` first (src/db.rs:28346; fn at src/db.rs:12272). A refused combination
    writes nothing.
  - It calls `std::fs::create_dir_all(dir)`.
  - It runs `match std::mem::take(&mut opts.encryption)` (src/db.rs:28356-28370):
    - `None`: `vault::refuse_marked(dir)` (src/db/vault.rs:453), which refuses a directory that
      holds `supdb.cipher`;
    - `Cipher(c)`: `cipher_controller(&mut opts)` (src/db.rs:12259-12263), then
      `vault::Vault::create(dir, &c)` (src/db/vault.rs:803).
  - The engine's directory then becomes `held.dir()`, the decoy (src/db.rs:28371-28372).
  - `Shared.pager` becomes `held.vault().pager().clone()` (src/db.rs:28434; the open's at
    src/db.rs:29156).
- **`Db::open`** (src/db.rs:28680):
  - The same match is at src/db.rs:28691-28706. Its `None` arm calls `dirkey::refuse_locked` and
    `vault::refuse_marked`. Its `Cipher` arm calls `dirkey::refuse_locked`, `cipher_controller`,
    and `vault::Vault::open` (src/db/vault.rs:820).
  - The decoy is at src/db.rs:28708-28709. A `pager::Hold` covers the open's own reads (src/db.rs:28715).
- **`cipher_controller`** is the only option a cipher changes: `ZPages::Auto` becomes `Off` unless
  `zpages_under_cipher` is set (field src/db.rs:3903, default false at src/db.rs:4178). No option
  is refused under a cipher. The searches `grep -n 'err(.*cipher' src/db.rs` and a search for
  "under a cipher" inside `err(` strings found no refusal.

### `Vault::create` (src/db/vault.rs:803-818)

- It refuses a non-empty directory: `read_dir(real)?.next().is_some()`.
- It refuses a platform with no memfd: `drop(memfile()?)`.
- It draws the store key (`Key::draw`) and a store id.
- It calls `write_marker` (vault.rs:435-450): `supdb.cipher.tmp` is written, synced, renamed to
  `supdb.cipher`, and the directory is synced. The marker is `MARKER_MAGIC`, `MARKER_VERSION = 1`,
  the `key_id` in clear, the store id, and the store key wrapped by `cipher.master`.
- `Vault::new` (vault.rs:766-780) sets `decoy = real.join(MARKER)` and makes a
  `Pager::new(cipher.cache.unwrap_or_else(default_cache))`.
- `register` (vault.rs:786-801) pushes the vault into `REGISTRY` and bumps `VAULTS`. A second
  register of the same decoy is refused: "the store under a cipher is open in this process already".
  A plain store has no such lock (`grep -n 'flock\|LOCK\b' src/db.rs` finds none).

### `Vault::open` (vault.rs:820-852)

- It reads the marker. A missing marker is refused: "a cipher was given to a store that is not kept
  under one".
- It checks the version and unwraps the key with `cipher.master`. A wrong key is refused.
- It removes `supdb.cipher.tmp`.

### The decoy directory

- The engine is handed `real/supdb.cipher`, a regular file. Any file operation that does not pass
  the door fails with "not a directory".
- The door is the free functions at vault.rs:1863-2229: `create`, `create_rw`, `open`,
  `open_write`, `open_append`, `open_slot`, `rename`, `open_mapped`, `hard_link`, `remove_file`,
  `exists`, `is_file`, `len`, `names`, `read`, `sync_dir`, `real`, `open_dir`, `persist`,
  `persist_one`, `persist_form`, `companion`, `kept`, `map`, `check` and `hold`.
- Each calls `locate(path)` (vault.rs:667), which loads `VAULTS` first (vault.rs:655). That load is
  the door's counter: with no vault in the process, every door call is one load and the standard
  library's call.

### The pager

- `vault::map` (vault.rs:2205-2214):
  - a path of no vault goes to `MmapBytes::open`;
  - a name that `passes` (wal-, spare-, manifest*; vault.rs:562) goes to `MmapBytes::open` of the
    real file;
  - any other name goes to `MmapBytes::paged(v.page_of(&name)?)`.
- `MmapBytes::paged` (src/bytes.rs:365) builds `Own::Paged` with `lend = 0` (src/bytes.rs:272-279).
  - `as_slice` (bytes.rs:376-379) returns a slice of `lend` bytes, which is empty for a paged mapping.
  - `lend_at` (bytes.rs:481-488) refuses anything past `lend`.
  - `piecewise` (bytes.rs:489-491) is true.
- `Paged::ensure` (src/db/pager.rs:1312-1338) fills chunks on demand. A chunk is
  `CHUNK = 4096 - TAG = 4080` bytes (pager.rs:108-109). A unit is 256 chunks
  (`unit_chunks`, pager.rs:122-124), 255 pages.
- Under debug assertions a read with nothing pinned panics (pager.rs:1316-1320).

### Is there an identity or stand-in cipher already

No identity cipher exists. Three stand-ins exist, and all are `cfg(test)` of the library, so
integration tests and a `cfg(debug_assertions)` library build cannot use them:

- `vault::tests::Toy` and `vault::tests::cipher(key)` (src/db/vault.rs:2231-2292, under
  `#[cfg(all(test, target_os = "linux"))]`), used by `db::cipher_looks` (src/db.rs:44237);
- `pager::tests::Toy` (src/db/pager.rs:1633);
- `crypt::tests::Toy` (src/crypt.rs:1160).

The integration tests use `common::Gcm` over `ring` (tests/common/mod.rs:197-235). `ring` is a
dev-dependency, so the library cannot use it.

A conform cipher has to be written new, in library code under `#[cfg(debug_assertions)]`, against
`crypt::Aead` (src/crypt.rs:141-151):

- `encrypt` leaves the buffer as it is and returns a tag.
- `decrypt` compares the tag.
- The tag should depend on the nonce, the associated data and the buffer, for example a CRC32C
  of the three widened to 16 bytes. A constant tag would let a fill from the wrong chunk, file or
  offset pass. With this tag the nonce and associated-data binding (chunk index, file id, header)
  still refuses a misplaced chunk, and the cipher costs a checksum, not AES.
  The choice of checksum is not verified; any keyed mix of nonce, aad and body would do.

### What today's vault does under an identity cipher: the differences on disk

The engine's directory `d` holds the files below. Under conform the real directory `d` holds the
second list.

| plain store, in `d` | conform store, in `d` (real) |
|---|---|
| no marker | `supdb.cipher`, and `supdb.cipher.tmp` for an instant at create |
| `wal-<id>`, `spare-<id>`: plain header, frames in the clear | the same names (`passes`, vault.rs:562). The header is `WAL_HEAD_SEALED`, kind byte and wrapped key, `WAL_SEALED_LEN` long (`Wal::begin`, src/db.rs:4386-4396). Each record is sealed by `LogCipher` (vault.rs:1779-1860). Lengths and bytes differ from the plain log. |
| `manifest`, `manifest.alt`, `manifest.prev-*`, `manifest.tmp` | the same names. Each slot is sealed by `Vault::seal_manifest` (vault.rs:1633) and carries tokens and wrapped keys. |
| `seg-*.sup`, `pcs-*.sup`, `par-*.sup` | absent. Each finished segment is an envelope `x-<32 hex>` (`disk_name`, vault.rs:505-515). Until its envelope is written (`vault::persist`, called at src/db.rs:13685, 13853, 30420, 30968, and inside `manifest_image`), a segment is a memfd and nothing in `d`. |
| `ord-*.oidx` | `x-<hex>.o` (`Kind::Ord`, vault.rs:470) |
| `zpg-*.zpg`, `zpg-dead-*.zpg`, form temps | `x-<hex>.z`, written under `x-<hex>.z.tmp` and renamed (`ENVELOPE_TMP`, vault.rs:517). Never named by a manifest; found as a companion (vault.rs:1489). |
| `direct-*.tmp` (an ordered run's file and its log) | absent. The run's file is a memfd until the run closes (`vault::open_mapped`, vault.rs:1977). Each run commit also goes to the WAL (`Db::log_run`). |
| `seal-*.tmp`, `compact-*.tmp`, `tier-*.tmp` | absent (memfds). |

Other differences that are not files:

- Every finished file is an envelope. The header is in the first page. The plaintext comes after it
  in 4080-byte chunks, each followed by its 16-byte tag. Under the identity cipher the plaintext
  appears at shifted offsets.
- A process crash emulated by `drop` loses every memfd. Copying `d` file by file loses them too.
- One more thread runs per store: `supdb-pager`, the evictor (pager.rs:341-344).
- `Db::cipher_counts()` answers `Some`.
- Plain-only behaviour switches:
  - `look_mask` is 0 instead of 63 (src/db.rs:19162-19168);
  - `not_asked` is `REST_HERE` (src/db.rs:19152-19159);
  - `writer_rests` (src/db.rs:19144-19150);
  - `scan_rested` takes a scan above `SCAN_REST = 1024` (src/db.rs:21699-21708);
  - an ordered run's commits are logged to the WAL.

---

## 2. The substitution point

- **`vault::resolve` does not exist.** The command `grep -n 'fn resolve\|resolve(' src/db/vault.rs`
  finds nothing. The encryption is resolved in two places, the two `match
  std::mem::take(&mut opts.encryption)` arms: src/db.rs:28356 (`Db::create`) and src/db.rs:28691
  (`Db::open`).
- A `vault::resolve(dir, &mut opts, creating) -> Encryption` called at the head of each match
  keeps the rule in one place.

### What a substitution has to set

1. **`Encryption::Cipher(conform_cipher())`.** A `StoreCipher::new(Identity, |_| Identity)` with a
   reserved `key_id`, for example `u64::MAX - 0xC0F` (the value is a suggestion).
2. **Leave `opts.zpages` as the test asked.** Set `opts.zpages_under_cipher = true` before the
   match, or skip `cipher_controller` for a conform store. Otherwise every plain test under the
   default `ZPages::Auto` (src/db.rs:3996, `impl Default for Options`) runs with the controller off, which is a different
   engine.
3. **The cache.** `cache_bytes` unset means `default_cache` (vault.rs:640-646): half of
   `zfile::read_budget` and at least 64 MiB. Every test store is far under that, so no eviction
   happens unless it is forced (see item 5). Tier 2 sets a small cache, but the pager cannot hold
   "4 pages":
   - eviction is by unit, 256 chunks, about 1 MiB (pager.rs:122-124, 1210-1260);
   - `hard = limit + max(limit/16, 4 units)` and `swamp = limit + max(limit/2, 6 units)`
     (pager.rs:319-321);
   - the pass's goal is `over + max(budget/16, 1 unit)` (pager.rs:612-615).

   The smallest working budget in use is the fuzz's 16 KiB (`CIPHER_CACHE.set(16 << 10)`,
   tests/fuzz.rs:1266, 1279).
4. **Only on create, and on an open of a conform store.**
   - At `Db::create`: substitute `None` always.
   - At `Db::open`: substitute `None` only where the marker's key id is the conform id.
     `vault::key_id(dir)`, vault.rs:397, reads it without the key.
   - The reason: a store that a test built by hand as plain, or one created under a real cipher,
     must open as it was built. tests/cipher.rs:250-268 opens a cipher store with `None` and
     requires the refusal "cipher". A blanket substitution on open would answer "key is wrong".
5. **Nothing for `Kernel` or `Cipher`.** Those tests chose their mode.
6. **What a conform store must not trip.**
   - `Vault::create` refuses a non-empty directory. `recreate.py` found 8 tests that call
     `Db::create` twice on one variable name. Three were read: tests/zpages.rs:1391,
     tests/db.rs:12403 and tests/db.rs:15999. In all three the name was rebound to a fresh
     directory. The other five are not verified.
   - `register` refuses a second open of one store while the first is alive. No search can find
     the tests that do this. Not verified.

### What it costs

- **Release.** The call and the identity cipher are behind `#[cfg(debug_assertions)]`. The
  pattern is `guard_on` (pager.rs:1569-1578): a `const fn ... -> false` when the cfg is off. A
  plain open runs the `None` arm as today, with zero added instructions. The bench binaries and the
  shipped crate use the release profile. CLAUDE.md: "the wasm module and the suite's binaries keep
  the release profile".
- **Checked profile, conform off.** One `OnceLock<bool>` load per `create` or `open`, filled once
  per process from `std::env::var_os("SUPDB_CONFORM")`. This is the `guard_on` pattern. Nothing
  per operation: a plain store has `shared.pager = None`, so `ATTN_PAGED` (src/db.rs:16520) is
  never set and no rest or hook below is reached.
- **Checked profile, conform on.** Each open also reads the marker file (`vault::key_id`), and
  the store pays everything a cipher store pays. Only the test binaries see this.

---

## 3. The tests

### How the count was made

- **Tests.** Every `fn` with a `#[test]` attribute line before it. `grep -c '#\[test\]'` agrees
  for every file. There are no macro-generated `#[test]`s: `grep -n macro_rules tests/*.rs` finds
  `trace!` and `both!` in tests/fuzz.rs, and neither emits a `#[test]`.
- **Comments** are blanked before matching. Strings are kept for the name patterns.
- **Features of a test** are taken transitively through every non-test fn of the same file and of
  tests/common/mod.rs. The call graph is built by name: any word-boundary occurrence of a helper's
  name in a body counts as a call. That covers method syntax (`.form_files(`), fns passed by value,
  and fns in `impl` blocks. It over-approximates when a helper's name is shared. There are no
  renamed imports in tests: `grep -n '^use .* as \|^    use .* as ' tests/*.rs` finds none.
  Tests import `Db` as `supdb::Db` or through `use supdb::{Db,...}`.
- **"opens"**: regex `\bDb::(create|open)\s*\(`. `Db::create` (src/db.rs:28345) and `Db::open`
  (src/db.rs:28680) are the only constructors. `grep -n '^    pub fn [a-z_]*(dir: &Path' src/db.rs`
  finds just these two.
- **"enc"**: `\bencryption\b|\bEncryption::|\bStoreCipher\b`.
- **"by path"**, any of:
  - an `fs` call:
    `std::fs::(read|write|read_dir|metadata|OpenOptions|remove_file|copy|File|read_to_string|rename|read_link|symlink_metadata|hard_link|set_permissions)\b`.
    No test file imports `std::fs` (`grep -n 'use std::fs'` finds nothing), so every call is
    spelled `std::fs::`. `remove_dir_all`, `create_dir_all` and `temp_dir` are excluded: a temp
    directory's own make and removal work under conform.
  - a store-file name literal:
    `"(seg-|pcs-|par-|wal-|spare-|ord-|direct-|zpg-|x-)(["0-9{]|dead)|"manifest(["._{]|-?prev)|"supdb\.cipher|"[^"\s]*(\.sup|\.oidx|\.zpg|\.tmp|\.o|\.z)"`.
    The literal handed to a file's `dir(` helper is blanked first.
  - a mapping or residency read: `MmapBytes::open|Blob::open|SparseBlob::open|/proc/self/maps|mincore`.
- **Two helpers are cleared by hand**, because their fs call is not on a store file:
  - `watchdog_exit` (tests/db.rs:13319) opens `/dev/stderr`;
  - `oracle_in` (tests/db.rs:406) calls `read_dir` only inside the `panic!` message of a failed
    reopen (tests/db.rs:459).
- **Spot checks.** Every one of the 46 opt-outs of tests/db.rs was read in `why.py`'s output.
  Three false positives were found and fixed (dir names like `dir("direct-refs")`, the watchdog,
  the oracle's message). The other files' opt-outs were checked by helper, not test by test.
- **Subclasses of an opt-out**, from the transitive bodies:
  - mapping: `m`;
  - otherwise bytes, if it reaches
    `std::fs::(read\b|write|OpenOptions|copy|File|remove_file|rename|hard_link|set_permissions)`
    or `read_to_string` of a non-`/proc` path;
  - otherwise observe: names, `read_dir` and `metadata` only.

### The table

| file | tests | open a store | set a cipher | plain store tests | opt-out | of which bytes / observe / mapping | run under conform | `Db::create(` / `Db::open(` sites |
|---|---|---|---|---|---|---|---|---|
| tests/db.rs | 226 | 226 | 0 | 226 | 46 | 31 / 14 / 1 | 180 | 183 / 102 |
| tests/durable.rs | 168 | 163 | 42 | 121 | 104 | 98 / 6 / 0 | 17 | 96 / 102 |
| tests/zpages.rs | 136 | 136 | 5 | 131 | 102 | 35 / 56 / 11 | 29 | 72 / 32 |
| tests/fences.rs | 11 | 11 | 0 | 11 | 9 | 3 / 6 / 0 | 2 | 11 / 1 |
| tests/settle.rs | 7 | 7 | 0 | 7 | 5 | 0 / 5 / 0 | 2 | 7 / 0 |
| tests/leak.rs | 1 | 1 | 0 | 1 | 1 | 0 / 0 / 1 | 0 | 4 / 1 |
| tests/checksums_off.rs | 1 | 1 | 0 | 1 | 0 | - | 1 | 2 / 1 |
| tests/cipher.rs | 24 | 24 | 24 | 0 | (all) | - | 0 | 26 / 38 |
| tests/dirkey.rs | 6 | 6 | 6 | 0 | (all) | - | 0 | 9 / 12 |
| tests/fuzz.rs | 5 | 5 | 5 (see below) | 0 | 0 | - | 2 | 3 / 2 |
| tests/mapped.rs | 1 | 1 | 1 | 0 | (all) | - | 0 | 2 / 1 |
| tests/segwriter.rs | 19 | 0 | 0 | 0 | 0 | - | untouched | 0 / 0 |
| tests/blob.rs | 14 | 0 | | | | | untouched | |
| tests/crypt.rs | 8 | 0 | | | | | untouched | |
| tests/dict.rs | 4 | 0 | | | | | untouched | |
| tests/ranges.rs | 6 | 0 | | | | | untouched | |
| tests/forged.rs | 1 | 0 | | | | | untouched | |
| tests/resident.rs | 1 | 0 | | | | | untouched | |
| **total** | **639** | **581** | **83** | **498** | **267** | **167 / 87 / 13** | **231 (+2 fuzz)** | **415 / 292** |

Notes on the table:

- **tests/fuzz.rs.** The five tests all reach `with_cipher` (tests/fuzz.rs:416-428), which sets a
  cipher only when `under_cipher()` is true.
  - `fuzz_range` (tests/fuzz.rs:1242) and `concurrent_range` (tests/fuzz.rs:1578) run their stores
    in the clear unless `ZFUZZ_CIPHER` is set. They would run under conform: both their `z` and `r`
    stores are substituted.
  - The three `*_under_a_cipher*` tests (tests/fuzz.rs:1252, 1264, 1277) set a cipher on `z`.
    Their plain `r` would be substituted too, which is harmless: the differential then compares two
    paged stores against the model.
  - The fuzz crashes by `drop` and reopen, never by copying files (tests/fuzz.rs:1070-1071), so it
    has no opt-out.
- **The 83 tests that set a cipher** (cipher 24, dirkey 6, durable 42, fuzz 5, mapped 1, zpages 5).
  Leave them out of conform wholesale, except the two plain fuzz tests: 81 opt-outs.
  - They test the paged path already.
  - Some carry plain controls that would break once substituted:
    - tests/cipher.rs:191-205 requires the plain store's keys to be readable in its files;
    - tests/cipher.rs:256-268 requires a cipher on a plain store to be refused "in the clear";
    - src/db.rs:44228 asserts the plain arm's `REST_HERE`/`LOOKS_NONE` words.
  - Whether each of durable.rs's 42 two-arm tests would pass with its plain arm substituted is not
    verified.
- **Unit tests in src/.** 474 unit tests by `grep -c '#\[test\]' src/*.rs src/db/*.rs`. Two open a
  store, both in src/db.rs:
  - `the_forms_word_under_a_cipher_is_a_rest_and_stays_one` (src/db.rs:44228) asserts the plain
    arm's internals and must opt out;
  - `a_writer_that_has_lent_its_upkeep_leaves_the_sweep_to_the_thread_that_has_it`
    (src/db.rs:44590) would run.

  The rest (zselect 151, zpages 59, zfile 46, crypt 24, flatindex 21, ordindex 19, and others) open
  no store and are untouched.
- **forms.md's estimates, redone:**
  - tests/zpages.rs: 136 tests, 245 lines with `std::fs::` (93 without `remove_dir_all` and
    `create_dir_all`); 102 opt-outs + 5 cipher.
  - tests/db.rs: 226 tests, 128 lines with `std::fs::` (118 without); 46 opt-outs.
  - tests/segwriter.rs: 19 tests, 41 `std::fs::` lines (32 without), and **0 open a store**. It
    writes segments with `SegmentWriter` and reads them with `Blob` by path. Conform does not
    change it at all and needs no opt-out there.
  - tests/durable.rs: 305 `std::fs::` lines, the largest file by that count.

  Command: `grep -c 'std::fs::' FILE` and `grep 'std::fs::' FILE | grep -vc 'remove_dir_all\|create_dir_all'`.
- **Crash states built by copying and tearing:**
  - tests/durable.rs: **125 tests** reach `copy_store`, `power_cut`, `power_cut_logs`,
    `ss_copy_with_lost_pages`, `os_copy` or `zero_prefix` (`crash.py`). 37 of them also run a
    cipher arm.
  - tests/db.rs builds its crash windows by writing the WAL, the manifest and segments in place.
    These are the 31 "bytes" opt-outs, for example tests/db.rs:215, 324, 2960, 3300, 14847 and
    16379. Only 1 reaches its own `copy_store` (tests/db.rs:13646, used by tests/db.rs:13671).
  - `copy_store` in tests/durable.rs:43 copies whatever regular files are in `d`. Under a cipher it
    copies the envelopes, the log and the manifest and loses the memfds, which is how the 37
    cipher arms already crash.
- **How tests make stores.** No helper in tests/common makes a store (`grep -n 'Db::create\|Db::open'
  tests/common/mod.rs` finds none). Each file has its own `dir()`/`scratch()` for the directory and
  calls `Db::create` directly: 415 `Db::create(` and 292 `Db::open(` call sites in tests/
  (`grep -c`). So:
  - An opt-out is one line per test, `common::plain_files_only("reason")` at the test's top, plus
    one line in `tests/data/conform-skips.txt`. That is about 267 + 81 + 1 lines.
  - Or one line per helper where the path work is in a helper. The helpers that put the most tests
    out (helper, file:line, tests whose body names it directly):
    - `form_files`, tests/zpages.rs:325, 81 tests;
    - `copy_store`, tests/durable.rs:43, 32;
    - `under_cipher`, tests/durable.rs:81, 41 (sets a cipher);
    - `wal_names`, tests/durable.rs:3050, 9;
    - `power_cut`, tests/durable.rs:1618, 9;
    - `run_ino`, tests/durable.rs:1643, 9;
    - `copy_dir`, tests/zpages.rs:673, 9;
    - `fence_keys`, tests/zpages.rs:342, 7.

    From `classify.py --helpers`. The count is of direct references.
  - A marker inside the helper, rather than at the test's top, is fragile: a test that reaches the
    helper only on one branch would opt out late, after its store was already created under
    conform. The opt-out has to run before the test's first `Db::create`.
  - `plain_files_only` cannot be a thread-local set by the test if the store is created on another
    thread. A process-wide switch is wrong too, because the harness runs tests in parallel. The
    workable shape is a thread-local read at `Db::create`. Tests that create a store on another
    thread are not counted; not verified.
- **The "observe" class (87).** These tests look at the store's shape by file name: partitions by
  `par-` name and fence (`common::seg_names` tests/common/mod.rs:51, `partition_fences`
  tests/common/mod.rs:70), form files by `zpg-` (`form_files`), WAL files by `wal-`, `ord-` names,
  sizes by `metadata`. A `#[doc(hidden)] pub fn Db::file_names(&self) -> Vec<String>` would return
  the logical names under either mode: in the clear the directory's names, under a cipher
  `Vault::names` (vault.rs:1131-1140, the open files' logical names plus the passing ones). A
  `Db::file_len(name)` would do the same for lengths (`vault::len`, vault.rs:2039). With these, and
  the helpers rewritten to use them, most of the 87 could run under conform. Each still has to be
  read for whether it then opens the file by path; not verified.
- **Tests that run under conform but may fail for non-layout reasons** (`suspects.py`, words
  advice, resident, madvise, mincore, fadvise, willneed, ZInject, anon, rss, budget):
  - tests/db.rs: 3438 and 3499 assert advice; 4266 counts the trim by resident bytes; 4981 is a
    `zpages_inject` store short of memory.
  - tests/zpages.rs: 2977, 3090 and 3172 count willneed asks out of core; 5574, 5652 and 5755 are
    `ZInject` controller tests.
  - The ten tests/db.rs hits on "budget" are the block cache's own budget and are probably fine.
  - Whether each passes under conform is not verified. They are the first to read when the first
    run is red.

---

## 4. What conform would hold on the remainder

The 231 tests that run, plus the 2 plain fuzz tests. Path counts are tests whose transitive body
names the path (`paths.py`):

| path | tests reaching it (of 231) |
|---|---|
| point read (`read_all`, `get`, `count`, `read_vec`, `values_at`) | 193 |
| scan (`.scan(`) | 167 |
| merges (`flush`, `settle`, `compact`) | 190 |
| a handle (`.reader(`, `Reader`) | 110 |
| a snapshot (`Isolation::Snapshot`, `snapshot(`) | 7 |
| reopen (`Db::open`) | 44 |
| the compact form forced (`ZPages::Always`) | 29 |
| an injected controller (`zpages_inject`) | 20 |
| threads | 40 |

The block cache is on by default (`scan_block_cache: true`, src/db.rs:4065), so most scans run through it. Not counted
separately.

### What it catches on reached paths

- **A reader that borrows whole.** Under a paged mapping `as_slice` is empty (bytes.rs:376-379),
  `lend_at` refuses (bytes.rs:481-488), and `Blob::flat` is `None`. A new reader that addresses
  into a whole borrow reads nothing and answers absent or 0. That is a wrong value in any test that
  compares values against a model or the plain answer. Most of the 193/167 do.
- **A file operation that bypasses the door.** The engine's directory is a regular file, so the
  call fails with "not a directory" (CLAUDE.md, "the engine reaches its files through `vault::`").
  Any new `File::open`, `std::fs::*` or `memmap2` call in form code on a store path becomes an error
  under conform. Today the door test (vault.rs:2876, `the_engine_names_no_file_but_through_the_door`)
  reads db.rs's source only. This is the strongest catch conform gives on today's vault.
- **A thread that reads with nothing pinned.** A debug assertion in `Paged::ensure`
  (pager.rs:1316-1320) panics, in every conform test that reaches such a thread.
- **A whole-file read that should have rested.** Under a small cache the cache's peak (`PagerCounts`)
  is readable. Asserting it needs tier 2.

### What it catches only with the eviction forcing of item 5

**A borrow kept across a rest.** At the default cache (64 MiB or more against test stores of a few
MiB) the evictor never runs, so nothing is given back and a stale borrow reads the right bytes.

Even with a small budget a stale borrow is seen only if all of these hold:

1. a pass ran between the rest and the reuse;
2. that pass marked the unit;
3. the second chance did not spare it. `mark` spares a unit with a chunk read since the hand last
   passed (pager.rs:664-705), and the resting thread's own unit was just read.
4. `SUPDB_PAGER_GUARD` turned the given-back pages to `PROT_NONE` (pager.rs:1236-1240), or the
   zeroes changed an answer.

### What it cannot catch

- **Advice that does nothing.** Advice through `flat()` on a paged mapping is a silent no-op.
  forms.md names blob.rs:1239, blob.rs:1276 and zfile.rs:3783 at 3df85d22; those lines were not
  re-located in this export. The answers are the same with or without the advice. Needs forms.md
  R3's per-site advice count.
- **Files with no envelope yet.** Their chunks are never evicted: `mark` skips a file with no
  `back` (pager.rs:694). These are a seal's output before `vault::persist`, a merge's piece before
  `Emitter::leave`, and an ordered run's file while `RunMap` maps it (`MappedFile::owed`,
  vault.rs:609-627). A borrow kept across a rest into those files is invisible to conform.
- **The 267 + 81 opted-out tests.** Every crash window built by hand is in them: 125 durable tests
  and 31 db.rs byte tests. So conform holds none of the plain crash windows under the paged path.
- **Out-of-core paths in the 211 tests with no injected budget.** `pieces_willneed`,
  `blocks_willneed`, `zpages_drop_raw`, the crossing and builds ahead all wait for the store to be
  out of core (forms.md §3 point 6).
- **Speed, and the plain store's cost.** Conform checks answers only.
- **Options no test sets** (item 5, G-options).
- **Concurrency outcomes that depend on timing.** Conform changes timing (the evictor thread,
  envelope writes at landings). A race may be hidden or exposed. Neither is a property of the
  change under test.

---

## 5. The hooks of tier 1 and tier 2

### `SUPDB_PAGER_GUARD` (`pager::guard_on`, src/db/pager.rs:1569-1578)

- Under `cfg(debug_assertions)` it reads the variable once into a `OnceLock`. Without the cfg it
  is a `const fn` returning false.
- It is used at two places:
  - `Paged::fill` (pager.rs:990-1001): `mprotect` READ|WRITE over the pages the chunk lies in,
    before the copy and decrypt;
  - `Paged::free` (pager.rs:1236-1240): `mprotect PROT_NONE` over the pages given back.
- Pages never filled are mapped `PROT_READ|PROT_WRITE` from the start (`map_shared`,
  pager.rs:1528-1545). So the guard makes a page unreadable only after it has been given back once.
- A chunk is 4080 bytes and shares a page with its neighbour, so after a fill of one chunk the
  other half of the page is readable unfilled (the doc comment at pager.rs:1558-1568 says so).
- forms.md's tightening ("PROT_NONE until filled") means:
  - map `PROT_NONE` at `map_shared` when the guard is on;
  - unprotect per fill;
  - accept that a page is readable from its first chunk's fill. With 4080-byte chunks it cannot
    be exact today. It is exact only with 4096-byte chunks, which is S3.

### The rests

Every place a reader says it holds nothing lent (call sites by `grep -rn PATTERN src`; the
enclosing function was found with an awk search back to the last `fn`):

- **`pager::Hold::rest`** (pager.rs:178-184), called at:
  - `zfile::...::passed` (src/db/zfile.rs:4571), the form build, a rest each page;
  - `MergeRest for &pager::Hold::step` (src/db.rs:12817-12825), every `MERGE_REST = 256` steps
    (src/db.rs:13284);
  - `Db::open` (src/db.rs:28954, 28985);
  - `z_sample_start` (src/db.rs:41727).

  The `MergeRest::step` call sites are in `merge_ranks` (src/db.rs:13135), `merge_keys`
  (src/db.rs:13222) and `compact_run` (src/db.rs:13642). The other `.step(` hits in that range are
  `steps.step("...")` timing stamps, and src/db.rs:41555 is `zsel.step`. `NoRest` is the plain
  type (src/db.rs:12809-12816). `compact_job` picks `vault::hold(&dir)` or `NoRest`
  (src/db.rs:12780-12792).
- **`Reader::rest`** (src/db.rs:21556-21569): `page_pin` and an answer of `crowded`. Called inside
  `abandon_slow` (src/db.rs:22670-22684), which reaches `build_rest` at src/db.rs:22676 and 22684.
- **`Reader::build_rest` / `build_rest_unless`** (src/db.rs:21594-21620). Callers:
  `abandon_slow` (22676, 22684) and `ahead_look` (src/db.rs:32359).
- **`Reader::scan_rested`** (src/db.rs:21652-21697). It rests every `SCAN_REST = 1024` keys
  (src/db.rs:13278), and only the outermost scan of a store under a cipher takes it (`scan_in`,
  src/db.rs:21699-21708).
- **`Reader::unlent`** (src/db.rs:21536-21550), 11 call sites:
  - `take_fresh_mem` 31231;
  - `land_seal` 32476;
  - `maint_request` 32574;
  - `hold_within` 35706;
  - `hold_for_upkeep` 35731;
  - `join_upkeep` 35761;
  - `answer` 38981;
  - `join_seal` 39524;
  - `land_front` 39561;
  - `join_tier` 40267;
  - `join_merge` 43735.
- **`Reader::unlend`** (src/db.rs:21626-21637), 5 call sites: `sync_wal` 28045, `retire_covered`
  39832, `retire_seg` 40311, `sync_owed_slot` 40816, `publish` 40933.
- **Operation start and end.** `paged_begin` (src/db.rs:19106-19129) breathes when the cache is
  swamped. `end_slow` and `leave_slow` (src/db.rs:19064-19095) run `Pager::check` and the
  snapshot's `page_idle`.
- **The two primitives everything above ends in:**
  - `Readers::page_pin` (src/db.rs:10737-10745), with `page_idle` and `page_back` at
    src/db.rs:10763-10773, for slots;
  - `Hold::pin` and `Hold::rest` (pager.rs:150-184) for holds.

### Where "evict at every rest" hooks

- In `Readers::page_pin` and `Hold::rest`, behind a `cfg(debug_assertions)` conform flag. Both are
  reached only when `shared.pager` is `Some`, so a plain store pays nothing even in the checked
  profile.
- **The hook cannot evict synchronously on the resting thread and wait.** The evictor's pass waits
  for every slot and hold pinned before its epoch bump (`Pager::pass`, pager.rs:605-663). A rest
  that waited for a pass would wait for every other pinned reader. CLAUDE.md records such a ring
  ending only at a timeout ("`settle` waited for the upkeep thread's pass pinned ... three waits in
  a ring"). The hook has to be asynchronous:
  - set a "give back all" goal;
  - turn off the second chance in `mark` for that pass, since a resting thread's own unit is the
    one most recently read;
  - `nudge` (pager.rs:475-486).
- **Sampled eviction at operation ends** belongs in `leave_slow` or `end_slow`.

### Cache sizes the cipher tests use

- tests/cipher.rs: `let cache: u64 = 2 << 20;` at tests/cipher.rs:375, 529, 648, 772, 1391, 1623
  and 1779, used at tests/cipher.rs:377, 535, 653, 782, 1394/1408, 1625 and 1783.
- tests/fuzz.rs: 16 KiB (`CIPHER_CACHE.set(16 << 10)`, 1266 and 1279), or `ZFUZZ_CIPHERCACHE`.
- tests/zpages.rs's five cipher tests (6777, 7227, 7268, 7592, 7651) set their own; not read.

### `one_at_a_time` (tests/cipher.rs:53-58)

- A process-wide `Mutex<()>` taken by the 9 tests that hold a cache to its budget plus a slack
  (`grep -c 'one_at_a_time()' tests/cipher.rs` = 9).
- The reason, from its doc: four such tests on four cores stalled each other's evictor, and the
  larger store's peak read 3% over its bound once.
- Tier 2's small-cache subset, if it asserts peaks, needs the same lock. Tier 1 asserts no peak
  and needs none.

### The `ZInject` seam (src/db/zfile.rs:4780-4930, `Options::zpages_inject` src/db.rs:1986)

- It is per store and lives in the options. Its fields are `budget`, `refault_rate`, `clock_ns`,
  `faults`, `fault_cost_ns`, `decode_cost_ns` and `records_resident`. A field left at its sentinel
  is read from the platform.
- What the engine reads through it (enclosing fns found by an awk search back to `fn`):
  - `budget`: `settle_room` (src/db.rs:24483-24507), `z_recode_look` (41142), `z_budget` (42622),
    `z_model` (42634);
  - `refault_rate`: `settle_room`, `z_recode_look`, `z_census` (42487), `z_model`;
  - `clock`: `z_now` (41499);
  - `faults`: `zcensus_faults` (19315);
  - `fault_cost`: `zwindow_close` (19378);
  - `decode_cost`: `z_recode_look`, `z_sample_collect` (41770);
  - `records_resident`: `z_covers_resident` (42157), `z_prebuild_next` (42186).
- An injected budget makes the controller see "out of core" today: no `Memory` interface is
  needed for that.
- Under a cipher today, though, the model's budget is of two caches in series and has never been
  held to a measurement (CLAUDE.md). So tier 2 under an injected budget would test that the
  controller buys and serves, not that it is right.
- Substituting `zpages_inject` for every store would change what the 211 tests without one assert
  (builds, installs). It belongs on a named subset.

### The fuzz's draws (tests/fuzz.rs)

- `pick_cfg` (tests/fuzz.rs:265-305) always draws:
  - `seal_bytes`, `partition_bytes`, `l0_trigger`, `inline_bytes`;
  - `scan_block_cache`, through `cache_asked` (tests/fuzz.rs:254-263), which is false unless
    `ZFUZZ_CACHE` is set;
  - `upkeep`, `publish_in_background`, `sync`;
  - ops, keys, big.
- Under `ZFUZZ_OPTVARY` it also draws `compact`, `promote`, `zpages_trusted_views`,
  `zpages_memo_first`, `zpages_block_cursor`, `zpages_record_coding`, `zpages_fence_tree`,
  `zpages_memo_ahead`, `scan_start_cache`, `zpages_store_all` and `zpages_raw_tails`.
- Environment flags set more: `upkeep_keep_bytes`, `scan_cache_auto_bytes`, `merge_parallel`,
  `recycle_wal`, `zpages`/`zpages_min_bytes`, `forms_settle_every`/`forms_settle_builds`/
  `scan_cache_bytes`/`forms_to_fit`, `zpages_parse`, `zpages_own_decoder`,
  `zpages_recode`/`zpages_recode_keep`/`zpages_both`/`zpages_drop_raw`, `promote_entries`,
  `zpages_promote_copies`, and `encryption`.
- `ZFUZZ_CIPHER` and `ZFUZZ_CIPHERCACHE` set `encryption` and `cache_bytes` (`with_cipher`,
  tests/fuzz.rs:416-428).
- **What the gate runs.** `scripts/check.sh test` runs `cargo test --profile checked`
  (scripts/check.sh:56-58) with no `ZFUZZ_*`, so each fuzz test runs `DEFAULT_SEEDS = 4` seeds
  (tests/fuzz.rs:105) with the block cache off and no option variation.
- **Coverage of `Options`.** `Options` has 220 `pub` fields (struct at src/db.rs:862-3919; regex
  `\bpub\s+([a-z_0-9]+)\s*:` over the struct with comments stripped). tests/fuzz.rs sets 38 of
  them, counting `o.FIELD =` and the fields of the `Options { .. }` literal in `opts`. **182 are
  set by no draw or flag**; they are listed in `options_not_drawn.txt`, the input to G-options.

---

## 6. Cost and risk

### Time today, by CLAUDE.md's account

- The test group's compile is "about a minute and a half" from nothing under `checked`, and under
  one after a one-line engine change.
- The tests take about four minutes ("thirty-seven [minutes] ... for four minutes of tests" before
  the compile was fixed). The test time itself has not been re-stated since; not verified.

### What a second run adds

- The run reaches the 231 tests and the 2 fuzz tests. 180 of the 231 are in tests/db.rs, the
  binary whose "longest tests run over a minute under `checked`".
- The slowdown is not measured. What a store under a cipher pays in core comes from CLAUDE.md's
  pairings with a real cipher: loads 0.84-0.92x, scans 0.80-0.94x, the scan mix 0.78-0.87x. An
  identity cipher removes the cipher's share, a tenth of a miss by the profile CLAUDE.md quotes.
  Envelope writes at every seal and merge, and memfd copies, remain.
- With eviction forced at rests, every rest is a nudge and every refetch is a copy of the chunk
  from the envelope's mapping. The cost per rest is unknown.
- A first estimate is a second run of 4-8 minutes. That is a guess, not verified.

### Watchdogs and time bounds in the tests that run

- 5 of the 231 reach `Watchdog::arm` (tests/db.rs:13337-13360). It is 60 s, and it ends the binary
  with code 101 through `watchdog_exit` (tests/db.rs:13319) with no `test result` line.
- 61 of the 231 contain a `from_secs(` or `elapsed() <`/`>` bound: db.rs 36, zpages.rs 19,
  durable.rs 6 (`paths2.py`). Bounds seen: `from_secs(1)` ×22 in tests/zpages.rs, 20, 30, 60
  and 120.
- A slower store trips these first. Per CLAUDE.md, a 101 with no summary is read as a watchdog
  before anything else.
- `cargo test` stops at the first failing binary. The conform group should pass `--no-fail-fast`.
  The default `test` group does not (scripts/check.sh:58).

### Memory and threads

- Every conform store holds each segment in a memfd until its envelope is written, plus a pager
  thread.
- The harness runs tests in parallel. With `default_cache` the budget is large and nothing is
  given back, so a test's memory is its store's size twice over (memfd and envelope in the page
  cache) at the most. This is an inference; not verified.

### Where it belongs

- A group of its own in scripts/check.sh, for example `conform`: `SUPDB_CONFORM=1 cargo test
  --profile checked --no-fail-fast`. It needs no rebuild, because the binaries are the `test`
  group's.
- A CI job of its own, out of the default set until its time is measured. This is the treatment
  `quick` and `fscrypt` get (scripts/check.sh:26-33).
- Like `fscrypt` (scripts/check.sh:96-106), it must prove it ran: assert a count of tests run per
  binary against `--list`. Per CLAUDE.md, a run of no tests is green.

---

## 7. Recommendation

**Build tier 1 on today's vault now. Do not build the crash-state layer API on today's vault.** The
numbers from item 3:

### Why tier 1 now

- The parts tier 1 exercises are the read contract: `Pieces`/`slice_at`, `Bytes::piecewise`,
  the holds, the rests, `unlent`/`unlend`, `paged_begin`/`end_slow`, the door's decoy directory and
  `Paged::ensure`'s pin assertion. design.md §6 lists every one of them under "Stays" through S3.
  A conform pass built on them now keeps working when the page layer replaces the envelope arm.
- Its cost is bounded and known:
  - the substitution at two match arms (src/db.rs:28356, 28691);
  - an identity `Aead` in library code under `cfg(debug_assertions)`;
  - a reserved key id read with `vault::key_id` at open;
  - `common::plain_files_only` with `tests/data/conform-skips.txt`, about 349 lines: 267 plain
    opt-outs, 81 cipher tests, 1 unit test;
  - an asynchronous "give back all, no second chance" mode in `Pager::mark`/`pass`, nudged from
    `Readers::page_pin` and `Hold::rest`;
  - the tighter guard (`PROT_NONE` at `map_shared`).
- It holds 231 + 2 tests that open a store: 193 reach point reads, 167 scans, 190 merges, 110
  handles and 29 the compact form. Through the decoy it also holds every file operation in form
  code to the door, which today's text test does not.

### Build the name accessor with it

- A `#[doc(hidden)] Db::file_names` and `file_len` over `vault::names`/`vault::len`.
- The 87 "observe" opt-outs could then run. That moves the run set from 231 to as many as 318 of
  498 plain tests (64%), with no dependence on the envelope format.
- Whether each of the 87 needs nothing else is not verified.

### Do not build the crash-state API now

- The 167 "bytes" opt-outs read or tear a file's bytes at offsets of the plain format: WAL frames,
  manifest slots, segment sections, form pages.
- Under today's vault the log and manifest are sealed by the engine (decision 2 moves them behind
  the door at S6), segments are memfds until their envelope, and a crash loses the memfds.
- A "discard since sync, tear" API built now would be built against the envelope arm that S3
  deletes, and built again at S3. 125 of these tests are durable.rs's crash windows. They should
  wait for S3's `pagefile` and its fault-injection mode (design.md §6, "New").

### Tier 2

- **The fuzz half is buildable now.** The two plain fuzz tests run under conform with `cache_bytes`
  at 16 KiB, with `ZFUZZ_CACHE=all` and `ZFUZZ_OPTVARY` in a CI job. The poison draw, an identity
  `Aead` whose `decrypt` returns false at the k-th call, needs no new seam: the pager poisons on a
  failed decrypt (pager.rs:1004-1018) and every later operation answers the error.
- **The injected-budget subset is buildable through `ZInject`.** But under a cipher it tests a model
  nothing has held to a measurement, and `cipher_controller` must be bypassed. Leave its assertions
  on what the controller buys until G3 builds `Memory`.
- **"A cache of 4 pages" cannot be expressed on today's pager.** It gives back 1 MiB units with
  4-6 MiB of slack before anyone waits. 16 KiB is the working minimum. An exact small cache is S3's.

### What forms.md should change

- G2's opt-out estimate is 267 plain-store opt-outs out of 498, plus 81 cipher tests. It is not
  "about 250 references in zpages.rs and 149 in db.rs".
- tests/segwriter.rs is outside conform entirely: it opens no store.
- `vault::resolve` is new code, not an existing function.
- The pass's catch of "a borrow kept across a rest" depends on forced eviction ignoring the second
  chance. It cannot see files that have no envelope yet.
