# E8 ("once"): memory held once under a cipher, out of core

Tree 3df85d22, clean. This file is the plan only: nothing was edited, built or timed while writing it. Every file:line below was read at 3df85d22. E7 means `work/e7-ooc/notes.md` and its `pairs/`, `samples-*`.

The question is this. Today a store under `Encryption::Cipher` that does not fit its 640 MiB bound holds memory twice: the plaintext lives in the pager's memory files (shmem, about 337 MiB, which cannot be reclaimed without swap) and the ciphertext lives in the kernel's page cache (about 240 MiB). Suppose the plaintext cache gets nearly all of the bound and the ciphertext is not kept. How close does the point-read rate then come to the clear store's, which reads at 0.28-0.33x today?

The answer below is a prediction that **holding the memory once does not close most of the gap**. The reason is that the plaintext cache uses its memory less well than the page cache does: a 4080-byte chunk takes two pages, and memory is given back by the megabyte. The runs test that prediction. The kill line is in section 8.

---

## 1. What the code does today

### The miss path

- **`Paged::ensure`, pager.rs:1312-1339.** This is the one way into a paged file.
  - For an ask inside one chunk, it does one SeqCst load of the chunk's state. If the chunk is not `HOT`, it calls `slow(c)` (1329-1332).
  - For an ask across chunks, it walks `a..=b` one chunk at a time (1334-1338). **This is the one place where a reader's range is known in chunks.**
- **`Paged::slow`, pager.rs:1027-1111, the `EMPTY` arm (1075-1106).** It moves the chunk EMPTY→BUSY, takes the unit back from an evictor's mark (`rescue`, 1132-1141) or waits for it (`gone`, 1144-1149), then calls `fill(c)`. After the fill it sets the chunk `COLD`, bumps `fills`, and adds `chunk_len` (4080 bytes) to `held` (`Pager::add`, 1097 and 455-463).
- **`Paged::fill`, pager.rs:966-1023. A miss reads its ciphertext from the envelope's mapping.**
  - It slices `back.map[stored_off(c) .. +n+TAG]` (977-986).
  - It `copy_from_slice`s that into the memfd mapping at `c*4080` (1003-1004). That write is the fresh shmem page fault E7 priced at 1.88 µs a read.
  - It decrypts in place (1006-1008).
- **The envelope mapping.** It is a `memmap2::Mmap` opened in `Back::open` (pager.rs:732-754) and advised `MADV_RANDOM` there (744-746). So a miss whose ciphertext is not cached is a 4 KiB major fault on that mapping, and the page stays in the page cache, charged to the group.
- **Where the stored chunks sit.** `stored_off(c) = DATA_START + c<<12` (crypt.rs:324-326), with `DATA_START = 4096` (crypt.rs:125). So every stored chunk is exactly one 4096-aligned page of the file, and chunks `a..=b` are one contiguous, page-aligned range. That is what makes an `O_DIRECT` read of a chunk, or of a run of chunks, simple.

### The evictor

- **The unit.** A unit is 256 chunks: 255 pages, 1 MiB (pager.rs:119-125). Its state is LIVE, EVICTING or FREEING (102-104).
- **The loop.** `Pager::run` (571-599) wakes when nudged (an `add` past the limit, 460-462) or every 20 ms. It runs `pass` while `over() > 0` (582).
- **A pass** (605-663):
  - It asks for `over + max(budget/16, 1 unit)` (612-615).
  - It marks units (`mark`, 669-706).
  - It bumps the reader table's epoch and waits for every pinned slot and every `Hold` to pass it (624-647).
  - It frees each marked unit that nobody rescued (`free`, 1210-1262). Freeing is `madvise(MADV_REMOVE)` over the unit's pages (`give_back`, 1585-1595), which punches the memfd.
- **Recency is a one-bit clock, kept per chunk and acted on per unit.**
  - A read sets a chunk `HOT` (`slow`'s COLD arm, 1033-1043; `ensure`'s fast path sees `HOT`).
  - The hand (`Pager::hand`, 680-704) walks units file by file, at most twice round. A unit with any `HOT` chunk has its chunks reset to `COLD` and is passed over (`Paged::mark`, 1154-1170). A unit with none is marked whole.
  - There is no per-chunk eviction and no LRU order. E7 counted 1026 passes per 5M reads and about 153 units given back a pass. Between two passes there are about 4,900 uniform reads (the read pass is uniform: run.rs:2127, 2361) over roughly 700 units of records. So nearly every unit has a `HOT` chunk at each look, and the second sweep decides. In effect this is FIFO by unit, which costs nothing extra under uniform reads, where recency carries no information.
  - The hash-slot units are read about eight times as often, so they stay. This is inferred from the code, not measured.

### `held` against resident shmem

- **What `held` counts.** It counts present chunks times their length, 4080 bytes each (1097, 1253-1259, `present_bytes` 1272-1280).
- **What shmem counts.** It counts memfd pages touched. Chunk `c` starts at byte `c*4080`, which is 16·c short of a page boundary. So every chunk not divisible by 256 straddles two pages.
  - An isolated chunk is therefore 8 KiB of shmem against 4080 bytes of `held`, a ratio of 2.0.
  - A whole unit is 255 pages for 256 chunks, a ratio of about 1.0.
- E7's 1.2-1.3x (shmem median 334-340 MiB, max 349-354, against `held_at_end` 268-291) sits between those two bounds. That is consistent with partly filled units. The arithmetic is from the code; the cause is **not verified** by a measurement. The runs below give a second and third reading at higher fill density (larger X), where the ratio should fall toward 1.0 if the straddle is the cause.
- `map_shared` sets `MADV_NOHUGEPAGE` (pager.rs:1546-1553), so huge pages are not a cause.

### The lines

- `hard = limit + max(limit/16, 4 units)` and `swamp = limit + max(limit/2, 6 units)` (pager.rs:320-321). Both are fixed at `Pager::new` from the first limit.
- `crowded()` is `could_give > hard` (497-499). `swamped()` is `could_give > swamp` (523-525). `could_give` is held less unbacked, meaning files with no envelope yet (509-512).
- Past `hard`, the store's own work waits at its rests (`breathe`, 551-567). Past `swamp`, a caller's read waits (`breathe_swamped`, 529-539).
- In a read-only pass nothing waits until `held` is 1.5x the limit. **So for a large X, shmem can reach the group limit long before any reader is slowed.** That bears on the OOM risk in section 4.

---

## 2. How the budget reaches the pager, and how an arm is made

### The budget

- `StoreCipher` has `cache: Option<u64>` (vault.rs:191-196), set by `cache_bytes` (vault.rs:233-236).
- `Vault::new` calls `Pager::new(cipher.cache.unwrap_or_else(default_cache))` (vault.rs:776).
- `default_cache()` (vault.rs:637-646) is `read_budget().budget / 2`, with a floor of 64 MiB.
- `read_budget` (zfile.rs:5210-5266) is the v1 group's `limit − (rss + rss_huge) − margin`. `margin` is `max(limit/20, 64 MiB)` (zselect.rs:2232-2246). The v1 `rss` excludes shmem.
- It is read once, when the store opens. In a bounded `ab` run each pass is its own process, so the read pass reads it at the store's reopen, with the anon of a freshly started process.
- `Pager::limit` is an `AtomicU64` (pager.rs:323), read on every `add` and `pass` through `budget()` (379-381). **Changing it at run time is one store**; only `hard` and `swamp` are fixed fields.

### The bench arms

- `Policy` has `cipher: bool` and `cipher_cache: u64` (engines.rs:1253-1259; default 0 at 1971).
- `create_cipher64` is `Policy { cipher: true, cipher_cache: 64 << 20, .. }` (engines.rs:2207-2216).
- `with_policy` builds `Encryption::Cipher(StoreCipher::new(..).cache_bytes(..))` (engines.rs:7222-7230).
- The name comes from `name()` (engines.rs:7510-7519).
- The arm is opened through the match at engines.rs:10104-10105, and its guarantee comes from the list at engines.rs:9757-9758 (Durable).
- The cipher counts are exported to the `ab` table at engines.rs:8540-8549 (`cipher_*` rows; E7's `.out` shows them as `count cipher_*`).

A scratch arm needs five things:

1. a `create_*` function;
2. a line in the open match;
3. a line in the guarantee list;
4. a branch in `name()`;
5. new `Policy` fields, carried into `Supdb` as `cipher_cache` is (engines.rs:7240).

---

## 3. The mechanisms, as arms

The mode goes through `StoreCipher`, not through the environment. This file warns that an environment variable set for a pairing is set for both arms. Here the comparator has no cipher, but the baseline `supdb-cipher` would pick the mode up too.

| arm | mechanism | budget X (plaintext) |
|---|---|---|
| `supdb-cipherb384` | **B**: `cache_bytes(X)` and nothing else. The mapping is kept, and the ciphertext is left to the kernel's reclaim. | 384 MiB |
| `supdb-cipherb432` | B | 432 MiB |
| `supdb-cipherd320` | **D**: every fill is an `O_DIRECT` pread into an aligned buffer. The envelope mapping is not touched on the read path. | 320 MiB |
| `supdb-cipherd384` | D | 384 MiB |
| `supdb-cipherd432` | D | 432 MiB |
| `supdb-cipherddyn` | D, with the budget recomputed (section 4) | 0.75 × `read_budget().budget`, every 100 ms |
| `supdb-cipherdndyn` | **DN**, built but run only if D fails the in-core check: `preadv2(RWF_NOWAIT)` buffered first, `O_DIRECT` on `EAGAIN` | as ddyn |

B needs no engine change at all; it is today's code with `cipher_cache` set. That is also why it is the control: if B reads as well as D, then not caching the ciphertext needs no mechanism beyond the budget, because the memcg's reclaim takes the ciphertext anyway once shmem crowds it out.

### The scratch patch: smallest form

**src/db/pager.rs**

- Add `once: u8` to `Pager`: 0 is today, 1 is D, 2 is DN. Add `dyn_pct: u8` (0 means a fixed budget). Add a `Pager::new_with(limit, once, dyn_pct)`, and keep `new` calling it with zeros.
- Derive `hard` and `swamp` from the current limit inside `crowded`/`swamped` instead of storing them. That is two lines, so that a recomputed limit moves them too. In the fixed-budget arms the values are as today.
- Add to `Back` a `direct: OnceLock<File>`, opened lazily at the first D fill as `OpenOptions::read(true).custom_flags(libc::O_DIRECT).open(&self.path)`. `Back::open` keeps its signature.
- Change `fill(c)` to `fill_from(c, stored: &[u8])`. The existing body is unchanged after the slice is obtained.
- **In `slow`'s EMPTY arm, when `once != 0`:**
  - Extend the run: after this chunk goes EMPTY→BUSY, try EMPTY→BUSY on `c+1 ..= last`, stopping at the first chunk that will not move. Here `last` is the least of the unit's last chunk, `c + K − 1`, and the end of a hinted run.
  - Rescue the unit once; all the run's chunks are in one unit.
  - Do one `pread` of `(last−c+1) << 12` bytes at `stored_off(c)` into a thread-local 4096-aligned buffer of 128 KiB.
  - Then `fill_from` each chunk, set it COLD, bump `fills` and `add` its length.
  - On a short read or a failed tag: put every BUSY chunk of the run back to EMPTY and poison, as the single fill does.
- **The run's length, `K`:**
  - **1** for a point read.
  - **The asked range** when `ensure`'s multi-chunk branch (1334-1338) set a thread-local hint `(self as *const _, a, b)` before its walk.
  - **The range a `willneed` asked for.** Under D, `Paged::willneed` (1476-1500) must not `madvise(WILLNEED)` the mapping, because that would fill the page cache with ciphertext nobody reads. It records the range as the thread's hint and returns 0. This covers a scan's planned prefetch (blob.rs:2271-2283).
  - **32 chunks (128 KiB, the device's `read_ahead_kb`)** when the thread holds a `pager::Hold`. That is the merge and the open (pager.rs:129-197); it needs a thread-local count of holds bumped in `Hold::pin` and drop. **Whether every merge read is under a `Hold` is not verified**; the `run_chunks` counter below says whether runs happened.
- **New counts** in `Counts`/`PagerCounts` (227-241, 246-): `direct_reads`, `direct_bytes`, `direct_us` (the wall time in pread), `run_chunks` (chunks filled by a pread of more than one chunk), `nowait_hits` (DN), and `limit_lo`/`limit_hi` (the dynamic budget's range).
- **`SUPDB_PAGER_TRACE=1`.** The evictor prints one line a second on stderr: `pagertrace <CLOCK_MONOTONIC> held <B> limit <B> fills <n> evicted <n> direct <n>`. This joins `held` to the sampler's shmem by time, which is what reads the shmem/`held` ratio during a pass and not only at the end.
- **The dynamic budget.** At the top of `run`'s loop (571-599), rate-limited to 100 ms: if `dyn_pct > 0`, call `zfile::read_budget()` and store `limit = budget × dyn_pct / 100` (floor 64 MiB). That is about 15 lines. `read_budget` reads four small `/proc` and cgroup files, so ten times a second costs well under 1% of a core.
- **`SUPDB_ONCE_FORCE=d|dn`**, read in `Pager::new`. It is for the correctness tests only, so that the existing pager tests and tests/cipher.rs run through D. No timing run sets it, and the scrub below unsets every `SUPDB_*` anyway.

**src/db/vault.rs**

- `StoreCipher` gains `once: u8` and `dyn_pct: u8`, with `pub fn scratch_once(self, u8)` and `pub fn scratch_dynamic(self, u8)`.
- `Vault::new` (776) passes them to `Pager::new_with`.

**bench/src/engines.rs**

- `Policy` gains `cipher_once: u8` and `cipher_dyn: u8`, carried into `Supdb`.
- Add seven `create_*` functions, the seven names in `name()` (each arm's own string, branching on `cipher_once`/`cipher_cache`/`cipher_dyn` before the `cipher_cache != 0` branch), seven lines in the open match, and seven in the guarantee list.
- Export the new counts beside 8540-8549.

**Size and what it costs the shipped arms.** The patch is about 200 lines. It adds one load and branch to the `slow` path of `supdb-cipher`: the `once` test, taken only on a miss. It also adds the two-line `hard`/`swamp` derivation. It changes nothing for a plain store.

### Is F worth building?

**No, not as a first arm.** F keeps the mapping and drops each page behind its fill (`MADV_DONTNEED` on the mapping's range, then `POSIX_FADV_DONTNEED`). Three things in the code and the notes argue against it.

1. **The drop often does nothing.** `posix_fadvise(DONTNEED)` drops only folios wholly inside its range. The envelopes are written a megabyte at a time, so the page cache may hold them in folios of up to 1 MiB (pager.rs:1439-1443; CLAUDE.md on folios). A 4 KiB drop behind a fill then drops nothing while the load's folios stand.
2. **It costs more on every miss.** It adds two syscalls a miss, and the `madvise` on a shared mapping sends a TLB-shootdown IPI. E7 put 37% of the evictor thread in those IPIs.
3. **It keeps a cost D removes.** F keeps the envelope mapping's fault (E7: 1.10 µs a read), which D removes outright.

D removes strictly more than F does. F is worth building only if `O_DIRECT` itself turns out to be the cost: if D's pread time per I/O exceeds a buffered major fault's 32-38 µs (E7) by more than a few µs.

---

## 4. The budget

### What must fit

| term | size | source |
|---|---|---|
| group limit | 640 MiB | |
| read-pass RssAnon | 38-47 MiB | E7 |
| shuffled-load RssAnon | 154-183 MiB | E7 |
| shmem | up to about 1.3 × X | E7 (max 354 MiB against `held` of 268-291 MiB), and `held` overshoots X by up to `hard`, X/16 |
| floor for everything else | about 30-60 MiB | not measured: the bench binary's text (the binary is 370 MB, of which only the text in use counts), page tables, slab, the envelopes' headers and the log |

In a read pass under D, nothing else is reclaimable but that floor, so: 1.3·X + 47 + 30..60 ≤ 640, which gives **X ≤ 410-430 MiB**.

### The points

- **320 MiB.** Safe: shmem about 416 MiB, with about 170 MiB left. Under B, that leaves room for some ciphertext cache.
- **384 MiB.** The central point: shmem about 500 MiB, with about 90 left.
- **432 MiB.** The edge: shmem about 560 MiB, with about 30 left. This one may thrash the binary's text or be OOM-killed.
- **Dynamic.** 0.75 × `read_budget().budget`. That is about 0.75 × (640 − 45 − 64) = **398 MiB** in the read pass and about 0.75 × (640 − 183 − 64) = **295 MiB** in the shuffled load. With 1.3x shmem: 517 + 45 = 562 MiB in the read pass, and 384 + 183 = 567 MiB in the load, before files being written.

### At the limit with nothing reclaimable

There is no swap, so shmem cannot be reclaimed. The memcg first reclaims file pages: the ciphertext under B, then the binary's text and anything else. A thread whose own text is being thrashed takes major faults that are not envelope reads.

Under D the envelope reads are not page faults at all, so **any major fault on the D reader thread is that thrash**. That makes `read majfaults_per_op` under D a direct alarm. When nothing is left, the memcg OOM killer kills the pass's process.

- **The evidence to keep:**
  - the sitting's exit status, together with how `ab` reports a pass it lost (pair.sh's header says a bound kill ends the sitting with 137 once the table is printed);
  - `dmesg | grep -i "memory cgroup out of memory"`;
  - the sampler's `oom_kill` and `failcnt` from `memory.oom_control` and `memory.failcnt`.
- **An OOM kill is a result.** It is reported in the table with its evidence and is not rerun. If d432 or b432 is killed, no larger point is tried. If d384 is killed, which is not expected, d432 and b432 are skipped.

### Should the prototype recompute its budget?

**Yes, for the load,read run.** A fixed X sized for the read pass's 45 MiB of anon leaves no room for the shuffled load's 183: 1.3 × 432 + 183 = 745 MiB. That is the `ddyn` arm, and it is small: the 15 lines in `run` plus the two-line `hard`/`swamp` derivation.

**What it does not cover.** Files being written (`unbacked`, pager.rs:509-512) count in `held` but cannot be given back, and the recomputed budget does not subtract them. E7 read `cipher_peak` at 632-642 MB in the load,read sittings, while its 2-second sampler never saw shmem above 311 MiB in that phase. Which is right is unresolved. The load,read run samples every 0.5 s and records `memory.max_usage_in_bytes` to settle it.

---

## 5. Ranges

**Yes: the patch reads a missing run in one pread.** The range is known in three places:

- `Paged::ensure`'s multi-chunk branch (pager.rs:1334-1338), for any ask that crosses chunks;
- `Paged::willneed` (pager.rs:1476-1500), which is where `Blob`'s scan prefetch plan (blob.rs:2271-2283) hands the exact ranges a scan will walk;
- the thread being under a `Hold` (pager.rs:151-197), which is what the merges and the open read under.

The first two give an exact range. The third gets a fixed 128 KiB lookahead, because a merge asks one record at a time and walks its inputs in order.

**What is lost.** A run is cut at a unit's end (1 MiB) and at the first chunk that is already present or busy, so a sequential read of a half-present range becomes several preads. Point reads stay one chunk a pread: there is nothing to read ahead, and the envelope is advised random today too (pager.rs:744-746).

**The risk.** Merges that read outside a `Hold` read 4 KiB a pread, at roughly 4 KiB / 35 µs ≈ 115 MB/s. That would show in the load,read run's closing sync, and in `run_chunks` being near zero against `direct_reads`.

---

## 6. The runs

### Setup, once, with nothing timed

All paths are under `S=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/once`.

1. **Check the tree.** `cd /home/user/supdb && git status --short` must be empty.
2. **Apply the patch** with Edit, then `git diff > $S/once.patch`.
3. **Run the tests** (no timing run on):
   ```
   cargo test --profile checked --lib pager -- --test-threads=4
   SUPDB_ONCE_FORCE=d  cargo test --profile checked --lib pager
   SUPDB_ONCE_FORCE=d  cargo test --profile checked --test cipher
   SUPDB_ONCE_FORCE=dn cargo test --profile checked --test cipher
   SUPDB_ONCE_FORCE=d ZFUZZ_CIPHER=1 ZFUZZ_CIPHERCACHE=16384 cargo test --profile checked --test fuzz fuzz_range -- --exact
   ```
   The last one is a short range of seeds. Each run is believed only after its `--list` has shown the tests it names. Each run gets `TMPDIR=$S/tmp`. About 10 minutes, compiles included.
4. **Build the bench and save it:**
   ```
   cd bench && cargo build --release && cp target/release/bench $S/bench-once
   ```
   That is a 370 MB copy; check `df -h /` first. About 5-8 minutes.
5. **Revert the tree:**
   ```
   git apply -R $S/once.patch
   git status --short      # must be empty
   ```
   At the very end of the session, with no timing run on, run `cargo build --release` in `bench/` so that `target/release/bench` is the clean tree's again. About 5 minutes.
6. **Copy the samplers.** Copy `e7-ooc/sampler.py` to `$S/sampler.py`, adding `memory.max_usage_in_bytes`, `memory.failcnt` and `memory.oom_control`'s `oom_kill` to each line. Copy `phasemem.py` unchanged.

### Every sitting

```
S=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/once
BIN=$S/bench-once
PARENT=/sys/fs/cgroup/memory$(awk -F: '$2=="memory"{print $3}' /proc/self/cgroup)
UNSET="-u GLIBC_TUNABLES -u MALLOC_ARENA_MAX $(env | sed -n 's/^\(SUPDB_[A-Za-z0-9_]*\)=.*/-u \1/p')"
SETS="SUPDB_LAG_MARK=1 SUPDB_LOAD_PHASES=1 SUPDB_ZPAGES_TRACE=1 SUPDB_PAGER_TRACE=1"

# before each sitting
ps -eo pid,etime,args | grep -E 'bench|cargo|rustc|perf' | grep -v grep   # must print nothing of ours
df -h / ; ls -A $S/tmp                                                    # tmp must be empty
python3 -I $S/sampler.py "$PARENT" $S/samples/$NAME-$k.txt 1 & SPID=$!    # 0.5 for load,read

# the sitting (k=1: A,B; k=2: B,A)
cd $S/pairs && env $UNSET TMPDIR=$S/tmp $SETS $BIN ab --arms $ARMS --size $SIZE --reps 1 $OPTS \
  > $NAME-$SIZE-$k.out 2> $NAME-$SIZE-$k.err ; echo $? > $NAME-$SIZE-$k.rc

# after each sitting
kill $SPID
dmesg | tail -100 | grep -iE "out of memory|oom|drop_caches"   # an idle-reclaim drop_caches inside the window is noted
ls -A $S/tmp                                                    # remove by name what is listed, e.g. rm -rf $S/tmp/supdb-ab-<pid>
ls $PARENT | grep supdb-bench                                   # must print nothing
git -C /home/user/supdb rev-parse --short HEAD > $NAME-$SIZE-head.txt; echo "set:$SETS bin:$BIN" >> $NAME-$SIZE-head.txt
```

These are the `bench ab` invocations pair.sh makes (pair.sh lines 44-58), with the same scrub of `GLIBC_TUNABLES`, `MALLOC_ARENA_MAX` and every exported `SUPDB_*`. The differences are three:

- the saved binary;
- `--reps 1`, which E7 passed (the default is 12, main.rs:290);
- sitting 2 runs even if sitting 1 failed, so that a kill in one order is reported beside the other order.

`SUPDB_PAGER_TRACE` is set for both arms. It prints only, and only from a cipher store.

`OPTS` for the primary cell is `--bound 671088640 --passes read --ops-div 1`.

**Time.** The estimates come from E7's sittings: 162-165 s for a read-only cipher-against-plain sitting, and 189-207 s for load,read.

### The run list (strictly one at a time)

| # | ARMS (sitting 1) | size | OPTS | estimate | purpose |
|---|---|---|---|---|---|
| R0 | supdb-cipher,supdb-nozpages | 5M | primary | 6 min | the baseline, taken again in this session from the patched binary. It should read 0.28-0.33x, as E7 did; if it does not, the patch moved the shipped arm, and nothing after it is read until that is explained |
| R1 | supdb-cipherd384,supdb-nozpages | 5M | primary | 5 min | the central D point |
| R2 | supdb-cipherd320,supdb-nozpages | 5M | primary | 5 min | D below |
| R3 | supdb-cipherd432,supdb-nozpages | 5M | primary | 5 min | D at the edge (an OOM is a result) |
| R4 | supdb-cipherb384,supdb-nozpages | 5M | primary | 6 min | B at the central point |
| R5 | supdb-cipherb432,supdb-nozpages | 5M | primary | 6 min | B at the edge |
| R6 | supdb-cipherddyn,supdb-nozpages | 5M | primary | 5 min | the dynamic budget in the read pass |
| R7 | best-once,supdb-nozpages | 5M | `--bound 671088640 --passes load,read --ops-div 1`, sampler at 0.5 s | 8 min | does the shuffled load's closing drain survive, and at what rate |
| R8a | supdb-cipher,supdb-nozpages | 5M | `--bound 671088640 --passes scan --ops-div 1` | 5 min | the scan baseline (E7 has none) |
| R8b | best-once,supdb-nozpages | 5M | as R8a | 5 min | the scan pass |
| R9 | best-once,supdb-cipher | 1M | `--passes load,read,scan --reps 4` (no bound, no `--ops-div`) | 4 min | a store that fits must pay nothing |
| R10 | supdb-cipherdndyn,supdb-cipher | 1M | as R9 | 4 min | **only if** R9 shows D losing in core (expected: first touches go to the device) |
| R11 | optional: E7's `profile.sh` with best-once in place of supdb-cipher | 5M | primary, perf for 20 s | 6 min | the miss path's non-device cost under D. Only if the time is left |

**Total.** About 65 minutes of timed runs (R0-R9) plus 10-15 minutes of gaps and cleanup, and 8-10 minutes for R10 and R11 if run. Setup takes about 25 minutes and the final rebuild about 5, with no timing run during either. That is well inside 2.5 hours, which leaves room for nothing to be retried silently: a run that dies is reported as it died.

**Choosing the best once-arm.** It is the once-arm with the highest reads/s ratio against `supdb-nozpages` in both sittings, among those not killed whose sampled usage stayed under 625 MiB.

- **For R7:** use `ddyn` if it reads within a tenth of the best fixed arm. Otherwise run the best fixed arm, and report a kill there as the answer to section 4's question: a fixed budget does not survive the load.
- **For R8b and R9:** use the same arm as R7.

**Disk.** Both arms of a 5M sitting are about 0.75-0.8 GB each. The saved binary is 0.37 GB, and about 6.9 GB is free. So `df` should stay above 4 GB. Below 2 GB, stop and clean.

---

## 7. The counters that say the mechanism fired

**From the `ab` table, each arm, each sitting:**

- `read reads_per_s` and `read p99_us`.
- `cipher_fills`, `cipher_evicted_units`, `cipher_held_at_end` and `cipher_peak` (the last two should sit near X, with `peak` ≤ X + X/16 in a read pass).
- `cipher_evictor_longest_wait_us`, `cipher_waits` and `cipher_rescues`.
- The new counters: `cipher_direct_reads`, `cipher_direct_bytes`, `cipher_direct_us` and `cipher_run_chunks`, plus `cipher_limit_lo` and `cipher_limit_hi` for ddyn.

What each arm should show:

- **D.** `direct_reads` ≈ `fills` in the read pass, with `run_chunks` near 0 there. `direct_us / direct_reads` is the device time an I/O, to hold against E7's 32-38 µs a fault.
- **B and baseline.** `direct_reads` = 0.

**`majfaults_per_op` and `read_bytes_per_op`.**

- For B and the baseline these are the device faults, as in E7.
- For D, `majfaults_per_op` should be near 0, since an `O_DIRECT` read is not a fault. Anything left is thrash (section 4).
- `read_bytes_per_op` should be about 4096 × the misses a read **if** the kernel accounts `O_DIRECT` reads in a task's `read_bytes`. It is expected to (submit_bio accounts every read bio), but that is **not verified** on this kernel.
- **`read_us_per_io` is meaningless for D**, since it divides by faults.
- Device truth: the sampler's `vda_rsect` (from `/sys/block/vda/stat`) over the pass's window, divided by the reads.

**The sampler**, from outside the process every 1 s (0.5 s in R7), joined to the phase marks by `phasemem.py`:

- the group's `usage`, `max_usage_in_bytes`, `failcnt` and `oom_kill`;
- `rss`, `shmem` and `cache − shmem`;
- the child's `RssAnon` and `RssShmem`;
- `pgmajfault` and `workingset_refault_file`.

**For a once-arm under D, `cache − shmem` must fall to tens of MiB in the read pass.** It starts with the ciphertext the load left, which the kernel reclaims as shmem grows. Its value through the second half of the pass is the number to read. If it stays above about 100 MiB, something is still caching ciphertext, and the arm is not "once". The prime suspect would then be a `willneed` or open path left on the mapping.

**shmem / held.** `pagertrace`'s `held` beside the sampler's `shmem` at the same second gives the ratio. It is read at each X: 1.2-1.3 at 280 MiB in E7, and falling toward 1.0 at 432 if the straddle is the cause.

---

## 8. Predictions, written before any run

### Inputs from E7 (per read)

- **Plain store:** 6.8-8.1 µs a read (123-148k reads/s), 0.14-0.16 device faults a read at 32-39 µs each, and about 2.1 µs of non-device work.
- **Cipher today:**
  - 22.6-24.4 µs a read;
  - 0.684 plaintext misses a read at a `held` of about 280 MiB, of which 0.44 a read go to the device;
  - about 7.9-9.5 µs of non-device miss path. AES is 0.6 of that, the memfd fresh-page fault 2.7 and the envelope mapping 1.6.
- **Cipher at 64 MiB:** 1.14 misses a read.
- **Plaintext size:** about 721 MiB (5M × 116 B × 1.303).

### The miss model

Under uniform reads, misses scale with the uncached fraction. Two fits bracket it:

- proportional to the uncached fraction: m(X) = 1.12 · (1 − X/721);
- linear through E7's two points: m(X) = 0.684 − 0.0021 · (X − 280).

| X (MiB) | misses a read |
|---|---|
| 320 | 0.60-0.62 |
| 384 | 0.47-0.53 |
| 432 | 0.36-0.45 |

### Under D

Every miss is a device read. With device time of 32-38 µs and non-device miss path of 7-9.5 µs, a read takes 2.1 + m × (39-47.5) µs.

| arm | µs a read | against supdb-cipher | against plain |
|---|---|---|---|
| d320 | 25.5-31.6 | 0.72-0.96x (a loss: today 35% of misses are served by the page cache, and under D none are) | 0.21-0.32x |
| d384 | 20.4-27.3 | 0.83-1.2x | 0.25-0.40x |
| d432 | 16.1-23.5 | 0.96-1.5x | 0.29-0.50x |
| ddyn (X ≈ 398 in the read pass) | as d384 to d432 | | |

**Faults and memory under D:**

- Device reads a read equal the misses: 0.36-0.62, against E7's 0.44 today. So **the device term moves by a tenth either way, not by the 3x E7's two-thirds attribution would suggest**.
- shmem/`held` should fall from 1.2-1.3 toward 1.1 at 432. That is uncertain, since it rests on the straddle cause, which is not verified.
- `cache − shmem` should be under 50 MiB in the second half of the read pass.

### Under B

At the same X, B should read within ±10% of D. The page cache left is 640 − 47 − 1.3X ≈ 90 MiB at 384 and about 30 at 432. That serves about 15% and about 5% of misses, worth 1.5-3 µs a read. Against that, B pays the mapping fault and the memcg's direct reclaim, 1-2.5 µs a miss more than D.

- **If D beats B by more than a tenth,** the mapping and reclaim costs are larger than E7 priced them.
- **If B beats D by more than a tenth,** `O_DIRECT` costs more than a buffered fault here.

### The other runs

- **R7 (load,read).**
  - Ordered load: as E7, 0.61-0.74x.
  - Shuffled load: its survival under ddyn is **not predicted with confidence**. `unbacked` files are outside the recomputed budget, and E7's peak `held` of 632-642 MB is unexplained.
  - If it survives, the closing sync should be within the E7 cipher range of 13.8-14.9 s, or worse if the merges read 4 KiB a pread (section 5's risk, seen as `run_chunks` ≈ 0).
- **R8 (scans).** The scan pass's misses are a few chunks a scan. With exact runs from `willneed`, D should read within ±15% of the baseline cipher scan, since the gain and loss are as for point reads. **This is the least grounded prediction here**, because E7 has no scan pass under the bound.
- **R9 (in core, 1M, no bound).**
  - Under D, every first touch of a chunk is a device read where today it is a copy from the page cache: about 37k chunks × 35-40 µs ≈ 1.3-1.5 s of device time on the first read pass.
  - The predicted read cell is **0.4-0.7x of supdb-cipher, a loss**, since the store's ciphertext is in the page cache right after the load. That is what R10's DN is for: DN should read within 5% of supdb-cipher.
  - The scan cells move less. The load is unaffected, as no write path changed.

### Kill line

**Holding the memory once is not, on its own, worth moving budget ownership if the best surviving once-arm reads both of these, in both sittings:**

- under **1.25x of `supdb-cipher`** (R0, the same session), and
- under **0.45x of the clear store**.

In that case:

- The faults term is the plaintext cache's efficiency per MiB, not the double holding: the 4080-byte chunk's straddle, 1 MiB units, and chunk-granular misses.
- That efficiency is what a design changes first (whole-page chunks, page-granular eviction), and this prototype cannot show it.

The lever **is** worth the design change if a once-arm reads **0.6x of the clear store or better in both sittings, with its usage held under the bound and no kill**. That would also mean the predictions above were too pessimistic, and the miss model is to be corrected from the run's own `fills`.

Between the two lines, the result is "necessary, not sufficient". The design then needs both the budget and the cache's efficiency, and the report says which term moved.

**A D arm that wins only by being killed less is not a win.** Any arm whose `failcnt` rose or whose reader took more major faults than the baseline is read as thrash first.

---

## 9. What the prototype cannot show about a real design

- **Whole-page chunks.**
  - Today's 4080-byte chunk takes two plaintext pages unless it is the first of its unit. An isolated chunk is 8 KiB of shmem for 4080 bytes of data, and shmem runs 1.2-1.3x of `held` (E7). A store that keeps whole-page chunks, with the tags apart, holds about a fifth more useful plaintext in the same bound, and takes one fresh-page fault a miss instead of up to two.
  - Every once-arm here carries the straddle, so its result is a lower bound on what holding once with whole-page chunks would give.
- **Eviction by the megabyte.** Units of 256 chunks, with a one-bit clock per unit and two sweeps (pager.rs:665-706, 1154-1201).
  - Under the read pass's uniform keys, recency carries no information, so granularity costs little there.
  - A skewed workload (the mixes, a Zipfian) would gain from page-granular eviction, and nothing here measures that.
  - The 1 MiB punch also costs the evictor its `MADV_REMOVE` and its IPIs (E7: 6.7% of a core). That is unchanged.
- **The census and the controller.**
  - Under D a device read is not a major fault, so `ru_majflt`, the bench's `majfaults_per_op` and `read_us_per_io`, the group's refaults and `Paged::resident` (pager.rs:1380-1428, `mincore` over a mapping D never faults) all stop seeing the cipher store's misses.
  - The compact form's controller is off under a cipher as shipped, so nothing here breaks. A real design that owns the budget must give the controller and the census another source, such as the pager's own fills.
- **The budget's owner.**
  - The fixed X points are hand-chosen for this bound and this process's anon. The dynamic arm follows the process's anon, but not files being written (`unbacked`), not other stores in the process, and not other processes in the group.
  - A real design must also decide what `swamp` is. At 1.5x the limit (pager.rs:321) it is past an OOM for any X above about 330 MiB here.
- **Readahead and queue depth.** D's point reads are one 4 KiB `O_DIRECT` pread each, synchronous, on one thread.
  - Only the single-threaded read pass is run. The threaded pass, and an asynchronous design (`io_uring`, batched misses), are not tested.
  - The merge lookahead of 128 KiB under a `Hold` is a guess sized to the device's `read_ahead_kb`.
- **First touch in core.** D sends every first touch to the device, even when the ciphertext is in the page cache (R9). DN's `RWF_NOWAIT` probe is one answer, but it lets cached ciphertext linger. Whether a real design should populate from the written plaintext instead (the design's write-through) is not tested.
- **Writes.** The write path is unchanged: envelopes still go through the page cache, dirty and then reclaimable. The load cells measure only the effect of a different read budget beside them.
- **Machine and noise.**
  - This is one virtio disk on a Firecracker guest with four vCPUs, so `O_DIRECT` latency and memcg reclaim costs are this machine's.
  - Every figure has two sittings, one in each order. A difference under a tenth is not resolved by them.
  - The host runs `idle-reclaim` drop_caches. dmesg shows them at idle moments only (none inside E7's windows), and each sitting checks its window.
