# Freeing retired files on a third tier (#30): where it stands

Patches against `dcdd042`, each whole (`git apply`):

- `reaper-v1.patch`: `Options::retired_reaped` (off). A retired segment, its
  ordered index and its form file renamed to a dead name (`dead-N`), kept
  with a `Weak<Seg>`; once nothing holds the segment its pages are given
  back (`vault::let_go`) and a thread at nice 19 frees it (`vault::
  unlink_held`, the discard at the idle I/O class under a gibibyte
  waiting). Open sweeps dead names; a pause of the segment work holds the
  reaper's turn; close frees the rest. Arms `supdb-reap`,
  `supdb-cipherautoreap`; counts `retired_buried/freed/waiting_peak_mb`.
  Five tests (tests/db.rs, tests/cipher.rs) and two vault unit tests; 12
  mutants, every one killed (mut_reap.py in /tmp/claude-0/ci). The tree's
  lists (couplings `vault::let_go`, `vault::unlink_held`; door thread
  count; cost-entries `Db::hold_reaper`) are not declared yet.
- `reaper-pool-scratch.patch`: v1 plus a crude pool -- a plain store's
  retired segments written over by the next segment's writer (cut to
  length at `into_inner`). No crash safety. Pricing only.
- `reaper-inline-scratch.patch`: v1 with the dead files freed by the
  segment work itself, one a turn, after every other step
  (`retired_reap_inline`). Its db tests' hold seam parks the segment work
  under it (the pause test hangs): pricing only.

What a free costs here: / is ext4 with no journal, mounted `discard`, so an
inode freed is a synchronous discard of every block, 0.43 ms a MB (64 MB:
28 ms). `discards.py` samples every thread's kernel stack every 3.6 ms and
keeps the ones in a free: at 10M under 1.25 GiB a full rewrite put 6.0 s of
22 on the segment work in the clear and 5.4 of 25 under a cipher, the
writer 0.25 and 0.65 s, the load's drain 0.9-1.9 s on the writer, 1,192 of
1,483 samples at a reference's last drop (`__fput`) and not at an unlink.

Pairings (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 sh scripts/pair.sh A B
10000000 --bound 1342177280 --passes load,lag --reps 2 --ops-div 10`):

- v1 (idle class, BE past 1 GiB), cipher vs `supdb-cipherauto`: clean
  pass 0.66x and 0.68x, pass after the full rewrite 0.85x and 0.58x,
  shuffled load 0.92x and 0.90x; plain vs `supdb`: clean pass 0.75x and
  0.36x, 1% burst 0.14x and 0.23x, shuffled load 1.07x and 1.10x, the
  sweep's sum a tenth better. 2.3-2.8 GB of dead files waited at once.
  The trace had the reaper freeing 1.4 s inside the clean pass: the
  load's backlog drained into the reads and the durable burst after it.
- pool scratch, plain: clean pass 0.51x and 0.93x, 1% burst 0.46x and
  0.66x, sum 0.85x and 0.79x.
- cuts of 4 MB at the idle class, no BE: cipher 1% burst 0.28x and 0.29x;
  the plain arm died of SIGBUS: `unlink_held` hands back the file when a
  promotion's link still names the inode, and the cuts truncated a live
  partition. A cut is held to the inode's link count.
- `recycle.py`: a 64 MB file written fresh with the old one freed, 103 ms
  (write 12, fsync 61, free 28); written over a dead file's blocks, 60 ms.
