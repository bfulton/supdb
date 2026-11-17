# Pagefile: a sealed page file under the vault door (final, after verification)

The design starts from the synthesis draft (`design-draft.md`): every file of a store under `Encryption::Cipher` goes under one page layer that knows no formats. Five verifiers read it: cryptography, crash consistency (with a model in Python), engine fit, performance, and completeness. This version applies every fatal and serious finding. Where a finding could be answered from the code instead of by a change, the code is cited. Citations are to the tree at `3df85d22` and were read unless marked "not verified". What could not be settled is listed under §12 and in the open list.

The large changes from the draft:
- **Keys.** The store key K_s no longer seals anything under a random nonce. Each file gets a key derived from K_s and its file_id, and headers, key wraps and names are sealed under that.
- **Nonces.** Every nonce under a session key comes from the Handle's own counter, never from an engine value.
- **Tag pages.** Tag pages are sealed, and the records carry their MACs, so a tag entry's version, presence and session are authenticated.
- **No header sync.** No sync is ever issued on its own. The header rides the file's first sync.
- **Log recovery.** Recovery verifies back to the last synced home, and re-homes tag pages that never landed.
- **Recycle.** A recycled file draws a new file_id and drops its old sessions.
- **Run output.** An ordered run's output leaves the Log class at its close and becomes a trusted Once file.
- **Ciphertext mapping.** A finished file is read through a read-only mapping of its ciphertext and holds no descriptor. `ru_majflt` keeps counting device reads, and the census is unchanged.
- **Write-through.** A file being written is written through into the pager's plaintext backing. That backing is the run's held view, and it keeps in-core first reads hits.
- **Sweep.** The open's sweep never deletes a name that fails to authenticate.
- **Marker.** The marker's version moves to 2.

## 1. The seam

The cipher sits below every formatter and above the kernel, in one module (`src/db/pagefile.rs`) behind the `vault::` door. The door returns `vault::File { Os(std::fs::File), Pg(Arc<pagefile::Handle>) }` where it returns a raw `File` today. Every write, read, sync, writeback hint and fadvise is one match beside its syscall.

**Above the door**, `SegmentWriter`, the direct run, `write_ord`, the form builder, the WAL and the manifest write plaintext at plaintext offsets, byte for byte what a plain store writes.

**Below the door**, each logical file is one sparse ciphertext file. Plaintext page p is exactly one 4096-byte stored page, and its tag is kept apart.

The layer knows these things and nothing else:
- pages;
- logical lengths;
- per-page versions;
- sync points;
- sealed names;
- one durability class per file, given at create;
- one generic transition, `finish`, which says "this file will not be written again".

It knows nothing of segments, runs, forms, tokens, fences or companions.

This is the owner's "after the compression interface and before the disk interface" taken literally:
- Every compression, layout and record decision is already made in the plaintext bytes the formatter hands down.
- The layer's vocabulary is the file API the kernel offers: write at an offset, sync, rename, link, map, advise.
- What the layer adds is the classes. They are I/O contracts a plain filesystem gives for free, such as "trusted only after its final sync" or "a torn rewrite reads as damaged". They are not forms.

**Platform.** It is Linux-only, as `Cipher` is today: `memfd_create` (pager.rs:805-826), `MADV_POPULATE_WRITE`, `MADV_REMOVE`, and README.md:289-290. A portable plaintext backing is possible, but it is not designed here. See decision 5.

## 2. Door surface and the file disciplines

```
enum File { Os(std::fs::File), Pg(Arc<pagefile::Handle>) }
  write_at, write_all_at, write_vectored(_at), append(&[IoSlice]),   // O_APPEND: the layer owns the offset
  read_at, read_exact_at, read_to_end, seek, set_len, len, metadata_len,
  sync_data, sync_all, try_clone,
  writeback(at, len, Wait),              // Wait::No = SYNC_FILE_RANGE_WRITE (db.rs:6072-6085);
                                         // Wait::Both + drop = zfile's drop_written_range (zfile.rs:3920-3943)
  advise(at, len, Advice),               // WillNeed / DontNeed / Random: posix_fadvise or madvise
  finish(),                              // no write follows; Log -> trusted Once (see §4)
  as_os() -> Option<&std::fs::File>      // plain-only sites that need a std File (see the list below)
vault::create(path, Class) / create_rw(path, Class) / open / open_write / open_append / open_dir
vault::map(path) -> MmapBytes            // plain mmap, or MmapBytes::paged over the pager (bytes.rs:365)
vault::open_mapped(path, cap)            // RunMap's growing view (db.rs:11461): maps the pager's backing
vault::rename / hard_link / remove_file / names / len / exists / read / sync_dir
vault::preallocate(file, len)            // plain arm = today's zero loop (db.rs:4252)
vault::recycle(spare, path)              // plain arm = today's rename + open_write (db.rs:4280)
vault::paged(path) -> bool               // a predicate open_seg selects by, so the plain open is unchanged
```

The class reaches `SegmentWriter::create` through a field of `SegmentOptions`. That function is public, and seals (Once) and the direct run (Log) share it (db.rs:6626, 8021). The field is read once, at create.

**Sites that take a raw descriptor today.** S1 routes each through the door, translating offsets to stored offsets under `Pg`:
- `start_writeback` (db.rs:6071-6085);
- `fadvise_dontneed_range` and `fadvise_dontneed` (db.rs:14904-14931);
- zfile's `drop_written_range`, `drop_behind` and `swap_ahead` (zfile.rs:3920-4046). `swap_ahead`'s bare `File::open(form)` (zfile.rs:3972) becomes `vault::open`.
- `ZFile`'s `file: Option<File>` for `drop_cache` (zfile.rs:3255-3259);
- `Vec<Option<std::fs::File>>` (db.rs:12762).

The plain-only sites keep their bodies byte for byte through `as_os()`: `memmap2::Mmap::map(&file)` in the plain build (zfile.rs:4265), `MmapOptions::map` in RunMap's plain arm (db.rs:11469), and `metadata().len()` in `open_append` (db.rs:4326).

The door test `the_engine_names_no_file_but_through_the_door` (vault.rs:2876-2905) is extended in two ways:
- it reads zfile.rs, ordindex.rs, bytes.rs and pager.rs as well as db.rs;
- it bars `File::open`, `as_raw_fd`, `posix_fadvise` and `sync_file_range` outside the door module.

**Calls that go away:** `persist`, `persist_one`, `persist_form`, `companion`, `kept`, `Kind`, `passes`, the memfd write-once table, and tokens.

`Mapped`/`MappedFile::owed` **stay**, renamed as the pager's "mapped backing" (see §4, write-through). The draft said they would go. They cannot: the run's reads after its close would otherwise decrypt what was just written. CLAUDE.md records exactly that regression.

There are five classes (the draft's four, with Regen split in two), chosen at create. Each is a contract that holds whatever the file contains.

| class | contract | logical names |
|---|---|---|
| **Once** | Trusted once it carries a verified `FINAL` record, or while the Handle that wrote it is alive in this process. A file without either is untrusted, and its failures are its own (§4, Trust). Patches before `FINAL` are made in place. | seal and merge outputs `seg-`/`pcs-`/`par-*.sup` and their temps `compact-*.tmp`, `tier-*.tmp`, `seal-*.tmp` (db.rs:12715, 12983, 27751, 29406) |
| **Log** | A process crash keeps every write call. A power loss keeps the last synced state, and a torn tail is the end. A page that is home and covered by a synced checkpoint is never overwritten, except at a new version under the rules of §4. `finish` makes it a trusted Once file. | run temp `direct-*.tmp` (db.rs:28696), `wal-*`, `spare-*` (db.rs:26970-26974) |
| **Replace** | Rewritten whole in place. A torn rewrite reads as damaged, never as a mix of versions. | `manifest`, `manifest.alt` (db.rs:11595); `manifest.prev-*` are hard links of these, under the same Handle |
| **Regen** | Regenerable, and verified whole at every open of the store, as the plain store verifies it whole. A failure at that verification is the file's `InvalidData`, and the file is recomposed. After that, the file is trusted. | ordered index `ord-*.oidx` and its `.tmp` (db.rs:27252, 127) |
| **Regen-lazy** | Regenerable, never verified whole. A failure anywhere is the file's: the form is taken out of service, never the store. | form file `zpg-*.zpg`, `zpg-dead-*.zpg`, its temp (zfile.rs:88-89, 3726-3727, 6136) |

The draft called the ordered index `.o` and the form file `.z`; those are today's stored suffixes. The rename-shape manifest arm (`manifest_slots: false`, which writes `manifest.tmp`; db.rs:11773-11798) is refused by name under `Pg`. It is an arm priced in the clear, and its rename over the manifest is the one hot rename-over that §4's rule would put a directory sync on.

**The marker** `supdb.cipher` is the layer's root, written as today: temp, sync, rename, directory sync (vault.rs:434-448). Its version moves to 2 (`MARKER_VERSION`, vault.rs:363-364, refused unless equal at vault.rs:384-390).
- A v1 store, today's branch format, is refused by name, and nothing in its directory is touched.
- During S3-S5 both formats exist. The marker records the format, and each arm opens its own.
- `rewrap` is unchanged, because it re-wraps K_s and does not rotate it (vault.rs:405-428).

## 3. On-disk layout and keys

The page is P = 4096. A group is 128 pages (512 KiB), and an extent is 65,536 pages (256 MiB).

| stored offset | size | content |
|---|---|---|
| 0, 4096 | 4 KiB each | Header A and header B. Clear: magic `SUPDBPF2`, version u16, class u8, file_id[16], nonce12. Sealed under K_f: hdr_seq u64, and a session table of up to 48 entries {session key wrapped under K_f (nonce12, ct32, tag16), base_page u64, kind u8 (`restart` or `continue`), first_seq u64}. The newest valid hdr_seq wins. |
| 8 KiB .. 1 MiB | 1016 KiB, sparse | Journal ring of frames (Log, Replace), and the `REC` slots A and B (Once, Regen) |
| `E(e) = 1 MiB + e·258 MiB` | 2 MiB | Tag region of extent e. The tag page of group g (g < 512) is at `E + g·4096`. Each holds 128 entries of 32 B and is AEAD-sealed. |
| `E(e) + 2 MiB + (p & 0xFFFF)·4096` | 4 KiB | Data page p, with e = p >> 16 and group (p >> 7) & 511 |

**Address math.** Only shifts, masks and adds. Data starts 1 MiB-aligned, and plaintext offset k MiB lands at a 1 MiB-aligned stored offset, so a writer's 1 MiB piece (db.rs:5713-5747) is one aligned stored write. The kernel then builds the folios it builds for a plain store, which is the folio rule in CLAUDE.md. The pager's unit is 256 pages, exactly 1 MiB, and `Per`'s multiply goes.

**Overhead.** 0.78% of the data in tag pages, plus a 3 MiB prefix. The prefix stays a hole except for the headers and the frames or records actually written. The apparent size is therefore at least 3 MiB a file, while the allocated size is small. A copy that does not keep holes, or a filesystem without them (FAT/exFAT, some SMB), inflates every file by up to 3 MiB (decision 6). `bench`'s size counts allocated blocks (bench/src/engines.rs:342-364), so the suite is unaffected.

**Tag entry** (32 B): tag16, version u32, rsv 12. There is no session byte and no PRESENT flag. The session that sealed page p is derived from the authenticated table (§4, Ownership), and presence is "below the authenticated length".

A tag page is sealed in place under the session key of the session that writes it. Its 16-byte MAC goes into the records:
- a `GROUP` frame and every later `CKPT`, for Log;
- the `FINAL` record, for Once and Regen;
- the `REC`, for Replace.

**Frame:** a clear part of 16 B (magic u16, session idx u8, kind-hint u8, seq u64, body_len u32), then a body sealed under the session key. Its entries:
- `TAIL{off, bytes}`: the bytes appended since the previous frame, never the whole partial page;
- `HOME{first, count, versions, tags}`: pages sent home in this call;
- `GROUP{g, version, mac}`: group g's tag page went home;
- `OVR{page, version, ct4096, tag}`: an override image;
- `TRUNC{len}`;
- `CKPT{len, synced_home, home, tail page image, open-group tags, override list, group MACs since the last restart}`;
- `REC{len, flags (FINAL), tag-page MACs, the open group's tags, override list}`: the Replace record, the Once and Regen close.

**Names.** A directory entry is:

`x` + base64url(file_id16 ‖ nonce12 ‖ AEAD_Kf(logical name, padded to a multiple of 16 ‖ name_seq u64 ‖ flags u8 ‖ prev_fid_lo u64) ‖ tag16)

The AAD is `"name"` ‖ store_id ‖ file_id.

The longest logical name is the `par-` form, 115 bytes with u64 fields at full 20-digit width (crypto verifier; db.rs:12710-12714, FENCE_MAX db.rs:4724). It pads to 128, so the entry is 16+12+(128+17)+16 = 189 bytes, which is 252 base64url characters plus `x` = 253. That fits under NAME_MAX 255, with two characters to spare. A full 16-byte prev_fid would take it to 263, which is why only the low half is kept. The low half is only a check that the inode is the previous life's (§4, Recycle).

Shorter names give shorter spellings. The "sealed shape" is `x` plus base64url of at least the minimal decoded length. The budget is a test at S2 (`the_longest_logical_name_seals_under_name_max`), since two characters is the margin; not verified beyond this arithmetic.

The file_id is in the clear in the entry. It is random, and it reveals only that two entries are links of one file, which the inode numbers reveal anyway.

### Keys and sealed objects

| object | key | nonce (96 bit) | AAD |
|---|---|---|---|
| store key K_s in the marker | the caller's | as today | as today |
| file key K_f | **derived**: K_f = AEAD_Ks(nonce = file_id[0..12], 0^32).ct | (deterministic) | `"pfkdf"` |
| header A/B body | K_f | random 96 | `"pfh"` ‖ store_id ‖ file_id ‖ class ‖ slot |
| session key wrap | K_f | random 96 | `"pfk"` ‖ store_id ‖ file_id ‖ idx |
| sealed name | K_f | random 96 | `"name"` ‖ store_id ‖ file_id |
| data page p | session key S_i | `0x10` ‖ p (u56) ‖ ctr u32 | file_id ‖ p ‖ ctr ‖ valid_len u16 |
| tag page g | S_i | `0x12` ‖ g (u56) ‖ ctr u32 | file_id ‖ g ‖ ctr |
| frame / `REC` | S_i | `0x11` ‖ seq u64 ‖ 0 (u24) | file_id ‖ seq ‖ body_len ‖ session idx |

**K_s seals nothing under a random nonce.** Its only use is the derivation, one call per file_id. Calling it again with the same nonce and the same plaintext gives the same output and reveals nothing more. That needs the hook to be a deterministic AEAD, which every standard AEAD is (stated as an assumption of the hook). The derivation's nonces are distinct while file_ids are, and file_ids are 96 random bits, so a collision reaches 2^-32 at about 2^32 files. A store creating ten files a second takes 13 years to make 2^32.

Under a K_f, the random nonces are few: a file has two header slots, one wrap per session, and a name per create, rename or link, which is tens per file over its life. The draft's lifetime budget of about 2^28 sealings under K_s (crypto finding 3) is gone. The derivation is built from the `Aead` hook alone (crypt.rs:141-151) and needs no hash primitive.

**A session** is a Handle's writing life within one open of the store. It has a fresh random 256-bit key and a counter `ctr` that starts at 0 and advances at every sealing attempt, whatever the I/O result. Every data page, tag page and frame takes the next ctr, so (p, ctr) and seq never repeat under one key.
- A write that fails and is retried reseals at a new ctr. That answers the retry finding (db.rs:4436-4456: `pending` is resent at the same offset).
- A Replace page's version is the ctr, never the manifest's seq (db.rs:27668, 37730).
- An `OVR`'s version is the next ctr, so a second patch of one page cannot reuse a nonce.
- At ctr = 2^32 the session ends and a new one begins (16 TiB of page sealings).

A unit test enumerates every nonce format and asserts the domains are disjoint. A hook under the crash fuzz records every (key, nonce) sealed and asserts that none repeats.

**The Handle registry** belongs to the store's open `Vault`. There is one Handle per file_id while the store is open, shared by every name and clone: `chain`'s second handle (db.rs:4498), the helper's dup (db.rs:6151, 6218), the run's hard link (db.rs:28989), and the WAL opened twice at open (db.rs:27932, 27936). A manifest slot therefore gains one session per open of the store, not one per publish (db.rs:11718).

## 4. Protocols

**Handle.** It holds:
- `len`;
- the tail page;
- the open group's tags;
- the override table;
- the ring head and the pinned spans;
- the session table;
- ctr;
- a write-state lock and a sync sequencer;
- while writable, the stored file's descriptor;
- once finished, a read-only shared mapping of the stored file's extents, with no descriptor.

Writes and syncs on one Handle from two threads are real: the helper's `dup.sync_data()` beside the writer (db.rs:6217-6218), and the presync thread beside `finish(w)` (db.rs:29029-29038). That is why the Handle has both the lock and the sequencer.

**Write (all classes).**
1. Bytes go into the tail page.
2. A page that fills is sealed (next ctr) into a reused 2 MiB staging buffer, and its tag goes into the open group.
3. A completed group's tag page is sealed (next ctr).

A write call is then up to three syscalls:
- one `pwritev` of the full data pages at their stored offset;
- one `pwrite` of the tag page, when a group completed;
- for Log, one `pwrite` of the frame.

The draft's "one pwritev" could not hold, because the three regions lie at different stored offsets. E5 prices the syscalls. io_uring is not proposed.

Per class:
- **Log.** The same call writes one frame: `TAIL` with the bytes since the previous frame (about 40 B of overhead), `HOME` for pages sent home, and `GROUP` for a completed tag page. So every write call is in the page cache when it returns. That is process-crash parity under a buffered policy, held by the model: 60,600 process-crash states, 0 violations.
- **Once and Regen.** No frame per write. At the end of each write call the partial tail page is not written; the Handle holds it. A Regen file's close record and tail are written at `finish` or at the Handle's last drop, unsynced. After a process crash, a Regen file whose Handle had not dropped has no close record:
  - the ordered index is recomposed at the next open (the open verifies it whole anyway);
  - a form file is refused at its attach and rebuilt.

  A plain store keeps those bytes in the page cache, so this is a regeneration after a process crash, not a loss of data. In practice it touches only files in flight, since `write_ord_tmp` writes once and drops the file (db.rs:99-131).
- **Replace.** Pages are rewritten in place at fresh ctrs. The `REC` holding every page's tag is written in the same call as the slot's single `write_all_at` (db.rs:11719), not at the sync, because that sync may run on another thread (db.rs:11706-11721).

**Write-through into the plaintext backing.** A written page's plaintext also goes into the pager's memfd backing for that file, as a HOT page that can be given back once it is home. It goes there when the cache is below its crowding line (`Pager::crowded`) and is skipped otherwise.
- This keeps a new segment's first reads hits in core: the first scan after a load, the landing's `same_blocks` and ranks, a merge's reads of pieces just sealed, and the finish's own read-backs of the key section (db.rs:6196-6199, 7753-7770).
- Under a bound the pages can be given back once home, unlike today's outputs, which are held whole until their envelope.
- A file opened through `open_mapped` is written through always, whatever the cache's state, from that call on. RunMap is opened before the first `stream_out` (db.rs:28736-28741), and `vault::open_mapped` maps that backing linearly for RunMap's direct slices (db.rs:10899, 11289). The backing is owed (`MappedFile::owed`) and cannot be given back until the last mapping drops, which is the table's drop. It then becomes the pager's backing for the file, already HOT.
- Fresh pages per run byte: one memfd page plus the ciphertext's page cache. Today it is the memfd, plus the WAL frames' page cache, plus the envelope's at close.
- S3 and S4 price the write-through with `SUPDB_LOAD_PHASES=1 SUPDB_DRAIN_STEPS=1`.

**Patch** (`write_all_at` below what was written):
- **Once, before `FINAL`.** Rewrite home in place at a new ctr. Rewrite the tag page in place if the group was complete. This is safe because the file is not trusted until `FINAL`. The `FINAL` record carries the tag-page MACs, so only the final tag-page images verify. A captured v0 page put back fails: the tag page's entry names the new version, and the tag page's MAC is in `FINAL`.
- **Log.** Never home while the file is Log. The new image goes as an `OVR` frame, is carried in every later `CKPT`, and is read from the override table on a miss. The bound is 4 per file, asserted by a counting test:
  - the run's finish patches page 1 (the section header at key_off = SUPER, db.rs:7727-7729) and page 0 (db.rs:7818-7824). The run is created with `head_reserve: 0` (db.rs:6676, 8021), so the reserve copies at db.rs:7745-7772 do not reach it. Three verifiers checked this by reading.
  - the WAL's chain byte (db.rs:4498-4504) is the third.
  - A patch that falls inside the current tail page, such as a chain byte while page 0 is still the tail, is a `TAIL` frame at an older offset, not an override.
- **Replace.** The whole slot is rewritten, as above.

**`set_len` on a Log file.** This is WAL replay's cut at open (db.rs:27929-27934), the only truncation of an engine file (crypto verifier).
- A `TRUNC{len}` frame under the current session, then the engine's `sync_data`, which follows already.
- The page containing len becomes the Handle's tail. Pages at or past it that were home are re-sealed only at fresh ctrs, and only after a synced `CKPT` no longer needs their old images.

**`finish` (Log → Once).** It is called by the engine before the run's final `sync_all` (db.rs:29005).
- The layer writes the override pages and the tail page home at fresh ctrs, seals the last group's tag page, and writes a `FINAL` `REC`: len, every tag-page MAC, the override list with the overridden pages' versions and tags. The tag page of a completed group is never rewritten here. Overridden pages are named in the record instead.
- The engine's sync makes all of it durable.
- An open that chooses a `FINAL` record verifies the pages that `finish` wrote (at most 4 plus the last tag page). If any fails, it falls back to the ring's newest synced `CKPT`, whose `OVR` frames still hold the images. After that the file is a trusted Once file, its ring is no longer consulted, and the ring is punched at the next idle moment (not designed in detail).
- This answers "an ordered-loaded store has thousands of Log files". Once finished, an ordered run's output, a partition or piece kept under a hard link (db.rs:28982-28989, 38812), reads like any segment, and damage to it is a poison, not a truncation.

**Sync.** Syncs run through the per-Handle sequencer. A sync that finds one in flight waits for it, then writes its own record.
- **Log.** If the frames since the last `CKPT` pass 256 KiB, a `CKPT` is written at write time, never only at a sync. Then `fdatasync`. A `CKPT` is never synced on its own.
- **Once.** A `REC` (not `FINAL`) into the `REC` slot that is not the last completed sync's, then `fdatasync`. The helper's syncs (db.rs:6218) write these. The final sync after the finish writes `FINAL`.
- **Replace.** The engine's own `fdatasync`.
- **Regen.** `REC`, then `fdatasync`, only where the engine syncs: `plan.sync` for the form file (zfile.rs:4378-4383), `ord_durable` for the index.

No sync is issued that the engine did not issue. In particular **the header is never synced on its own**. It is dirty data of the same file, so the file's first `fdatasync` covers it.
- A Log or Replace file with no valid header reads as empty. `Wal::replay` already answers an empty buffer as an empty log (db.rs:4554-4556), and `manifest_read_all` skips an empty slot (db.rs:11898-11902).
- A Once or Regen file with no valid header is damaged: unnamed and swept, or recomposed, or rebuilt.

This removes the fdatasync per create that four verifiers found: on every seal output, merge piece, unsynced `.oidx` and form file, on the writer's path at a direct run's first commit (db.rs:28694-28698), and at every rotation (db.rs:4181).

**Ring (Log).**
- **Allocation.** Frames are allocated by skipping pinned spans, never by wrapping into them. The model blocked 2,754 sequences with a wrap-in-order allocator and none with skipping.
- **Pinned spans.** Three are pinned:
  - the last *completed* sync's base `CKPT` to its sync frame;
  - the span of every sync in flight, from its issue to its completion. The sequencer knows which syncs are in flight. Without this pin the model lost 244,032 of 262,144 states.
  - the newest `CKPT` to the head.
- **Bound.** A `CKPT` at write time keeps each span under about 300 KiB. If the ring is full of pinned spans, a write waits for the sync in flight. If there is none, which a buffered policy can produce, it writes a `CKPT`, which unpins the older head span. This is never a sync.

**Ownership of a page.** Every session entry has a base_page and a kind.
- `restart` (create, recycle) ignores every older session.
- `continue` (a resumed WAL) owns pages from its base_page on, and older sessions keep the pages below it.

A reader computes owner(p) from the authenticated table as the newest session with base_page ≤ p, after the newest `restart`, and decrypts p under that session's key alone. An old session's sealing of a page past the cut (crypto finding 1b) therefore never verifies. Frames are ordered by (session idx, seq), and frames of sessions before the newest `restart` are ignored. That answers the recycle case (crash finding 3).

**Table capacity.** The table holds 48 sessions; one header body has room for that at about 80 B an entry.
- **Replace:** a session is dropped once a synced whole rewrite under a newer session exists.
- **Once, Regen:** one session.
- **A resumed Log file whose table is full:** the layer compacts it at that open. It writes the recovered content under one `restart` session to a layer-internal temp spelling (logical name + `.pfcopy`) and syncs it. Then rename-over, as below. The layer's open removes any `.pfcopy` name it finds. A WAL that is not rotated for 48 opens pays one copy of up to its 32 MiB at the 49th open. S2 tests 60 opens.

**Recycle.** The spare gets a new file_id: a new K_f and a new header with one `restart` session. The rename to the new spelling carries `flags.recycled` and the low half of the spare's previous file_id.

At a crash the header may lag the rename. A Log name flagged `recycled` whose inode's header still carries the previous file_id reads as an empty log, which is today's `recycles` state moved into the layer. A file_id mismatch in any other case is refused. The new header is written unsynced and covered by the next sync. An attacker can produce the empty-log case only by pointing the recycled spelling at the previous life's own inode or a copy of it. That is a rollback of the kind the accepted model already allows.

`recycle_wal` defaults to false (db.rs:3938).

**Preallocate.** Under `Pg` it writes zeros at the stored data offsets, then `sync_all`, as the plain arm does at db.rs:4237-4262. A later commit then overwrites blocks that are already allocated, and the zeros fail their tags under the session, as zeros do today. It is only under `recycle_wal`.

**Recovery (Log), with the model's fixes.**
1. Take the newest valid header. If there is none: empty for a Log file, reading as an empty log.
2. Scan the ring for frames that verify under a session after the newest `restart`.
3. From each `CKPT`, newest first, take the longest run of contiguous (session, seq) frames that verify.
4. On choosing a `CKPT`, verify every data page from `synced_home` *rounded down to its group's start* up to the run's home end, against the `CKPT`'s open tags, the `HOME` tags, or the tag page whose MAC a `GROUP` or `CKPT` names. The rounding is load-bearing: the model's variant without it failed.
5. If any page fails, fall back to the next older `CKPT`, ending at the pinned base of the last synced state.
6. Re-home, under the new session, the tag page of every group that a frame in the replayed run closed. Those tag pages were never durable.

The fixed model gives 0 violations in 1,583,118 'any' states at maxlen 3, and in 242,400 prefix, 633,340 offset and 60,600 process states at maxlen 6. The no-pin mutant is caught (model: `scratchpad/seam/work/crash/model.py`, `inflight.py`). The model does not yet include the session bases, `TRUNC`, `finish` or the header lag. E4 adds them.

The extra cost is a read of the unsynced span at an unclean open, which replay and `recover_direct` (db.rs:6948-6952) read anyway.

**Read and miss.** The pager keeps its protocol: states, mark/era/grace, `Slot.page`/`oldest_lent`, `Hold`, the rests and `check`. `CHUNK` becomes 4096 (pager.rs:108-109) for `Pg` files and stays 4080 for envelopes while both formats exist. The plaintext backing stays a memfd.
- A hit is the one load and compare in `ensure` (pager.rs:1312).
- A miss:
  1. CAS EMPTY→BUSY.
  2. Take the tag from the Handle's open group or override table, or from the tag tier. The tier holds tag pages loaded a group at a time, each verified once against its MAC; it is pinned up to B/16, LRU past that, and charged to the budget.
  3. Copy the stored page from the Handle's read-only ciphertext mapping into the memfd page. A file still being written has no such mapping yet, so this is `pread` on the Handle's descriptor.
  4. Decrypt in place, then mark HOT. The hook decrypts in place (crypt.rs:141-151).
- A miss on a file's tail page that is not home is served from the Handle's memory.
- A range the reader names (the windows, a scan's span, a build's run, `willneed`) is filled as one `MADV_POPULATE_WRITE` over the memfd range, then one copy and decrypt a page.

Copying from a kept mapping, rather than `pread`, is chosen for three reasons:
- a device read through the mapping is a major fault, so `ru_majflt`, the census, the sampler's price of a fault and the controller's model keep reading what they read today (zfile.rs:4942-4955, db.rs:18384-18390). The performance verifier measured that `pread` reads zero there (198 device reads, `ru_majflt` 0, `ru_inblock` 1584).
- a finished file holds no descriptor, so `tests/cipher.rs:463-475` stands and `RLIMIT_NOFILE` does not bound the store.
- it is today's miss without the straddle.

If E2 finds `pread` cheaper by more than its noise, the census instead reads `ru_inblock/8` from the same getrusage call. That is no added syscall. The pager's own miss count stays a separate column, because it includes kernel-cache hits.

**Residency and advice.** These are `Paging::resident`, `forget` and `willneed` (pager.rs:1374-1500, bytes.rs:510-631).
- They act on the Handle's ciphertext mapping: `mincore` over it for `resident`, and `madvise` over it for `willneed` and `forget`.
- They use the Handle's file descriptor while it is held. They never use a path: today `forget` reopens `back.path`, which a rename to a new spelling would break.
- A finished file's mapping is sized to its stored extents. A file still being written is remapped at each 1 MiB piece, beside the write syscall.
- An S3 test holds `records_resident` (db.rs:8224-8233) under `Pg` to a drop and a willneed.

**Evict.** Unchanged: mark, era bump, wait for `oldest_lent`, then `MADV_REMOVE` per 1 MiB unit. Nothing is dirty, so nothing is written back.

**Trust.** The pager's failure state is per file, with a trust flag on each file.

A failure poisons the store (`Pager::check`, as today) in these files:
- a Once file that is trusted, by its `FINAL` or its live writing Handle;
- a Log file below its recovered length;
- a Regen file after its open-time verification.

A failure is that file's error, and never sets the store's failure, in these files:
- **A Once file without `FINAL`**, as a pending seal output under `manifest_pending` is. `pending_whole` (db.rs:28304-28324) then answers "not whole" and the slot before is the store, as on plain. An S3 test tears a pending segment under the cipher and requires the slot before.
- **A Regen file during its verification.** The ordered index is verified whole at every open, by the piece, through the pager, by its own CRC. This is parity with the plain store, whose `OrdIndex::open` checks the CRC over the whole mapping (ordindex.rs:423-428). Under a cipher today the check is skipped for paged sources (ordindex.rs:413). A failure recomposes, as `open_ord` does (db.rs:8503-8526). After the verification a failure is a poison. There is no online recompose. This answers the engine finding that a head read of an unlatched failed index answers 0 (ordindex.rs:512-524).
- **A Regen-lazy (form) file.** `ZFile::page_bytes → None` leads to `ZFile::fail` and the raw form (zfile.rs:2141-2157). Every site that reads a form page has to treat None as that failure. The list of those sites is owed (open item 3, ROADMAP.md:256-257).
- **A Replace file.** `read` returns the file with failed pages as zeros, or one zero page if the `REC` fails. The plain `manifest_open` CRC then calls it torn, and `manifest_read_all` takes the other slot (db.rs:11890-11913). Returning `Err` would abort that function through its `?`.

**Rename, link, remove.**
- `name_seq` comes from a store-wide counter, set at open to the maximum seen plus one, so a new spelling outranks every existing entry.
- **Rename to a name that does not exist** (the common case: a seal temp to its segment name, a new `.oidx`): `rename(2)` from the old spelling to a new spelling.
- **Rename over an existing logical name** (rare: a form file rebuilt, zfile.rs:4360; an index rewritten at open, db.rs:105/119; a rebuilt segment, db.rs:27764): `rename(2)`, then `sync_dir`, then `unlink` of the old target. An open that finds two authenticating entries for one logical name keeps the higher name_seq and unlinks the other. The `sync_dir` removes the dependence on the filesystem committing the two directory operations in order. It costs a directory sync on rare paths only; the one hot rename-over, the manifest's rename shape, is refused under `Pg`.
- `hard_link` is a bare `link(2)` to a new spelling with the same file_id. One Handle covers both names. Today's same-store check stays (vault.rs:1990-2006).
- `remove_file` is `unlink`.
- `sync_dir` and `open_dir` act on the real directory.

**Open.**
1. Read the marker. Refuse a v1 marker by name.
2. List the directory. An entry with the sealed shape (`x` plus base64url of at least the minimal length) is decrypted. An entry that authenticates enters the name table. An entry that has the shape but fails authentication is left in place and reported (`CipherCounts`). An entry without the shape is left alone, as the plain open leaves names it does not know (db.rs:27638-27645). The marker's own names are excepted by name.
3. Resolve duplicates by name_seq.
4. If the manifest names a file whose entry is missing or failed, refuse the open.
5. Headers are opened lazily, at a file's first open. A Log or Replace file's ring frames are loaded at its first open; after `finish` the Log files are the live WAL, the spare and the current run.

The engine's own sweep then runs by logical name (db.rs:27632), unchanged.

**Temps a crash leaks.** `compact-`, `tier-`, `seal-` and the index's `.tmp` are not swept by the plain open either. Under a cipher today they vanish as memfds; under `Pg` they would leak as files nobody can read. The engine's open sweep removes logical names ending `.tmp`, other than `direct-*.tmp`, in both modes. This is a plain behaviour change: beside the open's directory listing, it is a fix of a plain leak. Whether any `.tmp` is meant to survive an open is not verified (E0 item 4).

## 5. Crash windows against the plain engine

| kind | process crash | power loss | vs plain |
|---|---|---|---|
| seal/merge output (Once) | pages in the page cache; unnamed until the manifest, so swept | before `FINAL` + fsync: unnamed and swept; after: whole | equal |
| pending seal output (`manifest_pending`) | as plain | no `FINAL`, so a file error, so "not whole", so the slot before | equal |
| run temp (Log) | every write call's pages and frame are in the page cache | the last synced state, recovered as §4; finish patches are `OVR` frames until `finish` | equal for the file |
| finished run output (Once after `finish`) | as plain | `FINAL` chosen if its pages verify, else the ring's synced `CKPT` | equal |
| WAL (Log, S6) | as plain | torn tail = end; `TRUNC` covered by replay's sync (db.rs:27934); chain byte as `OVR` or `TAIL`, synced by the `wal.sync()` after it (db.rs:27065-27077) | equal |
| spare / recycle | as plain | header lags the rename: an empty log, the `recycles` state, in the layer | equal |
| manifest slot (Replace) | as plain | torn: zeros, CRC fails, the other slot is the store; no header: empty slot, skipped | equal |
| ordered index (Regen) | Handle not dropped: no close record, recomposed at open | torn: open verification fails, recomposed | plain keeps a process-crash index; here it is recomposed |
| form file (Regen-lazy) | as the row above, rebuilt | torn: that file out of service, rebuilt | better than today's cipher (today it poisons); equal to plain |
| names | rename(2), link(2), unlink | a rename-over syncs the directory before the unlink | equal |
| marker | temp, sync, rename, directory sync (vault.rs:434-448) | the same | equal |

No page or sector write is assumed atomic. Every torn page fails its tag. A tag page of a group a synced `CKPT` or `FINAL` covers is never rewritten. A Once file rewrites its tag pages only before `FINAL`. Ring pages re-dirtied by frames appended behind synced frames are rewritten with identical bytes in the sectors those frames hold, which is the plain WAL's own exposure.

**The store-level property.** Today a run's commits are frames in the sealed WAL (`log_run`), so a damaged run reads as a cut of one log. Once the run is its own log, a byte changed in the run temp's newest frame recovers the run at its previous commit while later WAL commits replay. A plain store already behaves so: `recover_direct` stops at the last good marker. This is plain parity and a weakening of today's cipher property. It is decision 1.

**Losing the fuzz's crash model.** CLAUDE.md says the cipher fuzz was the strictest test of the crash order, because a dropped cipher store lost every memfd. Under `Pg` a dropped store's files are whole in the page cache, as a plain store's are. So the door gets a fault-injection mode under a test flag. A "crash" discards each `Pg` file's writes since its last completed sync, and optionally tears the last one sector by sector. `ZFUZZ_CIPHER=1` runs with it, so the power-loss order is held again, more strictly than a drop. E4's mutants are carried into that run.

## 6. What goes and what stays (estimates from the maps, not counted line by line)

**Goes:**

| item | lines |
|---|---|
| vault.rs: the memfd write-once table, `Kind`, `companion`, `kept`, `persist*`, the sweep's form rules, the `.z` cascade, tokens and wrapped keys in `seal_manifest`/`open_manifest`/`adopt_all` (at S3); `LogCipher` and `passes()` (at S6); `Ahead` | about 1,450 of 2,230 non-test |
| db.rs: the WAL's cipher branches (4134-4580, at S6), `RUN_LOGGED`/`Direct::logged`/`log_run` (4746, 11520, 28219-28223, 28834-28881, at S4), the persist sites (12574, 12865-12866, 13033-13034, 28999, 29525), the manifest's late envelopes (11661-11685), `CipherCounts::late`, then `cipher_controller` and `zpages_under_cipher` (11529, 27305, 27604) at S5 | about 600 |
| pager.rs: the 4080 chunk, `Per`, the 255-page unit, adopt-from-envelope fills (966-1000), and most of the unbacked-file accounting in `could_give` (509), once the envelope arm is deleted | about 250 |
| ordindex.rs: the paged skip of the CRC (413-428), replaced by the CRC through the pager | about 10 |

**Stays:**
- the door and the decoy directory;
- the marker (version 2) and `rewrap`;
- `MappedFile::owed`, as the backing of a mapped file being written;
- the pager's state machine, grace, rests, `Hold`, `crowded`/`swamped`/`breathe`, `could_give` and `check` (now per file);
- the readers' ask path (`Bytes::piecewise`, `Sect`/`Pieces`, `scan_pieces`/`scan_either`, `Sec`'s arms, about 2,800);
- `ATTN_PAGED`/`paged_begin`/`end_slow`;
- every rest and `unlent` site;
- zfile's held build (`SegSrc::Kept`/`Walked::Held`, zfile.rs:4302-4320), selected by `vault::paged(path)` instead of `vault::kept` (C10 checked);
- crypt.rs v1 and web/crypt.mjs, frozen;
- `Wal::replay`'s empty-buffer rule and plain replay unchanged. The cipher branch of replay goes at S6.

**New:**
- `pagefile.rs`, about 2,100: the draft's 1,800, plus `finish`, ownership, compaction and the fault-injection mode;
- `vault::File` and its call sites, about 350;
- tests and the crash model, about 1,700.

**Public API change.** `CipherCounts` loses `persisted`, `persisted_bytes`, `adopted`, `left`, `swept` and `late` (vault.rs:1723-1747). It gains `sessions`, `frames`, `ckpts`, `misses`, `range_fills`, `tag_tier_misses`, `failed_names` and `compactions`. The readers of the old fields are tests/zpages.rs, tests/fuzz.rs:1052 and 1221, bench/src/engines.rs:8512 and examples/cipher_store.rs:139 and 165 (some read only `cache`).

## 7. Why a store in the clear pays nothing

- Plain `MmapBytes` and every reader stay byte for byte. `lend_at` (bytes.rs:480-487) is untouched. WARM, at S7, is decided at map time by which `MmapBytes` a cipher store builds, never by a branch in a reader.
- The only new code a plain store runs is the `File::Os` arm. It runs once a 1 MiB piece (db.rs:6046), once a finish patch, once a commit's write and once a sync or hint: a branch beside a syscall.
- The class is read once, at create. `vault::paged` is read once per open of a file.
- The ordered index's plain open keeps its mapped whole-file CRC.
- The temp sweep runs once per store open, beside its directory listing.

**What is not claimed.** Widening `File` from a 4-byte descriptor to a 16-byte enum with `Arc` drop glue inside `AlignedWriter`, `Wal` and `SegmentWriter` can move inlining and spills in per-key functions, even though the match runs only at syscalls. CLAUDE.md records count moves of that kind: `emit_unsealed`, `ZFile::view_of`. Removing `RUN_LOGGED` saves nothing, because the bit sits in a byte the commit already tests. No removal is claimed as a plain saving until a count shows it.

**Counted rows.** Callgrind by function, each probe built at `origin/main` from this tree's example sources and at the tree, read with `callgrind_annotate` side by side, never by the total alone:
- `read_only.rs supdb 100000 1 n-none`;
- `lag0_probe.rs supdb 100000 1`;
- `work_probe.rs 100000 whole` (the writer's round; the named risk is `write_buf`'s inlining with a `File` enum);
- `merge_walk.rs 100000 4 1`;
- `load_only.rs supdb 100000 1` (`commit_direct`, a seal's open and landing, the ordered index's open check);
- `read_only.rs` and `lag0_probe.rs` with `supdb-nocachezpages` (the form's point read and scan);
- **added:** a probe that seals, for `SegmentWriter::create_with` (db.rs:6550). Whether `work_probe` reaches a seal is not verified, so the row is a seal explicitly.

The threshold is zero added instructions in any per-key, per-entry, per-block or per-step function.

## 8. Predictions

| number | today | predicted | rests on | guess? |
|---|---|---|---|---|
| miss, single page | 4 µs | 3.0-3.4 µs | Today an isolated 4080-byte chunk lies across two fresh memfd pages, about two faults (pager.rs:108-125: 256 chunks in 255 pages). A whole-page chunk takes one fault (about 1.9 µs, probe.md), plus the copy (about 0.3-0.75), the cipher (about 0.5), the amortised `MADV_REMOVE` (0.22-0.34, probe.md) and a tag lookup. The draft's "fault stays" reasoning gave 3.8-3.9, which is no gain. The real reason is half the faults. | E2 measures it as one operation; the kill is above 3.4 µs |
| miss, 64-page range fill | about 4 µs a chunk | 1.5-2.0 µs a page | one `MADV_POPULATE_WRITE` (about half a fault's price in bulk, CLAUDE.md) and a copy and decrypt a page | E2 |
| out-of-cache point read (64 MiB cache, 1M keys) | 0.17x | 0.19-0.21x | t = base + 1.03·miss, base from the in-core 0.92x (a plain read of about 0.85 µs): 3.0 µs gives 0.21x, 3.25 gives 0.20x, 3.4 gives 0.19x | bounded by E2 |
| out-of-cache past about 8B (store more than 8 times the plaintext budget) | not measured | worse than the row above by about 0.75 µs on a part of the misses | Tags pinned up to B/16 cover stores up to 128·B/16 = 8B. Past that, 1 - 8B/size of misses also read a tag page: from the kernel's cache, or from the device under a cgroup bound. The 1M under 64 MiB cell (about 150 MB, about 1.2 MB of tags against 4 MiB pinned) is unaffected. 10M under 64 MiB (about 12 MB of tags) is affected. | arithmetic; E2's tag arm |
| ordered load | 0.68x durable, 0.52-0.57x buffered, 2.5 B/B | 0.85-0.95x durable, 0.78-0.90x buffered, 1.35-1.45 B/B | The measured removals: the log double write (20-50 ms of commits over 300), the drain's envelope (43-55 ms of 77-94) and the 14 ms unlink (CLAUDE.md). They total 62-118 ms and match the 0.68x gap if the plain load is about 130-250 ms; the plain load's length is not verified. Added: about 5 ms of AEAD per 40 MB, about 40 B a frame plus a ring page and a quarter tag page a commit, a `CKPT` per 256 KiB of frames, two to three syscalls a commit where plain makes one writev, and `finish`'s few pages. The fdatasync count equals plain's, now that the header is not synced. | the added side is a guess until E5 |
| compact form under a bound, against raw under the cipher | 0.73x | the ratio the clear store shows in a forced pairing, within a tenth | Measured: 0.60 faults a read through the form against 0.43 raw, and time follows faults (1/0.73 = 1.37 against 0.60/0.43 = 1.40). A cold form page today costs two stored pages and will cost one. If the premise holds, the form takes about 0.30 faults a read against 0.43, about 1.4x of raw if time follows faults. Raw does not change, because a 120-byte record rarely straddles and the hash slots stay resident. | rests on E1 (forced, in the clear) and E1b (the straddle premise) |
| in-core point read | +339 instructions over 818 | +320-335 through S6; with WARM at S7, +20-60 | Of the 339 (CLAUDE.md): the asks, the window, the crowded look, the failure look, and 95 of `not_lent`; only `Per` and the straddle cases go | the S7 figure is a guess until E3 |
| in-core first pass after a load or landing | hits (outputs held whole as memfds) | hits while the cache is below its crowding line (write-through); misses otherwise | §4 write-through | read in S3 with `SUPDB_SCAN_OPS=1` |
| memory under a bound | outputs held whole and not givable until their envelope | givable once home | §4 write-through | read with `lagcensus` and `cache_bytes_at_end` |
| out-of-core point read under a group bound (5M keys, 640 MiB) | 0.29x of clear, 274 MiB held in 337 MiB of shmem | whole pages at today's budget 0.36x (measured, E9); at the budget a load survived 0.37x (measured, n=2); 0.42-0.44x at 487 MiB held and 0.56-0.59x at 541, read pass only | misses a read = 1.16(1 − held/669 MiB); µs a read ≈ 2.6 + 36 × device faults + 4.5 × misses (a regression over E9's sittings, not a profile). Shmem/held 1.00 against 1.23 | measured for the layout; the budget a load survives is not established |

Scans in core, 0.89-0.94x, are unchanged until S7.

## 9. Experiments owed before building (cheapest and most decisive first)

- **E0, an audit by reading (no build).** Confirm:
  1. Three verifiers have confirmed by reading that the run's finish patches only pages 0 and 1. That item is closed.
  2. Every create that a manifest later names is followed by a `sync_dir` before the manifest.
  3. No `read()` caller other than the manifest slots, WAL replay and `recover_direct` reads a Log or Replace file. The engine verifier confirmed this: `vault::read`'s one caller is `recover_direct` (db.rs:6949). Closed.
  4. No `*.tmp` other than `direct-*.tmp` is meant to survive an open.
  5. Where a WAL write error leads (db.rs:4436-4456), and whether the engine continues after one. ctr makes it safe either way; this only bounds the test.
  6. Every site that reads a form page treats `page_bytes → None` as `ZFile::fail`.

  *Kill:* a `.tmp` that must survive an open drops the temp sweep for that prefix.
- **E1, the form's upper bound in the clear, forced.** An arm with `ZPages::Always` (its name is not verified) against `supdb-nozpages`, 5,000,000 keys, `--bound 671088640 --passes read --reps 1 --ops-div 1`, both forms standing from the start. It is not an `Auto` pairing, whose buy moves with the order (CLAUDE.md). *Kill:* the form not ahead of raw out of core in the clear makes the form prediction moot. S5 then drops its controller step and keeps `cipher_controller`.
- **E1b, the straddle premise, without the layer.** One of these:
  - **(a)** rerun the existing `supdb-cipherauto` / `supdb-cipher` bounded pairing and read the pager's chunk misses per read beside the kernel's faults per read; the faults should be about 2x the form-page misses.
  - **(b)** a scratch arm in the clear, form forced, whose `ZFile::page_bytes` also touches page+1, paired against the same arm without the touch under the bound; it should reproduce about 0.73x.

  *Kill:* faults not about twice the misses means the 0.73x is not the straddle, and the form row is withdrawn.
- **E2, the miss, as a C probe** in `scratchpad/seam/work/`, cycling over 100k pages, with the machine to itself. Arms:
  - **(i)** today's exact shape: a 4080-byte copy out of an envelope mapping, decrypted across two fresh memfd pages;
  - **(ii)** a 4096-byte copy out of a ciphertext mapping into one fresh memfd page, with AES-GCM in place (OpenSSL);
  - **(iii)** `pread` into one fresh memfd page, then decrypt;
  - **(iv)** decrypt into a warm scratch buffer, then `pwrite` the full page into the memfd's descriptor (a full-page shmem write may skip the zeroing; not verified on 6.18);
  - **(v)** a 64-page `MADV_POPULATE_WRITE` plus copy and decrypt.

  Every arm adds the amortised `MADV_REMOVE` per page and a tag-tier lookup; one variant adds a tag-page read per miss. Run at 1 and 3 threads. *Kill:* (ii) or (iv) above 3.4 µs leaves the out-of-cache cell within noise of 0.17x, and the design then stands on the load and the form. If (iii) beats (ii) by more than noise, the census switches to `ru_inblock` as §4 says.
- **E3, the plain cost and WARM's headroom.** Callgrind, not timing-sensitive, so it can run on a shared machine.
  - At S1: the rows of §7.
  - On a scratch branch: a cipher `map` returning `lend = len` while the store fits, then `read_only.rs supdb-cipher 100000 1 n-none` and `lag0_probe.rs` against `supdb`.

  *Kill:* any plain per-key function moving stops S1 until the shape reads level. WARM under +100 is the condition for S7.
- **E4, the crash model, extended.** The model at `scratchpad/seam/work/crash/model.py` already carries the fixes of §4 and passes. Extend it with:
  - session bases (`restart` and `continue`) and a resume after a crash;
  - `TRUNC`;
  - `finish` and its fallback;
  - the header lagging the first sync;
  - the recycle header lagging the rename;
  - Replace with the `REC` written at the write and the sync on another thread;
  - table compaction.

  Mutants that must fail it:
  - no in-flight pin;
  - verifying from `synced_home` without rounding to the group;
  - no re-home of tag pages;
  - old-session frames accepted after a `restart`;
  - a ctr reused on retry;
  - `FINAL` trusted without verifying its pages;
  - an `OVR` written home before `finish`.

  *Kill:* an acknowledged loss the model cannot close halts S4, and the run keeps `log_run`.
- **E5, the device cost of a Log commit.** A C probe of 100 B, 4 KiB and 133 KB commits with `fdatasync`, against both the plain run's commit (records, a marker, one fdatasync, db.rs:6917-6936) and the plain WAL (prefilled under `recycle_wal`, db.rs:4237-4262). It reads `/sys/block/vda/stat`, `strace -T` and the fdatasync count, and counts fdatasyncs per create (it must be zero). This prices the A and F write cells before S4.
- **E6, tag residency** after S3: `mincore` over the tag regions and the tag tier's counts after the bounded read pass (adapting `scripts/resident.py`), at 1M and 10M keys under 64 MiB.

## 10. Stages (the gate green at each end)

- **S1: `vault::File` with only `Os`.** Includes `preallocate`/`recycle`/`paged`, the raw-descriptor sites of §2 through the door, the class in `SegmentOptions`, and the extended door test.
  - Tests: unchanged.
  - Gate: the rows of §7 equal to `origin/main`, and `sh scripts/check.sh`.
- **S2: `pagefile.rs` standalone, under a stand-in cipher.** The layout, the five classes, ctr and sessions with their ownership, frames, `CKPT` with `synced_home`, the pins, overrides, `TRUNC`, `finish`, the sequencer, recovery, names with name_seq, rename-over, the compaction, recycle, and the fault-injection mode.
  - Hand-built torn states: torn home, torn frame, torn `CKPT`, a concurrent sync, a lapped ring, recycle at rotation with the header lagging, duplicate names, a rename-over crash between the rename and the unlink, a torn Replace, a resumed file at 60 opens.
  - The nonce-format enumeration test, the uniqueness hook, and the NAME_MAX test.
  - A mutant pass of at least 40, held to firing, with mechanical operator flips beside the list.
  - Not wired.
- **S3: Once, Regen and Regen-lazy wired**, for segments, merge pieces, `.oidx` and `.zpg`, and the run temp as a Once file written through (its durability still carried by `log_run`). All under `Options::cipher_pages`, arm `supdb-cipherpages`, marker v2. `supdb-cipher` stays the envelope shape and stays the default, so its series continues.
  - The directory under the arm: sealed spellings for every file except the WAL and the manifest slots, which still pass under their plain names, sealed by the engine and kept by the sweep's "no sealed shape, leave alone" rule. There are no tokens in the manifest and no envelope-named segment.
  - Pager geometry per file: 4096 for `Pg`, 4080 for envelopes.
  - Tests: `tests/cipher.rs` on both arms; `ZFUZZ_CIPHER=1 ZFUZZ_CIPHERCACHE=16384` with `ZFUZZ_CACHE=all ZFUZZ_OPTVARY=1`, several seeds a core, with the fault-injection mode on the pages arm; the pending-segment tear; `records_resident` under a drop and a willneed; a flipped byte in a `.oidx`, which must recompose at open and poison after it; a flipped byte in a `.zpg`, which must take the form out of service.
  - Pairings:
    - `pair.sh supdb-cipherpages supdb-cipher 300000`, with `SUPDB_SCAN_OPS=1` for the first passes;
    - `pair.sh supdb-cipherpages64 supdb-cipher64 1000000 --reps 4`;
    - `pair.sh supdb-cipherpagesauto supdb-cipherauto 5000000 --bound 671088640 --passes read --reps 1 --ops-div 1`, with builds and installs read beside it, and a forced pair as E1.
- **S4: the Log class for the run temp, and `finish`.** `log_run`/`RUN_LOGGED`/`Direct::logged` are deleted under the arm.
  - Tests: the torn states in `tests/cipher.rs` with mutants (an override written home, no pin, no sequencer, `finish` trusted unverified); the counting test of 4 overrides; the open's bytes read per finished run held to a handful of pages.
  - Pairings, with `SUPDB_LOAD_PHASES=1 SUPDB_DRAIN_STEPS=1` and `/sys/block/vda/stat`:
    - `pair.sh supdb-cipherpages supdb-cipher 300000 --passes load` and the buffered twins `supdb-ingestcipherpages supdb-ingestcipher`;
    - then against `supdb`.
  - `supdb-cipherlog` and `supdb-ingestcipherlog` (bench/src/engines.rs:1260-1266) go, with their last pairing recorded in the PR.
- **S5: the census under `Pg`.** A test that the controller reads the census under a cipher, through the ciphertext mapping's major faults or `ru_inblock` (E2). Then `cipher_controller`/`zpages_under_cipher` are deleted, keyed on E1 and S3's pairings.
- **S6: the WAL, the spare and the manifest slots under the layer** (Log and Replace).
  - `LogCipher`, `passes()`, `seal_manifest` and the cipher branch of `Wal::replay` are deleted.
  - Tests: `a_log_cut_anywhere_opens_as_the_commits_before_the_cut` (tests/cipher.rs:952) is replaced. A cut of the stored file at every byte is no longer a crash state, because a cut inside the ring or the tag region drops every home page. Its replacement applies E4's torn states to the stored file. Plus `ZFUZZ_RECYCLE=1`, the Replace torn test, and an open with a slot that has no header.
  - Pairings: the buffered mixes `pair.sh supdb-ingestcipherpages supdb-ingestcipher 300000 --passes mix` with `SUPDB_MIX_KINDS=1`.
  - Flip the default: `supdb-cipher` becomes the pages shape, `supdb-cipherenv` keeps the envelope until it is deleted, and the pairings are recorded in the PR.
  - The docs step: rewrite each claim that is no longer true:
    - README.md:257-258, 273-275, 281-284, 289, 292-294, 296-297, 303-305 (add apparent against allocated size);
    - ROADMAP.md:171-180, 217-223, 237-250, 256-268;
    - bench/DESIGN.md:281-296;
    - CLAUDE.md's vault and pager sections.
- **S7: WARM.** Files whose plaintext fits 0.85·B are born or promoted WARM with `lend = len`, keeping the blob id; carrying `ranks`/`bounds` across is not verified. Demotion republishes `lend = 0`. A WARM file still reachable from a pinned old state counts as not givable, under a cap. Counted by E3 at its end.

**Tests that die, are rewritten, or stay** (the completeness verifier's list, assigned):

| test | fate |
|---|---|
| vault unit tests: the envelope written twice, `a_kept_file_lives_in_memory...` | die with the envelope arm (S6) |
| vault unit test `the_log_and_the_manifest_pass_to_the_directory` (vault.rs:2514) | dies at S6 |
| vault unit test `a_sealed_manifest_opens_whole_or_not_at_all` | rewritten as the Replace test |
| `tests/cipher.rs` tests that find files by stored name (`x-`, `.o`, `.z`, `wal-`: lines 175, 359, 484, 957, 992, 1023, 1712, 1861) | rewritten through a debug seam `vault::stored_name(logical)` |
| `stored_bytes` (tests/cipher.rs:356-362) | counts `st_blocks` |
| snapshot/restore (tests/cipher.rs:209-232) | kept; a copy densifies, which is allowed |
| `a_changed_byte_in_a_form_file_is_refused_or_the_form_is` (tests/cipher.rs:1941) | rewritten to require the form out of service |
| `tests/cipher.rs:463-475` (no descriptor) | kept for finished files; files being written are allowed one each |
| `tests/zpages.rs` cipher tests (6775, 7213, 7266, 7590, 7640) | rewritten: `zpages_under_cipher` goes at S5, `persist_form`'s cold mode becomes a `writeback(Wait::Both)` and a drop |
| `tests/mapped.rs:96-110` | kept |
| the fuzz | gains the fault-injection mode |

## 11. Considered and rejected

- **userfaultfd or a PROT_NONE transparent space.**
  - Default Docker, Podman and containerd seccomp deny userfaultfd.
  - M2's fault plus `mprotect` is 7.1-11.2 µs before any read or decrypt (probe.md), against a whole miss of 4 µs today.
  - The handler-thread mode is 30-45 µs.
  - SIGBUS mode returns EFAULT on a caller's `write(2)` of a lent value.
  - It would put signal handlers in an embedded library, running the caller's cipher in signal context.
- **Deleting the pager and the ask path.** Nothing else holds a miss near 3-4 µs in every container.
- **An 8 KiB tail slot per commit.** It costs device bytes on every small commit, against a frame sized to the bytes appended.
- **Rewriting tag pages in place for Log files, or at `finish`.** One torn write puts 128 durable tags behind it. `finish` names overridden pages in `FINAL` instead.
- **Clear, unauthenticated tag pages** (the draft). They let a stale version, an old session or a non-present page verify (crypto findings 1 and 2).
- **A per-session page range sealed in the header**, the crypto verifier's first fix. A session's range is not known when its header is written. Ownership by base_page plus sealed tag pages gives the same refusal, with nothing to rewrite later.
- **Random nonces under K_s for headers, wraps and names** (the draft). It has a lifetime budget of about 2^28 sealings under a key that is never rotated.
- **Versions taken from engine values** (the draft's Replace "version = seq"). They repeat across a failed write and a reopen.
- **A header sync per session** (the draft). It is a sync the policy never asked for, on every create.
- **Deterministic (SIV-like) spellings, so that `rename(2)` over a name is atomic.** They cannot bind the file_id, so swapping two inodes would authenticate. Random spellings with a directory sync on the rare rename-over are cheaper than that loss.
- **Unlinking every entry that fails to decrypt** (the draft). That turns a flipped bit or a stray file into destroyed data.
- **`pread` as the default miss.** It blinds `ru_majflt` and holds a descriptor per file. It stays as E2's arm (iii).
- **Fresh's single-sync doublewrite and its fold fsync.** The home copy can tear before its image is durable, and a fold is a sync the policy never asked for.
- **Anonymous `MAP_NORESERVE` plaintext.** It is charged in full under `vm.overcommit_memory=2`, and it moves plaintext into the `RssAnon` that the controller's budget subtracts (zfile.rs:5260-5267).
- **O_DIRECT reads as a default.** Segments are published before their fsync, O_DIRECT has no readahead, and a filesystem can refuse it.
- **Verifying Regen-lazy files at open, or a boot-id rule.** The open's cost would grow with the forms. Verifying the ordered index at open is kept, because the plain store already does it.
- **WARM through a cold callee in `lend_at`.** CLAUDE.md measured a second callee in a branch at 2-7 instructions an entry.
- **Keeping `Kind`/companion/tokens through the stages.** Companions lose their names at reopen once `persist` goes.
- **Keeping `LogCipher` for good** (lead). It is form knowledge in the vault; it is kept only until S6.
- **Link plus unlink for `hard_link`.** The run's close needs both names (db.rs:28989).
- **Every open beginning a new WAL file** instead of compacting a full session table. It changes the plain engine's open for a cipher-only bound.

## 12. Owner's decisions, and what remains open

**Decisions:**

1. **Store-level integrity once the run is its own log.**
   - (a) Plain parity: each file recovers to a prefix of itself, and the `tests/cipher.rs` property is restated per file.
   - (b) Bind each run commit's sequence into the WAL, at a small write per run commit.
   - Recommendation: (a). (b) brings back part of the double write, and a plain store already has (a).
2. **The WAL, the spare and the manifest under the layer (S6).**
   - (a) Move them at S6 behind an arm, after the run's Log class has passed E4 and the fuzz with fault injection.
   - (b) Keep `LogCipher`, `passes()` and `seal_manifest` for good.
   - Recommendation: (a).
3. **The plaintext budget.** Measured in `once.md` (E8) and `page.md` (E9), 5M keys under 640 MiB.
   - (a) The caller's `cache_bytes`, with today's default of half the free budget read at open.
   - (b) The layer owns one budget behind `layer::Memory`: recomputed as the process runs, counted in resident pages, shared across the process's cipher stores, with the caller's `cache_bytes` as a cap.
   - (c) As (b), and no ciphertext cached (direct reads).
   - Recommendation: **(b)**. (a) stays the default only until the layer exists.
   - Why: misses a read follow the plaintext held on one line, 1.16(1 − held/669 MiB), in both layouts, and per MiB the pager is 5-10% better than the kernel's cache on uniform reads. So the rate is set by how much plaintext the bound lets the store hold. A fixed large budget is killed in a load (shmem is not reclaimable); (c) added 5-8% unresolved, loses the fault counters, and gives up the ciphertext cache that is the only reclaimable room.
   - What (b) has to do: subtract anon, files being written that cannot be given back, and the other stores' shares; keep a margin sized by anon's growth in one recompute interval; reserve before the store's own large allocations (the mapped blocks) rather than sample after; draw both wait lines against the group limit less anon and less what cannot be given back, never as multiples of the budget.
   - Not established: the margin. 85% of `read_budget` survived load and read in 2 of 2 sittings (0.37x of clear); 94% was killed in 1 of 2. Both ran on today's vault, which holds a file being written whole; §4's write-through makes those pages givable once home. The margin is a gate on the stage that builds the budget (survive N of N load,read sittings; two stores in a process; a caller whose own memory grows), not a number chosen here.
4. **Leakage accepted.** In exchange, every name is hidden, `wal-` and `manifest` included. What leaks:
   - the file count;
   - stored sizes to the page; apparent sizes leak the extent count;
   - each commit's size to within about 40 B, from the frame lengths;
   - which pages are rewritten, and how often, to anyone watching the device over time (tag pages are sealed, so a copy at rest does not show this);
   - how often a store is reopened, from header rewrites;
   - which entries are links of one file, from the clear file_id, as inode numbers already show;
   - timing;
   - plaintext in memfd pages, which can be swapped, as today.

   Recommendation: accept and document it. Padding `TAIL` frames to 64-byte buckets is the cheap mitigation if commit sizes matter.
5. **Platform scope.** `Cipher` stays Linux-only, or gets a portable backing (`shm_open`/`MAP_SHARED|MAP_ANONYMOUS`, `MADV_FREE` or a remap in place of `MADV_REMOVE`, and copy fills). Recommendation: Linux-only through S7, said in the README, with a portable backing as a roadmap item priced in the same pairing.
6. **Filesystems without holes.** The 3 MiB stored prefix makes every file at least 3 MiB on FAT/exFAT and some SMB. Either accept it and document it, or design a small-file layout (tags inline after the data for files of one group). Recommendation: accept for now; most files are small only in tests.

**Open, stated plainly:**
- E4 has not yet modelled session bases, `TRUNC`, `finish`, the header lag, the recycle lag, Replace or compaction. The protocols for those are designed but not checked.
- The sealed name is two characters under NAME_MAX at the longest `par-` name, and that is held only by arithmetic until the S2 test. A longer logical name in a later change would break it.
- The list of every site that reads a form page and must treat None as `ZFile::fail` is not made (E0 item 6, ROADMAP.md:256-257).
- The ordered index has no online recompose. A failure after its open-time verification poisons the store, as today.
- A Regen file whose Handle had not dropped is regenerated after a process crash, where a plain store keeps it.
- The tag tier past 8B adds a read to part of the misses. Shrinking the entry (tag plus an implicit version base per group, 16-20 B) is not designed.
- Whether helper threads read Regen-lazy pages on behalf of another operation and drop the failure is not verified.
- Punching a finished Log file's ring is not designed in detail.
- The `.tmp` sweep is a plain behaviour change pending E0 item 4.
- The out-of-core prediction rests on E2. The form prediction rests on E1 and E1b. The load prediction's added side rests on E5.
- The tag pages are outside the residency and advice paths in E9's prototype (`resident`, `willneed`, `forget` cover the data range only). The tag tier has to sit inside them; unmeasured past a store whose tags do not stay hot.
- Page-granular eviction is dropped as an out-of-core effort for uniform reads (E9: the 1 MiB clock is no worse per MiB than the kernel's cache). Skew is not resolved.
- The controller's model under a cipher, two caches in series, has never been held to a measurement (CLAUDE.md). This design keeps the kernel's fault counter readable but does not validate the model.
