# Check of borrows/notes.md

Export read: `scratchpad/plan/tree` (682b2c09). Paths are relative to it.
Nothing built or run. "Inferred" means from the language's or the tool's
documented behaviour, not compiled.

## 0. Searches rerun, and the other ways in

Rerun and matching the notes:
- `grep -rn from_raw_parts src | wc -l` = 44. Per file: bytes 5, blob 1,
  wasmapi 14, pager 5, blocks 2, zpages 6, crypt 3, db 6, flatindex 2. Matches.
- `grep -rn 'impl.*Bytes for' tests examples bench/src bench/examples experiments --include=*.rs | wc -l`
  = 11 (examples 1, tests 10). A multi-line search
  (python `impl\s*(<..>)?\s*[\w:]*Bytes\s+for` with DOTALL, all `**/*.rs`)
  finds no impl header split over lines. `bench/tests/*.rs` (not in the
  notes' dirs) has none. Matches.
- `grep -rn lend_at --include=*.rs .`: definition bytes.rs:79, override
  bytes.rs:481, one call blob.rs:507 (`Sec::get`). No outside use. Matches.
- `grep -n 'indexed!(' src/blob.rs` = 8. Matches.
- `grep -n '\.flat()\|\.pieces()' src/blob.rs` = 17. Matches. That pattern
  cannot see `key.pieces(&src)` / `blk.pieces(&src)` (blob.rs:965, 980, 989,
  1037, 1044): `grep -n '\.pieces(&\|\.get(&src\|\.get(&self\.src\|blk\.get\|key\.get' src/blob.rs`
  finds them; the notes list them under `Sec` anyway.
- Other ways to build a slice from a pointer:
  `grep -rn 'slice_from_raw_parts\|read_unaligned\|ptr::read(\|as \*const \[\|as \*mut \[\|\.cast::<\['`
  finds only zlz4.rs:733 (`read_unaligned` inside the LZ4 decoder, over a
  buffer it is handed). No missed slice construction.
- `grep -rn 'memmap2\|MmapOptions' src`: besides bytes.rs and RunMap
  (db.rs:12167, 12199), pager.rs:720/736 (the pager's own `memmap2::Mmap`
  of the envelope's ciphertext, a whole file mapping, inside the pager) and
  zfile.rs:4264 (`memmap2::Mmap::map(&file)` in `build_file_seen`, a plain
  build's whole mapping made outside `vault`). Neither is in the notes'
  1.6 table. Both are inside modules that may hold them, but zfile.rs:4264
  is a G-door site.
- `grep -rn 'slice_at(' src | grep -v 'fn slice_at'` = 58 sites. See 2.A:
  this class is the one the notes do not treat.
- `grep -rn 'scan_either\|scan_whole\|scan_refused\|macro_rules!' src`:
  `by_piece` does not exist in this export. `scan_either` call sites
  (D, not listed in the notes): db.rs:20689, 20743, 20874, 29520;
  zfile.rs:2730. Macros: blob.rs `walk_fixed!` 779, `emit_record!` 882,
  `indexed!` 941; wasmapi.rs `on!` 332, `any!` 343 (no section borrow).
- Outside callers: `grep -rn` of each item over tests, examples,
  bench/src, bench/examples, bench/tests, experiments, web. experiments/ffi
  uses only `Db, Options, SegmentOptions` (capi/src/lib.rs:24). bench/src
  names `Blob<MmapBytes>`, `Blob<Encrypted<MmapBytes,Gcm>>`, `MmapBytes::open`
  (engines.rs:9445-9641), none of the items being made private.

## 1. What is wrong

1. **Count "non-test in-crate uses of MmapBytes::as_slice = 7".** The notes'
   own list has six: ordindex.rs:419, 678, 940; zfile.rs:3784, 3802, 4266.
   `grep -rn 'as_slice()' src`, each hit's receiver read: the other hits are
   `Vec`s (vault.rs:870, 2761, 2842 included). Correct count: **6**. Test
   uses: bytes.rs:659, vault.rs:2409, zfile.rs:7482, 7483, 7492, 7494, 7498,
   7501, 7507, 7529, 7530 = **11**.

2. **"Neither choice confines `lend_at` to `blob`: the token constructor is
   `pub(crate)` at most" (notes 3.2).** Wrong, inferred from the
   visibility rules. Put the token type in `blob`, not in `bytes`:
   `pub(crate) mod lend { pub struct Token(()); impl Token { pub(in crate::blob) fn new() -> Token { Token(()) } } }`
   inside blob.rs, and `fn lend_at(&self, off: u64, len: usize, _: crate::blob::lend::Token)`
   in `Bytes`. `pub(in crate::blob)` from `crate::blob::lend` is a legal
   ancestor path. `bytes` can name the type because the module is
   `pub(crate)`. Only `blob` can then make a Token, so only `blob` can call
   `lend_at`, by the compiler and with no clippy. Outside crates still cannot
   name the type, so they cannot override or call it. The `private_interfaces`
   lint compares the type's nominal visibility (`pub struct`), so it should
   not fire; `unnameable_types` is allow-by-default. Not compiled.
   The one other wanted caller, `OrdIndex::head`, holds a concrete
   `MmapBytes`, so it can use a `pub(crate)` inherent method (keep
   `as_slice` as a `pub(crate) fn lent()`), with no trait call at all.
   That also avoids changing `head`'s code (see 3.3).

3. **Step 3 rewrites (`OrdIndex::open` CRC by the piece; `Blob::open_with`'s
   checksum row by the piece at open).** Both skips are deliberate and
   documented at the site: ordindex.rs:409-415 ("its checksum is not taken,
   since that would ask for the file whole and what such a source lends has
   passed a check of its own") and blob.rs:975-979. Under a cipher the
   pager's AEAD tag covers every chunk. Verifying at open asks for the whole
   ordered index and the whole key section at every open. That breaks
   tests/cipher.rs:477-493 in `a_store_larger_than_its_cache_reads_as_itself_within_it`,
   which holds a reopen to `c.fills <= 6 * files` and to under a quarter of
   the stored bytes ("a handful of chunks whatever the file's length"), and
   tests/cipher.rs:1705-1716 and 1764 hold the same for form files. So
   "N" is the wrong class for these two under the cipher. They are N only
   for a piece source with no check of its own: the test source
   `Piecewise` (tests/blob.rs:234), and the conform pass's identity `Aead`
   (forms.md "The conform pass"). The rewrite has to say which. Two options
   that keep the cipher's open bound: verify per piece on first use (the
   `verified` bits, blob.rs:2946 `verify_span` shape), priced per read
   under the cipher; or give the byte source a "checked" bit (an
   authenticating source skips, a plain piece source verifies). The notes'
   option "(a) the whole row at open through Pieces" is ruled out by that
   test. "Build `top` through `head(i)` for both kinds" at open is ruled
   out the same way: it is one read of every page of the file
   (ordindex.rs:413-415).

4. **Red test 3(b) ("today opens and may answer wrong").** Over `Piecewise`
   that is true. It must not be written so that the fix that passes it
   breaks the cipher open bound in item 3. Pair it with the cipher test as
   the guard.

5. **"What `vault::open` returns for a kept segment... not verified" (notes
   1.7, 2, 5).** It can be read. `vault::open` (vault.rs:1892-1899) sends a
   kept name to `Vault::reopen_written` (vault.rs:1113-1127). That returns
   `Err(Unsupported)` for a file that has its envelope, and the memory file
   only while the file is still being written. So:
   - `compact_run` db.rs:13579-13584: `vault::open(..).ok()` is `None` for
     every finished input. No fadvise. With `record_offset` `None`
     (blob.rs:1298) nothing at all is dropped. This confirms the notes' N.
   - `Seg::drop_raw_pages` db.rs:8919-8929: `if let Ok(f)` skips. The first
     half (`blob.advise_dontneed()`, bytes.rs:589-614) already calls
     `Paging::forget(0, len)`. So this site is **right** under a cipher. It is
     not N, and it needs no rewrite for R3.
   - `ZFile::drop_cache` zfile.rs:3255-3259: `self.file` is `None` under a
     cipher (zfile.rs:2020-2022 says so), and `map.advise_dontneed()`
     forwards to `forget`. Also **right**.

6. **`compact_run`'s drop-behind and `warm_file` are listed as fixes. They are
   behaviour changes under the cipher.** warm_file's comment (zfile.rs:3780-3782)
   says a form file under a cipher "has no page cache to warm". That is a
   design statement, even though the envelope's ciphertext is in the
   kernel's cache. Turning on a merge's drop of input ciphertext, or a warm
   of a form file's ciphertext, is a mechanism made to work under a mode.
   CLAUDE.md requires it to be priced under the mode before its default is
   chosen. CLAUDE.md also records the opposite case: "The store dropped its
   own ciphertext", where a drop of the envelope's cache cost the
   out-of-cache pass 53 MB of device reads. The notes give red tests and no
   pairing. Each needs a pairing (for example `pair.sh supdb-cipher64
   supdb-cipher ...` with the change as an arm), or an option that is off
   under a cipher until priced.

7. **Step 1: "add `resident`, `forget`, `populate`" as new defaulted
   methods.** `Bytes` already has `advise_willneed` (bytes.rs:149),
   `resident_at` (167), `advise_dontneed_range` (178, forwards to
   `Paging::forget` for a paged mapping, bytes.rs:534-537) and
   `advise_dontneed` (130), and `MmapBytes` answers all of them for both
   kinds. New methods beside these give two definitions of the same advice,
   which is what R3 ("written once") forbids. The real gaps are smaller:
   (a) a span residency question (the existing `resident_at` is one page);
   (b) a mapped file's `advise_dontneed_range` does `madvise` only, with no
   `posix_fadvise`, because no fd is kept; (c) the populate of `warm_file`.
   Rename or extend the existing methods, and delete the address-taking
   duplicates.

8. **Step 1: "`Own::Map` keeps the file's descriptor".** This has a cost
   the notes do not price. Today `MmapBytes::open` (bytes.rs:342-349) and
   `from_map` drop the file after the mapping. Keeping it holds one
   descriptor per mapped segment and per ordered index for the store's
   life. `ZFile` already keeps one per form file (zfile.rs:1869
   `file: Option<File>`). A store at ten million keys with hundreds of
   partitions and pieces (CLAUDE.md cites 336 pieces) then holds over a
   thousand descriptors, against a common `RLIMIT_NOFILE` of 1024. Nothing
   in `src` raises or checks the limit (`grep -rn 'RLIMIT_NOFILE\|EMFILE' src`
   is empty). Not measured. Alternatives: fadvise by path at the site
   through `vault` (as `drop_raw_pages` does today), or `dup` only for the
   duration of the call. `from_map` is `pub`; changing its signature breaks
   no outside caller (`grep -rn from_map tests examples bench experiments`
   is empty) but is a public API change.

9. **Step 1's red test** (a mapped file's drop empties the page cache, read
   by `mincore`) reads the kernel's answer to advice. Cargo.toml:37-40 says
   such tests are compiled out under `cfg(supdb_emulated)` (the qemu-user
   CI leg, where `madvise` is a no-op). The test needs that gate, or it is
   red on that leg for a reason that is not the code.

10. **Step 6 red test wording.** "each passes today and fails to compile
    after" reverses how `compile_fail` reads. A `compile_fail` doctest is
    red when the snippet compiles (today) and green when it fails to compile
    (after). The intended meaning is right; the sentence is not. Also: once
    `as_slice` is deleted, the doctest passes on E0599 (no such method), and
    it would pass just as well on a typo. Doctests do run in the gate:
    check.sh:56-58 runs `cargo test --profile checked` with no `--tests`,
    and the lib has `rlib` in its crate types (Cargo.toml:11).

11. **"`#[allow]` on `Sec::get` and `OrdIndex::head`... the same mechanism
    forms.md's G-door uses".** There is no `clippy.toml` in the export
    (`ls` of the root). G-door creates it. So the notes' step 6 depends on
    G-door having landed, and the landing order does not say so. In addition, the
    lint gate is `cargo clippy --release --all-targets -- -D warnings`
    (check.sh:62). That has two consequences:
    - Under `--release`, `cfg(debug_assertions)` code is not compiled, so
      it is not linted. The conform pass is planned as debug-assertions-only
      code (forms.md "The conform pass"), so a whole borrow written there
      passes clippy.
    - It runs for the host target only, so `cfg(target_family = "wasm")`
      code (wasmapi.rs) is never linted.
    Item 2 makes clippy unnecessary for `lend_at`.

## 2. What was missed

A. **`slice_at` of a whole section is a whole borrow, and nothing in R2 as
   the notes implement it stops one.** `Bytes::slice_at` is public and R2
   explicitly allows it. For a mapped file, `slice_at(off, section_len)` is
   `as_slice` restricted to a range, by the same bounds check
   (bytes.rs:469-476). For a paged mapping it goes to `slice_slow`, which
   `ensure`s the whole range (bytes.rs:394-404). That makes the section
   present whole: the cost R2 exists to prevent. Making `as_slice`
   and `lend_at` private therefore does **not** make "a reader that borrows
   whole" fail to compile. That class is held only at run time, by the
   conform pass and the asks ratchet (R15).

   Sites that ask for a whole region today:
   - in `src`: zfile.rs:2052 (`slice_at(t.res_off, res_len)`, the form
     file's resident part, copied at attach); zpages.rs:7522 and 7556 (tests).
   - outside: tests/zpages.rs:82 (`check_segment`) and tests/zpages.rs:1694
     (`regions_of`, which also subtracts pointers, :1697-1699); and
     bench/examples/zsize.rs:290. The notes list only tests/zpages.rs:84
     and zsize.rs.

   The public `Encrypted` source (crypt.rs:1003) is not `piecewise`. Its
   `slice_at` of a whole section decrypts the section, so the outside API
   already has the shape R2 forbids.

   The feasibility conclusion should say: privatisation removes the two
   unbounded accessors; whole-range `slice_at` is held by conform and R15,
   not by the compiler.

B. **Base pointers taken by `slice_at(0, 1)`:** blob.rs:1414
   (`record_region_span`, listed by the notes), zfile.rs:7432 and 8373
   (tests, not listed). Under a paged source `slice_at(0, 1)` succeeds
   (pager memory), so this idiom yields a valid-looking base for either
   kind. Only the `record_regions()?` before it keeps blob.rs:1414 from
   answering for a paged source.

C. **Pointer-subtraction offsets in `willneed_ranks`, `willneed_dir` and
   `record_offset`** (blob.rs:1258, 1291, 1302: `recs.as_ptr() - sec.as_ptr()`).
   The notes' rewrite covers these through header offsets but does not
   name the lines. They are the same pattern as `Mapped`'s
   `recs_at`/`dir_at` (zfile.rs:4294-4295).

D. **`MmapBytes::map()`** (bytes.rs:382, private) hands out
   `&memmap2::Mmap`, which derefs to the whole file. Its one user is
   `advise_willneed` (bytes.rs:615). It is module-private and harmless, but
   it belongs in the 1.6 table, because R2's list is "every accessor that
   hands out a whole section".

E. **`FlatIndex::dir_entries(&self, sec: &[u8])`** (flatindex.rs:2307, pub)
   and the other public `FlatIndex` methods that take the section as a
   slice (`parse`, `regions`) are public API over a whole section. Outside
   callers of `parse`/`regions`: tests/zpages.rs:83-84 and 1695-1696;
   bench/examples/zsize.rs:291-292.

F. **scan_either call sites** (section 0) are D and are not listed.

## 3. Sampled sites (fifteen or more, each read with context)

Right as classified: ordindex.rs:419 (N, `OrdIndex::open`); 678 (D,
`head`); 940 (speed only, `seek_exact_hint`, enclosing fn at 906);
zfile.rs:3784 and 3802 (N, `warm_file`); 3795 (`resident_pages` with
`lend = 0`, N); 4266 (base pointer, `build_file_seen` File arm); 4273
(`record_regions`, W); 5065 (D, sample); db.rs:8956-8960 (D,
`records_resident`); 42429-42437 (D, `z_cross`, including the
`use crate::bytes::Bytes as _` at 42435); blob.rs:1240, 1277, 1299 (N);
1212, 1326, 1352 (speed only); 1502/1521 (D); 2159, 2206, 2311, 2463 (D);
1160 (D, `blk_sec`); 2871 (loud: `SparseBlob` errs); 967/991/1020 (D);
db.rs 9718, 9972, 10120, 10200, 17778, 17790 (anonymous blocks from
`blocks::alloc`: db.rs:9708, 9994, 10086, 17701, and recycled blocks from
the same).

Wrong or incomplete:
- blob.rs:980 and ordindex.rs:419/425-428: N only for a piece source with
  no check of its own (item 1.3).
- db.rs:8919-8929 (`drop_raw_pages`) and zfile.rs:3255 (`drop_cache`) are
  right under a cipher, not N (item 1.5).
- bytes.rs:422 `resident_pages`: confirmed N for paged (it clamps to `lend`).
  Its outside use, tests/zpages.rs:5325, is confirmed.

## 4. Feasibility, attacked

- **Language rules the notes state.**
  - "A trait's methods share the trait's visibility": right.
    Visibility qualifiers on trait items are rejected (E0449).
  - "`pub(in path)` takes only an ancestor": right.
  - A sealed supertrait breaks the 11 outside impls: right.
  - The blanket-`Sealed` aside is beside the point: a blanket impl seals
    nothing.
  - Token sealing (an outside crate can neither override nor call the
    method): consistent with the known "partially sealed trait" pattern.
    Inferred.
  - The confinement claim is wrong (item 1.2).
- **Hot path.** Replacing `as_slice().get(at..at+HEAD)` in `OrdIndex::head`
  (ordindex.rs:671-682) with `lend_at` changes its code. `lend_at` is
  `checked_add` on u64, a compare with `lend`, then `ptr.add`. The site's
  own comment says that going through `slice_at` cost "an instruction a
  head", and head runs at every seek step. Keeping a `pub(crate)` inherent
  accessor for `OrdIndex` changes no instruction. Recommend that over the
  deletion.
- **Monomorphised paths.** A ZST token argument should add nothing
  (inferred). `Blob` methods are monomorphised in bench/src
  (engines.rs:9449-9456) and in tests, so G-cost row 8 is the place to
  see it.
- **Recommendations whose costs were not priced:**
  - descriptors (item 1.8);
  - cipher open decrypt bound (item 1.3);
  - cipher behaviour changes needing pairings (item 1.6);
  - duplicated advice methods (item 1.7).

## 5. Corrected counts

| what | notes | checked |
|---|---|---|
| from_raw_parts sites in src | 44 | 44 |
| ... addressing a file mapping or the pager memory file (non-test) | 6 | 6 (bytes 378, 403, 475, 487, 509; pager 1003). The pager's ciphertext mapping (pager.rs:720) is reached by slicing, not by from_raw_parts |
| outside impls of Bytes | 11 | 11 |
| outside overrides of lend_at | 0 | 0 |
| non-test in-crate uses of MmapBytes::as_slice | 7 | **6** |
| test uses of MmapBytes::as_slice in src | not given | 11 |
| outside uses of as_slice | 0 | 0 |
| `.flat()`/`.pieces()` in blob.rs | 17 | 17, plus 5 `Sec::pieces(&src)` calls the pattern misses |
| indexed! call sites | 8 | 8 |
| scan_either call sites | not given | 5 (db.rs 20689, 20743, 20874, 29520; zfile.rs 2730) |
| outside callers broken by the visibility changes | 1 | 1 (tests/zpages.rs:5325). Note that tests/zpages.rs:1694 and zsize.rs:290 keep whole borrows through `slice_at`, which no change touches |
| `slice_at(` call sites in src | not given | 58 (`grep -rn 'slice_at(' src \| grep -v 'fn slice_at'`) |

## 6. Not verified

- Each of these is inferred, not compiled:
  - `private_interfaces` does not fire for a `pub struct` token in a
    `pub(crate)` or private module used in a `pub` trait method;
  - a token constructor `pub(in crate::blob)` compiles where the trait
    lives in `bytes`;
  - an outside crate cannot write the overriding signature.
- Whether clippy `disallowed_methods` matches a trait method called through
  a generic `B` (the notes' open question; item 1.2 removes the need).
- Whether rustdoc checks `compile_fail` error codes on this toolchain.
- The descriptor count of a large plain store with fds kept per mapping.
- The instruction count of `head` through `lend_at` against `as_slice`.

## 7. Verdict

The inventory stands with corrections. Sites, kinds and counts are right
but for the as_slice count (6, not 7), the scan_either sites, the outside
whole-section `slice_at` sites, and two sites called N that are right
under a cipher (`drop_raw_pages`, `drop_cache`).

The feasibility conclusion is partly wrong:
- `lend_at` can be confined to `blob` by the compiler (token in `blob`),
  so clippy is not needed for it.
- Visibility cannot make "a reader that borrows whole fails to compile"
  hold, because `slice_at` of a whole range is allowed and is the same
  borrow.

The R3 rewrites need revision before anyone builds from them:
- The two open-time checksum rewrites break the cipher's open bound, which
  a test holds.
- The fd-keeping plan has an unpriced descriptor cost.
- The new advice methods duplicate existing ones.
- The cipher-side drop-behind and warm are behaviour changes that need a
  pairing.

The landing order must put G-door's `clippy.toml` before step 6, if
clippy is kept at all.
