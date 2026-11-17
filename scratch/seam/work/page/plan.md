# E9 ("page"): whole-page chunks under a cipher, out of core, and whether the cache's size is the whole story

The tree is 3df85d22 and clean, on branch claude/inspiring-brahmagupta-d7y8n7. This file is only the plan. Nothing was edited, built or timed while it was written. Every file:line reference was read at 3df85d22.

`W=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/page` is this experiment's directory. `O=.../seam/work/once` is the experiment before it.

**Nothing is committed or pushed.** `git status --porcelain` must be empty whenever a timing run is on and at the end.

## 0. The question, and what is already known

The setting is 5M keys under a 640 MiB bound (`--bound 671088640`), with uniform point reads and `--ops-div 1`.

| arm (binary) | reads/s | ×clear | misses a read | plaintext held | shmem |
|---|---|---|---|---|---|
| supdb-nozpages (clear) | 119-159k | 1 | 0.137-0.170 faults | page cache 575-594 MiB | 0 |
| supdb-cipher (bench-once) | 40.7-42.3k | 0.28 | 0.684 | 273-278 MiB | 335 |
| b432 (bench-once) | 48-50k | 0.33-0.34 | 0.428 | 417-431 | 516-520 |
| ddyn (O_DIRECT, 75% of read_budget) | 46-47k | 0.30-0.33 | 0.490 | ~380 | 485 |

The line to test fits all four points: misses a read = 1.17 × (1 − C / 660 MiB), where C is the plaintext cached. At C = 273 it gives 0.684, at 418 it gives 0.428, and at about 585 it gives about 0.13 for the clear store.

If the line is right:
- the cipher's out-of-core gap is how much plaintext the bound lets it hold;
- "the uncounted factor of two" in once.md (item 4) does not exist;
- what wastes the bound is the 4080-byte chunk's straddle (shmem 1.22-1.27x of held), a budget of half, and headroom.

The line is a four-point fit, and its 660 MiB is derived, not measured. This experiment measures 660 (S below) and tests two hypotheses.

- **H1, cache size alone.** The clear store under a bound that leaves it C MiB of page cache takes as many faults a read as the cipher store takes misses while holding C MiB of plaintext. No patch is needed.
- **H2, whole-page chunks.** With the tags kept apart, shmem equals held. The same footprint then holds 1.22-1.27x the plaintext, and a budget that takes most of the bound holds 480-550 MiB. By the line, that is 0.32 / 0.25 / 0.20 misses a read at 480 / 520 / 550, or about 0.47x / 0.58x / 0.70x of clear.

Out of scope: O_DIRECT. A miss is read through the envelope's mapping, as the design chose.

---

## 1. H1: the clear store alone, under reduced bounds (no patch)

### How the arm is run

`bench ab` takes exactly two arms. An arm against itself is allowed and is documented as the control: the USAGE text in bench/src/main.rs:66-67 says so, and `cmd_ab` (main.rs:271-289) checks only that two known names were given.

So run `--arms supdb-nozpages,supdb-nozpages`. This is the cheapest honest partner: no second engine, and both readings are the quantity wanted.
- Under a bound each pass runs in a process of its own (main.rs:322 `plan.fresh_process = plan.bound.is_some()`).
- Each arm builds its own store, at `$TMPDIR/supdb-ab-<pid>/<arm>-<rep>-<i>` (bench/src/run.rs:3251, 3272).
- So one sitting with `--reps 1` gives **two independent readings**, each a fresh process with its own ordered load and read pass.
- Order does not exist in a self-pair, so "two sittings with the order swapped" becomes the two readings of one sitting. A second sitting at a bound is run only if the two readings' faults a read differ by more than 5%.
- The phase marks name only the arm, so the two readings are told apart by their order in the `.err`: the first `read start`/`read end` pair is i=0. The sampler tells them apart by the spec name (`<arm>-<rep>-<i>`).

`--passes read` still runs the ordered load first: it is how the store is built (O/pairs/r0-5000000-1.err shows `load start` and `load sync` before `read start`). The shuffled load does not run.

### The bounds

At 640 MiB the clear store holds 575-594 MiB of page cache (sampler `cache − shmem`, second half of the read pass) with RssAnon 35-53 MiB. That leaves an overhead of about 55 MiB (46-65) for anon, page tables and the binary.

The bound for a target C is therefore C + 55 MiB, rounded to a whole MiB. `bound_arg` refuses anything else (main.rs:96-99) and anything under 256 MiB (main.rs:89-94, run.rs:221).

| target C | bound (MiB) | `--bound` (bytes) | compared with |
|---|---|---|---|
| 418 | 473 | 495976448 | b432's 0.428 misses at a held of 417-431 |
| 273 | 328 | 343932928 | supdb-cipher's 0.684 at 273-278 |
| 520 | 575 | 602931200 | H2's x520 arm below |

The target is only how a bound is chosen. **The comparison uses the C measured in the run**, two ways:
1. the sampler's `cache − shmem`, median over the second half of the read pass;
2. `scripts/resident.py` over the store directory, three times in the read pass. It reports the page-cache bytes of the store's own files, by kind, through `mincore`, faulting nothing in.

(2) is the figure compared with the cipher's `held`. (1) includes the binary's text and anything else in the page cache.

### The store's size on disk: S, the "660 MiB"

`$W/storesize.py` (§3.0) runs beside the sampler in every sitting. Every 5 s it logs, for each directory under `$W/tmp/supdb-ab-*/`:
- the apparent bytes by file kind (resident.py's `kind()`: `seg`, `par`, `pcs`, `*.ord`, the WAL and the manifest, and for a cipher store `x-…` by suffix);
- the allocated bytes (`st_blocks × 512`);
- a `CLOCK_MONOTONIC` time.

Every 30 s, for a store directory with no `x-` files (a clear store), it also runs `python3 -I /home/user/supdb/scripts/resident.py DIR TAG` and logs its output.

S is defined as the apparent bytes of the files a point read touches (the segments and partitions, `seg`/`par`/`pcs`), not the ordered indexes (`.ord`, which point reads do not read). Both are reported. In a cipher store the same quantity is the envelopes' total less 1/256 for tags and less 4 KiB of header each. Since a cipher store holds the plain store's bytes as plaintext, S should agree with the clear store's figure to within a percent. It is read in the H2 sittings as a check.

### What H1 predicts, and what refutes it

The line, with S measured, is re-fitted as m = k(1 − C/S). Predictions, with S = 660:

| bound | C (pred.) | faults a read (pred., H1 true) | band | reads/s (pred.) |
|---|---|---|---|---|
| 473 | 418 | 0.43 | 0.36-0.49 | 50-60k (1/(2.4 + 0.43 × 36) µs) |
| 328 | 273 | 0.69 | 0.58-0.79 | 33-40k |
| 575 | 520 | 0.25 | 0.21-0.29 | 80-95k |

**H1 is refuted** if, at the measured C, the clear store's `read majfaults_per_op` differs from the cipher's misses a read at the same C by more than **15% relative or 0.04 absolute, whichever is larger**, at either of the two points where a cipher measurement exists (C ≈ 273 and C ≈ 418). The cipher's value at the clear's measured C is interpolated along the cipher's own measured points (273 → 0.684, 314 → 0.62, 373-382 → 0.49-0.51, 417-431 → 0.428), not along the fitted line.

- **Clear lower by more than the band.** The kernel's LRU keeps more useful bytes per MiB than the pager's megabyte clock: the "factor" exists, and page-granular eviction is worth designing. In that case H2's predictions, which assume the line, are too optimistic by that factor, and are re-derived from the clear curve before H2 is read.
- **Clear higher.** The pager is the better cache per MiB, and the cipher's gap is size alone a fortiori.

The clear store's rates are a side result. Clear at C = 418 against b432's 48-50k at the same misses prices the cipher's non-device time per miss directly, with the device term equal on both sides.

---

## 2. H2: the scratch patch, whole-page chunks with the tags apart

### 2.1 The shape, and why a compile-time switch

- **The layout.** A new envelope version (2, "tags apart") is written only by the vault and read only by the pager. The public envelope (`crypt::encrypt`, version 1, `tests/data/crypt-v1.bin`, `web/crypt.mjs`) is not touched.
- **The switch.** `pub(crate) const PAGE_LAYOUT: bool = true;` in pager.rs selects it. `false` gives back today's 4080-byte layout, from the same tree and harness.
- **Why compile time.** A run-time switch is not as small. The pager's `CHUNK` is a `const` that every read divides by (`ensure`, pager.rs:1325-1334, whose comment says "the divide is a multiply the compiler writes"), and a per-file chunk length would put a real divide, or a branch, on every read of both arms.
- **The binaries.** `bench-page` (true) and `bench-p4080` (false). They are compared across binaries through the clear arm each pairing carries, and by absolute rate, which was stable to a few percent across sittings in E8 while the clear arm moved ±15%.
- **The control.** bench-p4080 differs from bench-page in that one constant. It is the in-session control for today's cipher (run 4, §3), and the binary for today's cipher in the skewed mix.

### 2.2 Start from once.patch, keep its budget, trace and counters, and take its O_DIRECT path out

```
cd /home/user/supdb && git status --porcelain        # must print nothing
git apply $O/once.patch
```

Then remove by hand (Edit) every part of it that belongs to the O_DIRECT miss path. The hunk positions are those in once.patch.

**src/db/pager.rs, remove:**
- the thread-locals `HOLDS`, `HINT` and `RUNBUF`, and `ONCE_ANY`, `RUN_MAX` and `RunBuf` with its impls (patch lines 232-270);
- the `HOLDS` bumps in `Hold::drop` (274-281) and `Pager::hold` (373-380);
- the `once` field, the `SUPDB_ONCE_FORCE` match and the `ONCE_ANY` store in `new_with`. `new_with` becomes `new_with(limit: u64, dyn_pct: u8)`;
- `Back`'s `direct` and `buffered` fields, and their opening in `Back::open` (455-492);
- the `fill`/`envelope`/`fill_from` split (492-542). Go back to the original `fill`, which §2.4 then edits;
- `slow`'s `if self.pager.once != 0` branch (543-555);
- the whole of `fill_run` (556-718);
- the `HINT` sets in `ensure` (722-731) and in `willneed` (732-744);
- the counters `direct_reads`, `direct_bytes`, `direct_us`, `run_chunks` and `nowait_hits`, in `Counts`, `PagerCounts` and `counts()`. Keep `limit_lo` and `limit_hi`.

**src/db/pager.rs, keep:**
- the derivation of `hard` and `swamp` from the current limit inside `crowded`/`swamped`, with the `unit_bytes` field;
- the `dyn_pct` field and the 100 ms recompute at the top of `run` (the patch's 424-455), which stores `limit = max(read_budget().budget × dyn_pct / 100, 64 MiB)`;
- the `trace` flag and the `pagertrace` line. Drop its `direct`/`runchunks`/`nowait` fields from the format string.

**src/db/vault.rs:**
- remove `once` and `scratch_once`;
- keep `dyn_pct` and `scratch_dynamic`;
- `Vault::new` (vault.rs:776 in the tree) calls `Pager::new_with(cipher.cache.unwrap_or_else(default_cache), cipher.dyn_pct)`.

**bench/src/engines.rs:**
- remove `cipher_once` and the seven `create_cipher_{b384,b432,d320,d384,d432,ddyn,dndyn}`, with their `name()`, `guarantee` and `open` lines and their seven `cipher_direct*/run/nowait` counter rows;
- keep the `cipher_limit_lo`/`hi` rows;
- keep `cipher_dyn: u8` in `Policy` (engines.rs:1259 area), its default 0 (1971) and its carry into `Supdb` (387, 6607, 7240);
- `with_policy` (7222-7230) calls only `.scratch_dynamic(cipher_dyn)`.

**Acceptance for this step:** `grep -nE 'once|O_DIRECT|HINT|RUNBUF|ONCE_ANY|fill_run|direct_' src/db/pager.rs src/db/vault.rs bench/src/engines.rs` names nothing of the patch's (the shipped tree's own uses of the word "once" in comments excepted).

### 2.3 src/crypt.rs: version 2 of the header, and its writer

All references are to src/crypt.rs.

**Constants.** Beside `VERSION` (131), add `const VERSION_APART: u32 = 2;`.

**`Header` (164-169).** Add a field `apart: bool`.
- `Header::new` (179-208) keeps its signature and sets `apart: false`.
- Add `pub(crate) fn new_apart(plain_len, chunk_shift, id, key_id) -> Result<Header>`. It makes the same checks and sets `apart: true`. The overflow check at 203-206 holds unchanged, since the stored length's formula is the same in both layouts (below).
- `parse` (211-232) accepts `VERSION` (apart false) and `VERSION_APART` (apart true) and refuses any other version as today.
- `encode` (247-256) writes `VERSION_APART` when `apart` is set.
- Add `pub(crate) fn apart(&self) -> bool`.

**The layout of version 2:**

```text
0                         header page, as v1 (48 bytes, then zeroes to 4096)
4096 + c·S                chunk c's ciphertext, S = 1 << chunk_shift bytes; the last chunk is what is left (S·k ≥ plain_len)
4096 + plain_len          the tag region: tag of chunk c at 4096 + plain_len + 16·c
4096 + plain_len + 16·chunks   end of file
```

- **The tail region's place and its rounding.** It starts immediately after the last ciphertext byte, with no padding.
  - Every byte after the header page is then under a tag: ciphertext under its chunk's tag, and a tag changed fails its chunk.
  - There are no new unauthenticated zeroes to check.
  - `stored_len` (297-299), `DATA_START + plain_len + chunks·TAG`, is the same formula for both layouts, so `checked_header`'s length check (446-452) needs no change.
  - The cost is that the region is not page-aligned. One tag in 256 lies across two pages of the file (when its offset mod 4096 > 4080). The region is 1/256 of the plaintext: about 2.6-2.8 MiB of the 5M store's files, and touched on every miss, so it is expected to stay in the page cache's active list (verified by §2.6's fault check).
- **The last partial chunk** is `chunk_plain_len(last)` bytes of ciphertext at `stored_off(last)`, with its tag in the region like every other. The empty file is one chunk of no bytes and one tag, as in v1 (`chunks()` ≥ 1, 291-293), so the stored length is 4096 + 16.
- **`chunk_len`** (265-267): `if self.apart { 1 << self.chunk_shift } else { (1 << self.chunk_shift) - TAG }`.
  - `chunks()` (291-293) and `chunk_plain_len` (335-337) follow from it unchanged.
  - Make `chunk_plain_len` `pub(crate)`.
- **`stored_off`** (325-327) is unchanged: `DATA_START + (c << chunk_shift)`. In v1 that is the chunk with its tag; in v2 it is the ciphertext alone, page-aligned.
- **New `pub(crate) fn tag_off(&self, c: u64) -> u64`:**
  - v2: `DATA_START + plain_len + c·TAG`;
  - v1: `stored_off(c) + chunk_plain_len(c)`.
  So one call site serves both layouts.
- **New `pub(crate) fn stored_end(&self, c: u64) -> u64`:** `stored_off(c) + chunk_plain_len(c) + if apart { 0 } else { TAG }`. That is the end of chunk c's bytes in the data region.
- **`open_probe` (303-305) and `stored_range` (310-322)** describe a contiguous chunk-and-tag, so they are v1-only. Their only readers are `Encrypted` and the browser, and both refuse or never see v2. Add `debug_assert!(!self.apart)` to each.

**Nonce, AAD and the first chunk.** These are unchanged.
- The associated data is the 48-byte encoded header (`encode`, which now carries the version), so every tag covers the layout's version as well as the length.
- The nonce is still `id[..8] || chunk index` (`nonce`, 340-346).
- `Encrypted::open_with` (834-853) verifies chunk 0 before believing the header. That rule is unchanged for v1. `Encrypted` does not read v2: after `checked_header` in `open_with` (835) and in `decrypt` (479), refuse `hdr.apart()` with `refused("an envelope with its tags apart is a store's file, read through its cache")`.
- The pager has never verified at open (`Back::open`'s doc, pager.rs:730-731, says "nothing decrypted"). It trusts `plain_len` for its geometry and lends nothing until a fill's tag has covered the header, which is how a changed header is refused today: the first fill fails and poisons. Version 2 keeps exactly that. A forged `plain_len` must still match the file's length (`checked_header`) and fails every tag.

**The writer.** Add `#[cfg(unix)] pub(crate) fn encrypt_tags_apart<S, W, A>(src, out, aead, opts: &EncryptOptions) -> Result<Header>`. `EncryptOptions` (351-366) is not changed, since its struct literals in tests/crypt.rs:123/255/332, crypt.rs:1209/1783 and examples/crypt_cost.rs:256 would break. The writer does this:
1. It draws the id as `encrypt` does (381-384).
2. `hdr = Header::new_apart(src.len(), opts.chunk_shift, id, opts.key_id)?`, and `aad = hdr.encode()`.
3. `buf` holds the header page as in `encrypt_with_id` (410-412), and `tags = Vec::with_capacity(chunks·TAG)`.
4. For each chunk it reads the plaintext into `buf`, then `tags.extend_from_slice(&aead.encrypt(&hdr.nonce(c), &aad, &mut buf[at..]))`. It writes whole megabytes as 419-423 do, so every data piece is still at a megabyte's offset.
5. At the end it does `buf.extend_from_slice(&tags)` and writes `buf` in pieces of `WRITE_PIECE`.
6. It returns `hdr`.

The tags are held in memory until the end: 16 B a chunk, about 270 KB for a 70 MB partition and 2.8 MB at the most for this store's largest file.

### 2.4 src/db/pager.rs: the geometry, the fill and the ranges

- **108-109.**
  ```rust
  pub(crate) const PAGE_LAYOUT: bool = true;   // SCRATCH(page): false = today's 4080
  pub(crate) const CHUNK: usize = if PAGE_LAYOUT { 1 << CHUNK_SHIFT } else { (1 << CHUNK_SHIFT) - TAG };
  ```
  `CHUNK_SHIFT` stays 12.
- **The unit (119-125, `unit_chunks`).** Unchanged: 256 × (page/4096) chunks. With whole pages that is 256 pages, exactly 1 MiB aligned to a MiB of the memory file, so a megabyte stays the unit as asked.
  - The doc's "the least count whose plaintext is whole pages" becomes false (that is 1). Note it as a SCRATCH comment.
  - `Pager::new`'s `unit` (318) and `pass`'s margin (615) are `unit_chunks() × CHUNK` = 1 MiB.
  - `free` (1226-1228) gives back `lo·CHUNK .. hi·CHUNK`, now whole pages exactly.
- **The divides by CHUNK.** These are in `ensure` (1325-1334), `piece_end` (1342), `piece_from` (1350, 1358), `resident` (1384-1385), `willneed` (1481-1482) and `stored_pages` (1514-1515). They become shifts at compile time; no edit is needed.
  - Crypt's `Per` (crypt.rs:786-820) is the multiply-shift divide of `Encrypted`, which the pager does not use. It is unchanged and does not see v2.
- **`Back::open` (737-743).** Refuse `hdr.chunk_shift() != CHUNK_SHIFT || hdr.apart() != PAGE_LAYOUT` with "an envelope of another chunk size or layout than a store's files are written in", and add `debug_assert_eq!(hdr.chunk_len(), CHUNK)`. A v1 store opened by bench-page is therefore refused by name, never misread. No sitting reuses a store across binaries anyway.
- **`fill` (966-1023), the one place a miss reads the envelope.** Replace 976-986 and 1003-1005 with reads of the ciphertext and the tag apart:
  ```rust
  let n = self.chunk_len(c);
  let at = back.hdr.stored_off(c as u64) as usize;
  let t = back.hdr.tag_off(c as u64) as usize;
  let (Some(ct), Some(tg)) = (back.map.get(at..at + n), back.map.get(t..t + TAG)) else {
      return Err(/* the existing UnexpectedEof "envelope is shorter than its header says" */);
  };
  ...
  dst.copy_from_slice(ct);
  let tag: [u8; TAG] = tg.try_into().expect("a tag's length");
  ```
  - This is right for both layouts, because v1's `tag_off` is `at + n`.
  - The tag is read through the envelope's mapping, so a tag page not in the page cache is a major fault on the reader, counted like every other.
  - The guard's `mprotect` (990-1001) and the destination `base + c·CHUNK` (1003) need no edit.
  - A miss now writes one fresh memory-file page, where an isolated 4080-byte chunk wrote two.
- **`resident` (1388-1389 and 1416-1417).** Replace `stored_off(x) + self.chunk_len(x) + TAG` with `back.hdr.stored_end(x as u64)`. The tag page's residency is not asked; that is a hint, and 256 chunks share a tag page.
- **`stored_pages` (1516-1518).** The same replacement. `forget` and `willneed` then advise the data pages of the range alone.
  - Under v2 a tag page is faulted at most once per MiB of a range, so `willneed` adds no advice for it.
  - `forget`'s whole-file drop (1452-1454, `0..map.len()`) already covers the tag region.
- **The layout line.** In `Pager::new_with`, when `trace` is set, print once:
  ```
  pagerlayout chunk {CHUNK} apart {PAGE_LAYOUT} unit {unit_bytes}
  ```
  Every sitting's `.err` then says which layout its binary linked (the CLAUDE.md rule on a library's features: the binary says which it linked). `sit.sh` checks for it.

### 2.5 src/db/vault.rs: the writer, and everything that writes an envelope

`write_envelope_at` (vault.rs:1162-1187) is the only writer of the store's envelopes. It is reached from:
- `persist_with` (1331, a seal's or a merge's segment; 1374, its ordered index), which is also the ordered run's close (`close_run` → `persist`) and the manifest's late envelopes (`manifest_image` → `persist`);
- `persist_form` (1457, a form file under its temp name).

Change its two calls at 1173 and 1178 to `if pager::PAGE_LAYOUT { crypt::encrypt_tags_apart(..) } else { crypt::encrypt(..) }`.

Nothing else writes an envelope. `Back::open` (vault.rs:911) is the only reader. `len` (976-978) reads `Header::parse(..)?.plain_len()`, which parses v2. So the load, seals, merges, the ordered run, the form files and the reopen all take the new layout through these two functions.

### 2.6 Every other site that knows 4080, and what happens to it

Found with `grep -rn '4080\|CHUNK\|TAG\|stored_off\|plain_per\|chunk_len' src tests examples bench/src web`.

| site | what it is | under the patch |
|---|---|---|
| crypt.rs:109 `TAG`, 265-337, 410-427, 473-508, 826-1060, `Per` 786-820 | the v1 layout and its readers | unchanged; v2 is added beside them, and `Encrypted`/`decrypt` refuse v2 |
| crypt.rs tests 1239-1240 (`chunk_len() == 4080`), 2005/2014 (`keep_bytes: 16·4080`) | v1 headers and readers | unchanged, pass |
| pager.rs:108-109, 737-743, 976-1005, 1388-1417, 1516-1518 | the geometry and ranges | §2.4 |
| pager.rs test helper `envelope` (1673-1682) | writes the tests' envelopes with `crypt::encrypt` | `if PAGE_LAYOUT { crypt::encrypt_tags_apart } else { crypt::encrypt }`, or `Back::open` refuses every test envelope |
| pager.rs:1766 `assert_eq!(CHUNK, 4080)` | `a_unit_is_whole_pages` | `assert_eq!(CHUNK, if PAGE_LAYOUT { 4096 } else { 4080 })`, plus `unit_chunks()·CHUNK == 1 << 20` under PAGE_LAYOUT |
| pager.rs:1783-1800 | the ranges test: `(4079, 2)`, `(4080, 4080)`, `held == 5·4080 + len % 4080` | the reads stay (with 4096 chunks they still touch 6 chunks: 0, 1, 244-246 and the last); the `held` line becomes `5·CHUNK + len % CHUNK` (len % 4096 = 123) |
| pager.rs:2040-2061 | the poison test: flips `DATA_START + 2·4096 + 100` (still chunk 2's ciphertext in both layouts), reads `2·4080 + 5` and looks at `base + 2·4080` | `2·CHUNK + 5`, `base + 2·CHUNK`, `read(&p, 0, CHUNK)`, `&bytes[..CHUNK]` |
| pager.rs 1814, 1852, 1886, 1998, 2080; vault.rs:2584 | `unit_chunks() × CHUNK` and `div_ceil(pager::CHUNK)` | symbolic, unchanged |
| tests/cipher.rs:491 `c.fills * 4080 < stored_bytes(&dir) / 4` | a bound on the open's decrypts | expected to hold (the product is 0.4% low under 4096); not edited |
| tests/cipher.rs:1757 `len() / 4096` | the form file's stored chunks | unchanged meaning |
| examples/cipher_store.rs:143 `fills × 4080` | printed MB | 0.4% low; cosmetic |
| zpages.rs:6540, tests/blob.rs:368 | a page's room and a piece size, nothing of the envelope | untouched |
| tests/crypt.rs, tests/data/crypt-v1.bin, web/crypt.mjs, web/crypt.test.mjs | the public v1 envelope | untouched; **expected to pass**, and if they fail the patch has touched the public layout: stop and find where |

**New tests**, under `#[cfg(test)]`, SCRATCH-marked:
- **crypt.rs.** A v2 header survives encode/parse, says `apart()`, has `chunk_len() == 4096`, and `tag_off(chunks − 1) + 16 == stored_len()` for lengths 0, 1, 4095, 4096, 4097 and 3·2^20 + 123. Also, `Encrypted::open` and `decrypt` refuse a v2 envelope by the refusal's words (CLAUDE.md: a refusal is tested by what it says).
- **pager.rs, `a_changed_tag_in_the_tag_region_fails_its_chunk`.** Flip one byte at `hdr.tag_off(3)` of a v2 envelope. Reading chunk 3 answers nothing, `check()` names "chunk 3 failed its check", and chunks 0-2 read. Flipping the last byte of the file fails the last chunk.
- **pager.rs, layout and tag read.** A v2 envelope of a 3 MiB + 123 file reads every range of the ranges test, and the poison test holds with the flip in the ciphertext region.

### 2.7 Correctness before any timing

Run with `TMPDIR=$W/tmp`, nothing timed and nothing else running. Believe each run only after its `--list` shows the tests it names (CLAUDE.md). Run `rm -rf $W/tmp/supdb-next-*` between runs.

```
cd /home/user/supdb
cargo test --profile checked --lib crypt
cargo test --profile checked --lib pager -- --test-threads=4
cargo test --profile checked --lib vault
cargo test --profile checked --test cipher --no-fail-fast
cargo test --profile checked --test crypt            # v1 unchanged: must pass
cargo test --profile checked --test dirkey           # the cipher/kernel refusals
cargo test --profile checked --test zpages cipher    # the compact form under a cipher
SUPDB_PAGER_GUARD=1 ZFUZZ_CIPHER=1 ZFUZZ_CIPHERCACHE=16384 ZFUZZ_FROM=0 ZFUZZ_TO=200 \
  cargo test --profile checked --test fuzz fuzz_range -- --exact
cargo test --profile checked --test fuzz fuzz_range_under_a_cipher_with_the_compact_form -- --exact
node web/crypt.test.mjs                               # if node is present; must pass
```

**Then the busy run:** seeds 200-600 of the third line, as four processes of 100 seeds each, each with a `TMPDIR` of its own. CLAUDE.md: a failure that needs the machine busy is run busy, several seeds a core.

- **Must pass:** all of the above.
- **Expected to fail before the §2.6 edits:** the pager tests with 4080 literals. They are edited, not deleted, and must pass after.
- **Known flake:** `the_builders_rest_at_every_block_of_a_store_of_wide_values` failed once in four on the once tree. If it fails here, save the patch (`git diff > $W/page.patch`), revert, run the test four times on the clean tree, and re-apply (`git apply $W/page.patch`) before attributing it. All of this happens before the build, with no timing run on.
- **No test may read a wrong byte.** The fuzz compares every answer against the model and a plain store, `SUPDB_PAGER_GUARD` makes a page read after it was given back a fault, and the cipher tests compare answers. Any wrong answer stops the experiment; it is not timed around.

**Sanity in the bench binary** (after the build, before any timed run, untimed, `TMPDIR=$W/tmp`, each followed by removing its store by name):
1. `$W/bench-page ab --arms supdb-cipherxdyn94,supdb-nozpages --size 100000 --reps 1 --passes read,scan` (unbounded): rc 0, a `pagerlayout chunk 4096 apart true` line, `cipher_fills = 0` (the files are adopted whole).
2. `$W/bench-page ab --arms supdb-cipherxdyn94,supdb-cipherx480 --size 1000000 --reps 1 --bound 268435456 --passes load,read --ops-div 10`: rc 0, `cipher_fills > 0`, `cipher_evicted_units > 0`, no poison and no error.
   - This is the write side whole: the ordered load's run closes into v2, the shuffled load seals and merges into v2, and each pass's fresh process reopens the store, so every file is read back through `Back::open`'s v2 check.
   - Repeat with bench-p4080 and look for `pagerlayout chunk 4080 apart false`.

### 2.8 The arms (bench/src/engines.rs, in both binaries)

| arm | Policy | what it is in bench-page |
|---|---|---|
| supdb-cipher | unchanged | the default budget, `read_budget/2` read at open (vault.rs:640-646): about 265-280 MiB |
| supdb-cipherx432 | `cipher_cache: 432 << 20` | fixed; the same held as b432, so it isolates the layout at an equal miss rate |
| supdb-cipherx480 | `480 << 20` | fixed |
| supdb-cipherx520 | `520 << 20` | fixed |
| supdb-cipherx550 | `550 << 20` | fixed; the edge (shmem 550 + anon about 45 leaves about 45 MiB); an OOM kill is a result |
| supdb-cipherxdyn94 | `cipher_dyn: 94`, `cipher_cache: 0` | dynamic, the same shmem footprint as ddyn's (0.75 in held × 1.255 = 0.94 in shmem) |
| supdb-cipherxdyn85 | `cipher_dyn: 85` | dynamic, a little under |

Each arm needs five things: a `create_*`, a `name()` branch before `true if self.cipher_cache != 0 => "supdb-cipher64"` (engines.rs:7516) keyed on `cipher_cache`/`cipher_dyn`, a line in `guarantee` (9758 list, Durable), a line in `open` (10105), and the Policy field.

The `x` names are layout-neutral on purpose: the layout is the binary's, and every `head.txt` names the binary.

**The dynamic budgets.**
- `read_budget` is limit − (rss + rss_huge) − margin (zfile.rs:5210-5266, margin max(limit/20, 64 MiB)). In the read pass that is 640 − 45 − 64 ≈ 530 MiB, so xdyn94 takes about 499 MiB and xdyn85 about 451.
- In the shuffled load (anon 156-174 MiB) it is about 406, so about 382 and 345.
- The v1 `rss` excludes shmem, so the budget does not shrink as the cache grows.
- What it does not subtract: files with no envelope yet (`unbacked`), which are in shmem and cannot be given back. In ddyn's load,read that was what took shmem's max 30-60 MiB above the limit.

**The line a caller's read waits at.**
- `swamped()` is `could_give > limit + max(limit/2, 6 MiB)` (pager.rs:523-525, as derived in once.patch). For xdyn94's 499 MiB that is about 750 MiB, 110 MiB past the group's 640. For x550 it is 825.
- `crowded()`, where the store's own work waits, is limit + limit/16: 530 for xdyn94.
- So in a read pass **no caller's read ever waits before the group's OOM**. The only guard is the evictor keeping `held` near the limit (a pass gives back over + max(limit/16, 1 MiB), pager.rs:612-615).
- The patch does not change this. The evidence that it held is `cipher_peak` ≤ limit + limit/16 in the read pass, `cipher_breaths` = 0, and no `oom_kill`.
- The design consequence, a line below the group limit where a caller's read waits, is recorded, not built.

---

## 3. The runs

### 3.0 Setup (untimed, before the first run)

1. `git status --porcelain` empty. Apply and edit (§2.2-2.6). Then `git diff > $W/page.patch`. Run the tests (§2.7, about 25 minutes).
2. Build:
   ```
   df -h /
   cd bench && cargo build --release && cp target/release/bench $W/bench-page
   ```
   0.37 GB, and 6.1 GB is free now.
3. Set `PAGE_LAYOUT = false` with one Edit, then:
   ```
   cargo build --release && cp target/release/bench $W/bench-p4080
   git diff > $W/page-4080.patch
   ```
   It must differ from page.patch in that one line.
4. Revert:
   ```
   git checkout -- . && git status --porcelain     # must print nothing
   ```
   At the very end, with nothing running, `cd bench && cargo build --release` so that `target/release/bench` is the clean tree's again.
5. **The bench-side change for run 8 only** (`SUPDB_MIX_ONLY`). It goes in step 1's patch, so that both binaries carry it. In bench/src/run.rs:2510, the mix loop iterates the letters of `std::env::var("SUPDB_MIX_ONLY")` when it is set, and `YCSB_ORDER` otherwise.
   - A repeated letter runs the mix again on the same store, with its mix string `ycsb-C#2` for `phase_mark` and `counting.end`. Only the first is pushed to `ycsb_ops_s` (2531).
   - It is a harness selection, the same for both arms of a pairing.
6. **Tools in `$W`:**
   - `sampler.py`: copy `O/sampler.py` unchanged.
   - `phasemem.py`: copy `O/phasemem.py`.
   - `report.py`: copy `O/report.py`. Drop the direct rows from `ROWS` and add `count cipher_breaths`, `ycsb-C ops_per_s`, `ycsb-C majfaults_per_op`, `ycsb-C#2 …` and `count cipher_limit_*`.
   - `storesize.py`: new, as §1 describes. It reads only directory entries and `stat`, and runs `resident.py` by subprocess.
   - `sit.sh`: `O/sit.sh` with these changes:
     - `S=$W`;
     - `BIN=${BIN:?}` from the environment;
     - `SETS="SUPDB_LAG_MARK=1 SUPDB_LOAD_PHASES=1 SUPDB_ZPAGES_TRACE=1 SUPDB_PAGER_TRACE=1 $EXTRA"`;
     - the busy check also matches `bench-page|bench-p4080|bench-once`;
     - it starts `storesize.py $W/tmp $W/samples/$NAME-$k.size` beside the sampler and kills it after;
     - it writes `BIN` into `head.txt`;
     - after each sitting it greps `.err` for `pagerlayout` and prints the line, so a sitting run with the wrong binary is seen at once.

   Each sitting is still `env $UNSET TMPDIR=$W/tmp $SETS $BIN ab --arms A,B --size SIZE --reps 1 OPTS`, sitting 2 swapped, with the same `ps`, tmp-empty, dmesg and leftover-group checks, and stores removed by name.

**Every option the suite reads from the environment is set for both arms**, through `SETS`. The `UNSET` scrub removes every other `SUPDB_*`, `GLIBC_TUNABLES` and `MALLOC_ARENA_MAX`.
- `SUPDB_PAGER_TRACE` prints, and only from a cipher store.
- `SUPDB_MIX_ONLY` selects the mix for both arms.
- None of these turns the scan timers on (`SUPDB_LAG_COUNTERS`, `SUPDB_MIX_COUNTERS` and `SUPDB_SCAN_OPS` are not set), so the scan pass and the mixes are read against the clear arm with the timers off on both sides.

`P="--bound 671088640 --passes read --ops-div 1"` is the primary option set.

### 3.1 The run list: strictly one at a time, in this order

The times are from E8's sittings: the clear read pass is 35 s, the ordered load 4-5.5 s, and the cipher read pass is 5M ÷ rate.

| # | command (from `$W/pairs`) | est. | what for |
|---|---|---|---|
| 1 | `BIN=$O/bench-once sh $W/sit.sh h1-473 supdb-nozpages supdb-nozpages 5000000 1 --bound 495976448 --passes read --ops-div 1` | 4 min | H1 at C ≈ 418 |
| 2 | `BIN=$O/bench-once sh $W/sit.sh h1-328 supdb-nozpages supdb-nozpages 5000000 1 --bound 343932928 --passes read --ops-div 1` | 5.5 min | H1 at C ≈ 273 |
| 3 | `BIN=$O/bench-once sh $W/sit.sh h1-575 supdb-nozpages supdb-nozpages 5000000 1 --bound 602931200 --passes read --ops-div 1` | 3 min | H1 at C ≈ 520 |
| 4 | `BIN=$W/bench-p4080 sh $W/sit.sh r0 supdb-cipher supdb-nozpages 5000000 1 $P` | 6 min | today's cipher in this session, in the control binary: must read 0.28-0.33x and 0.684 misses, or the patch moved the 4080 path and nothing after is read until that is explained |
| 5a | `BIN=$W/bench-page sh $W/sit.sh xdyn94 supdb-cipherxdyn94 supdb-nozpages 5000000 1 $P` | 4.5 min | the design's candidate |
| 5b | `… x520 supdb-cipherx520 …` | 4 min | the curve |
| 5c | `… x480 supdb-cipherx480 …` | 4.5 min | the curve |
| 5d | `… pdef supdb-cipher …` | 6 min | the default budget with whole pages |
| 5e | `… x550 supdb-cipherx550 …` | 3.5 min | the edge (a kill is a result) |
| 5f | `… xdyn85 supdb-cipherxdyn85 …` | 5 min | the dynamic fallback |
| 5g | `… x432 supdb-cipherx432 …` | 5 min | the layout at b432's held |
| 6 | `BIN=$W/bench-page sh $W/sit.sh lr-BEST supdb-cipherBEST supdb-nozpages 5000000 0.5 --bound 671088640 --passes load,read --ops-div 1` | 7 min | survival and rate through the shuffled load's merges and seals |
| 7 | `BIN=$W/bench-page sh $W/sit.sh scan-BEST supdb-cipherBEST supdb-nozpages 5000000 1 --bound 671088640 --passes scan --ops-div 1` | 3 min | the scan pass |
| 8a | `EXTRA=SUPDB_MIX_ONLY=CC BIN=$W/bench-page sh $W/sit.sh mixc-page supdb-cipherBEST supdb-nozpages 5000000 1 --bound 671088640 --passes mixes --ops-div 1` | 2 min | skew, whole pages |
| 8b | `EXTRA=SUPDB_MIX_ONLY=CC BIN=$W/bench-p4080 sh $W/sit.sh mixc-4080 supdb-cipher supdb-nozpages 5000000 1 --bound 671088640 --passes mixes --ops-div 1` | 2 min | skew, today's cipher |
| 8c | `EXTRA=SUPDB_MIX_ONLY=CC BIN=$W/bench-p4080 sh $W/sit.sh mixc-clear supdb-nozpages supdb-nozpages 5000000 1 --bound <(held_BEST + 55) MiB in bytes> --passes mixes --ops-div 1` | 2 min | the clear store at the page cache that matches BEST's held, under skew. bench-once has no `SUPDB_MIX_ONLY`, so this runs in bench-p4080, whose clear arm is untouched by the patch |

**Which arm is BEST.** It is the arm of 5a-5g with the highest ×clear in both sittings, among those not killed whose sampled usage median stayed under 630 MiB and whose reader took under 0.01 major faults a read beyond its misses (no text thrash).
- For run 6 it must be a dynamic arm: a fixed budget sized for the read pass was killed in the shuffled load at 432 × 1.22 of shmem, and there is no reason one at 480+ survives. So take xdyn94 if it read within a tenth of the best fixed arm; otherwise still xdyn94, and report the fixed arm's lead as the price of a budget that survives.
- **If xdyn94 is killed in run 6** (either sitting), run 6 again with xdyn85 (+7 min, taken from run 7).
- Runs 7 and 8a use the arm that survived run 6.

**Sitting 2 of every pairing swaps the order** (sit.sh does this). For the self-pairs (1-3, 8c), the two sittings are the two readings of one sitting (§1). Add a second sitting only if they disagree by more than 5% in faults a read.

**Total.**

| block | time |
|---|---|
| runs 1-3 | 12.5 min |
| run 4 | 6 |
| runs 5a-5g | 32.5 |
| run 6 | 7 |
| run 7 | 3 |
| run 8 | 6 |
| **all runs** | **67 min** |
| gaps and cleanup | 8 |
| **machine time** | **about 75 min** |

- **What is cut if time runs short, from the bottom:** run 8 (all three), then run 7, then 5g (x432), then 5e (x550). Whatever is cut is named as cut in the report.
- **Setup is not in the 75.** Patch and tests take about 25-30 minutes, two builds about 10-12, and the final clean build about 5.
- **Disk.** The two new binaries are 0.74 GB. Stores are about 0.8 GB an arm, and `df` must stay above 4 GB.

### 3.2 What to read from each sitting

**From the ab table** (`report.py`):
- `read reads_per_s`, `read majfaults_per_op`, `read read_bytes_per_op`, `read p99_us`;
- `count cipher_fills` (÷ 5M = misses a read), `cipher_held_at_end`, `cipher_peak`, `cipher_evicted_units`, `cipher_breaths`, `cipher_limit_lo` and `cipher_limit_hi`.

**From `phasemem.py`/`report.py`'s joins** (sampler and pagertrace on `CLOCK_MONOTONIC`):
- the group's usage, median and max;
- shmem, median and max;
- `cache − shmem` in the second half of the read pass;
- RssAnon;
- failcnt's rise and `oom_kill`;
- **shmem/held**: the sampler's shmem against pagertrace's `held` at the same second, as in E8.

**From `storesize.py`:**
- S (segment bytes), and the ordered indexes apart;
- the cipher store's envelope bytes;
- for the clear stores, resident store bytes by kind.

**Derived per read:**
- non-device µs = µs a read − `majfaults_per_op` × 33.5;
- per miss = (µs a read − 2.4 − `majfaults_per_op` × 33.5) / misses.

---

## 4. Predictions, registered before any run

### Inputs

- Clear: about 2.4 µs a read with no fault, and about 33.5 µs device a fault (E7, E8).
- Cipher today, B path (b432): 20.4 µs a read at 0.428 misses, so about 42 µs a miss, which is 33.5 device + 8.5 non-device.
- Whole pages remove one fresh page and one fault entry of an isolated chunk's two (E7's split: memfd fault 2.7 µs, extra fault entries 1.8). That predicts 6-7.5 µs non-device a miss, so **40 ± 1.5 µs a miss**.
- The line: m = 1.17(1 − C/660), until S is measured. If S differs, every prediction below is restated with S before its run is read.

### H1 (runs 1-3)

As §1's table. The registered verdict: **H1 holds** within the band at both comparison points. A lean, not a confidence: the kernel's active list may keep the hash slots slightly better, so clear may sit 0-10% under the cipher at the same C.

S is predicted at 640-690 MiB of segment bytes.

### Run 4 (today's cipher in bench-p4080)

- 40-45k reads/s;
- 0.28-0.33x of clear;
- misses 0.684 ± 0.01;
- shmem/held 1.21-1.26.

Anything outside these means the patch's harness or dyn code moved the shipped path.

### Runs 5a-5g (whole pages)

`held` is the pagertrace median over the pass's second half.

| arm | held (MiB) | misses a read | shmem/held | reads/s | ×clear | fate |
|---|---|---|---|---|---|---|
| supdb-cipher (default) | 265-285 | 0.66-0.71 | 0.99-1.03 | 41-47k | 0.28-0.33 | survives; shmem 270-290 against today's 335 |
| x432 | 425-435 | 0.42-0.44 | 0.99-1.03 | 50-56k | 0.34-0.39 | survives; the same misses as b432 and 1-3 µs a miss faster |
| x480 | 470-482 | 0.30-0.34 | 0.99-1.03 | 63-72k | 0.42-0.50 | survives |
| x520 | 510-522 | 0.23-0.27 | 0.99-1.03 | 76-88k | 0.52-0.61 | survives; reclaim continuous (failcnt rise in the millions, as b432) |
| x550 | 540-552 | 0.18-0.22 | 0.99-1.03 | 90-105k | 0.62-0.72 | 60% survives the read pass; a kill or text thrash (reader `majfaults_per_op` above misses + 0.01) is a result |
| xdyn94 | 485-510 | 0.27-0.31 | 0.99-1.03 | 68-78k | 0.47-0.55 | survives |
| xdyn85 | 440-460 | 0.36-0.40 | 0.99-1.03 | 56-63k | 0.39-0.45 | survives |

**For every arm:**
- `read majfaults_per_op` = 0.96-1.00 × misses. That means device faults ≈ misses, since the page cache left for ciphertext serves under 4%, and the tag region is resident: **no more than misses + 0.01**.
- `cipher_breaths` = 0.
- `cipher_peak` ≤ limit × 1.07.

The ×clear columns carry the clear arm's ±15% between sittings. The absolute rates are the comparison.

### Run 6 (xdyn94, load,read)

| quantity | prediction |
|---|---|
| survives both sittings | 60% |
| shuffled-load limit | 370-395 MiB |
| shmem max in the load | 420-470 MiB |
| read pass | as 5a, 0.47-0.55x |
| ordered load | 0.61-0.74x (E7, E8: the write side is unchanged but for the tag region's tail write) |
| shuffled load | 0.55-0.80x, unresolved |
| `load device_bytes_per_byte` | within 1% of today's (the stored length is identical) |

If it is killed: xdyn85 survives, at 0.39-0.45x read.

### Run 7 (scan pass, BEST)

0.55-0.95x of clear, against R8a's 0.40-0.47x for today's cipher. This is the least grounded prediction here: R8b's d432 confounded run preads with the budget, and this run has neither.

### Run 8 (ycsb-C twice, Zipf θ = 0.99, unscrambled, so the hot keys are contiguous)

The first C starts with the pager's cache cold (a fresh process) and is mostly compulsory misses. **The second C (`ycsb-C#2`) is the reading.** Its rate comes from the `phasemark ycsb-C#2` lines.

| arm | misses (or faults) a read, second C | ×clear |
|---|---|---|
| BEST, whole pages | 0.01-0.05 | 0.55-0.95 |
| today's cipher (8b) | 0.04-0.10 | 0.35-0.70 |
| clear at the matched bound (8c) | 0.008-0.04 | – |

---

## 5. Kill lines

**The reading is wrong (H2 refuted as stated)** if any of these holds:
- every whole-page arm holding 500 MiB or more (xdyn94, x520, x550) reads **under 0.45x of clear in both sittings**;
- or shmem/held does not fall to **1.06 or under** for any whole-page arm;
- or misses a read at held 480 or more sit **more than 15% above** k(1 − held/S), with S measured and k re-fitted on runs 1-4.

Then the line does not carry over to the new layout, and the next experiment is a profile of a miss under whole pages (E8's R11, not run).

**The design's layout is worth it for out-of-core reads** if an arm that **survives run 6 (load,read)** reads **0.55x of clear or better in both sittings** of its read cell. "Worth it" means the 4096-byte stored page with the tags apart, plus a budget the layer owns and recomputes.

**Between the two.** For example, x520/x550 at 0.55x+ read-only while only xdyn85 survives the load, or xdyn94 at 0.45-0.55x. Then the layout is necessary and the budget's owner decides the rest. The report says which term moved:
- misses against the line;
- non-device µs a miss against 40 ± 1.5;
- shmem/held;
- headroom (`cache − shmem` and usage).

**H1:**
- **Refuted** as in §1: clear differs from the cipher at the same C by more than 15% (or 0.04) at C ≈ 273 or C ≈ 418.
- **If refuted with clear lower:** the "factor" exists. Whole pages will under-deliver against §4 by about that ratio, and page-granular eviction moves up the design's order of work.
- **If H1 holds and H2 lands in its §4 bands,** the cipher's out-of-core gap is the plaintext the bound lets it hold, end of story, and the remaining gap is budget and headroom.

**Skew (run 8):** megabyte FIFO costs under skew if BEST's second-C misses a read exceed 8c's clear faults a read at matched C by **more than 1.5x**, while run 5's uniform misses lay within H1's band. Otherwise the run does not show it.

**A result that is a kill:** an OOM kill (dmesg `Memory cgroup out of memory`, the sampler's `oom_kill`, rc 1 with `killed under the bound`) is reported with its evidence and not rerun. If x520 is killed read-only, x550 is not run.

---

## 6. What this cannot show

- **Page-granular eviction.** The unit stays a megabyte.
- **The tag tier.** The design keeps the tags as sealed tag pages in a pinned tier. Here they are a plain tail region read through the mapping. Its residency is inferred from the faults count, not read with `mincore`.
- **Asynchronous misses and threaded reads.** Only the single-threaded read pass is run.
- **A budget that subtracts files being written, or two stores in one process.** xdyn takes 94% of its own `read_budget` per store.
- **The swamp line below the OOM.** Recorded, not built.
- **A reopened store that fits.** In-core cells are not run: E8's R9 showed `ab` adopts what it wrote, so nothing is filled in core.
- **Differences under a tenth.** One rep a sitting does not resolve them.
