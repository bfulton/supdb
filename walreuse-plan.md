# f57: recycling WAL files — registered before the code

The durable load on x86 (`EXT.22`, 0.694x of LMDB) has one fdatasync per
batch on either side, so the barrier count is not the gap. A two-minute
measurement on this host says what might be: on ext4 an fdatasync of an
append that grows a file costs 0.42-0.75 ms per 100 KB and an fdatasync of
an overwrite into blocks already allocated and written costs 0.23-0.33 --
the growing file commits an inode change through the journal each time,
the overwrite does not. LMDB's commit is an overwrite. The next engine's
WAL is a growing append, and a 1,000-op batch is about 100 KB.

## The change

`NextOptions::recycle_wal`. A seal rotates to a new WAL; today that is a
fresh file and the retired one is unlinked once its segment is published.
With the flag, the retired file is kept in a small pool and the next
rotation *renames* it into place and writes from offset 8 over the stale
frames, so every block a commit touches is already allocated and written;
the first WAL is pre-written with zeros to the seal size for the same
reason. Replay must then stop at the new tail rather than read a stale
frame from the file's previous life: each frame's CRC is xored with a mix
of the WAL's id, so a frame written under another id fails its check and
the walk stops there. The WAL magic moves to `\x03`, so a WAL from before
this is refused by name, not misread.

## Predictions

- **P57.1 -- durable ordered ingest rises by at least 1.10x** with the
  pool on, arms interleaved in one process, `Sync::Always`, 1,000-op
  batches, 1M keys. The saving is 0.2-0.4 ms of a ~1.9 ms batch.
- **P57.2 -- device write bytes are within 1.05x**: the pre-written first
  WAL adds one seal's worth once; recycled files add nothing.
- **P57.3 -- shuffled arrival gains at least as much**, since the barrier
  is the same fraction of a batch there.
- **P57.4 -- reads after the drain do not differ.** Nothing on the read
  path knows what a WAL file looked like.
- **P57.5 -- c4-crash still holds with the pool on**; the stale-tail
  problem is the one new hazard, and the CRC seed is its answer.

## What would refute it

An ingest gain under 1.05x says the fdatasync's journal cost is not on
the commit path at this batch size -- the drive's flush dominates and the
microbenchmark's difference was the page cache's. A c4 failure says the
seed is not enough and a stale frame can be adopted.

## Outcome (full, two runs)

**Run 1** (pre-write in 1 MB pieces): ingest a tie both ways (1.019x,
1.004x, no difference), commit phase 0.966 -> 0.734 s sequential and
0.968 -> 0.729 uniform -- P57.1's mechanism -- and device bytes **2.18x
and 1.76x**, P57.2 refuted by a factor the pre-write could not explain.
The microbenchmark found it: an overwrite into a file pre-written in 1 MB
pieces costs 11.2x its bytes at the device, into one pre-written in 4 KB
pieces 1.04x, and 100 KB overwrites over frames written 100 KB at a time
(the recycled shape itself) 1.04x. The page cache sizes a folio by the
write that creates it, and a byte dirtied inside a 1 MB folio writes the
megabyte back. Kernel 6.18, ext4.

**Run 2** (pre-write in 4 KB pieces, the recorded one): device bytes
+64.0 MB in each arm, exactly the two pre-written files; commit phase
0.960 -> 0.782 s sequential (19%) and 0.910 -> 0.858 uniform (6%);
ingest still a tie (0.984x, 0.954x). P57.1 and P57.3 refuted at the gate,
P57.2 refuted by the pre-write alone, P57.4 held, P57.5 held (c4: 120/120
with the flag on in half the trials and 15 tears landing on stale frames).

The flag stays off. The commit-phase saving is real and would survive on
a store that outlives one seal cycle, where the pre-write is paid once;
in a fresh 1M-key load it is paid back. What the decomposition also says
is where the durable load now goes: the commit phase is ~45% of the
window, waiting on seals ~14%, and the rest is the caller's thread
building frames and memtable entries -- the next lever is compute, not
the barrier.
