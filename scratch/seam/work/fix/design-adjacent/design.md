# The Log commit with its record beside the data (design-adjacent)

Tree `3df85d22`, branch `claude/inspiring-brahmagupta-d7y8n7`. Nothing under `/home/user/supdb` was touched, and no timed probe was run. The only computation was a model search (seconds to minutes) in this directory, run with no bench process alive.

Files here:
- `design.md` (this note);
- `onereq.py`, `mincycle.py`, `mincycle_idrw.py`: the model search of §2;
- `results.txt`: its outputs.

## 0. The answer to "can we test the fix first?"

Yes, and the first test is already done, as a model. It says what the fix costs before any probe is built.

**A commit whose record sits beside its data cannot be one write without paying in bytes.**
- The record of commit k has to sit where commit k+1 begins, which is at the old tail page's home.
- Commit k+1 may not overwrite that record, because it is the only durable copy of the old tail and of the open group's tags.
- So the next commit either writes in two places, or carries the page it cannot send home inside its own record.

An exhaustive search over every placement (§2) gives the cheapest steady states under the most favourable assumptions: a durable policy, one writer, a perfectly regular commit size, and only the previous record protected.

| commit completes m pages (record r pages) | one write per commit | two writes per commit |
|---|---|---|
| m=1, r=1 (4 KiB) | **2.5** pages a commit | 2.0 |
| m=2, r=1 | **4.0** | 3.0 |
| m=3, r=1 (13.3 KB) | **5.5** | 4.0 |
| m=4, r=1 | **7.0** | 5.0 |
| m=1..3, r=2 | 3.5, 5.0, 6.5 | 3.0, 4.0, 5.0 |
| m=0 (inside the tail page) | r (two slots alternate) | - |

The law is **1.5m + r against m + r**. Each one-write schedule found alternates two kinds of commit:
- a *carry* commit: the record goes ahead of the next commit's reach and carries the completed pages, and nothing is sent home;
- a *flush* commit: the carried pages and its own go home in one run, with the record at the tail.

So one request costs m/2 pages a commit, and every one of them reaches the device twice. At the four sizes:
- **100 B:** the design's ring already makes it one write (E5's A: 2.13 requests with the flush), and adjacency adds nothing.
- **4 KiB and 13.3 KB:** one write is possible for 0.5 and about 1.6 extra pages a commit. Whether one fewer request (10-40 µs here) beats 3-13 µs of extra transfer is the probe of §11.
- **133 KB:** one write costs about 16 extra pages (64 KiB, +48% of the commit's bytes, about 110-130 µs of transfer) to save one request. It loses.

What that means for the decision:
- **The run temp keeps two writes.** Its durable commits are the load's 133 KB batches (§1). The data goes home and a record goes to the ring. That is A's request count, and the model says A's count is the floor at that size.
- **The WAL should not be a paged Log file: decision 2 is (b).** A WAL is read only in order, at replay. Its natural form is a journal of sealed records, one per write call. That is one write with no carried bytes, which no paged Log commit can match at 4 KiB or more. If its name is to be sealed like every other file's, the same mechanism can move behind the door as a *Journal* class (§10). The cost is the same as (b), and the code is LogCipher's moved, not new.
- **The fix is worth one probe.** It needs three arms (§11). They decide whether the carry/flush mode is worth keeping for synced commits of a few pages, and confirm the WAL verdict on the device.

## 1. What the engine writes, checked against db.rs and E5

The run temp (Log class) is written as follows.
- **The commit.** `commit_direct` (db.rs:28690) streams the batch and calls `mark_stream` (db.rs:6870). That function writes only the whole 1 MiB pieces the batch completed: `stream_flush(false)`, db.rs:6893.
- **The sync.** A sync is due under `SyncPolicy::Always` (db.rs:28799-28807). The durable arm runs `Always` and the buffered arm runs `EveryN(u32::MAX)` (bench/src/engines.rs:7193-7197). `sync` (db.rs:6917-6936) writes the rest with `stream_flush(true)`, then calls `sync_data`.
- **The write itself.** `stream_flush` (db.rs:5707-5750) makes one `writev` per piece, never longer than a piece. It splits at a piece boundary and gives a writeback hint per whole piece (`start_writeback`, db.rs:6071).
- **What the door sees.** Under the durable policy, every commit is one or two writes starting at the previous end, which is unaligned, and then an fdatasync. Under the buffered policy it is whole aligned 1 MiB pieces, and a tail only at the flush.
- **A sync beside the writer.** The segment writer's helper (db.rs:5893-5931, 6182-6230) runs `dup.sync_data()` once `sync_ahead` (8 MiB, db.rs:7992) is unsynced, from another thread. Under the buffered policy that is a sync in flight beside the writer.
- **The batch sizes.** E5's B arm models this (`e5.c`; notes "B: the plain run's commit"). The marker is 24 bytes (db.rs:4076). The suite's ordered load commits 133 KB batches and the mixes 13.3 KB ones (E5's task text). The mixes go through the WAL, not the run.

The WAL is written as follows.
- **The commit.** `Wal::write` (db.rs:4436-4455) makes one `write_all` of `pending`, whole batches after `mark_commit`. Under a cipher it is one sealed record per call (`seal_into`).
- **The sync.** `sync_wal` (db.rs:27055-27081) syncs the loose predecessors, then `chain()`, then `sync`.
- **The chain byte.** `chain()` (db.rs:4498-4507) rewrites one header byte at offset 7 through a second handle. It is the WAL's only write at an older offset.
- **Recycling.** `recycle_wal` defaults to false (db.rs:3938), so every WAL file is fresh and grows at every commit.

The device's price list, from E5's raw outputs (raw/pooled.txt, raw/iocost.txt), with no journal on `/`:

| what | cost here | source |
|---|---|---|
| one write in allocated blocks, plus the flush | 90-115 µs | iocost `con`/`sep` k=1; Cp 100 B 108 |
| each further separate write in the same sync | about 10-40 µs | iocost `sep` k=1→2 (+14 median); E5 notes |
| each further 4 KiB page in one write | about 6-8 µs | iocost `con` k=1→33 |
| growth: a write past the allocated end, which makes the sync write the inode | 45-91 µs | Cf − Cp: 45 / 91 / 77 / 54 at 100 B / 4 KiB / 13.3 KB / 133 KB |
| growth on journaled ext4 (a journal commit and a second flush) | hundreds of µs (loop device, indicative) | raw/jext4-summary.txt: B 550 against Cp 166 at 4 KiB |

`/sys/block/vda/queue` reports `write_zeroes_max_bytes 0`, logical block 512, physical 4096, `write back`. Growth therefore cannot be bought ahead without writing zeros: `FALLOC_FL_WRITE_ZEROES` needs the device's write-zeroes, and unwritten extents need a metadata write at their conversion.

Growth costs more than a separate request here, and the plain run pays it at every commit (B: probe/meta 1.01 io a commit, raw/tracecount.txt). Keep that in mind when reading every ratio below.

## 2. Why one write is a trade of bytes

**The geometry.** Data page p has one stored home. The record of a synced commit k holds two things that exist nowhere else on the device: the old tail page's bytes, and the open group's tags.
- If commit k+1 overwrites the record's stored pages and tears (sectors land old or new, and unsynced writes land in any subset), the synced state is lost. So commit k+1 must not write them. Neither may any write until a newer sync completes.
- A single write of commit k's data is a contiguous run, and the record inside it sits at or beyond the end of the data, at the tail page's home or further on.
- Commit k+1's first new home page is that same tail page. Its single run therefore starts under the record, or has to leave a hole that only a later, separate write can fill.

The single-write way out is to place the record *ahead* of everything the next commit will write, and that means carrying the commit's pages in it.

**The search** (`mincycle.py`) makes this exact on an abstract device.
- Positions are stored data pages, relative to the tail.
- Each commit completes m pages and writes one interval (or two, for the comparison) that avoids the previous record's pages.
- Inside the interval the record is a contiguous run on pages that are not home. It carries the image of every complete page that is not home.
- Pending pages in the interval go home. Pages already home may be rewritten with identical bytes, and positions past the tail may be filled with junk.
- The search computes the minimum mean pages written per commit over every infinite schedule (Karp's minimum mean cycle on the reachable graph after pruning dead ends).

`mincycle_idrw.py` also lets a commit rewrite the protected record with its own bytes, which is torn-safe. The minima did not move (m=1: 2.5; m=2: 4.0).

The model is generous on purpose:
- it uses the durable policy;
- only one record is protected (with a buffered policy or a sync in flight there are up to three);
- m is the same every commit, so prediction is perfect;
- the record is one page plus the carried pages.

Real conditions can only cost more. Outputs are in `results.txt`. Reproduce with `python3 -I mincycle.py <m> <pmax> <r> <W> <1|2>`. The runs take seconds, except `mincycle_idrw.py` at minutes.

The minimal one-write cycle at m=1, as printed (relative positions after each commit):

```
carry:  pending {}   protected record at 0   -> write [2,3]: record of 2 pages carrying page -1, nothing home
flush:  pending {-1} protected record at 2,3 -> write [-2,0]: pages -2,-1 home, record at 0
```

The one-commit delay of the task's sketch is the carry. The delayed page cannot be sent home in the next commit's single run either, unless that run reaches back over the record written in between. So the delay becomes a carry of the whole commit every other commit, or it becomes a second write. Nothing cheaper exists in the model.

## 3. The design, named ADJ: one record format, three placements

This is the design to build if the probe says the carry/flush mode pays. It is also what the probe's arms implement.

**What it keeps from design.md §3:**
- page P = 4096; groups of 128 pages; extents of 65,536;
- headers A and B at 0 and 4 KiB;
- the prefix area 8 KiB..1 MiB;
- tags apart, with tag region `E(e) = 1 MiB + e·258 MiB` and the tag page of group g at `E(e) + g·4096`;
- data page p at `E(e) + 2 MiB + (p & 0xFFFF)·4096`, with e = p >> 16.

**One change to the prefix area:** it holds record slots, 8 slots of 2 pages at 8 KiB + 2i·4 KiB, not a ring of delta frames. Every record is self-sufficient, so the slots are only places to put one.

**One change to tag pages:** they are chained. The AAD of tag page g carries the MAC of tag page g−1, so a record names the newest tag page's MAC and nothing older (§6).

### 3.1 The record

```
clear (32 B):  magic "PFR1" u32 | session idx u8 | kind u8 (RING|ADJ) | pages u16 |
               seq u64 | attempt u24 | rsv u8 | body_len u32 | at u32 (first stored page index)
sealed body:   L u64                    logical length after this write call
               synced_home u64          home frontier at the newest completed sync (verify bound)
               F u64                    pages [0,F) are home except those in `pending`
               chain_g u32, chain_mac16 the newest tag page written, and its MAC
               open_tags[]              (tag16, ctr u32) for every home page in groups >= chain_g+1
               pending[]                (page u64, plaintext 4096): complete, not home
               tail                     plaintext [F_t*4096, L) of the tail page
               ovr[]                    (page u64, ctr u32, plaintext 4096), <= 4: finish's patches
tag16
```

The size is 32 + 16 + 40 + 20·open + 4104·pending + t + 4108·ovr bytes, rounded up to whole pages. The open tags average about 1.3 KB (at most 2,540 B) and the tail about 2 KB, so a record without carried pages is one page about 60% of the time and two pages otherwise. The 60% assumes uniform t and open count and is not measured.

### 3.2 Placement per write call

These definitions apply after the call's bytes are appended:
- T = L >> 12, the tail page, and t = L & 4095;
- C = the pages this call completes (the old tail up to T−1);
- Q = the pending set before the call, and H = Q ∪ C;
- m = |C|, and m̂ = the predicted m of the next synced call (the maximum of the last four synced calls' m);
- P = the protected stored pages: the pages of the record of the newest completed call (A_pc), of the record covered by the newest completed sync (A_sync), and of the record each sync in flight covers (§4).

Nothing in P is ever written, not even identically.

| case | when | the write(s) | requests (data side) |
|---|---|---|---|
| **0 inside a page** | m = 0 (100 B commits; the WAL chain byte if the WAL were Log) | record → the first free ring slot | 1, no growth (slots allocated after first use) |
| **1 whole pieces** | t = 0 and the call is k·256 pages at a 1 MiB plaintext boundary (the buffered run's `mark_stream` calls) | homes of H, one aligned 1 MiB write per piece; record → ring slot | 1 per piece + 1; unsynced, coalesced at writeback |
| **2 flush (S1)** | 1 ≤ m ≤ M, and the interval J = [home(min H), home(T) + r − 1] lies in one extent with J ∩ P = ∅ | one write over J: homes of H (sealed at fresh ctrs), then the record at the tail's home; Q := ∅ | **1** |
| **3 carry (S2)** | 1 ≤ m ≤ M, and case 2 fails because a protected record sits on a page of H (always so right after a flush) | one write: the record carrying H at [home(T) + m̂ + r, ...), nothing home; Q := H | **1** |
| **4 two writes (TWO)** | m > M; or case 2 fails for a reason case 3 cannot fix (m > m̂, an extent boundary, \|H\| > q_max) | homes of every page of H whose home ∉ P (one write per contiguous run); record → ring slot; blocked pages stay in Q | **2** (+1 per extra run) |
| **5 group completed** | any case where a group's 128th page goes home | that group's tag page, sealed and chained, at the tag region | +1 (tags apart; §8 for inline) |

**Why the carry's offset is m̂ + r.** The next flush writes up to `home(T_next) + r − 1`, where T_next = T + m_next. That stays below the carried record when m_next ≤ m̂. When m_next > m̂ the flush would reach the carried record, so the call falls to case 4.

**The cases the task asks about, in these terms:**
- **A commit inside one page** is case 0.
- **A commit that completes the tail page and leaves a new partial tail** is case 2 or 3 by the alternation. Under case 4 it is A's shape: the old tail's home is free because records under case 4 live in the ring, so nothing is blocked.
- **A commit that ends exactly on a page boundary** (t = 0) is the same, with an empty tail in the record.
- **A commit that completes many pages** is case 4 once m > M.
- **The previous record sitting where the commit needs it** is exactly case 3. Under case 4 it cannot arise, since case 4's records never sit on a home.

**Choosing M.** Case 2 or 3 saves one request a commit (10-40 µs) and costs m/2 carried pages (about 3.5m µs, and m/2 pages of device bytes twice). Break-even is m between about 3 and 11 on this device. M is set from the probe, and the probe's sizes are 1, 3 and 32 pages. q_max = 2M + 2.

### 3.3 Inside a page and at the slots

The ring slots are 8 slots of 2 pages. A record takes the first free run of slots, so a record of up to 6 pages (with overrides) takes 3 slots. At most 1 + 1 + (syncs in flight, at most 2: the helper's and the close's presync, db.rs:6218, 29025) records are protected, so a free run always exists.

The slots are written over and over in allocated blocks. Case 0 and the record write of cases 1 and 4 therefore never grow the file. That is what makes E5's A one write and no growth at 100 B.

## 4. The sync rule

- The layer issues no sync of its own, as design.md §4. The engine's fdatasync is the only barrier, and the header rides the first one. A resumed Handle syncs once before its first header write, per E4 F1.
- **The sequencer.** Each sync is registered on the Handle at its issue, together with the record of the newest completed call at that moment. That record joins P until the sync completes.
- **On completion,** A_sync becomes that record if it is newer than the current A_sync. The previous A_sync, and every record older than the newest completed call and not named by a sync in flight, leaves P.
- **Who syncs.** The writer's `sync_data`, the helper's `dup.sync_data` and the close's presync all reach the same Handle through the registry (design.md §3, Handle registry), so the sequencer sees all three.

## 5. The recovery rule (Log file, unclean open)

1. **Header.** Take the newest valid header and its session table (design.md §4, with E4's F1 and F7 fixes).
2. **Candidates.** Read the stored file once, as `recover_direct` already reads its whole file (db.rs:6948-6952). Candidate records are every ring slot and every data-area page whose clear prefix has the magic and a plausible header. Verify each candidate's AEAD under the session its idx names, with AAD file_id ‖ clear part ‖ `at`. A data page that merely looks like a record fails here.
3. **Order.** Sort the verified records by (session, seq, attempt), newest first.
4. **Choose.** For each record R, check:
   - the tag chain from `chain_g` back verifies;
   - every data page in [round_down(R.synced_home, 128), R.F) that is not in `pending` verifies, against R's `open_tags` or its group's tag page.

   Choose the first R for which both hold. The rounding is not load-bearing with sealed tag pages (E4's finding) and is kept because it is cheap.
5. **The plaintext.** The plaintext is the home pages below F, R's pending images, R's tail, and R's overrides applied, up to R.L. The run temp is then rewritten by `recover_direct` as a new piece. The WAL is not a Log file in the recommendation, so there is no resume path to add.
6. **Bounds.** The candidates are finite, and nothing loops.

## 6. The nonce rule

| object | key | nonce (96 bit) | AAD |
|---|---|---|---|
| data page p, sealed once when it goes home | session S_i | `0x10 ‖ p (u56) ‖ ctr u32` | file_id ‖ p ‖ ctr ‖ valid_len |
| tag page g | S_i | `0x12 ‖ g (u56) ‖ ctr u32` | file_id ‖ g ‖ ctr ‖ MAC(tag page g−1) |
| record (ring or adjacent), with its pending images, tail and overrides inside its one body | S_i | `0x11 ‖ seq u64 ‖ attempt u24` | file_id ‖ clear part ‖ at |
| WAL journal record (§10, if moved behind the door) | S_i of the journal file | `0x13 ‖ seq u64 ‖ attempt u24` | file_id ‖ clear part |

- **Counters.** ctr advances at every page or tag-page sealing attempt. seq advances per record, and attempt per resend of one seq (E4 F3's fix). The first byte separates the domains.
- **Carried pages.** A page carried in several records is plaintext inside each record's AEAD. It is sealed as a page exactly once, when it goes home, at a fresh ctr.
- **Retries.** A failed write call is resealed: pages at new ctrs, the record at attempt+1. Its stored positions are garbage, since the call never completed, so it was never A_pc.
- **Reopen.** A reopen begins a new session with a fresh random key.
- **Identical rewrites.** The protocol never makes one, so no sealing is ever repeated.

Under one key, therefore, no (p, ctr), (g, ctr) or (seq, attempt) recurs across any rewrite, retry, crash or reopen. The uniqueness hook and the nonce-format test of design.md §3 hold this rule.

## 7. The crash argument

The invariants:
- **I1.** No write touches a stored page in P.
- **I2.** While the file is Log, a page written home is never written again. Overrides never go home before `finish` (E4 F5: deferred, with FINAL carrying the overrides). A tag page is written once, except E4 F2's re-home behind `sh_rehome`.
- **I3.** Every record is self-sufficient. From its own bytes, the home pages below its F that are not pending, and the tag chain it names, it yields exactly the plaintext after its write call.
- **I4.** Every page and tag page a record names was written in that record's call or before it.

**Power loss.** Let S be the newest completed sync and A_sync its record.
- A_sync's call completed before S was issued. By I4, so did every page and tag page A_sync names. At S's completion all of them are durable.
- From S's issue onward, A_sync's pages are in P. They stay there until a newer sync completes (I1), at which point a newer record is A_sync.
- By I2, the pages A_sync names are never written again.

So any tear or any subset of later writes leaves A_sync and its dependencies intact. A_sync verifies, and recovery reaches it at worst: the result is **at or after the last synced state**.

**Never bytes that were never written.**
- Everything recovery returns is authenticated under a session key, at a nonce and AAD that bind position, version and file.
- A record chosen in step 4 is the exact state after its own write call. Its pages are the images I2 says are unique at their homes, and the version in its tags pins each one.
- The result is therefore the state after some write call: a **prefix of the writes**, never a mixture.
- A newer record whose homes did not all land fails step 4 and is skipped.

**Process crash.** The page cache keeps every completed call.
- A_pc's pages are in P for the whole of the next call (I1), so a call cut short by the crash leaves A_pc and its dependencies whole. Recovery returns at least the last completed call.
- The in-flight call's record wins only if it landed whole and everything it names verifies.

**Torn sectors.** Every overwritten page is outside P. It is a stale record, a junk position past the tail, or a home written for a record that may not land, so a tear costs only states newer than A_sync and A_pc.

**Syncs in flight.** Each one pins its record until it completes (§4). Whichever completes, its record is intact. E4's no-pin mutant shows what goes wrong without this: 206,729 LOSS.

**What this argument does not cover, and E4 has not modelled:**
- the placement of case 3 under a buffered policy with three protected records;
- the scan of the data area for candidates;
- the mix of ring and adjacent records.

A model family for these is owed before anything is built (§13).

## 8. The read map, the 1 MiB pieces, and inline tag pages

**Finding plaintext page p while the file is Log:**
- A miss first compares p with the Handle's `lo_unhomed` (min of Q and T, one atomic load).
- Below it, p is home at `E(e) + 2 MiB + ((p & 0xFFFF) << 12)`. Its tag comes from the Handle's open tags or from the tag tier (tag page `(p >> 7) & 511` at `E(e) + (g << 12)`). These are shifts, masks and adds, as design.md §3.
- At or above it, p is in the Handle's memory (the pending images or the tail), never on disk.
- Q is non-empty only between a carry and the next flush, which is one commit.
- After `finish` the Handle has no pending pages, and the map is the fixed one alone.

**The writer's pieces.** Case 1 writes every whole piece as one write at a 1 MiB-aligned stored offset. Its record goes to the ring, never beside the piece, so no piece is ever split by a protected record. The folio rule in CLAUDE.md therefore holds for the buffered run, as it does in design.md. Cases 2-4 serve synced calls, which start mid-page in the plain engine too (`stream_flush(true)` writes from `written`), so they cost the folio rule nothing new.

**Inline tag pages** (group g's tag page right after its 128 data pages).

The layout:
- 129 stored pages a group, and an extent of 512·129 pages = 258 MiB as now;
- with q = p & 0xFFFF, data page p at `E(e) + ((q + (q >> 7)) << 12)`;
- tag page g at `E(e) + (((g << 7) + g + 128) << 12)`.

The read map gains one shift and one add.

What inline tags buy:
- the tag page then goes in the same write as the data page that completes the group, so case 5's extra request disappears;
- at the four sizes that is 0.008, 0.025 and 0.25 requests a commit at 4 KiB, 13.3 KB and 133 KB (m/128), or about 5-10 µs of a 133 KB commit;
- for cases 2 and 3 the saving is 0.06 requests a commit or less at M ≤ 8.

What inline tags cost:
- A plaintext 1 MiB piece i becomes 258 stored pages at stored page 258i. That is 1 MiB-aligned only when i ≡ 0 mod 128, and otherwise aligned to 2^(1+v2(i)) pages: half the pieces to 8 KiB, a quarter to 16 KiB, and so on.
- Every piece the writer writes whole is then a folio ladder, not one 1 MiB folio. That is the shape CLAUDE.md priced as reads trailing by a few percent at every rung, there for 9 pieces of 40, here for 127 of 128.
- What a ladder costs a read through the pager's copy-on-miss is not measured.

**Recommendation: tags apart.** The request inline tags save is at most about 1-2% of the one commit size where it is frequent. Their cost lands on every read.

## 9. Requests and bytes per commit at the four sizes

These assume the durable policy, one writer, tags apart, and no journal on ext4. "Data" means the writes of the commit. Every arm adds the flush (1). Growth is the inode write of a write past the allocated end. Values for A and B are E5's measurements (raw/pooled.txt, raw/tracecount.txt). The ADJ values are predictions from §2 and §3.

| commit | A (design.md ring) | ADJ, case 2/3 (one write) | ADJ, case 4 or `T` (two writes, self-sufficient ring records) | B (plain run) | J (WAL as sealed journal, = (b)) |
|---|---|---|---|---|---|
| 100 B | 1 data, growth 1/40, total 2.13 measured | case 0: 1 data, growth 1/40, about 2.1 | same as case 0 | 1 data + growth, total 3.00 | 1 + growth, about 3 (fresh); 2 with `recycle_wal` |
| 4 KiB (m=1) | 2 data, growth, total 4.01 | 1 data; growth 0.5-1 (§13, risk 2); about 2.5-3.0 | 2 data, growth, about 4.0 | 3.00 | 3 / 2 |
| 13.3 KB (m≈3.25) | 2 data + 0.03 tag, growth, total 4.12 | 1 + 0.03; growth 0.5-1; about 2.6-3.1 (if M ≥ 4) | about 4.1 | 3.01 | 3 / 2.04 |
| 133 KB (m≈32.5) | 2 data + 0.25 tag + 0.13 piece split, growth, total 4.91 | not used (m > M); forced, about 2.9-3.4 requests **with 16 extra pages** | about 4.9 | 3.13 | 3.2 / 2.1 |

Bytes that reach the device more than once, per commit:

| commit | A | ADJ case 2/3 | ADJ case 4 / `T` | B | J |
|---|---|---|---|---|---|
| 100 B | the 100 B appended (TAIL frame) and the ring page around it: 1 ring page a commit to the device | the tail image (avg 2 KB) and the open tags (avg 1.3 KB) in a 1-2 page slot a commit | same | the page holding the end, rewritten with its old bytes: 1 page | the journal's last page: 1 page |
| 4 KiB | about 2 KB of new tail (E5: `tail_bytes/c=2038`), then again at home; a CKPT per 256 KiB of frames | tail image 2 KB + **0.5 page carried** (2 KiB); plus 1-2 record pages | tail image 2 KB in the record | the partial last page, once more | the partial last page |
| 13.3 KB | about 2 KB | 2 KB + **about 1.6 pages carried** (6.5 KiB) | 2 KB | as above | as above |
| 133 KB | about 2 KB; tag pages 0.78% once | (forced) 2 KB + **about 16 pages** (65 KiB, +48%) | 2 KB | as above | as above |

Predicted time against B, from the price list of §1 (ranges, not measurements):
- **ADJ case 2/3 at 4 KiB:** 0.73-1.11x of B without write-through. That is A0 (228 µs), less one request (10-40), less half a growth if it holds (0-46), plus half a page (3-4) and the seal (about 1).
- **ADJ case 2/3 at 13.3 KB:** 0.97-1.31x of B.
- **At 133 KB:** worse than A0 by 1.09-1.26x.
- **J against a paged-Log WAL at equal `recycle_wal`:** no worse at every size, by construction. At 100 B with fresh files, A's ring (104 µs) beats J (about 155) because the ring never grows. `recycle_wal` gives J the same (Cp 108).

## 10. Decision 2

**(b): keep the WAL sealed record by record.** Whether its code stays in `vault.rs` (LogCipher) or moves behind the door is a separate choice, listed below. The reason is structural, not a figure:
- The WAL's only reader is replay, which reads in order (`Wal::replay`). Its only writes at an older offset are the chain byte (db.rs:4498-4507) and the cut at open.
- A journal of sealed records is therefore one write per commit, with no page carried and no record that has to avoid a home.
- A paged Log WAL costs at least a second write or m/2 carried pages a commit at 4 KiB and above (§2). Its one advantage, no growth at tiny commits, is the plain engine's own `recycle_wal`.

If the owner wants the WAL's name sealed like every other file's (design.md decision 4), the move is a **Journal class**:
- one record per write call, `len u32 ‖ seq ‖ attempt ‖ AEAD body`, appended;
- replay returns the records that verify, in order, and stops at the first that does not. A torn tail is the end, as today.
- The chain byte is either a clear header field, as today (`WAL_HEAD_SEALED` plus the kind byte, db.rs:4131-4136), or a patch record that replay applies at its offset.
- `set_len` cuts at a record boundary that replay has recorded.
- A recycled file takes a new file_id, so the last life's records fail their tag, as `recycles` does today.

This is LogCipher's mechanism with a door-shaped interface. It has (b)'s device cost and drops the plain `wal-` name. E4's Log-class findings (F1 and F3) apply to it unchanged, and F2, F4 and F5 do not arise.

## 11. The test

The test extends `e5-commit/e5.c` with three arms, and nothing else changes: the window, the counters, `run.sh`, `tracecount.py`, and both filesystems.

| arm | what | answers |
|---|---|---|
| `O` | ADJ: case 0 into ring slots, cases 2/3 alternating for 1 ≤ m ≤ M (M = 8), case 4 above; self-sufficient records; m̂ = max of the last 4 | does one write with carried pages beat A and B at 4 KiB and 13.3 KB? Does the flush commit grow the file? (`probe/meta` io per commit by kind) |
| `T` | ADJ with case 4 for every m ≥ 1: homes + a self-sufficient record in a ring slot | prices self-sufficient records against A's delta frames at the same request count |
| `J`, `Jp` | WAL as a sealed journal: per commit one `pwrite` of `len‖nonce‖ct‖tag` appended (E5's D without the memfd half), fresh and over a prefilled file | decision 2 against A/Ap and against Cf/Cp |

- **Sizes:** 100, 4096, 13300 and 133000. Run 10 reps × 3 sittings, shapes rotated, plus `jext4` at 300 × 3.
- **Read:** the median and p90 per commit; ios a commit by kind from `block_rq_issue` (probe/data, probe/meta, flush); sectors a commit; and, for `O`, the carried bytes, the carry and flush counts, and the case-4 fallbacks.
- **Gate:** before each sitting, run the `pgrep -f 'bench ab|pair.sh'` loop, and keep the machine to itself.

What each outcome decides:
- **O against A0** at 4 KiB and 13.3 KB, per sitting.
  - Below 0.9 in every sitting at a size: cases 2/3 are kept for m ≤ that size's m, and M is set there.
  - Otherwise `O` is dropped and the Log class keeps two writes (A, or `T` if `T` ≤ A).
  - `O` at 133 KB is expected to lose. If it does not, the price list of §1 is wrong, and that is read before anything else.
- **T against A at every size.** If `T` is within noise or better, self-sufficient records replace delta frames and CKPTs. That removes the frame chain, CKPT spans and span pins from recovery. Otherwise A's frames stay.
- **J against A** (fresh) and **Jp against Ap** at 4 KiB, 13.3 KB and 133 KB.
  - Below 1 in every sitting: decision 2 is (b) on the measurement as well as the structure.
  - At 100 B, J > A on fresh files is expected and is answered by `recycle_wal`.
- **On jext4,** if the ranking flips (E5 found A/B at 0.94-0.96 there), the decision is the one taken on the default filesystem the store ships for. That filesystem is not this box's `/`. This is said in the PR.

## 12. Considered and rejected

- **The tail rewritten in place under a keystream that keeps the old prefix's ciphertext** (GCM and ChaCha are CTR underneath). This would make every rewrite of the tail page identical in its old sectors, so the tail would never need a second copy. It needs the hook to be prefix-consistent, which an SIV mode is not. The draft seals under one nonce would have to be discarded tags. And a crash that loses unsynced tail bytes followed by different bytes reuses keystream (a two-time pad). The `Aead` hook (crypt.rs:141-151: encrypt and decrypt in place under a nonce, nothing about prefixes) promises none of this.
- **Two copies of each record, so the next commit may destroy one.** Both copies must lie in the commit's one run, and the next run starts at the old tail. Keeping one copy out of its reach is case 3's offset again, paid twice.
- **Inline tag pages** (§8). They save at most 0.25 requests a commit, and only at 133 KB. They break the 1 MiB alignment of 127 pieces in 128.
- **Growth bought ahead with `FALLOC_FL_WRITE_ZEROES`.** vda has no write-zeroes (§1). Unwritten extents need a metadata write at conversion. Zeros written ahead put every byte on the device twice, which E5's Ap/Cp price outside their window.
- **The commit record in an inode xattr,** which rides the growth's inode write. It is too small (about 100 B in-inode), specific to the filesystem, and its fdatasync semantics are not verified.
- **A run temp written as a journal and laid out in pages at `finish`.** That is today's envelope at close (43-55 ms of a drain, CLAUDE.md), which the design exists to remove.

## 13. Risks, and what was not verified

1. **The model is page-granular and favourable,** with one protected record and a constant m. Its bound is a lower bound. The real cost of cases 2/3 under a buffered policy or a sync in flight is not modelled.
2. **"The flush commit does not grow the file"** rests on the carry's record having allocated the flush's range ahead (§2's m=1 trace). Filling holes inside i_size may still make the sync write the inode. This is not verified; the `O` arm's `probe/meta` reads it.
3. **The flush is one request only if ext4 put the carried record's blocks and the earlier homes next to each other on disk** (delalloc goal blocks). This is not verified; `block_rq_issue` reads it.
4. **Device bytes rise.** Cases 2/3 send m/2 pages twice: +12% at 4 KiB and +37% at 13.3 KB of the commit's bytes. The suite reports device bytes per byte stored (2.5 under today's cipher, 1.3 plain).
5. **Self-sufficient records grow with the open group.** They are 1-2 pages against A's sub-page delta frames, and up to 6 pages while finish's overrides stand. At 100 B that is about +25% device bytes against A (one page against about 1.4 average; assumed uniform, not measured).
6. **Recovery reads the whole stored file** to find candidates in the data area. `recover_direct` reads its whole file already. A WAL (not proposed) would add nothing, since replay reads all of it.
7. **The new placement needs its own crash-model family before building:** placement with up to three protected records, the data-area scan, ring and adjacent records mixed, misprediction, and the extent boundary. E4's families cover the ring, not this.
8. **E5's prices are this guest's:** a Firecracker virtio disk with a write-back cache, ext4 without a journal. The jext4 figures came from a loop device and are indicative. The cost of a separate request varied from 10 to about 40 µs between contexts (E5 against iocost).
9. **Under ADJ, a run's stored file keeps stale records past L** until `finish` (a few pages). `finish` and the punch that design.md left undesigned must cover them.
10. **The WAL chain byte as a patch record** (§10) is a choice, not checked against `Wal::replay`'s header parse (db.rs:4538 on). Keeping the kind byte in a clear header, as today, avoids the question.
