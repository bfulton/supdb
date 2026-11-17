## 1. The pager (src/db/pager.rs)

- **Chunk states** (`:94-98`): `EMPTY`, `BUSY` (one thread decrypting into it), `COLD`/`HOT` (present and lent; `HOT` means read since the evictor last looked), `DYING` (present in a marked unit, not lent until the unit is taken back). **Unit states** (`:102-104`): `LIVE`, `EVICTING`, `FREEING`.
- **Why units**: `CHUNK = 4096-16 = 4080` (`:109`), so neighbouring chunks share an OS page. The smallest run of whole pages is 256 chunks, which is 255 pages (`unit_chunks`, `:123`). Eviction works per unit, never per chunk (`:28-33`).
- **`ensure`** (`:1312`) is the fast path: one SeqCst load, `== HOT`, then `slow`. Under debug, `PINS>0` is asserted (`:1316`). **Invariant:** nothing is lent to a thread the evictor will not wait for.
- **`slow`** (`:1027`):
  - `COLD→HOT` by CAS.
  - `DYING`: `rescue`, then CAS `DYING→HOT`, then the unit is checked again. If it was re-marked, the chunk goes back to `DYING` (`:1066-1073`). **Invariant:** no chunk is lent out of a unit marked twice.
  - `EMPTY→BUSY`: `rescue`, then `fill`. If the unit is `FREEING`, the chunk is reset to `EMPTY` before waiting in `gone` (`:1082-1091`). **Invariant:** no fill lands in pages that are being given back.
- **`rescue`** (`:1132`) is the CAS `EVICTING→LIVE`.
- **`mark`** (`:1154`): any `HOT` in the unit is aged to `COLD` and the unit is skipped. Otherwise `LIVE→EVICTING` and every present chunk goes to `DYING`; it spins on `BUSY` (`:1188`).
- **`free`** (`:1210`): `EVICTING→FREEING`, wait until no chunk is `BUSY`, `give_back` (`:1585`), then `debug_assert` that no chunk was `COLD/HOT` (`:1247`).
- **Evictor `pass`** (`:605`): mark up to the goal, bump the era and `Grace::bump`, wait until `oldest() >= tag && holds_past(era)` (`:629-647`), then free. **Invariant:** pages go only once every reader pinned before the mark has left or rested.
- **`Hold`** (`:144`) is the pin for threads that have no reader-table slot (merges, open). `pin` (`:157`) is a Dekker store/reload. `rest()` (`:178`) re-pins at the current era and breathes if the cache is `crowded`.
- **`crowded`** (`:497`) is `could_give > budget + max(budget/16, 4 units)`. **`swamped`** (`:523`) is `+ max(budget/2, 6 units)`. Both are measured against `could_give` (`:509`), which leaves out unbacked files. **`breathe`/`breathe_unless`** (`:551/557`) and **`breathe_swamped`** (`:529`) sleep at most `BREATH = 20ms` (`:242`). **Invariant:** waits stay bounded, with store work waiting before caller reads.
- **`check`/`poison`** (`:436/426`) make the first tag failure sticky for the whole store.

## 2. Readers (bytes.rs, flatindex.rs, blob.rs)

- **`Bytes::lend_at`** (`bytes.rs:79`) returns `None` when the source is `piecewise()`. **`piecewise`** (`:94`); **`ASK_PIECES`** is `true` by default (`:105`) and `false` for `MmapBytes` (`:492`).
- **`MmapBytes::paged`** (`:365`) sets `lend: 0`. So `slice_at` (`:468`) always fails its bounds check and goes to the `#[cold] slice_slow` (`:395`), which calls `Paging::ensure`. `lend_at` (`:481`) refuses through the same bounds check. **`Paging`** trait: `:300-330`.
- **`Sect`** (`flatindex.rs:611`) is implemented for `&[u8]` (`:627`, plain slice ops) and for **`Pieces`** (`:662`, impl `:749`). Each read on `Pieces` is a `slice_at`. `Pieces::window` (`:695`) asks for the bytes to the piece end first, then grows the request, up to `RECORD_WINDOW = 16 MiB` (`:678`). There are 23 functions generic over `S: Sect`.
- **blob.rs**:
  - `Sec::read` (`:486`) keeps a piecewise section as `Lent{off,len}` and reads none of it.
  - `Sec::get` (`:504`) returns `lend_at` or `not_lent` (`:552`). `not_lent` builds `Unsupported` with no message, to avoid allocating.
  - `Blob::flat` (`:1018`) does `key.get(..).ok()?`. `Blob::pieces` (`:1035`).
- **Point read**: `indexed!` (`:941`) tries `flat()` first and falls through to `pieces()` under `cold_path`. It is used at 8 sites. Under a cipher, every lookup therefore asks for the whole section, is refused, drops the error, and then goes by the piece.
- **Scan, public API**: `scan_at` (`:2391`) tests `B::ASK_PIECES && piecewise()`, which is constant-false for `MmapBytes`.
- **Scan, engine**: the engine calls `scan_either` (`:2412`; call sites `db.rs:19759, 19813, 19944`).
  - It always runs `scan_whole` (`:2448`) first.
  - `scan_whole` returns `Err(by_the_piece())` (`:524-525`, `by_the_piece` at `:543`) when `flat()` is `None` and the source is piecewise, before walking anything.
  - `scan_refused` (`:2427`, `#[cold]`) then sees `Unsupported`, and only then is `scan_pieces` (`:2302`) entered. `scan_pieces` walks 64-record windows with `walk_fixed!` and falls back to `scan_records` (`:2361`).
- **Why it is written this way**: the docs give the reason. Naming `scan_pieces` inside `scan_whole` "cost the loop below three to four instructions an entry" (`:2443-2447`). Testing `piecewise` in `scan_at` was "three instructions a scan" (`bytes.rs:98-104`). The kind is "found in the refusal, on the branch a failed walk takes anyway" (`blob.rs:2406-2410`).

## 3. db.rs sites that exist for the cipher's read side

| site | what it does | pager invariant it serves |
|---|---|---|
| `PagerGrace` `:9656` (installed `:27471`, before any thread exists) | lets the evictor bump and read the reader table | grace wait |
| `Slot.page` `:9755`, `oldest_lent` `:9985` | per-slot "lent since" epoch, narrower than the pin; `u64::MAX` means idle | evictor waits for borrows only, not for held states |
| `page_pin` `:10007`, `page_set` `:10020`, `page_idle` `:10034`, `page_back` `:10040` | rest, clear, idle (debug unpin), re-pin | the same |
| `ATTN_PAGED` `:15624`; `paged_bits` `:18204` | cipher bit in the census byte | zero cost for plain stores |
| `begin_slow` `:18278`, `paged_begin` `:18176` | snapshot re-pin; read waits if `swamped` | reads wait only far past budget |
| `Entered::finish` `:16351`, `end_slow` `:18134`, `leave_slow` `:18152` | `pager.check()` replaces the answer; snapshot goes back to idle | poisoned bytes are never answered |
| `page_snap` `:15549` (set `:18574`, cleared `:18612-18622`) | a snapshot handle is idle between its reads | lifetime pin ≠ lend |
| `unlent` `:20606` (11 call sites, e.g. `:31066`), `unlend` `:20696` / `Unlent` `:12444` (5 sites, e.g. `:27063`) | idle while waiting on a thread or the device | a wait under a pin is a pin |
| `rest` `:20626` (used by `same_blocks` `:30638`) | re-pin, report crowded | landing comparison doesn't fill the cache |
| `build_rest(_unless)` `:20664/20672`, `abandon_slow` `:21740`, `REST_HERE` `:15637`, `writer_rests` `:18215`, `not_asked` `:18224` | builders rest per block; `WRITER_OP` `:16367` restricts this to the writer's own ops | no rest under a caller's callback |
| `look_mask` `:18237` (used `:23645`) | look every block (0) instead of every 64 (63) | bounded pinned span |
| `AHEAD_REST` `:15642` (set `:30812`), `ahead_look` `:30853` | builder-ahead rests | the same |
| `SCAN_REST = 1024` `:12459`, `scan_rested` `:20722` (`:20775`) | long scans re-pin every 1024 keys | a scan doesn't hold the store |
| `MergeRest`/`NoRest`/`&Hold` `:11982-12008`, `MERGE_REST = 256` `:12465`; `merge_ranks` `:12316` re-asks the keys after a rest | merges rest | the same |
| `vault::hold` (`vault.rs:2227`) at `:11970, 12943, 39674`; open `:27614` with rests `:27813/27844` | slotless pins | grace |
| `vault::check` (`vault.rs:2217`) at `:12865, 13033` | refuse merge outputs built from bad reads | poison |

## 4. Size estimate

Not verified line by line. Method: non-test spans read above, plus grep counts.

| piece | approx. lines |
|---|---|
| pager.rs (lines 1–1595, tests excluded) | ~1,600 |
| bytes.rs (`Paging`, paged kind, piece methods) | ~150 |
| flatindex.rs (`Sect`/`Pieces`, `:596-794`) | ~200, plus making 23 functions generic |
| blob.rs (`Sec` branches, `indexed!`, the four scan functions, error helpers) | ~250 |
| db.rs (sites above) | ~600 |

That is **about 2,800 lines**, roughly half of them doc comments. pager tests (~530) and the `vault.rs` read glue are on top of that.

## What would disappear vs remain

If plaintext were an ordinary mapping whose pages fault in and can be dropped at any time:

**Disappears:**
- the whole `ask`/lend protocol: `Paging`, `slice_slow`, `lend: 0`, `piecewise`, `lend_at`, `ASK_PIECES`, `Sect`/`Pieces`/`window`, `Sec` piecewise arms, `indexed!`'s second arm, `scan_pieces`/`scan_either`/`scan_refused`/`by_the_piece`/`not_lent`;
- the chunk and unit state machine and the epoch grace (`Grace`, `PagerGrace`, `Slot.page`, `oldest_lent`, `page_*`, `Hold`);
- every rest and breath site: `unlent`, `unlend`, `rest`, `build_rest`, `REST_HERE`, `AHEAD_REST`, `look_mask`, `SCAN_REST`/`scan_rested`, `MergeRest`, `MERGE_REST`, `page_snap`, `paged_begin`, `crowded`/`swamped`/`breathe`.

**Remains:**
- the cipher and the envelope format, decrypting on a page fault, and some decision about where plaintext lives and its budget;
- tag-failure reporting. A fault cannot return an `Option`, so `poison`/`check`, `end_slow`'s answer replacement and `vault::check` stay, or turn into SIGBUS handling;
- `ATTN_PAGED` only if `check` stays per read;
- the write side (memfiles, `persist`, the door), which is outside this slice.

One caveat, not verified: with a plain mapping, residency and the bound on plaintext become something some other component (kernel or userfaultfd) has to provide and account for, not the engine.