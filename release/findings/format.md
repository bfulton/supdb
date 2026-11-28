# supdb on-disk format: review before the release tag

Reviewed at C0 = `b3a043dc`, read-only, by reading: no cargo, no tests, no builds.
The checkout's HEAD is `dd5dbbbc`, three release-ledger commits later; `src/`,
`tests/`, `web/` and `Cargo.toml` are identical at the two (`git diff --quiet
b3a043dc HEAD -- src tests web Cargo.toml`), so every line cited below is C0's.
Nothing was ever released before this tag: `git tag -l` is empty and the crate is
0.1.0, so every "older version" the code reads today is a development build's.

Two version numbers matter, and they are easy to confuse. **STORE_FORMAT**
(`src/db.rs:22146`) is the store's version: 2 today, 1 for stores whose manifests
record none, and 3 (`STORE_FORMAT_ONE`) for a store kept in one file. The
**manifest layout byte** (`SUPDBMAN\x01..\x03`) is the slot file's own layout.
Every file also has a magic of its own.

## 1. Inventory

"Old reader" is what a reader from before this structure's current version does
with it. "New reader" is what a reader at or after the tag must do. "Spare" lists
reserved fields: whether each is zero today, and whether anything checks it.
"Bounds" lists each field whose width caps the store, and whether going past the
cap is refused or wraps.

| structure (where) | magic / version | old reader / new reader | spare and reserved | bounds | promise? |
|---|---|---|---|---|---|
| **Segment superblock**, 2 slots at 0 and 512, written identical (`format.rs`, `blob.rs:252-322`, `db.rs:13622`, `15367-15396`) | `MAGIC = 0x5355_5044_4200_0007`. The low bytes are the version, written native-endian as the byte-order mark. Covered by FNV-1a. | 0006 refuses at the magic. Today's `Blob` accepts exactly 0007 (`blob.rs:293`), so a later release that moves the magic must keep 0007 as a second accepted value. | Of 16 u64 fields: f1 `history_from` (=gen) and f15 `index_gen` (=gen) are written and never read. f9-11 (reuse log) are 0, unread and unchecked. f13-14 (redo arena) are 0; a nonzero first log word is refused (`log_probe_range`). The rest of the 4 KiB page outside the extension and the run note is zero and unchecked. | All u64. `high_water <= object_len` is checked. Compressed sections are refused (`blob.rs:362`). | **forever** |
| **Superblock extension** at 1024 (`blob.rs:118-246`) | `SX_MAGIC` plus the generation. FNV over its words and the copy of the 192-byte key-section header. | Optional: a missing or bad extension reads as none, so the sparse reader plans without it. Word 18 (payload) was added without moving the magic (0 means none). | Word 19 is 0, unchecked, under the FNV. | u64 | forever (a sparse open reads it) |
| **Run note** at 2048 (`db.rs:8783-8831`) | `RUN_NOTE_MAGIC` / `RUN_NOTE_CHAINED` ("SUPDRUN1/2"), start seq u64, CRC32C. Stays in the page of every finished segment that came from a run. | A missing note means a run from STORE_FORMAT 1 (`tests/runlog.rs:1945`). | none | seq u64 | forever |
| **Key section** (`flatindex.rs`) | `SFIX` native-endian plus `VERSION = 3`, exact match (`is_flat`). | Exact match only. `REC_VERSION` is copied into form files. | Header words 16-48 (`prev_*`), 144 `dir_cap`, 152 `dir_state`, and 104/112 slack and bump are features of the old in-place `Store`. The segment writer writes them 0 or equal; `flatindex::encode` (public) can still write them non-trivially. Words 176-191 are never written, zero, unchecked, covered by the checksum row. `piece_shift` is held to 10..=24. | Hash slot = `tag8<<56 \| off u32`, directory u32 a key, fence offsets u32: record region <= 4 GiB, refused at finish (`MAX_RECS`, `stream_trailer_in`). nkeys < 2^32 in practice (u32 ranks). Key <= 65,535 (`u16 klen`), refused by `SegmentWriter` (`db.rs:13950`, `14605`, `14788`); see F1. Extents a record < 32,768. Compact run < `COMPACT_RUN`, 14-bit count. The key hash (FNV-1a 64, slot `h & mask`) is part of the format and recorded nowhere. | forever |
| **Extent** (`index.rs:21`) | none (inside the key section) | n/a | `count` bit 31 is TOMBSTONE, bit 30 is FIXED. `block == u32::MAX` means INLINE. | `len`, `off`, `last` u32: a run <= 4 GiB. `count` is 30 bits. | forever |
| **Run markers** inside a run's record region (`db.rs:8772-8781`, `13916`) | 24 bytes: klen 0, n 0, `"SUPD"`, CRC u32, len u32, count u64 | They stay inside finished run segments. Every reader reaches records through the directory and never steps onto one; a reader that walks the region in order has to skip them. | none | batch length u32 | forever (state documented nowhere outside the code) |
| **Block table** (`flatindex.rs:3666-3880`) | `SBLK` native-endian. Entry size u32 must be 32. n u64. | Exact entry size; no version field. | `flags` bits 3-7 and the 7 pad bytes are 0, **unchecked, under no checksum**. | off u64; stored, uncompressed and cap u32. A block or value >= 4 GiB **wraps** (`db.rs:14874` `bytes.len() as u32`). | forever |
| **Blocks** (`block.rs`) | plain, LZ4 (`lz4_flex` block format), or chunked (`u32 chunk, u32 n <= 2^20, starts, CRCs`) | Readers take the chunk size from the header. | CRCs are written as 0 under `SegmentOptions::checksums = false`, and the file does not record that (F5). | u32 throughout | forever |
| **Segment names** (`db.rs:45585-45702`, `24236`, `48141`) | `seg-<id:08>-<end_seq:016>.sup`, `par-...-<lo hex>-<hi hex>.sup`, `pcs-...` | The open takes level, fences and covered sequence **from the name** (`Seg::open` ~16299). | none | id and seq u64 in decimal; fences <= 16 bytes in hex | forever |
| **Ordered index** `ord-<id>-<seq>.oidx`, `x-<tok>.o` under a cipher (`ordindex.rs:198-575`) | `SUPDORD1`, exact. CRC32C over everything. | Wrong magic, wrong CRC or a key-count mismatch is rebuilt from the segment (`Db::ord_rebuilt`). The `UNIFORM_AT` word was added without moving the magic (0 means none). | Words 40-63 are 0, unchecked, under the CRC. | n u64; seek plan u32 | **derived**: rebuilt |
| **Form file** `zpg-<id>-<seq>.zpg`, `x-<tok>.z` (`zpages.rs`, `zfile.rs`) | Block body `SUPDZPG2` version 2, page tag `ZPG2`. Stride body `SUPDZST1`; the sparse stride form is a second trailer version. Pages are bound to their segment by content (`bind32`, the segment's row CRC, sizes, timestamp, generation, rec version and magic, id, end seq). | A refusal at the trailer means rebuild. The trailer's zeros (212..CRC) and the page's zero byte **are checked**. Named by no manifest, except as rows of a one-file store. | checked zeros | page and rank u32 | **derived**: rebuilt |
| **WAL** `wal-<id:08>` (`db.rs:8766`, `9016-9301`, `10375-10506`) | `SUPDBWL` plus a kind byte: 4 chained, 5 loose, 6 loose-run (6 is STORE_FORMAT 2). A sector of zeros at the start reads as an empty log. | An unknown kind is refused ("not a supdb WAL"). **An unknown frame kind ends the walk as a torn tail and is not refused** (`10500`). | none | Frame `len u32 \| crc u32 \| seq u64 \| kind u8 \| uvarint klen \| key \| value`. `body as u32` is **unchecked** (`9243`, `9287`), so a record >= 4 GiB wraps and the replay cuts the log there. The sealed log uses `u32::try_from` and refuses (`vault.rs:2674`). seq u64. The CRC seed is u32, splitmix of the file id. | **forever**, including crash states |
| **Recycled / spare log** `spare-<id>` (`db.rs:44980`) | header rewritten in place | Stale frames fail by seed. Under a cipher, a last life's header at a rotation is read as empty **only when `recycle_wal` is on at the open** (`10357`). | n/a | n/a | disposable, but the crash state is promised |
| **Manifest slots** `manifest`, `manifest.alt` (`db.rs:22096-22806`) | `SUPDBMAN\x03 \| seq u64 \| len u32 \| crc32c(seq, body) \| body`. The body is `covered u64, n u32, (u16 len, name, flag u8)*`, then `format u32` and `needs_log u64`, both additive: a reader that stops at the names ignores them. Slots are never truncated, so a stale tail follows the body. | \x01 (no seq) and \x02 (no flags) are read. A layout byte above 3 is **refused as Unsupported, not taken for torn** (`22675`). STORE_FORMAT is checked against every valid slot before anything changes (`46329`). | Flag bits 1-7 are **ignored, unchecked**. | name <= 65,535; n u32; body u32 | **forever** |
| **Rename-shape manifest** (`manifest_slots = false`), `manifest.tmp`, `manifest.prev-N` | same bytes, one file | phase 1 keeps the reader | n/a | n/a | forever (reader only) |
| **Sealed slot** (cipher) (`vault.rs:2495-2547`, `2622`) | `SUPDBMAE\x01 \| seq \| len u32 \| nonce \| ct \| tag`. The plaintext is `u32 len \| manifest \| (token16 \| wrapped)*`. | Any other magic is `InvalidData`, which reads as **torn**: there is no newer-version refusal (F3). | none | u32 | forever if the cipher store is promised |
| **Run temp** `direct-<id:08>.tmp` (`db.rs:45581`) and **`cov-<a:016>-<b:016>`** (`48080-48100`) | A run file is a segment open for append: records-first, markers, the note at 2048. A `cov` file is empty or holds the run's first key. | Recovery reads both. | n/a | seq u64 | **forever** (crash recovery) |
| **Aux names**: `LOCK` (empty, `flock`, `db.rs:29865`), `*.guard` hard links, `dir-lost` (empty, `9178`), `wal-*.aside` (copies, never read), `seal-/compact-/tier-*.tmp` (swept) | names only | The open restores guards and honours `dir-lost`. | n/a | n/a | LOCK, guard and dir-lost names forever; the rest disposable |
| **Store in one file** `store.one` (`onefile.rs:66-98`, `444-484`, `1034-1150`) | Head `SUPDBONE \| version u32 = 1 \| crc32c`. 1 MiB head kept "for the stages after this one". The table rides behind the manifest's body: `SUPDBEXT \| len u32 \| crc(seq, body) \| end u64 \| n u32 \| (u16 name, off u64, len u64, first-page crc u32)*`. The manifest records format 3. | **The head's version is written and never checked at open** (`One::open` checks magic and CRC only). | head page past 16 bytes | u64 offsets | experimental (F2) |
| **Envelope** (`crypt.rs:22-300`), `tests/data/crypt-v1.bin` | `supdbAE\0 \| version u32 = 1 \| shift u32 (9..=24) \| plain_len u64 \| id 16 \| key_id u64`. Zeros to 4096 are checked. Each chunk is `1<<shift` with its tag; the nonce is `id[..8] \|\| u32 index`; the AAD is the header. | Exact version. Rust and JS both read the fixture. | none (the zero pad is checked) | > 2^32 chunks is refused | **forever** (published objects) |
| **Vault marker** `supdb.cipher` (`vault.rs:481-514`) and envelope names `x-<32 hex>[.o\|.z]` | `supdbCS\0 \| version u32 = 1 \| zero u32 \| key_id u64 \| store id 16 \| wrapped key` | Version and zero word are checked: exact. | zero word checked | n/a | forever if the cipher store is promised |
| **Kernel encryption** (`dirkey.rs`) | No supdb file. The directory's fscrypt v2 policy is the mark. | n/a | n/a | n/a | nothing to promise |
| **Checkpoint** (`Db::checkpoint`) | A store directory: a manifest and hard links | same as a store | n/a | n/a | covered by the store |

## 2. Findings

Ordered by severity. Each finding gives where it is, why it matters, and the fix.

**F1. High. A key over 65,535 bytes is silently truncated in release builds.**
- **Where:** `MemEntry.klen: u16` and `Entry.klen` (`db.rs:17237`, `17263`) are set by `key.len() as u16` at `19876`, `19885`, `19925` and `21671`, guarded only by `debug_assert!` (`19862`, `19916`). `Db::put`, `append`, `delete` and `Txn` (`47649-47708`, `48402-48484`) check nothing. The ordered path declines such keys (`48290`) and sends them down the general path. The WAL frame keeps the full key (uvarint length).
- **Why:** In release, a 70,000-byte key enters the memtable as its first 4,464 bytes. A read of the key misses. The seal then writes the truncated key, because it fits the segment writer's u16 check. Replay truncates it again. A checked build panics instead. The format's 65,535-byte key bound is the promise, and the API does not enforce it. The fuzz never draws such keys (`tests/fuzz.rs:714`), and ROADMAP's "No key or value can panic or stall the store, as a contract" is still open.
- **Fix:** Refuse keys longer than 65,535 bytes at every write entry point: an error kept for the commit, as `Db::lost` keeps one, or a documented panic. State the bound in the crate docs. Add a test that puts, reads, flushes and reopens a 65,536-byte key.
- **Status:** found by reading. Confirm it with that test before acting.

**F2. High, a decision. STORE_FORMAT 3 is an experimental layout, and the tag's reader accepts it.**
- **Where:** `check_store_format` accepts `OLDEST..=2` and also `== 3` (`db.rs:22162`). `Options::one_file` is hidden and `keep-experimental` in `release/options.csv`. ROADMAP's "The store in one file" still plans to move the log, the manifest slots and the ordered run into the file. `onefile::HEAD` reserves 1 MiB for exactly that. The head's version is never checked (above).
- **Why:**
  - Any store written with the option on would have to be read forever in a layout still being designed.
  - The numbering puts a layout where the next version number would go: the next plain-store version must skip 3. A reader from this tag would also parse a future plain "3" as a one-file store. That fails as a "manifest is truncated" error rather than as "a newer release's".
- **Fix:** Before the tag, do one of two things:
  - Refuse format 3 in the tag and say one-file stores are unsupported; or
  - Move the one-file layout off the version line, either to a flag or to a reserved high number (for example `0x8000_0000 | n`), so the line stays 1, 2, 3, ...
  
  Either way, check the head's version.

**F3. High, a decision; plus a Medium bug inside it. The format of a store under a cipher is still moving.**
- **The decision:** ROADMAP plans four layout changes for `Encryption::Cipher`: "The page layer", "An envelope written beside its segment" (a second layout), "An envelope whose chunks are whole pages", and "An ordered run under a cipher as its own log". `Options::encryption` is public and `keep-stable`. Recommendation: say plainly in the tag whether a cipher store's directory is covered.
  - If it is covered, every later layout is a version bump plus an upgrade path from envelope-per-file v1.
  - If it is not, mark `Encryption::Cipher` experimental for the format and refuse a cipher store across releases until it is stable.
  - `crypt` envelopes published with `encrypt` are separate. They are already fixed at v1, fixtured, and should be promised.
- **The Medium bug:** the sealed slot has no newer-version refusal. `open_manifest` answers anything but `SUPDBMAE\x01` with `InvalidData` (`vault.rs:2525`, `refused` at `395`). `manifest_read_all` treats that as torn and takes the other slot (`db.rs:22798`). The plain slot refuses a newer layout byte as `Unsupported` for exactly this reason (`22675`). So a later release that bumps the sealed layout, and leaves one old slot beside it, gets a misread from this tag. That breaks the README's "never opens a store it would misread".
- **Fix:** read `SUPDBMA` plus `E` plus the version byte, and return `Unsupported` for a version greater than 1, as `manifest_parse` does.

**F4. Medium. No store from this tag is on record, and the upgrade path is tested only on synthesised files.**
- **What exists:** STORE_FORMAT_OLDEST = 1 and the upgrade at open are tested: `db.rs:22809-23100` (the `store_format` module) and `tests/db.rs:8808` for a v1 manifest. That covers versions 1 to 2, a newer format refused before the directory changes, a newer layout byte refused, format 1 in the older slot, and a store from before versions. `tests/runlog.rs:1945` covers a run with no note.
- **The gap:** every one of those tests takes a store this tree writes and rewrites one marker: the manifest bytes or the run note. No test opens files a different build wrote. The one byte fixture is `crypt-v1.bin`. `store_format_names_the_files_magics` pins six magics and no layout, so a layout change made without moving a magic passes it. The magics it leaves out include SFIX v3, SBLK, SX, SUPDBMAE, supdbCS, SUPDBWE, SUPDBONE and SUPDBEXT.
- **Fix:** section 4.

**F5. Medium. Whether a segment's block checksums are real depends on an option that is not in the file, and that option is process-wide.**
- **Where:** `SegmentOptions::checksums = false` writes 0 for every block CRC and chunk CRC (`block.rs:399-439`, `db.rs:14876-14885`), through `block::CHECKSUMS`, a static set by whichever writer opened last (`db.rs:13765`, `14424`). Reads verify by `opts.segment.checksums` (`db.rs:24498`) and are skipped wherever the static is off (`blob.rs:2066`).
- **Why:**
  - A store written with checksums off and opened with the default refuses every value that lives in a block.
  - The browser's `Blob` refuses such a file too.
  - Two stores in one process with different settings write zero CRCs into the store that wants them; `tests/checksums_off.rs` puts its test in a process of its own because of this.
  - The option is `keep-stable` and public, so this ships.
- **Fix:** record "no checksums" in the file: a BlockRec flag bit, which is free today, or a superblock field such as f9, which is unread. Then drop the static and pass the setting per writer.

**F6. Medium. The public writer API reaches layout variants the engine never writes, so the promise would cover them.**
- **Where:** `pub mod flatindex` exports `encode` / `encode_inline` with `prev` checkpoint words, `insert_slack` (a second directory and the `dir_state` publish word) and `record_slack` (`recs_cap` and `bump`) (`flatindex.rs:1725-1815`). Every reader still parses these (`parse_head` at `2168-2206`). They are the old in-place `Store`'s.
- **Fix:** before the tag, make the store-era knobs crate-private, so that public writers produce only what `SegmentWriter` produces. Then a later release may refuse `dir_cap != 0` and nonzero `prev_*` without breaking any file this tag could make.

**F7. Medium. A format change the ROADMAP already names.**
- **Where:** the High item "A scan's limit counts the keys it answers" says "Open: whether a partition may hold a record with no value, a format change."
- **Fix:** decide it before the tag. Otherwise its first consequence is a segment magic 0008 and a reader that has to keep 0007.

**F8. Low. Spare fields are not checked, so none of them is spare.**
- **Unchecked fields:**
  - superblock f1, f9-11 and f15, and the zero page outside its parts;
  - SX word 19;
  - key-section words 176-191;
  - BlockRec flags bits 3-7 and its pad (under no checksum at all);
  - ordindex words 40-63;
  - manifest name flag bits 1-7;
  - the one-file head version (above).
- **Why:** a later release that gives meaning to any of them is silently ignored by this tag's reader. By contrast, form files check their zeros (`zpages.rs:4157`, `738`), and the envelope and vault marker check theirs.
- **Fix:** either check them (zero) now, free before the tag, or write down that any use of them moves the magic or STORE_FORMAT.

**F9. Low. Lengths past u32 wrap where they should be refused.**
- **Where:** the WAL frame `body as u32` (`db.rs:9243`, `9287`). The memtable chunk `value.len() as u32` (`20027`, `20036`), where `TOMB_LEN = u32::MAX` is also a sentinel. The block `bytes.len() as u32` (`14874`).
- **Why:** a value at or over 4 GiB is written with a wrapped length. The replay then cuts the log at that frame and drops the commits after it. The sealed log already refuses (`vault.rs:2674`).
- **Fix:** refuse at the API, and document the value bound beside F1's key bound.

**F10. Low. Unknown WAL frame kinds read as a torn tail.**
- **Where:** `db.rs:10500`.
- **Why:** a later release that adds a frame kind without moving the header kind byte or STORE_FORMAT would have this tag drop every commit after the new frame, with no error.
- **Fix:** say so in the format notes. Optionally, refuse a kind above 2 inside a batch whose commit frame verifies.

**F11. Low. The open upgrades a store in place, silently and for good.**
- **Where:** `db.rs:46336`, `46662-46700`.
- **Why:** by the same mechanism, a later release that opens a store from this tag makes it unreadable to this tag. Nothing asks first and there is no read-only open.
- **Fix:** say it in the statement. Consider making an upgrade a separate call or option, or making a checkpoint first.

**F12. Low. Read paths for development-era stores would be promised forever.**
- **What:** STORE_FORMAT_OLDEST = 1 brings in the scan of a directory with no manifest (`db.rs:46310-46315`, `46650`), SUPDBMAN v1 and v2, v3 bodies without the format word, and runs with no note. No release ever wrote these, and no real-file fixture exists for any of them.
- **Recommendation:** promise only STORE_FORMAT 2, which is what the tag writes. Keep the older paths as best-effort, or drop them in phase 1. phase 1 already keeps the v1-manifest reader on purpose.

**F13. Low, wasm. The 4 GiB refusal cannot fire, and 64-bit fields are narrowed without a check.**
- **Where:** `supdb_host_len() -> u32` (`wasmapi.rs:49`), with the JS side returning `handle.getSize()` and `cache.length` (`web/supdb.mjs:369`, `411`), so an object of 4 GiB or more arrives with its length taken mod 2^32. `src.len() >= MAX_OBJECT` (`305`, `648`) then fires only at exactly 0xFFFFFFFF. `supdb_open_plan(..., object_len: u32)` is the same.
- **Why:** reads past the wrapped length fail as short, and `off as u32` is reached only below it, so nothing is misread. The error is wrong, though. On wasm32 the header parsers also cast u64 words to usize unchecked: for example `rd_u64(..) as usize` throughout `Header::parse` and `parse_head`, and `sb.key_stored as usize` in `blob.rs` (`as usize` appears 61 times in `blob.rs` and 104 in `flatindex.rs`, not all of them on u64s). They rely on the bounds checks and the checksum row to hold, and the browser takes objects from elsewhere: ROADMAP's "read-only parsers fuzzed behind a valid checksum" says the wasm ABI is not driven at all.
- **Fix:** pass the length as an f64, BigInt or two halves and refuse at or over 2^32 in JS. Use `usize::try_from` in `Super::decode`, `Header::parse` and `parse_head`.

**F14. Low. Replay of a cipher crash state depends on an option at open.**
- **Where:** `recycle_wal` at `db.rs:10357`.
- **Why:** a store written while recycling, which is the default, and opened with recycling off refuses a crash left at a rotation. The option is a hidden test seam, so this matters little.
- **Fix:** read the state whatever the option says.

## 3. The questions asked

1. **What must be read forever, and what is derived.**
   - **Forever:** segment files (superblock, extension, run note, key section, extents, compact records, markers, block table, blocks and chunks); segment names, which carry fences and the covered sequence; both manifest slots (v3, and the v1/v2 reader that phase 1 keeps); the WAL and everything a crash leaves (loose kinds, a zero first sector, a recycled header); run temp files with their notes and markers; `cov-` files; the LOCK name; `.guard` and `dir-lost`; checkpoints; published `crypt` envelopes; and, if F3 decides yes, the vault marker, sealed slots, the sealed log and `x-` names.
   - **Derived, no promise:** ordered indexes and form files of every kind (block, stride, sparse stride). Each is refused at its magic or identity and rebuilt; `zpages` is hidden from the docs as "not yet an interface". Also derived: Bloom filters (memory only), `spare-*`, `manifest.prev-*`, `*.tmp` of seals and merges, and `wal-*.aside`.
   - **Caveat:** in a one-file store, ordered indexes and form files are rows of the manifest's table. That is one more reason to keep format 3 out of the promise.
2. **Experiments the tag would freeze.**
   - `one_file` / STORE_FORMAT 3 (F2).
   - The `Encryption::Cipher` layouts (F3).
   - `manifest_pending`'s flag byte. It is `keep-experimental`, but the byte is part of v3 anyway.
   - `SegmentOptions::checksums` writing zeros nothing records (F5).
   - The public `flatindex` store-era variants (F6).
   - The record-with-no-value question (F7).
   
   No longer an issue:
   - The form-file codings that phase 1 deletes (`zpages_record_coding`, `zpages_raw_tails_first`, the `zpages_group_bytes` constant): those files are derived.
   - `compact_records`, inlined at true: readers keep both forms.
   - `manifest_slots`, whose rename writer goes and whose reader stays: both freeze nothing new.
3. **STORE_FORMAT_OLDEST and the upgrade path.** Both are tested, from version 1 (several spellings of it) to 2, and the newer-version refusals are tested. All of it is on synthesised files (F4). Nothing tests a format-3 store's upgrade, or a newer sealed slot (F3).
4. **What to check in.** See section 4.
5. **The wasm reader's limits.** One object is at most 4 GiB (u32 offsets and length). The refusal of a larger one cannot fire (F13). nkeys and generation are returned truncated to u32. Counts go through hi/lo halves and are exact below 2^53. The format's own u64 fields (superblock, extension, key-section header) fit the 4 GiB object by construction; on wasm32 they are narrowed to usize without a check (F13).

## 4. Fixtures to check in at the tag

Model them on `crypt-v1.bin`:
- **Made once, by hand:** an `#[ignore]` writer test of the kind `write_the_envelope_on_record` is (`tests/crypt.rs:378`).
- **Never written again:** a later layout change gets a new directory beside the old one, never a rewrite of it.
- **Read by a test that is always on.**

Proposed files under `tests/data/store-v2/` (a few hundred KB in all, deterministic inputs):

- **`clean/`**: a store from `Db::create`, loaded and closed. It should hold:
  - partitions (`par-`) and a level-0 piece (`pcs-`);
  - inline runs, both compact and full;
  - fixed and prefixed runs;
  - multi-value keys and tombstones;
  - values in blocks above `inline_bytes`;
  - the empty key and the empty value;
  - a key at 65,535 bytes;
  - both manifest slots and the LOCK.
  
  Leave out the ordered indexes and form files, so the test also shows they are rebuilt. Optionally add one stale form file, to show it is refused rather than misread.
- **`crashed/`**: the same store after `Db::crash`. It should hold WAL frames past the manifest (with a loose kind), a `direct-*.tmp` run with its note and markers, a `cov-` file, and a `.guard`. This is what a later release must recover.
- **`segment-*.sup`**: standalone `SegmentWriter` output in both layouts (records-first and blocks-first, compressed and chunked), for `Blob`, `SparseBlob` and the browser. `web/` should read it under Node as `crypt.test.mjs` reads `crypt-v1.bin`.
- **`cipher/`**, only if F3 decides the cipher store is covered: the clean store under `ring` AES-GCM with a fixed test key.

The tests should:
- Copy each directory to a temp dir first, because an open sweeps, replays and upgrades.
- Open it with `Options::default()`.
- Hold every get, count and scan to a listing checked in beside it, or to the deterministic generator.
- Then write, flush, reopen and read again.
- Add one byte-level test: the current `SegmentWriter`, given the same input, writes `segment-*.sup` byte for byte, with the superblock's timestamp words and its two FNVs masked (the helper in `tests/common` already masks the second). That catches a layout change made without moving a magic, which nothing catches today.

## 5. Proposed compatibility statement for the tag

> A store written by this release (store format 2), and every file it holds that is not derived (segments, manifest slots, logs, and the run and recovery files a crash leaves), will be opened, recovered and read by every later release, which refuses rather than misreads anything it cannot read. Ordered indexes and compact-form files are caches that any release may refuse and rebuild, and no promise covers them. A later release that changes the format upgrades the store in place when it opens it, and from then on this release refuses that store. Files are little-endian, keys are at most 65,535 bytes, and an object for the browser reader is under 4 GiB. Encrypted objects written with `supdb::crypt` stay readable as envelope version 1; stores kept in one file and stores under `Encryption::Cipher` are experimental and carry no format promise. *(The last clause depends on F2 and F3.)*
