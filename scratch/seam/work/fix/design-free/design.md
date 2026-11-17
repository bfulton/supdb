# A Log commit in one device request: design (design-free)

This is a design only. Nothing was built and no probe or model was run for it. Every time below is one of two things:
- E5's pooled medians (`work/e5-commit/raw/pooled.txt`, `iocost-summary.txt`, `tracecount.txt`, `jext4-summary.txt`);
- arithmetic on those medians, marked **est.**

The shapes proposed here have not been measured. §9 gives the probe and the model that test them first. Code is cited at `3df85d22`. "Not verified" marks claims I did not check.

## 0. The answer

1. **The WAL and the spare do not belong in the paged Log class.** They become a **Stream** class: today's sealed record per write call (`LogCipher`, vault.rs:1767-1858), moved below the door.
   - A commit is one append, so it costs the plain WAL's requests by construction. E5 prices that shape as the plain WAL plus 32 B a record.
   - Names are hidden, and the engine's WAL cipher branches go, as the design's S6 wanted.
   - Decision 2 is then neither (a) as written (a paged Log WAL is E5's shape A: 1.29-1.46x at 4 KiB and above) nor (b) (cipher code kept in the engine). It is **(c)**, the record format under the door.
2. **The ordered run (the only paged Log file left) uses JRP**, short for "journal, then record page". Every door write call reaches the device as one contiguous request, one of two kinds:
   - a TAIL frame appended to a journal ring at the file's head, used while the bytes not yet home are few (< 128 KiB, and the call adds < 64 KiB);
   - a **homing**: the full pages not yet home, followed in the same `pwritev` by a **record page (RP)** that carries their tags and the partial tail page's bytes, written into fresh slots right after the previous RP.

   No slot that a page or an RP has used is ever written again. The data grid therefore shifts by an RP's one or two slots at each homing. A reader of a finished run adds one table lookup to the design's shifts and masks (§5.9).
3. **Why the grid has to move:** with fixed homes, a one-request commit has to write extra bytes at least the size of the *next* commit (§2). Design A pays a second request instead; JRP pays dead slots (about 3% of disk at 133 KB commits).
4. **est.** per durable commit of the run (B = the plain run, D = today's cipher run, A = the design; all from E5):

| commit | JRP path | JRP est. | B | D | A |
|---|---|---|---|---|---|
| 100 B | journal | ~105-115 µs | 149 | 160 | 103 |
| 4 KiB | journal | ~125 | 201 | 221 | 247 |
| 13.3 KB | journal | ~165 | 216 | 216 | 294 |
| 133 KB | home | ~425-440 | 386 | 488 | 571 |

JRP is ahead of B below about 64 KiB: the journal ring is preallocated, so the file does not grow and fdatasync writes no inode. JRP is about 1.1x of B at 133 KB, where it pays B's own growth plus the cipher's CPU. These are estimates, and E5b (§9) is the test.

## 1. What this device charges for a commit (E5)

| cost | evidence | size |
|---|---|---|
| **Growth.** A write into a never-written block, or one that moves `i_size`, makes fdatasync write the inode (`/` is ext4 with no journal). | iocost: `app k=1` 237 µs (3.74 ios) against `con k=1` 114 (2.03); tracecount: B and Cf `probe/meta` 1.01 io/commit, Cp 0.04 | +60-120 µs a commit |
| **A separate data request** (non-contiguous dirty ranges are separate block requests) | `sep k=2` 110 against `con k=2` 103; A0/B 1.20 at 4 KiB with one more data request | +10-40 µs, larger at 133 KB (A0 is +87 µs of sync time over B for +1.8 requests) |
| **Bytes** | `con` k=1 → 33: 114 → 311 µs, about 6 µs a page; the flush pays what the write-back cache holds (CLAUDE.md, 0.85 ms/MB) | ~1.5 µs/KiB |
| **Flush** | one per fdatasync; fua 0 | fixed |
| **CPU** of AES-GCM, staging and syscalls at 133 KB | A0 cpu 126 against B 84 | ~+40 µs |
| **Write-through** at 133 KB | A 571 / Aw 509 / A0 506 | pwrite +~30 µs CPU; memcpy +~65 |

Read the device's request count from the *dirty ranges at writeback*, not from the syscalls. B makes one to three `writev` calls a commit and reaches the device with 1.13 data requests (tracecount), because adjacent dirty pages merge into one request (`max_sectors_kb` 4096).

The queue limits, read now: logical sector 512, physical 4096, `write_zeroes_max_bytes` 0 (no cheap written-zero preallocation, so `FALLOC_FL_WRITE_ZEROES` is out), `fua` 0, no atomic-write unit. **I assume 512-byte sector atomicity, as the requirement says.**

On journaled ext4 (E5 `jext4`, indicative), growth costs a journal commit. B/Cp is 2.7x at 100 B and 1.9x at 133 KB, so the thresholds in §5.3 are per filesystem.

Order of costs: growth > a separate request ≈ 3-6 pages of bytes > the cipher.

## 2. Fixed homes and a one-request commit cannot both hold

**Claim.** Assume four things:
- every data page p has a stored slot f(p) fixed in advance, increasing in p;
- a commit's new data and its record reach the device as one contiguous stored range [x_n, y_n), with no holes;
- no request changes a sector that holds the last synced state's record unless a surviving copy exists elsewhere;
- record n is needed until a later record is synced.

Then commit n writes at least f(L_{n+1}) − f(L_n) bytes beyond its data. That is the next commit's size.

**Proof.**
1. Slots below f(L_n) hold durable data. They can only be rewritten with identical bytes, which cannot hold a record. So R_n ⊂ [f(L_n), y_n).
2. Commit n+1 writes data at [f(L_n), f(L_{n+1})) and must not touch R_n. So R_n ⊂ [f(L_{n+1}), y_n).
3. The request has no holes, so it wrote all of [f(L_n), y_n). ∎

A record that shares the sector holding L_n is inside step 2's range, so the claim covers it too.

**Consequences at 133 KB.** Fixed homes leave three choices:
- (i) about 133 KB of extra bytes per commit, roughly +200 µs at the flush;
- (ii) the record in a second, non-adjacent place, which is a second request (design A: data plus ring frame, plus a tag page every 512 KiB);
- (iii) homes that move.

JRP takes (iii): a record's slots are skipped by the data that follows.

The same argument rules out *displacing* data under the old record and carrying it in the new one: the new record must hold the displaced bytes plus its own header, so it grows with every commit.

## 3. The suggested shapes, judged

| suggestion | verdict |
|---|---|
| full pages home, record in the slot after them | Unsafe as stated: the next commit's data lands on the record (§2). Safe once that slot is never reused for data, which is JRP's home path. |
| data-journal everything into a contiguous log, home lazily | One request, but every byte twice. At 133 KB the second copy costs more than a request (§1). Below 64 KiB it is cheaper than a request plus growth, so it is JRP's journal path. |
| **per-sector sealing of the tail page** | **Sound under AES-GCM** if each complete sector is sealed once and never again, and only the last partial sector is resealed, at a fresh nonce each version: no (key, nonce) repeats. It does **not** remove the copy. The partial sector's new version has a new ciphertext, so a sector that lands new beside a tag that does not land loses that sector's synced bytes, and up to 511 B still ride in the record. It also costs 8 tags a page (3.1% against 0.78%) and 8 AEAD calls a page read. Rejected: JRP carries the whole partial page (≤ 4095 B) in the RP at no extra request. |
| reseal a growing tail page **under the same nonce**, so that earlier sectors keep their ciphertext | **Unsound, fatal.** Two GCM messages under one (key, nonce) give a tag difference that is a nonzero polynomial in the GHASH key H; recovering H forges any message under any nonce for which one valid ciphertext is known. ChaCha20-Poly1305 leaks that nonce's one-time Poly1305 key the same way. |
| keystream/MAC split: C = P ⊕ AEAD_N(0^4096).ct with N fixed per page, each version authenticated by an AEAD over empty plaintext with C's prefix as AAD under a fresh nonce | **Sound** for an AEAD whose ciphertext under a unique nonce is pseudorandom, provided no byte of a page changes under one N (a patch takes a new N) and the zero-message tag is never stored. Earlier sectors stay byte-identical, so the tail page can be rewritten in place with no copy. Cost: two AEAD passes a page on write and on every miss. That drops to one only for a *stream* AEAD (GCM, ChaCha20-Poly1305, CCM: ct = pt ⊕ keystream(K, N)), where the final page sealed normally under N has the same ciphertext. `Aead` does not promise that property (crypt.rs:137-151), and SIV/OCB lack it. Not used: it would save only the ~2 KB tail an RP carries. |
| tail page rewritten home in place, with a copy of its previous version in the same request | **Unsafe.** The copy is as unsynced as the overwrite, and any subset of sectors may land. The copy must be durable before the overwrite, so it belongs in an *earlier* request. JRP never rewrites the tail page at all: the previous homing's RP holds it until the page is full. |
| what a file nobody reads before replay can skip | The WAL as a Stream file has no pages, no tags, no ring, no write-through and no pins (§4). The run under JRP writes no tag pages until `finish` (tags ride in the RPs). |

## 4. The WAL and the spare: the Stream class

**Stored file:**
- Header, 64 B: clear magic `SUPDBST1`, version, class, `file_id[16]`, flags (`recycled`), `prev_fid_lo`, hdr nonce; sealed under K_f: `hdr_seq`. K_f as in design §3.
- Then records, byte-packed and appended: `len u32 (bit 31 = PATCH) | nonce12 | body | tag16`.
  - An APPEND body is the call's bytes. A PATCH body is `at u64 ‖ bytes`.
  - Key: K_rec = AEAD_Kf(0x15‖0^11, 0^32).ct, derived and deterministic.
  - Nonce: `prefix8 ‖ count u32`. The prefix is random per Handle (per open of the file), and the count advances at every sealing attempt; that is today's `Nonces`.
  - AAD: `"pfs" ‖ store_id ‖ file_id ‖ stored offset u64 ‖ len u32`.

**Rules:**
- **Write call** (`Wal::write` is one call per commit, of whole batches: db.rs:4436-4456): one record at the Handle's stored end, by `pwrite`. The layer owns the offset; `O_APPEND` is not used (today's `open_append`, vault.rs:1913-1929). The end advances only on success, so a retry (pending is resent, db.rs:4436-4455) overwrites a failed attempt's partial bytes with a record under the next nonce.
- **Patch.** A write below the logical length, which is the chain byte (db.rs:4498-4507), is a PATCH record.
- **sync:** `fdatasync`. The layer records the stored end at issue, to translate `drop_synced`'s fadvise (db.rs:4469-4478) and the writeback hints.
- **Read** (replay's `read_to_end`, db.rs:4546-4553): records from offset 64 to the first that fails its length or tag, with PATCHes applied in order. The result is today's `Wal::replay` loop (db.rs:4560-4580) moved below the door.
- **`set_len(n)`** (replay's cut): n must be some record's plaintext end, which it is because every record is whole batches; anything else is refused, as E4 F4 recommends. The stored file is truncated at that record's stored end.
- **Recycle and preallocate:** as design §4, with E4 F7's fix (a `recycled` name whose inode still has the previous `file_id` reads as an empty log).

**Device per commit:** one request, plus the inode when the file grows, plus the flush. That is Cf; with `recycle_wal` it is Cp. Bytes: payload + 32 B, each once. The page holding the end is re-sent as the plain WAL re-sends it.

**Crash:** append-only. A torn record fails its tag and is the end. Nothing durable is rewritten except identical sectors of the last page. Process crash: every completed call's record is in the page cache. Nonces: a repeat needs two opens of one file to draw the same 8-byte prefix.

`tests/cipher.rs:952` keeps its meaning: a cut at any byte is still a crash state.

## 5. The run: JRP

### 5.1 Layout (Log class, "interleaved" address mode)

| slots (4 KiB) | content |
|---|---|
| 0 | Header: clear `SUPDBPF2`, version, class=Log, `file_id`, nonce; sealed under K_f: the session key S wrapped, mode=interleaved. Written once at create, unsynced; the first fdatasync covers it (design §4). |
| 1-255 | Journal ring of frames, `fallocate`d at create (contiguous blocks; unwritten until lap 1 writes them) |
| 256 … | Data area: data pages and RPs interleaved. Data page p lives at slot **256 + p + dead(p)**, where dead(p) = the number of RP slots written before page p. |
| after the last data page, at `finish` | the final partial page, the tag pages, then FINAL |

**One session per file life.** The run temp is never reopened for writing. An unclean open reads it with `recover_direct`, writes a new segment from it and removes it (db.rs:27727-27770). So this class needs none of these:
- a session table beyond one entry, `continue`, TRUNC, recycle, the `.pfcopy` compaction;
- the resumed-header sync (E4 F1), re-homing (F2), tag pages during the Log phase, GROUP/HOME/CKPT frames.

**RP** (n_rp ≥ 1 consecutive slots, written once, never rewritten):
- Clear head, 32 B: magic `SUPDBRP1`, `n_rp u16`, kind (RP=1, FINAL=2), `seq u64`, `attempt u32`, `body_len u32`.
- Body, sealed under S with nonce `0x14 ‖ seq u64 ‖ attempt u24` and AAD `file_id ‖ "rp" ‖ slot u64 ‖ head`:
  - `len u64`: the logical length after the call;
  - `home u64`: H, every page below which is home;
  - `first u64`: the first page this homing wrote;
  - `ctr0 u32`: page `first+k` was sealed at `ctr0+k`;
  - `tags[(home−first)] × 16 B`;
  - `dead u64`: total RP slots, this one included;
  - `prev u64`: the slot of the previous RP;
  - the override list `{page, plaintext 4096}` (empty until a patch, §5.8);
  - `tail_len u16` and the tail's plaintext: bytes [H·4096, len), ≤ 4095.
- Then tag16.
- Size: about 33·16 + 2 KB average tail + 80 ≈ one slot at 133 KB, two when the tail is over ~3.3 KB, two to six for a homing of 1-4 MiB. An RP covers at most 1024 pages; a larger call is several `[pages][RP]` in the same `pwritev`.

**Frame** (ring): head of 16 B (`magic u16, kind, seq u64, body_len u32`), then a body sealed under S with nonce `0x11 ‖ seq ‖ attempt` and AAD `file_id ‖ "fr" ‖ head`, then tag16. Kinds: `TAIL{from u64, bytes}` and `OVR{page, plaintext}`. RPs and frames share one `seq`.

### 5.2 Nonces under S (random 256-bit per file; K_f per `file_id` as design §3)

| object | nonce | sealed |
|---|---|---|
| data page p | `0x10 ‖ p u56 ‖ ctr u32` | once, when full, at a fresh ctr; a retry reseals at a new ctr |
| frame | `0x11 ‖ seq ‖ attempt u24` | E4 F3's fix: a retry keeps seq and takes attempt+1 |
| tag page g (`finish`) | `0x12 ‖ g u56 ‖ ctr` | once |
| RP / FINAL | `0x14 ‖ seq ‖ attempt u24` | once per attempt |
| final partial page (`finish`) | `0x10 ‖ p ‖ ctr`, `valid_len` in AAD | once |

ctr and seq are monotonic in the one session, and the file is never written in a second session. So no (key, nonce) repeats across any rewrite, retry, crash and reopen. A unit test enumerates the formats, and a hook under the fuzz logs every (key, nonce), as in design §3.

### 5.3 Write rule: one door write call `(off, bytes)`, L' = max(L, off+len), J' = L' − H·4096

1. **Patch** (`off < L`):
   - inside the journal (`off ≥ H·4096`): a TAIL frame at the older offset;
   - on a homed page: an OVR frame carrying the page's new plaintext. Every later RP and FINAL carry it, and the page is never homed again (E4 F5 `final_ovr_rec`). The bound of 4 per file is design §4's (pages 0 and 1 of the run's finish).
2. **Home** if `len ≥ T_d` (64 KiB), or `J' ≥ T_h` (128 KiB), or the ring cannot place the frame past its pins:
   - seal pages H … ⌊L'/4096⌋−1 from the Handle's memory, at fresh ctrs;
   - build the RP with the tail [⌊L'/4096⌋·4096, L');
   - **one `pwritev` of `[pages][RP]` at slot 256 + H + dead**;
   - set H := ⌊L'/4096⌋, dead += n_rp, prev := the RP's slot.

   Every slot written is fresh, so the request is a pure append. A homing with no full page (ring full) is an RP alone.
3. **Otherwise journal:** one TAIL frame `{from = L, bytes}` at the ring head, skip-allocated past the pinned spans (design §4, Ring).
4. L := L'. Then the write-through into the pager's backing if the file is `open_mapped`, unchanged from design §4.

The engine's calls (B's shape in E5):
- whole-piece writes at mark time (db.rs:6891-6893, 5707-5753), which always home;
- at a sync, the rest (db.rs:6917-6936).

So a durable 133 KB commit is `[pages][RP]` (one request after merging, with two RPs when it crosses a 1 MiB boundary). About 6% of 133 KB commits split across a MiB boundary with a rest under 64 KiB; those journal one part and take two requests (13% cross a boundary × ~half). A buffered run (db.rs:28800-28807) homes its 1 MiB pieces as they complete and the rest at its n-th commit.

T_d and T_h are guesses for no-journal ext4; E5b sweeps them. On journaled ext4 growth costs a journal commit (§1), so T_d should be larger there.

### 5.4 Sync rule

The layer writes nothing at a sync. It issues `fdatasync` through the per-Handle sequencer and records `issued_seq`, the newest record written before the call.
- At completion, `synced_seq := issued_seq`, and the synced base is the newest RP with `seq ≤ synced_seq`.
- Syncs on two threads (the presync beside `finish`, and the helper if the run's writer starts one: `sync_ahead` is 8 MiB by default, db.rs:7992; whether the run's writer starts the helper is not verified) are in flight together and pinned.

### 5.5 Ring rule

Pinned (never reallocated):
- **P1**: frames from the synced base RP to `synced_seq`;
- **P2**: the same span for every sync in flight, from issue to completion (E4: without this, 206,729 losses);
- **P3**: frames from the newest RP to the head (process crash).

Each span holds less than T_h + T_d of TAIL bytes plus framing, about 200 KiB. With two syncs in flight that is ≤ 800 KiB of the 1020 KiB ring. If a frame cannot be placed, the call homes instead (§5.3 step 2), so the ring never waits.

A frame never straddles the ring's end. Ring pages are rewritten with identical bytes in the sectors that hold pinned frames.

### 5.6 Recovery rule (an unclean open: `recover_direct` reads the whole file, db.rs:6948-6952)

1. Header to S. No valid header means the file is empty.
2. Read the data area [slot 256, i_size) forward. A slot that begins with the RP or FINAL magic and whose record verifies (its AAD binds the slot) is a record.
3. Take RPs in seq order following `prev`. The state is the **newest RP R such that R, every earlier RP, and every page each of them homed all verify**. Pages are found at 256 + p + dead from the chain. With no such RP, R is the empty state.
4. Apply ring frames with `seq > R.seq`, contiguous in seq, taking for each seq the highest attempt that verifies. Each TAIL must begin at the current length (or be a patch below it). Stop at the first gap or failure.
5. The content is the home pages, R's tail and overrides, then the frames.

A FINAL that verifies (§5.7) is taken instead and the file is Once.

### 5.7 `finish` (Log → Once), with E4 F5's fixes

The engine calls `finish` before its last `sync_all` (design: db.rs:29005).
- One request, `[remaining full pages][final partial page][tag pages][FINAL]`, after the last RP.
- Tag pages hold 128 entries of `{tag16, ctr u32, rsv}` for page p at `tag_base + (p >> 7)` (the design's tag-tier format).
- FINAL is an RP of kind 2. It carries:
  - len;
  - the **dead table** (one entry per RP: the data page it precedes and the cumulative RP slots);
  - `tag_base` and every tag page's MAC;
  - the override list with plaintext;
  - `synced_H`, the home end at the last completed sync.

  Its last slot ends in a clear trailer `"SUPDBFN1" ‖ n_slots`, so an open finds FINAL by reading the file's last slot.

An open that picks FINAL verifies FINAL, the tag pages, the final page, the overrides, and **every page homed at or after `synced_H`** (`verify_sh`). On any failure it falls back to §5.6.

### 5.8 Patches

Only `finish`'s pages 0 and 1, which three verifiers confirmed by reading (design §4). They are OVR frames, then carried in RPs and FINAL, and the pages are never rewritten home (E4 F5).

### 5.9 Finding plaintext page p

- **Live file.** The Handle holds the dead table, all tags (20 B a page, 0.5% of the run), and the journaled bytes (≤ T_h + T_d). A page not yet home is served from memory (design §4, Read). The run's reads go through the write-through backing anyway.
- **Finished file** (after FINAL):
  - slot = 256 + p + dead(p), where dead(p) is a binary search over FINAL's table: about 300 entries for a 40 MB load, ≤ 14 steps for 10k;
  - tag = tag page `tag_base + (p >> 7)`, entry `p & 127`, through the tag tier;
  - overrides: ≤ 4 compared first.

  Against the design's shifts and masks this is one lookup on the miss path, ~10-30 ns of a ~3-4 µs miss (design §8), and nothing on a hit. A Once file keeps §3's layout, so the Handle carries an address mode (`dead.is_empty()` is the fixed layout).
- **1 MiB alignment.** The data area starts 1 MiB-aligned, but each RP shifts what follows. A writer's 1 MiB piece lands at `(256 + 256m + dead)·4 KiB`, not 1 MiB-aligned after the first RP.
  - The durable plain run already writes 133 KB at arbitrary offsets (B's folios are ladders too). Only a buffered run's whole pieces lose their 1 MiB folios.
  - The ciphertext's folios matter only on pager misses (design §4, Read). Unpriced.
  - Fallback if E5b's pagemap read or S4's pairing shows a cost: pad so each plaintext MiB starts on a stored 1 MiB boundary (holes). A homing that crosses a plaintext MiB then takes two requests (1 in 8 at 133 KB), and the apparent size grows to up to 2x.

### 5.10 Write-through

Unchanged: the run is `open_mapped`, written through and owed. It costs ~30 µs at 133 KB by `pwrite` (E5 Aw) and ~65 µs by memcpy (A). A lever outside this design:
- with `direct_recycle` off under a cipher, no `RunMap` is opened (db.rs:28736-28742; recycle defaults on, db.rs:4031, doc 3395-3409);
- the run's file is then read by nobody before its close, so the write-through can move to the close or go;
- that is an engine choice, priced like `supdb-norecycle`, and not designed here.

## 6. Crash argument

**The rule:** nothing durable is ever overwritten with different bytes.
- Homed pages, RPs, FINAL and the header are each written once, into fresh slots.
- A homing's request is a pure append past the newest RP, so a torn homing damages only its own fresh slots.
- Ring frames are reallocated only outside P1-P3.
- A ring page rewrite changes only sectors past the pinned frames. A sector that holds a pinned frame's end and a new frame's start is old (pinned frame intact) or new (both intact).

So no sector tear destroys a byte that a completed sync made durable.

**At least the last synced state.** A completed sync at `synced_seq` made durable every record written before it, and every page those RPs homed. Let R_s be the synced base.
- R_s and every earlier RP with its pages verify, so §5.6 step 3 picks R_s or a newer RP whose own chain verifies. A newer RP covers a longer state.
- If it picks R with `seq < synced_seq`, the frames from R to `synced_seq` are in P1, so step 4 reaches `synced_seq`.
- A newer RP that landed without its pages, or without an earlier RP's pages, fails step 3 and is skipped. Frames after it are unreachable across its seq gap, so the result is still a prefix of the writes.

**Only bytes that were written.** Every byte returned is under a seal bound to `file_id` and to its page/ctr, seq or slot, under the file's one session key. Torn or foreign bytes do not verify. Rollback to an earlier state of the file is the accepted model (design §5, decision 1).

**Process crash.** Every completed call left its frame or `[pages][RP]` in the page cache. P3 keeps the frames after the newest RP. A call killed mid-`pwritev` leaves a prefix whose RP or frame is absent or fails, so the recovered state is the previous completed call.

**Stream WAL.** Append-only. A failed attempt's bytes are overwritten only by its retry. The cut and recycle touch only bytes outside every recovered state (§4).

None of this is model-checked yet; E4b (§9) is owed.

## 7. Requests and bytes per commit

Durable, one fdatasync per commit. "Requests" are data requests at writeback / inode writes / flushes.

| | 100 B | 4 KiB | 13.3 KB | 133 KB |
|---|---|---|---|---|
| **run, JRP** path | journal | journal | journal | home |
| requests | 1 / 0 / 1 (inode in lap 1 only) | 1 / 1⁄32 / 1 (a homing every 32nd) | 1 / ~1⁄10 / 1 | 1 (+0.06 split) / 1 / 1 |
| device bytes | ring page 4 KiB | ring 8 KiB + homing 4 KiB avg | ring ~17 KiB + homing ~14 KiB avg | ~130 KiB pages + RP ~5 KiB + inode |
| payload reaching the device twice | 100% (frame, then home once per 128 KiB) | 100% | 100% | partial tail ~2 KB (in the RP, then home) + ~2 KB from split commits, ≈ 3% |
| est. µs | ≈ Cp 108 (+~1 seal) | Cp 115 + ~9 homing = ~125 | Cp 138 + ~28 homing = ~165 | B 386 + RP ~7 + CPU ~35-45 = ~430 |
| **WAL, Stream** | 1 / 1 / 1 (Cf 153; Cp 108 with `recycle_wal`) | Cf 206 (Cp 115) | Cf 215 (Cp 138) | Cf 381 (Cp 327) + ~25 seal |
| Stream payload twice | none; the last page re-sent as plain | none | none | none |
| for reference: A | 2.13 ios, 103 µs | 4.01 ios, 248 | 4.12 ios, 294 | 4.91 ios, 571 |

Bases:
- "Cp" stands for a commit into allocated, written blocks (E5's prefilled WAL). The ring reaches that state after lap 1; Cp's blocks were prefilled with zeros, so lap 1 here pays the unwritten-extent conversion (an inode write a sync) until it has written its 1020 KiB.
- The homing amortisation is a ~128 KiB append plus the inode, ≈ +270 µs every T_h of journal.
- The homing commit is a latency spike (~400 µs at 13.3 KB, once every ~10 commits). At T_h = 128 KiB, p90 at 13.3 KB may exceed B's 255; E5b reads p90/p99.

## 8. What it changes in design.md

**Gone from the Log class** (design §3-4 and §10's S2/S6 work):
- the session table beyond one entry, `continue`, TRUNC, recycle, the 48-session compaction;
- the resumed-header sync;
- GROUP/HOME/CKPT frames, tag pages during the Log phase, re-homing;
- the paged WAL, and S6's replacement of `tests/cipher.rs:952`.

E4's F1, F2, F4 and F8 no longer apply to paged Log files. F7 moves to the Stream class. F3, F5 and the in-flight pin stay.

**New:**
- RPs and the homing rule;
- the interleaved address mode with FINAL's dead table, plus the pager branch on it;
- the forward record scan at recovery;
- tag pages and FINAL written once at `finish`;
- the Stream class: `LogCipher` (~90 lines) plus PATCH, the `set_len` mapping, recycle and offset translation.

Not counted line by line. A guess: the Log part of `pagefile.rs` comes out a few hundred lines smaller, and the Stream class adds ~250 lines.

## 9. Test it first

**E5b, the probe** (`work/fix/design-free/e5b.c` from a copy of `e5.c`; wait for no bench process, one timed thing at a time):
- **Shapes:**
  - `J`: JRP as §5, without write-through, with the ring `fallocate`d;
  - `Jw`: J with write-through by `pwrite`;
  - `S` and `Sp`: the Stream record + fdatasync, over a fresh file and a prefilled one;
  - kept from E5: `B`, `A0`, `D`, `Cf`, `Cp`.
- **Run:**
  - sizes 100, 4096, 13300, 32768, 65536, 133000;
  - 1000 commits, 10 reps, shapes rotated, one process each;
  - `E5B_TD ∈ {16, 32, 64, 128} KiB` and `E5B_TH ∈ {64, 128, 256} KiB` at 13.3 KB and 32-64 KiB;
  - a buffered variant syncing every 8th commit for `J` and `B`.
- **Read:**
  - pooled and paired as E5;
  - p90/p99 from the latency dumps;
  - tracecount's requests by kind;
  - `st_blocks` (dead slots), homings, RP slots, frames and journaled bytes;
  - after a buffered J load, the ciphertext's folios through `/proc/self/pagemap` + `/proc/kpageflags` (CLAUDE.md).
- **Predictions (kill if missed):**
  - J's data requests ≤ 1.1 a commit at every size;
  - inode writes ≤ 0.15 a commit at ≤ 13.3 KB after lap 1;
  - J/B ≤ 0.85 at 4 KiB and 13.3 KB;
  - J/B ≤ 1.15 at 133 KB;
  - J/A0 ≤ 0.90 at 133 KB;
  - J/D ≤ 0.92 at 133 KB;
  - S/Cf within 1.05.
- **Fallback.** If J at 133 KB is not at least a request (~25 µs) ahead of A0, the interleaving is not worth its reader table. Then use **JF**, which is §5 without the interleaving:
  - the design's §3 fixed layout and its tag pages;
  - journal-first homing at T_h;
  - large commits as data + ring frame (A0's shape).

  JF matches JRP at ≤ 64 KiB.

**E4b, the model** (extend `work/e4-model/pfm.py`):
- **Objects:** header, ring slots, data-area slots written once, multi-slot RPs, FINAL. Power loss as any sector subset since the last barrier, plus process crash and syncs in flight. Thresholds scaled to the model (e.g. PB=4, T_d=2 pages, T_h=3 pages).
- **Checks:** E4's set (≥ last synced, never unwritten bytes, nonce log, LATENT).
- **Mutants that must fail:**
  1. a homing that starts at the newest RP's slot;
  2. the partial tail written home and not carried in the RP;
  3. P1 dropped once a newer unsynced RP exists;
  4. no in-flight pin;
  5. an RP trusted without its pages;
  6. an RP trusted without its predecessors' pages;
  7. frames applied across a seq gap;
  8. a retry reusing (seq, attempt) or ctr;
  9. FINAL without `verify_sh`;
  10. an OVR'd page homed before FINAL;
  11. Stream: a retry appended after a failed attempt's bytes rather than over them.

## 10. Risks and open items

1. **One request depends on the allocator.** ext4's delalloc must place a homing's fresh blocks right after the previous RP's block. B relies on the same; another filesystem, or a fragmented one, may split it. Not measured for JRP.
2. **Dead slots.** About 3% of disk at 133 KB commits, at most ~6% (one RP per call of ≥ 64 KiB, one per 128 KiB of journal). They last in run-born partitions until a merge rewrites them. Punching them would cost a discard each on this mount (CLAUDE.md), so they are left.
3. **Alignment.** Log-born files lose the stored 1 MiB alignment of a writer's piece (§5.9). Unpriced.
4. **The journal path writes payload twice** below 64 KiB, and puts a homing spike every 128 KiB of journal (§7). Thresholds are guesses; on journaled ext4 they move.
5. **Split commits.** About 6% of 133 KB commits take two requests.
6. **Ring lap 1.** It pays an unwritten-extent conversion per sync until 1020 KiB of frames have been written.
7. **Recovery reads the data area up to `i_size`.** That relies on fdatasync making a grown `i_size` durable, which is its contract. An RP beyond a stale `i_size` was never synced.
8. **The thresholds and the estimates in §0 and §7 are untested.** E5b decides them, and the crash argument is unchecked until E4b runs.
9. **Write-through stays,** at +30-65 µs at 133 KB. The `direct_recycle` lever (§5.10) is an engine change, not designed.
10. **Decision 1** (store-level integrity once the run is its own log) is unchanged. The Stream WAL keeps today's per-file cut property.
11. **Not verified:**
    - whether the run's writer starts the `sync_ahead` helper;
    - that the mixes' 13.3 KB batches always go to the WAL (CLAUDE.md: runs are keys above the store's greatest; the mix driver was not read);
    - replay's cut site db.rs:27929-27934 (cited from the design and E4, not read here).
