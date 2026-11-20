# Check of door/notes.md (G1, rule R1, gate G-door), at 682b2c09

All paths are relative to the export
`/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/tree`.
Nothing was built or run. My own scripts and outputs are in `door/check/`
(`tcount.py`, `thr.tsv`, `hits_rerun.tsv`, `g1_rerun.tsv`).

## Verdict

**Stands with corrections.** No class (a) hole exists, and I found a
second barrier the notes did not name. The library site list is complete
for what the proposed clippy entries bar. Thread counts reproduce exactly.
The corrections are these:
- several counts are wrong;
- one Rust rule is stated wrong (an inner `#![allow]` in `tests/common/mod.rs`);
- the ratchet in 4.5 can be escaped by four attribute spellings it does not count;
- the red input's expected diagnostic count is probably wrong;
- `MmapBytes::open` is barred by the proposed config but was not in any
  scan pattern, so the class (d) line lists are short;
- forms.md's own line numbers for R12 are stale;
- the notes did not mention the second existing source test (zselect.rs:3539).

The notes' two departures from forms.md need the owner's word, because
decisions.md item 9 says the contract "is adopted whole":
- the thread half is put off to S1;
- the lint runs on `--lib` only, and a source test is kept.

## 1. Reruns of the scout's own searches

- `python3 -I scan.py ../tree pats.json`: 703 lines, byte-identical to
  `hits.tsv`. `g1pats.json`: 273 lines, identical to `g1.tsv`.
- **Wrong:** "g1_outside.tsv rows with test flag '-' (32) minus 3 = 29".
  `awk -F'\t' '$3=="-"' g1_outside.tsv | wc -l` gives **30**, not 32. Minus
  the three `Reader::advise` false hits (db.rs 21157, 21718, 27778; I read all
  three, and they are `self.advise(..)` on `fn advise` at db.rs:20086), that
  leaves **27**, not 29. The 24-item table also covers db.rs:15836
  `libc::dlsym` (`heap::mallinfo2`), which `g1pats.json` does not match. So:
  27 approximated lines plus 1 dlsym line = **28 library lines over 24 items**.
  The item counts, 17 in db.rs and 7 in zfile.rs, check out line by line.
- The libc inventory reproduces:
  `grep -rohE 'libc::[a-z_0-9]+\(' src | sort | uniq -c` gives madvise 12,
  posix_fadvise 9, mincore 6, syscall 4, sync_file_range 4, sysconf 4,
  mmap 2, munmap 2, mprotect 2, clock_gettime 2, and 1 each of pwritev,
  memfd_create, getrusage, ioctl, dlsym, getrandom, malloc_trim, mallopt
  and sched_setscheduler. These counts include test sites, for example the
  posix_fadvise at zfile.rs:7461. The notes do not say that.
- The `cfg(debug_assertions)` attribute count reproduces: 21 (pager.rs 12,
  db.rs 9). I read the nine db.rs regions (8969, 10264, 10276, 10325, 10341,
  14220, 14229, 19220, 24676). None holds a barred call.
- The existing door test is as the notes say: vault.rs:2876-2905, db.rs
  lines 1-43818, 13 barred substrings, and the ordindex check (ordindex.rs:403
  holds the signature it looks for). The notes left one item unverified:
  whether a `//` inside a string literal in db.rs hides a barred call.
  **Now verified: none.** `grep -n '"[^"]*//[^"]*"' src/db.rs` returns nothing.

## 2. Different searches for the same things

- **Library code outside the allowed modules**, by
  `grep -nE '\bfs::|OpenOptions|File::|memmap2|libc::|"/proc|"/sys|"/dev'` per
  file. The extra db.rs hits are false hits: 6840 and 6845 (`libc::EIO`),
  15897 (`libc::c_int`), and 33282, 33387, 43229, 43276, 43347
  (`zfile::ZFile::attach`). zpages.rs, zselect.rs, zlz4.rs, blob.rs,
  flatindex.rs, block.rs, index.rs, reserve.rs, wasmapi.rs, format.rs and
  lib.rs have no library hit. **No library site is missing from 2.2.**
- **Path methods.** I ran `\.(exists|try_exists|is_file|is_dir|is_symlink|metadata|symlink_metadata|read_dir|canonicalize|read_link)\(`
  over db.rs, zfile.rs, zpages.rs, zselect.rs, ordindex.rs, blob.rs and
  flatindex.rs, with test regions excluded. There are 4 hits: db.rs 4524,
  28772, 32774 and zfile.rs 3975. All four are `File::metadata` on a door
  handle, which the config does not bar. There is no `Path::` file query in
  library code.
- **The same type under other spellings.** I searched
  `\bMmap(Mut|Raw|Options)?\b` with no `memmap2::` prefix required, and
  `from_raw_fd|into_raw_fd|OwnedFd|Command::|env::temp_dir|std::process`.
  Library code outside the allowed modules has no further site.
- **`extern` blocks.** `grep -rn 'extern\b' src`: the only block is
  wasmapi.rs:47. The only other foreign item is the type at db.rs:15829.
  There is no `asm!`.
- **Use lines.** I grepped every `use` of fs, os, io, thread, mpsc, Condvar,
  path, libc and memmap2. No fs, libc, memmap2 or thread item is renamed.
  **Missed:** I also parsed multi-line `use` statements
  (python, `\buse\s+[^;]*;` with `re.S`). That finds two renames the notes'
  one-line grep could not see: `lib.rs:109`
  `db::vault::{key_id as cipher_key_id, rewrap as cipher_rewrap}`. They are
  public re-exports of door functions that write the marker. They do not
  matter to clippy, which resolves by definition. They do matter to any
  *text* rule that names `vault::rewrap`.
- **The crate-root re-export of the plain mapping.** `lib.rs:95` has
  `pub use bytes::MmapBytes`, and test code calls `crate::MmapBytes::open`
  through it (zpages.rs:7519, 7520, 7552, 7553). Clippy resolves the re-export
  (inferred). A text test that looks for `bytes::MmapBytes::open` would not.
- **Macro bodies.** The five `macro_rules!` (blob.rs:779, 882, 941;
  wasmapi.rs:332, 343) hold no barred call. Agreed.
- **Threads.** I wrote my own scanner (`check/tcount.py`). It blanks
  comments and strings, then brace-matches each `#[cfg(test)]`. Its library
  counts for db.rs are: join 25, of which 39280 is the crate's `ZJob::join`,
  so 24; spawn 22, of which 6288, 6766, 38517 and 39659 are the crate's
  `spawn` fns, so 18 (10 `Builder::spawn` plus 8 `thread::spawn`); sleep 10;
  park 2; park_timeout 16; recv 3. That is **73**, as the notes say.
  pager.rs has 6 and flatindex.rs has 4. My first pass got pager.rs wrong,
  because I missed `cfg(all(test, target_os = "linux"))` at pager.rs:1595;
  the scout's scanner handles that form. On `Receiver` used as an iterator
  (`for .. in rx`, `rx.iter()`), which the notes left "not verified beyond
  pattern 8": I grepped `\brx\b` in db.rs. Every receiver is used by
  `recv`, `try_recv` or a field type. There is no iterator use. **Now
  verified.**

## 3. Counts corrected

| claim in notes.md | correct | how |
|---|---|---|
| "19 `tests/*.rs` files", "22 crate roots" (0, 4.3, counts) | **18** test files, **21** crate roots | `ls tests/*.rs \| wc -l` is 18: blob, checksums_off, cipher, crypt, db, dict, dirkey, durable, fences, forged, fuzz, leak, mapped, ranges, resident, segwriter, settle, zpages |
| "18 of the 19 hit directly" | 17 of 18 hit directly; resident.rs only through `tests/common/mod.rs` (11 hit lines) | grep per file (my pattern: `std::fs\|fs::\|File::\|OpenOptions\|memmap2\|libc::\|\.exists\(\)\|MmapBytes::open\|thread::\|\.join\(\)`) |
| "`tests/common/mod.rs` ... would cover the 15 crates that include it" | **14** include it (`mod common`), and see the Rust error in section 4 | `grep -c 'mod common'` |
| g1_outside library rows 32, after false hits 29 | 30, after false hits 27, plus 1 dlsym line | section 1 |
| zfile.rs test code "61 lines" (2.3) | 76 distinct lines (80 hits: fs 53, fsmeth 18, procsys 5, fileext 3, libc 1), **plus 6 `MmapBytes::open` lines no pattern could see** (5615, 7431, 7481, 7527, 8196, 8372) | `awk` over hits.tsv; `grep -rn 'MmapBytes::open' src` |
| zpages.rs tests "3 lines (7507, 7508, 7630)" | 7 lines: add 7519, 7520, 7552, 7553 (`crate::MmapBytes::open`) | same grep |
| db.rs tests "11 lines" | 13: add 43877, 43916 (`crate::bytes::MmapBytes::open`) | same grep |
| bytes.rs tests "2 lines" | 3: add bytes.rs:656 (`MmapBytes::open`). The module allow covers it either way | same grep |

The class (d) corrections change no module count under option A. Each
module is on the list already: db.rs `streaming_writes`, zfile.rs `tests`,
zpages.rs `tests`. They do show that the scan patterns in 2.1 do not cover
one entry of the proposed config, `supdb::bytes::MmapBytes::open`. The
pattern `\bMmap(Mut|Options|Raw)?\b` does not match `MmapBytes`. Add
`MmapBytes::(open|from_map)` to the scan.

## 4. What is wrong

1. **The Rust rule about `tests/common/mod.rs` (4.3).** The notes say "a
   `#![allow]` inside `tests/common/mod.rs` would cover the 15 crates that
   include it". It would not. An inner attribute at the top of a module file
   applies to that module (`common`) only, never to the crate root that
   declares `mod common;`. This is the language's documented attribute
   scoping. The 17 test files that name barred items themselves need their
   own `#![allow]`. Only resident.rs, which hits only through `common`,
   would be covered. Under option A the count is still 18 + 3 = 21 crate
   roots, as above.
2. **The ratchet in 4.5 can be escaped.** It counts the text
   `allow(clippy::disallowed_`. Each of the following silences the lint at
   an item and passes that count:
   - `#[expect(clippy::disallowed_methods)]` (`expect` is stable since Rust
     1.81);
   - `#[allow(clippy::style)]` (both lints are in clippy's `style` group,
     from clippy's lint list, not verified on CI's toolchain);
   - `#[allow(clippy::all)]`;
   - `#[allow(warnings)]`.

   A source attribute overrides a command-line `-D` (only `-F`/forbid
   cannot be overridden, and forbid would also refuse the door's own
   allows). The ratchet has to count, per file, any `allow(` or `expect(`
   naming `clippy::disallowed_`, `clippy::style`, `clippy::all` or
   `warnings`, inside `cfg_attr` too. Its red inputs should include one
   `expect` form and one group form.
3. **The red input's expected count (4.6, step 4).** It asks for "one
   `disallowed` diagnostic per inserted line, six". The line
   `let _ = memmap2::MmapOptions::new();` names `MmapOptions` as a path
   segment, which `disallowed_types` checks, and calls `MmapOptions::new`,
   which `disallowed_methods` checks. So that one line probably gives two
   diagnostics: seven in all. This is inferred from clippy's checking of
   `QPath::TypeRelative` self types, not verified. The check should require
   at least one diagnostic *on each inserted line*. It should not require a
   total.
4. **The mechanism in 2.2's `swap_ahead` argument.** The notes say
   `record_region_span` refuses because `self.src.slice_at(0, 1)` fails
   (blob.rs:1414). It returns `None` one step earlier. `record_regions()`
   calls `flat()` (blob.rs:1018-1023). That calls `Sec::get` → `lend_at`
   (blob.rs:504-508). For a piecewise source the section is always
   `Sec::Lent` (blob.rs:489-494), and `lend_at` refuses a paged mapping. The
   conclusion is unchanged. **There is also a second barrier the notes did
   not name.** Even if `record_region_span` answered `Some`, `z_cross`
   calls `vault::open(self.dir.join(&s.name))` (db.rs:42443) before
   `swap_ahead`. For a kept segment with its envelope that is `Err`
   (`reopen_written`, vault.rs:1113-1126), and the loop `continue`s. So
   `File::open(form)` at zfile.rs:3972 is unreachable under a cipher for two
   independent reasons. If the decoy were somehow reached, it would also
   fail closed.
5. **Small slips:**
   - db.rs:4777 is inside `fn drop_synced` (db.rs:4771), a call on
     `&self.file`, as the notes say (checked);
   - A1 lists no `mincore` in crypt.rs, but crypt.rs:2000 has one (test
     code, so A2);
   - "`--profile checked` ... 21 attributes" is right, but `cfg!(debug_assertions)`
     and `debug_assert!` (97 lines in db.rs) are type-checked under
     `--release` already, so only the 21 attribute regions are new to the
     door run.

## 5. What was missed

- **A second source test already bars files, in zselect.rs.**
  `the_selector_names_no_clock_thread_file_or_engine_type` (zselect.rs:3539-3567)
  reads zselect.rs up to its first `#[cfg(test)]`. It bars `std::thread`,
  `std::fs`, `std::io`, `std::sync`, `std::env`, `std::process`, `libc`,
  `crate::`, `super::`, `unsafe` and `static`. It is a stricter door for
  that one module. The rewrite of vault.rs:2876 must keep it or subsume it,
  and the notes should list it in section 1.
- **forms.md's R12 line references are stale at this tree.** It cites
  `mincore (db.rs:14936-14958)` and `/proc/thread-self (db.rs:13062, 13074)`.
  At 682b2c09 these are `resident_share` at db.rs:15796-15813 and
  `thread_minflt` and `thread_majflt` at db.rs:13881 and 13893.
  zfile.rs:4942-4950 still matches. The `read_budget` range is 5210-5270
  here against forms.md's 5199-5270. Builders following forms.md's numbers
  will land in the wrong code.
- **4.5's list of `/proc` and `/sys` literals is incomplete.** It omits the
  zfile.rs test literals 5293, 5300, 5305, 5319 (`budget_from`'s fake
  machine) and 5450 (`"/proc/thread-self/stat"`). It also omits the
  `"/process_api/..."` at zselect.rs:2485, which does not match `"/proc/`.
  The literal rule has to exempt test regions or list these lines.
- **Option A's other crates.** `web/floor/Cargo.toml` is a fourth
  manifest. The notes list only root, bench and experiments/ffi/capi.
  Nothing runs clippy in web/floor or in experiments/ffi/capi:
  `grep -n clippy web/build.sh experiments/ffi/run.sh` finds nothing.
  So the "empty clippy.toml" for capi (and for web/floor) is needed only if
  someone lints them by hand. bench needs one, because
  bench/scripts/check.sh:45 runs `cargo clippy --release --all-targets`.
- **Costs of option B that the notes did not price:**
  - `-D warnings` on a new profile. The door run lints the whole crate
    with every default clippy lint and rustc warning, under
    `debug-assertions = true`, which no `-D warnings` run has seen. Any
    existing warning inside the 21 `cfg(debug_assertions)` regions fails
    the door run for a reason that is not the door's. Either take one
    clean run before the gate lands, or narrow the run to the two lints,
    for example
    `-- -A clippy::all -D clippy::disallowed_methods -D clippy::disallowed_types`
    (lint-flag ordering, not verified). Rustc's own warnings would still need
    a decision.
  - `CLIPPY_CONF_DIR=scripts/door` is relative. Clippy-driver reads it in
    whatever working directory cargo gives rustc. Use `"$PWD/scripts/door"`.
    Not verified that the relative form fails; the absolute one cannot.
  - Disk. `target/door`, and the red step's copy with its own target
    directory, are two more check builds of supdb and its three dependencies.
    CLAUDE.md records the disk allowance as the first thing to fail. The red
    step should remove its copy and target when it ends. Size not measured.
  - Arch. Option B runs on both Linux runners, x86_64 and
    `ubuntu-24.04-arm`. Whether `libc::open64`, `mmap64`,
    `posix_fadvise64`, `fallocate64` and `ftruncate64` resolve on
    aarch64-linux-gnu in the `libc` version in Cargo.lock is not verified.
    If a newer clippy reports unresolvable config paths under `-D warnings`,
    the arm leg fails. Drop the `*64` names. No code uses them, and none is
    in R1.
- **Departures from the adopted contract.** decisions.md item 9 says
  "The contract of `forms.md` §3 is adopted whole". forms.md's G-door at
  G1 lists the thread calls and says "This replaces today's text test". The
  notes recommend three changes:
  - the thread entries go at S1, with a `std::thread::` count in the
    meantime;
  - `--lib` only;
  - keeping a source test.

  Each may be right. Each is a change to an adopted contract and goes to
  the owner, stated as such. They are not settled by the notes.

## 6. Sampled sites (19, read with context in the export)

Every classification I sampled is right.

| site | function | notes' class | read |
|---|---|---|---|
| db.rs:28348 | `Db::create` | (b) | right: `create_dir_all(dir)` runs before `opts.encryption` is taken; the real directory |
| db.rs:12167 | `struct RunMap` | (b) | right |
| db.rs:12199 | `RunMap::open` | (b) | right: the file is from `vault::open_mapped`, and `_held` keeps the memfile |
| db.rs:6567-6577 | `pwritev_at` | (b) | right; `#[inline(always)]` on the writer path, so an attribute-only allow changes no instruction |
| db.rs:6689-6700 | `start_writeback` | (b) | right |
| db.rs:9791-9810 | `populate_ahead` | (b), anonymous | right |
| db.rs:13581-13608 | `compact_run` | (b) | right: `vault::open(..).ok()`, which is `None` for an envelope |
| db.rs:13880-13901 | `thread_minflt`, `thread_majflt` | (c) | right |
| db.rs:15796-15813 | `resident_share` | (c) | right; its caller at db.rs:8957 goes through `record_regions`, so it is plain only |
| db.rs:15829-15840 | `heap::mallinfo2` | (c) | right; one allow on `mod heap` (db.rs:15828) would cover it and `resident_beyond_use` |
| db.rs:15858-15860 | `heap::resident_beyond_use` | (c) | right |
| db.rs:305-325 | `idle_io_priority`, `io_priority_class` | not a file | right |
| db.rs:10438-10457 | membarrier pair | not a file | right |
| db.rs:8919-8928 | `Seg::drop_raw_pages` | (b) / R3 | right |
| zfile.rs:3926-3941 | `drop_written_range` | (b) | right; callers 3858 and 4723 |
| zfile.rs:3957-3989 | `swap_ahead` | (b) | right, with the two barriers in section 4.4 |
| zfile.rs:4002-4045 | `drop_behind`, `ask_ahead` | (b) | right |
| zfile.rs:2003-2023, 3255-3259 | `ZFile` attach, `drop_cache` | (b) | right |
| zfile.rs:4198-4207, 4262-4265, 4379-4381 | `SegSrc`, `open_seg`, `build_file_seen` | (b) | right: `vault::kept(path)` (vault.rs:2197) chooses the arm |
| zfile.rs:4942-4950 | `thread_major_faults` | (c) | right |
| zfile.rs:5210-5270 | `read_budget`, `budget_from` | (c) | right; the allow on `read_budget` covers the closure. `budget_from` only calls the closure it is given, so it needs no allow |

## 7. Feasibility

I tried to find a case that breaks the conclusion.
- **A hole.** None found, by the searches in section 2. Under a cipher,
  every handle outside the door comes from `vault::open`, `vault::create`
  or `vault::open_mapped`, and every by-path call fails at the decoy
  (vault.rs:667-673, 681-683, 1892-1898).
- **A plain-store cost.** None. Attributes and a config file change no
  instruction. The optional swap of `File::open(form)` for `vault::open(form)`
  adds one atomic load (`VAULTS`), once per partition at the crossing, which
  is not a per-key path.
- **A test that can no longer run.** None. Option B adds a lint run and
  leaves the test group alone. The rewritten source test still runs on
  macOS, where the clippy door is skipped.
- **The gate itself.** The real weakness is the ratchet escape in section
  4.2. Without it, `#[allow(clippy::style)]` on a zfile.rs function puts a
  file call past the door with the gate green. With the wider ratchet, G1
  can land as the notes describe at 6 module allows and 24 item allows,
  with no engine change.

## 8. Not verified

- Every clippy behaviour the notes mark as inferred is still unverified:
  - path resolution of libc's glob re-exports, of foreign fns, and of
    crate-local `supdb::...` paths;
  - flagging of function values;
  - reporting of unresolvable paths;
  - `CLIPPY_CONF_DIR` in cargo's fingerprint;
  - that both lints are in `style`.
- Two diagnostics on the `MmapOptions::new()` line (section 4.3).
- That `posix_fadvise` and `sync_file_range` on a memfd do nothing. My
  inference agrees, from the kernel's no-writeback backing for shmem, but I
  did not read the kernel source.
- The door run's time and disk.
- That no warning exists today in the `cfg(debug_assertions)` regions under
  clippy's default lints.
