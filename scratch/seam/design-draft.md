# Pagefile: a sealed page file under the vault door (synthesis)

Base: `fresh`, the winner of two judges out of three, with its scope: every file goes under one page layer that knows no formats. It takes `lead`'s layout, its class discipline and its staging, and it answers every serious finding. The `risk` judge picked `lead` because it keeps the WAL and the manifest out of the layer. I decided against keeping them out for good, because that leaves the vault knowing the log and the manifest by name (`passes()`, `LogCipher`), which is the form knowledge the owner asked to remove. The risk judge's point is still taken as an order of work: the WAL and the manifest are the last stage, behind an arm, after the crash model has carried the run temp. Code citations were read for this draft unless marked "not verified".

## 1. The seam

The cipher sits below every formatter and above the kernel, in one module (`src/db/pagefile.rs`) behind the `vault::` door. The door returns `vault::File { Os(std::fs::File), Pg(Arc<pagefile::Handle>) }` where it now returns a raw `File`. Every write, read, sync, writeback hint and fadvise is one match beside its syscall.

Above the door, `SegmentWriter`, the direct run, `write_ord`, the form builder, the WAL and the manifest write plaintext at plaintext offsets, byte for byte what a plain store writes. Below the door each logical file is one sparse ciphertext file. Plaintext page p is exactly one 4096-byte stored page, and its tag is kept apart. The layer knows these things and nothing else:
- pages;
- logical lengths;
- per-page versions;
- sync points;
- sealed names;
- one durability class per file, given at create.

It knows nothing of segments, runs, forms, tokens, fences or companions.

This is the owner's "after the compression interface and before the disk interface" taken literally:
- Every compression, layout and record decision is already made in the plaintext bytes the formatter hands down.
- The layer's whole vocabulary is the file API the kernel already offers: write at an offset, sync, rename, link, map.
- What the layer adds is the classes. They are I/O contracts a plain filesystem gives for free, such as "this file is trusted only after its final sync" or "a torn write of this file must read as damaged". They are not forms.

## 2. Door surface and the file disciplines

```
enum File { Os(std::fs::File), Pg(Arc<pagefile::Handle>) }
  write_at, write_all_at, write_vectored(_at), append(&[IoSlice]),   // O_APPEND: layer owns offset
  read_at, read_exact_at, read_to_end, seek, set_len, len,
  sync_data, sync_all, try_clone, writeback(at,len), advise(at,len,Advice)
vault::create(path, Class) / create_rw / open / open_write / open_append / open_dir
vault::map(path) -> MmapBytes            // plain mmap, or MmapBytes::paged over the pager (bytes.rs:365)
vault::open_mapped(path, cap)            // RunMap's growing view (db.rs:11461)
vault::rename / hard_link / remove_file / names / len / exists / read / sync_dir
vault::preallocate(file, len)            // plain arm = today's zero loop, db.rs:4252
vault::recycle(spare, path)              // plain arm = today's rename+open_write, db.rs:4280
```

Calls that go away: `persist`, `persist_one`, `persist_form`, `companion`, `kept`, `Kind`, `passes`, `Mapped`/`MappedFile` (except the run's held view), and the memfd table.

There are four classes, chosen at create. Each is a contract that holds whatever the file contains.

| class | contract | callers |
|---|---|---|
| **Once** | Trusted only after its final sync and a name a manifest gives it; a crash before that leaves a file the engine sweeps. Patches before the final sync are made in place. | seal and merge outputs (`seg-`/`pcs-`/`par-`) |
| **Log** | A process crash keeps every write call; a power loss keeps the last synced state; a torn tail is the end. A page a write has made home is never overwritten in place. | run temp (db.rs:28698 path), WAL, spare |
| **Replace** | Rewritten whole in place; a torn rewrite reads as damaged, never as a mix of versions. | the two manifest slots |
| **Regen** | Regenerable. An authentication failure is this file's `InvalidData`, never a poison of the store. | ordered index `.o`, form file `.z` |

The marker `supdb.cipher` is the layer's root. It is unchanged (vault.rs:362-449): it wraps the store key under the caller's key and keeps `rewrap`.

## 3. On-disk layout

The page is P = 4096. A group is 128 pages (512 KiB), and an extent is 65,536 pages (256 MiB).

| stored offset | size | content |
|---|---|---|
| 0, 4096 | 4 KiB each | Header A and header B: magic `SUPDBPF1`, version u16, class u8, file_id[16], hdr_seq u64, a session table of up to 16 × {session key wrapped under the store key: nonce12, ct32, tag16}. The whole body is sealed under the store key. The newest valid hdr_seq wins. |
| 8 KiB .. 1 MiB | 1016 KiB, sparse | Journal ring of frames (Log, Replace, and Once sync records). |
| `E(e) = 1 MiB + e·258 MiB` | 2 MiB | Tag region of extent e: the tag page of group g (g < 512) is at `E + g·4096`, 128 entries of 32 B. |
| `E(e) + 2 MiB + (p & 0xFFFF)·4096` | 4 KiB | Data page p, with e = p >> 16 and group (p >> 7) & 511. |

**Address math.** Only shifts, masks and adds. Plaintext offset k MiB lands at a 1 MiB-aligned stored offset, so a writer's 1 MiB piece (db.rs:5726-5733) is one aligned stored write. The kernel then builds the folios it builds for a plain store, which is the folio rule in CLAUDE.md. The unit of the pager is 256 pages, exactly 1 MiB, and `Per`'s multiply goes. Overhead is 0.78% of the data plus a 3 MiB prefix that stays a hole except for the headers and the ring frames actually written.

**Tag entry** (32 B): tag16, session u8, flags u8 (PRESENT), rsv u16, version u32, rsv 8. A tag page is written once, when its group is complete. In Log and Replace files it is never rewritten (see §4).

**Frame**: a clear part of 16 B (magic u16, session u8, kind-hint u8, seq u64, body_len u32), then a body sealed under the session key. Its entries:
- `TAIL{off, bytes}` holds the bytes of the current partial page, sized to the bytes;
- `HOME{first, count, tags}` lists pages that went home in this call;
- `GROUP{g}` says that group g's tag page went home;
- `OVR{page, version, ct4096, tag}` is an override image;
- `CKPT{len, tail page, open-group tags, override list}`;
- `REC{len, all tags}` is a Replace record or a Once close.

**Names.** A directory entry is `x` + base64url(nonce12 ‖ AEAD_Ks(logical name padded to 128 ‖ file_id ‖ name_seq u64) ‖ tag16), 240 characters, under NAME_MAX 255. The longest logical name today is about 99 characters plus a suffix (db.rs:12711). A file_id is bound both in the name and in the header, so swapping two inodes is refused at open.

### Sealed objects

| object | key | nonce (96 bit) | AAD |
|---|---|---|---|
| store key in marker | caller's | as today | as today |
| header A/B body | store key K_s | `0x01` ‖ 88 random bits | `"pfh"` ‖ slot ‖ hdr_seq |
| session key wrap | K_s | `0x02` ‖ 88 random bits | `"pfk"` ‖ file_id ‖ class ‖ idx |
| sealed name | K_s | `0x04` ‖ 88 random bits | `"name"` |
| data page p | session key S_i (one per write session) | `0x10` ‖ p (u56) ‖ version u32 | file_id ‖ p ‖ version ‖ valid_len u16 |
| frame | S_i | `0x11` ‖ seq u64 ‖ 0 (u24) | file_id ‖ seq ‖ body_len ‖ session |

Every nonce under K_s is random behind a domain byte. A rollback therefore cannot replay a counter under a long-lived key, and this fixes fresh's header-nonce flaw.

A session is one Handle's writing life in one process: a new random 256-bit key, appended to the header table. The header is written A/B and synced before the first write under the session; that sync sits beside a file open, which the rules allow. Within a session one writer draws each (p, version) and each seq once. A unit test enumerates every nonce format and asserts the domains are disjoint. A test hook records every (key, nonce) pair sealed under the crash fuzz and asserts that none repeats.

A WAL resumed after a crash, or a recycle, is a new session, so no counter survives a process.

## 4. Protocols

**Handle.** There is one per file_id per process, shared by every name and clone (`chain`'s second handle, db.rs:4498; the helper's dup, db.rs:6218; the run's hard link, db.rs:28989). It holds:
- `len`;
- the tail page;
- the open group's tags;
- the override table;
- the ring head and pinned spans;
- a sync sequencer.

**Write (all classes).** Bytes go into the tail page. A page that fills is sealed into a reused 2 MiB staging buffer, with version 0 under the current session, and its tag goes into the open group. One write call issues one `pwritev` of its full pages. A completed group's tag page goes in the same call.
- **Log class**: the same call appends one frame holding the new tail bytes (`TAIL`, sized to the bytes, about 40 B of overhead) and the `HOME` tags, so every write call is in the page cache before it returns. That is process-crash parity under a buffered policy (CLAUDE.md, crash discipline).
- **Once class**: no frame per write.
- **Replace class**: home pages are rewritten in place at version = record seq.

**Patch** (`write_all_at` below what was written):
- **Once**: rewrite home in place at version+1; rewrite the tag page in place if the group was complete. This is safe because the file is trusted only after its final sync. The seal's segments are renamed before the fsync, but no manifest names them until after it, and the open sweeps what no manifest names (CLAUDE.md, crash discipline).
- **Log**: never home. The new image goes as an `OVR` frame and is carried in every later `CKPT`. It is read from the override table on a miss. The bound is 4 pages a file, asserted by a test that counts. The run's finish patches page 1 (the section header at key_off = SUPER, db.rs:7728) and page 0 (db.rs:7824). The run is created with `head_reserve: 0` (db.rs:6676), so the reserve copies at db.rs:7746-7771 do not reach it (not verified line by line; experiment E0). The WAL's chain byte (db.rs:4498-4504) is one override per life.
- **Replace**: rewrite in place at version = seq+1, then a `REC` frame at ring slot seq%2 holding len and every page's tag. One AEAD covers the whole slot, as today's sealed slot does, so a mix of versions cannot verify.

**Sync.** Syncs run through a per-Handle sequencer. A sync that finds one in flight waits for it, then writes its own record. This answers the slot-role race: the helper's `dup.sync_data()` (db.rs:6218) and the presync thread (db.rs:29030-29045) really do sync one file concurrently with its writer.
- **Log**: if the frames since the last `CKPT` pass 256 KiB, write a `CKPT` first. Then `fdatasync`. A `CKPT` is never synced on its own.
- **Once**: write a `REC` (len, tail, open-group tags) into ring slot A or B, never the slot of the last completed sync, then `fdatasync`. The final sync after the finish is what makes the file trusted.
- **Replace**: `REC`, then the engine's own `fdatasync`.

**Ring reuse (Log).** Two spans are pinned:
- the span from the last *synced* state's base `CKPT` to its sync frame;
- the span from the newest `CKPT` (synced or not) to the head.

A frame outside both may be overwritten. Under a buffered policy this costs no sync. A `CKPT` is written once the head span passes 256 KiB, so each span stays under about 300 KiB and the 1 MiB ring cannot lap a pinned frame. No sync the policy did not ask for is ever issued, which answers the fold-fsync finding.

**Read and miss.** The pager keeps its protocol: states, mark/era/grace, `Slot.page`/`oldest_lent`, `Hold`, rests and `check`. `CHUNK` becomes 4096 (pager.rs:108-109), and the plaintext backing stays a memfd, charged as shmem a page at a time.
- A hit is the one load and compare in `ensure` (pager.rs:1312).
- A miss:
  1. CAS EMPTY→BUSY.
  2. Take the tag from the tag tier (tags loaded a group at a time, pinned up to B/16, LRU past that, charged to the budget), or from the Handle's open group or override table.
  3. `pread` the stored page straight into the memfd page.
  4. Decrypt in place, then mark HOT.
- A range the reader names (the windows, a scan's span, a build's run, `willneed`) is filled as one `MADV_POPULATE_WRITE` and one `preadv` of up to 64 pages.
- The pager counts its own misses and refills per thread and feeds them through the existing seam where `thread_major_faults` (zfile.rs:4942) is read. Under `pread` fills, `ru_majflt` stays zero and the controller would read every store as fitting.

**Evict.** Unchanged: mark, era bump, wait for `oldest_lent`, `MADV_REMOVE` per 1 MiB unit. Nothing is dirty, so nothing is written back.

**Trust.** A failed tag:
- in a Once or Log file, below the length its own recovery established, poisons the store (`Pager::check`), as today;
- in a Regen file, is that file's `InvalidData`. The ordered index's open checks by the piece for `Pg` and recomposes on error (db.rs:8503-8526). A plain `Os` file keeps its whole-file CRC over the mapping, chosen once at open beside the open syscall. The form file is taken out of service. Nothing is verified at an unclean open.
- in a Replace file, `read` returns the file with failed pages as zeros, or one zero page if the `REC` fails. The plain `manifest_open` CRC then calls it torn, and `manifest_read_all` takes the other slot (db.rs:11890-11913). Returning `Err` instead would abort that function through its `?`.

**Rename, link, remove.**
- `rename` is `rename(2)` from the old sealed spelling to a new spelling with name_seq+1. If the target logical name existed, its entry is unlinked after. An open that finds two entries for one logical name keeps the higher name_seq and unlinks the other.
- `hard_link` is a bare `link(2)` to a new spelling with the same file_id. The run's close keeps both names (db.rs:28989).
- `remove_file` is `unlink` of the stored entry.
- `sync_dir` and `open_dir` act on the real directory. Every name operation is a kernel name operation, so name durability is the plain store's exactly.

**Open.**
1. Read the marker.
2. List the directory and decrypt the names. An entry that does not decrypt is swept, as a plain store's unnamed temp is.
3. Resolve duplicates.
4. Open headers lazily, at a file's first open.
5. Load a Log or Replace file's ring frames at its first open (at most 1 MiB, Log files are few).

The engine's own sweep then runs by logical name (db.rs:27632), unchanged.

**Recovery (Log).**
1. Take the newest valid header.
2. Scan the ring for frames that verify.
3. Starting from each `CKPT` in descending seq, take the longest run of contiguous seqs whose frames verify and whose `HOME`-listed pages verify. The state is the frame before the first failure.
4. A page sent home by frame k that fails ends the state at k-1, whose tail frames still hold that page's bytes. They do because those frames are pinned until a sync covers k's home write.

Falling back to an older header never happens unless the newest header itself fails. This answers the "discard acknowledged commits" finding. `recover_direct` (db.rs:6948) and the WAL replay then read the recovered plaintext as they read a plain file's.

**Recycle.** A new session is appended to the spare's header with len 0, the header is synced, and the spare is renamed. A crash at the rotation reads the old log's name on a file whose newest session has length 0: an empty log. The `recycles` exception in replay goes.

**Preallocate.** Under `Pg` it is a no-op, or `fallocate` past the logical length; the logical length is unchanged. The WAL's commits become first writes.

## 5. Crash windows against the plain engine

| kind | process crash | power loss | vs plain |
|---|---|---|---|
| seal/merge output (Once) | the page cache has pages and `REC`; unnamed by the manifest until fsync, so swept | before final `REC`+fsync: swept; after: whole | equal |
| run temp (Log) | every write call's pages and frame are in the page cache | the last synced state: longest verifying prefix from a pinned `CKPT`; torn home page → its tail frames; finish patches are `OVR` frames, home pages 0 and 1 untouched | equal for that file; see the store-level note below |
| WAL (Log, S6) | as plain | torn tail = end; chain byte is an `OVR` synced by the `wal.sync()` that follows it (db.rs:27078); a torn `OVR` leaves the loose kind, which plain also leaves if the sync is not reached | equal |
| spare / recycle | as plain | new session synced before the rename; stale data fails under the new session's key | equal; the `recycles` exception goes |
| manifest slot (Replace) | as plain | torn → `REC` or pages fail → zeros → CRC fails → the other slot is the store | equal |
| ordered index (Regen, unsynced) | as plain | torn → `InvalidData` → recomposed | equal |
| form file (Regen, synced after rename only with `plan.sync`, zfile.rs:4359-4383) | as plain | torn → that file out of service, rebuilt | better than today's cipher (today it poisons); equal to plain |
| names | rename(2), link(2), unlink | duplicate logical names resolved by name_seq | equal |
| marker | A/B | A/B | equal |

No page or sector write is assumed atomic. Every torn page fails its tag, no acknowledged byte lives only in a page that is overwritten, and tag pages of durable groups are never rewritten. Once files rewrite theirs only before they are trusted.

**The store-level property.** Today a run's commits are frames in the sealed WAL (`log_run`), so a damaged run reads as a cut of one log. Once the run is its own log, a byte changed in the run temp's newest frame recovers the run at its previous commit while later WAL commits replay. A plain store already behaves so: `recover_direct` stops at the last good marker. This is plain parity and a weakening of today's cipher property. It is decision 1 in §12.

## 6. What goes and what stays (estimates from the maps, not counted line by line)

**Goes:**

| item | lines |
|---|---|
| vault.rs: memfd table and write-once rule, `Kind`, `companion`, `kept`, `persist*`, the sweep's form rules, the `.z` cascade, tokens and wrapped keys in `seal_manifest`/`open_manifest`/`adopt_all`, `LogCipher`, `passes()`, `Ahead`, `MappedFile::owed` (vault.rs:468-526, 562, 609-635, 724-746, 889-1113, 1166-1182, 1299-1489, 1575-1657, 1773-1858, 2118-2197) | about 1,500 of 2,230 non-test |
| db.rs: the WAL's cipher branches (4134-4580), `RUN_LOGGED`/`Direct::logged`/`log_run` (4746, 11520, 28219-28223, 28834-28881), the persist sites (12574, 12865-12866, 13033-13034, 28999, 29525), the manifest's late envelopes (11661-11685), `CipherCounts::late`, then `cipher_controller` and `zpages_under_cipher` (11529, 27305, 27604) at S5 | about 600 |
| pager.rs: the 4080 chunk, `Per`, the 255-page unit, adopt-from-envelope fills (966-1000), most of the unbacked-file accounting in `could_give` (509) | about 250 |
| ordindex.rs: the paged-map skip of the CRC (413-428), replaced by a by-piece check for `Pg` | about 15 |

**Stays:**
- the door and the decoy directory;
- the marker and `rewrap`;
- the pager's state machine, grace, rests, `Hold`, `crowded`/`swamped`/`breathe` and `check`;
- the readers' ask path (`Bytes::piecewise`, `Sect`/`Pieces`, `scan_pieces`/`scan_either`, `Sec`'s arms, about 2,800);
- `ATTN_PAGED`/`paged_begin`/`end_slow`;
- every rest and `unlent` site;
- zfile's held build (`SegSrc::Kept`/`Walked::Held`, zfile.rs:4302-4320), selected by the source's kind (`piecewise()`) instead of `vault::kept`. A paged source cannot be kernel-mapped;
- crypt.rs v1 and web/crypt.mjs, frozen.

**New:**
- `pagefile.rs`, about 1,800;
- `vault::File` and its call sites, about 300;
- tests and a crash model, about 1,400.

## 7. Why a store in the clear pays nothing

- Plain `MmapBytes` and every reader stay byte for byte; `lend_at` (bytes.rs:480-487) is untouched. WARM, at S7, is decided at map time by which `MmapBytes` a cipher store builds, never by a branch in a reader.
- The only new code a plain store runs is the `File::Os` arm. It runs once a 1 MiB piece (`writev_all`, db.rs:6046), once a finish patch, once a commit's write and once a sync or hint: a branch beside a syscall, which the rule allows.
- Plain work is removed: the `RUN_LOGGED` test at a commit's end, each `vault::persist*` load and branch, and `cipher_controller`'s test at open.
- `preallocate` and `recycle` run today's bodies on the plain arm.
- The ordered index's plain open keeps its mapped whole-file CRC.

**Counted rows.** Callgrind by function, each probe built at `origin/main` from this tree's example sources and at the tree, read with `callgrind_annotate` side by side, never by total alone:
- `read_only.rs supdb 100000 1 n-none`;
- `lag0_probe.rs supdb 100000 1`;
- `work_probe.rs 100000 whole` (the writer's round, `AlignedWriter` holding a `File` enum: the named risk is `write_buf`'s inlining);
- `merge_walk.rs 100000 4 1`;
- `load_only.rs supdb 100000 1` (`commit_direct`; also a seal's open and landing, the ordered index's open check);
- `read_only.rs` and `lag0_probe.rs` with `supdb-nocachezpages` (the form's point read and scan).

The threshold is zero added instructions in any per-key, per-entry, per-block or per-step function.

## 8. Predictions

| number | today | predicted | rests on | guess? |
|---|---|---|---|---|
| miss | 4 µs | about 3-3.5 µs single page; about 1.5-2 µs a page in 64-page range fills | probe.md: pread from a cached file 0.75 µs into a present page; a fresh memfd write fault 1.9 µs, taken inside the pread's copy; decrypt of 4 KiB at about 7.9 GB/s, about 0.5 µs (CLAUDE.md, crypt_cost). The fault stays, so no design halves the miss. | the sum is from separate probes; E2 measures it as one operation |
| out-of-cache point read (64 MiB cache) | 0.17x | 0.19-0.22x | fills per raw read stay about 1.03, since a 120 B record rarely straddles 4080, so the gain is the miss cost alone | guess, bounded by E2 |
| ordered load | 0.68x durable, 0.52-0.57x buffered, 2.5 B/B | 0.85-0.95x durable, 0.78-0.90x buffered, about 1.35-1.45 B/B | measured removals: the log double write (20-50 ms of commits over 300), the drain's envelope 43-55 ms of 77-94, and the 14 ms unlink (CLAUDE.md, the run under a cipher). Added: about 5 ms of AEAD per 40 MB, about 40 B a commit frame, a `CKPT` per 256 KiB of frames. The run's held view keeps today's fresh pages. | the added side is a guess until E5 |
| compact form under bound | 0.73x | 0.95-1.05x of raw under the cipher | measured: 0.60 faults a read through the form against 0.43 raw, explained by every form page straddling two 4080-byte chunks (CLAUDE.md, the compact form). A form page is now one stored page with a resident tag. | the upper bound is E1; the tag tier's residency is E6 |
| in-core point read | +339 instr over 818 | about +320-335 through S6 (only `Per` and straddle cases go); WARM at S7 about +20-60 | the breakdown of the 339: asks, window, crowded look, failure look, 95 of `not_lent` (CLAUDE.md) | the S7 figure is a guess until E3 |

The scans in core, 0.89-0.94x, are unchanged until S7.

## 9. Experiments owed before building (cheapest and most decisive first)

- **E0, audit by reading (no build).** Confirm three things:
  1. The run's finish patches only pages 0 and 1 (db.rs:7728-7824 against `head_reserve: 0`, db.rs:6676).
  2. Every create that a manifest later names is followed by a `sync_dir` before the manifest.
  3. No `read()` caller other than the manifest slots, WAL replay and `recover_direct` reads a Log or Replace file.

  *Kill:* more than 4 overrides in a run's finish breaks the Log class's bound, so redesign the patch path before S4.
- **E1, the form's upper bound in the clear:** `bench/scripts/pair.sh supdb-autozpages supdb-nozpages 5000000 --bound 671088640 --passes read --reps 1 --ops-div 1`, with zpage builds and installs read beside it. *Kill:* the form not ahead of raw out of core in the clear makes prediction 4 moot; drop S5's controller step and keep `cipher_controller`.
- **E2, the miss, as a C probe** in `scratchpad/seam/work/`, cycling over 100k pages with the machine to itself. Arms:
  - `pread` of 4 KiB from a cached file into a fresh memfd page, plus AES-GCM in place (OpenSSL);
  - against a memcpy out of an envelope mapping into a fresh memfd page;
  - against a 64-page `MADV_POPULATE_WRITE` plus `preadv`.

  Run at 1 and 3 threads. *Kill:* a single-page fill at or above 4 µs and range fills above 2.5 µs a page drop the out-of-cache prediction to level; the design then stands on the load and the form alone.
- **E3, the plain cost and WARM's headroom.**
  - At S1: the callgrind rows of §7.
  - On a scratch branch: a cipher `map` that returns `lend = len` while the store fits, then `read_only.rs supdb-cipher 100000 1 n-none` and `lag0_probe.rs` against `supdb`.

  *Kill:* any plain per-key function moving stops S1 until the shape reads level; WARM under +100 is the condition for S7.
- **E4, a crash model** in Python, in the scratchpad, no timing. It models pages, frames, `CKPT`, pinned spans, overrides, the sync sequencer and recovery. It crashes at every write with every subset of in-flight pages landing and sectors torn, and asserts that recovery is at or after the last sync and a prefix of the writes. Mutants that must fail it:
  - no pin;
  - a `CKPT` not pinned;
  - an override written home;
  - recovery from the older header;
  - a concurrent sync claiming the in-flight slot.

  *Kill:* an acknowledged loss the model cannot close halts S4; the run keeps `log_run`.
- **E5, the device cost of the Log commit:** a C probe of 100 B, 4 KiB and 120 KiB commits with `fdatasync`, plain append against home plus frame, reading `/sys/block/vda/stat` and `strace -T`. This prices the A and F write cells before S4.
- **E6, tag residency** after S3: `mincore` over the tag regions and the tag tier's counts after the bounded read pass (adapt `scripts/resident.py`).

## 10. Stages (the gate green at each end)

- **S1, `vault::File` with only `Os`**, plus `preallocate`/`recycle`, through every door caller.
  - Tests: unchanged.
  - Gate: §7's callgrind rows equal to `origin/main`, and `sh scripts/check.sh`.
- **S2, `pagefile.rs` standalone**, under a stand-in cipher: layout, the four classes, frames, `CKPT`, pins, overrides, the sequencer, recovery, names, sessions.
  - Hand-built torn states: torn home, torn frame, torn `CKPT`, a concurrent sync, a lapped ring, recycle at rotation, duplicate names, a torn Replace.
  - The nonce-format enumeration test and the uniqueness hook.
  - A mutant pass of at least 30, held to firing.
  - Not wired.
- **S3, Once and Regen wired** for segments, merge pieces, `.o` and `.z`, under `Options::cipher_pages` (arm `supdb-cipherpages`), with the envelope shape kept as the default arm `supdb-cipherenv`.
  - Sealed names, no tokens in the manifest; pager geometry per file (4096 for `Pg`, 4080 for envelopes).
  - `RUN_LOGGED` still covers runs.
  - Tests: `tests/cipher.rs`, and `ZFUZZ_CIPHER=1 ZFUZZ_CIPHERCACHE=16384` with `ZFUZZ_CACHE=all ZFUZZ_OPTVARY=1`, several seeds a core, on both arms.
  - Pairings:
    - `pair.sh supdb-cipherpages supdb-cipherenv 300000`;
    - `pair.sh supdb-cipherpages64 supdb-cipherenv64 1000000 --reps 4`;
    - `pair.sh supdb-cipherpagesauto supdb-cipherauto 5000000 --bound 671088640 --passes read --reps 1 --ops-div 1`, with builds and installs read beside it.
- **S4, Log class for the run temp.** `log_run`/`RUN_LOGGED`/`Direct::logged` are deleted under the arm.
  - Tests: the torn states in `tests/cipher.rs` with mutants (an override written home, no pin, no sequencer).
  - Pairings, with `SUPDB_LOAD_PHASES=1 SUPDB_DRAIN_STEPS=1` and `/sys/block/vda/stat`:
    - `pair.sh supdb-cipherpages supdb-cipherenv 300000 --passes load` and the buffered twins `supdb-ingestcipherpages supdb-ingestcipherenv`;
    - then against `supdb`.
- **S5, the pager's miss counts into the census and `zselect` Input.**
  - A test that the controller reads them under a cipher.
  - Then delete `cipher_controller`, keyed on E1 and S3's pairing.
- **S6, WAL, spare and manifest slots under the layer** (Log and Replace).
  - Delete `LogCipher`, `passes()`, `seal_manifest`.
  - Tests: the log cut at every byte, `ZFUZZ_RECYCLE=1`, and the Replace torn test.
  - Pairings: the buffered mixes `pair.sh supdb-ingestcipherpages supdb-ingest 300000 --passes mix` with `SUPDB_MIX_KINDS=1`.
  - Flip the default, and delete the envelope arm with its pairings recorded in the PR.
- **S7, WARM.**
  - Files whose plaintext fits 0.85·B are born or promoted WARM with `lend = len`, keeping the blob id (carrying `ranks`/`bounds` across is not verified).
  - Demotion republishes `lend = 0`. A WARM file still reachable from a pinned old state counts as un-givable, under a cap.
  - Counted by E3 at its end.

## 11. Considered and rejected

- **userfaultfd or PROT_NONE transparent space.**
  - Default Docker, Podman and containerd seccomp deny userfaultfd, which leaves M2.
  - M2's fault plus `mprotect` is 7.1-11.2 µs before any pread or decrypt (probe.md), against a whole miss of 4 today.
  - The handler-thread mode is 30-45 µs.
  - SIGBUS mode returns EFAULT on a caller's `write(2)` of a lent value.
  - It would put signal handlers in an embedded library, running the caller's cipher in signal context.
  - `pread` fills blind the fault counters.
- **Deleting the pager and the ask path.** Nothing else holds a miss at 4 µs in every container.
- **An 8 KiB tail slot per commit** (env, minimal, transparent's J). The device bytes on every small commit, against a frame sized to the bytes.
- **Rewriting tag pages in place for Log files.** One torn write puts 128-255 durable tags behind it.
- **Fresh's single-sync doublewrite and its fold fsync.** The home copy can tear before its image is durable, and a fold is a sync the policy never asked for. Both are recorded bugs of that shape in CLAUDE.md.
- **Anonymous `MAP_NORESERVE` plaintext.** Charged in full under `vm.overcommit_memory=2`, and it moves plaintext into the `RssAnon` the controller's budget subtracts (zfile.rs:5260-5267).
- **O_DIRECT exclusive reads as a default.** Segments are published before their fsync; it has no readahead; it can be refused by the filesystem. At most a later arm.
- **Verifying regenerable files at an unclean open, or a boot-id rule.** The open's cost grows with the forms; lazy failure gives the same safety.
- **WARM through a cold callee in `lend_at`.** CLAUDE.md measured a second callee in a branch at 2-7 instructions an entry.
- **Keeping `Kind`/companion/tokens through the stages** (minimal). Companions lose their names at reopen once `persist` goes.
- **Keeping `LogCipher` permanently** (lead). That is form knowledge in the vault; it is kept only until S6.
- **link+unlink for `hard_link`.** The run's close needs both names (db.rs:28989).

## 12. Owner's decisions

1. **Store-level integrity once the run is its own log.**
   - Option (a): accept plain parity, so each file recovers to a prefix of itself and the property in `tests/cipher.rs` is restated per file.
   - Option (b): bind each run commit's seq into the WAL at a cost per commit.
   - Recommendation: (a). (b) brings back part of the double write, and a plain store already has (a).
2. **Moving the WAL and manifest under the layer (S6)** rather than keeping them engine-sealed. Recommended: yes, behind an arm, after S4's crash model has run a release. The owner's seam wants the vault to know no file by name.
3. **The default plaintext budget.** A caller-set cap today, against a cgroup-derived default. Recommendation: keep the caller's `cache_bytes`, add a cgroup-derived default only with S7.
4. **Leakage accepted:** file count, sizes to the page, commit sizes (from frame lengths), which pages are rewritten, and timing. Names are now hidden, including `wal-` and `manifest`. Recommendation: accept, and document it.
