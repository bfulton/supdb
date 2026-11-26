# One container file: what the file system costs today, measured

Probes for the question of a store in one file it manages itself (a log
beside it). All on this fleet's disk: ext4, no journal, mounted `discard`,
virtio with a write-back cache and no FUA (`/sys/block/vda/queue/fua` 0).

`prims.py DIR` -- the primitives (`prims.out`):

- a commit of 16.8 KB made durable: appended 0.25 ms, into a pre-written
  file 0.13, `O_DSYNC` the same, `O_DIRECT|O_DSYNC` 0.11.
- the same commit beside a writer of megabyte pieces: 0.15 ms where the
  writer's bytes stay in the page cache of another file; 180 ms where the
  writer hints each piece to the device (`sync_file_range`), in another
  file or in the commit's own; 208 ms where the writer is buffered in the
  commit's own file (the fdatasync writes its pages back); 1.5 ms beside
  an `O_DIRECT` writer in another file and 14 in the same. A flush pays
  for every byte anyone sent the device since the last one.
- 64 MB written and made durable: a fresh file 84 ms; an extent of a
  pre-written file 59; the same with each piece hinted and the range
  waited for before the fdatasync 47; `O_DIRECT` pieces 78.
- 64 MB given up: unlink 32 ms, a hole punched 29, reused 0.
- a 4 KB file made durable under a temp name, renamed, its directory
  synced and its predecessor unlinked 0.62 ms; 4 KB written in place and
  synced 0.12.

`inv.sh` then `inv.py DIR` -- an strace of `supdb` through the load and the
lag sweep at ten million keys under 1.25 GiB, the calls laid to phase,
thread and purpose (`inv.out`). The full rewrite's burst, 26.6 s traced:
the writer 18.1 s in the log's fdatasync (9,000 commits at 2.0 ms) and 1.1
in its writes; the segment work 7.4 s freeing (munmap 5.05, unlink 2.3)
and 0.5 in opens, closes and directory syncs; the seal, merge and build
threads 7.9 s in their files' syncs, 5.6 in their writes, 0.8 in names and
0.7 freeing. 9.6 GB went to the device for 1.3 GB of updates: log 1.5,
merge outputs 5.7, form files 2.3.

`flush.py DIR` -- whether a data file needs a sync of its own, with the
device's flush requests counted (`/sys/block/vda/stat`, field 16). A
landing of 64 MB: a fresh file synced, its directory synced, the manifest's
slot written and synced, 84 ms and three flushes; an extent of the data
file synced and then the slot, 54 ms and two; the extent's pages written
back by range (`sync_file_range`, waited for) and the slot's fdatasync the
one flush, 47 ms and one. An fdatasync of a file with nothing dirty still
reaches the device here, 0.05 ms and one flush. Sound only over blocks
written once before: a range's writeback covers no allocation, so a data
file that grows is synced itself where it grows.

`grow.py DIR` -- a landing of 64 MB by what its blocks were before it, two
runs. Never written, past the file's end or reserved by `fallocate`: 54-59
ms with the data file synced and then the slot, two flushes. Written once
before, with zeros or with a segment: 44-46 ms by range with the slot's
sync the one flush, 46-50 with the data file's own sync. Writing the zeros:
54-57 ms for the 64 MB; reserving them: 0.2-0.3. So a block's first use
costs about 12 ms in 64 MB, once, and writing zeros ahead to save it costs
four to five times that in bytes the next flush pays for.

`login.py DIR [BOUND]` -- whether the log can live in the data file
(`login.out`): 300 commits of 16.8 KB back to back beside a writer of
megabyte pieces in the data file. With the writer's bytes left in the page
cache: a log of its own 0.14 ms a commit; in the data file 189 ms by
`fdatasync`, which writes the whole file's dirty pages back, 0.22 by an
`O_DSYNC` write, which syncs its own range, and 0.17 by `O_DIRECT|O_DSYNC`.
With the writer hinting its pieces to the device, or under a bound of 256
MiB that writes them back for it, a commit pays for the writer's bytes
either way and the two layouts read alike: 170-180 ms plain under the
bound in both, 32-37 hinted in both. One shape stands apart: direct
synchronous commits in the data file read 0.13-0.16 ms beside any writer
and the writer fell from 1.3 GB/s to 30-90 MB/s, the commits holding the
file's lock across their flush.

`copy.py DIR` -- a checkpoint by copy: a gibibyte copied into a new file
(`copy_file_range`) and synced, 1.14-1.19 s with the source in the page
cache and 1.19-1.39 s with it out. No clone on this ext4 (`cp
--reflink=always`: Operation not supported).

## The experiments the plan owed (`scratch/plan/onefile.md`)

**X1, the frees' ceiling.** `reaper-inline-scratch.patch` with `reap_one`
answering false, so that nothing is freed until the close, built into the
`enc` binary and paired in core (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 sh
scripts/pair.sh supdb-reap supdb SIZE --passes load,lag --reps 3`, at
`dcdd042` with the patch): at a million keys 58 files and 567 MB never
freed, at two million 125 and 1.4 GB. The ordered load read for whichever
arm was listed first in all four sittings, by 4-8%; the shuffled load for
the arm that frees nothing in three sittings of four (1.29x, 1.03x, 0.99x,
1.11x, one of them in every pair); the sweep's weighted sum 1.08x, 0.95x,
1.01x, 1.12x. Nothing past what the position does. The disk here holds no
run of it at ten million keys under a bound, where the frees are 8 s of
the full rewrite's traced 26.6. So by this the frees are worth a few
percent of a cell in core, and the bar is not cleared by them.

**X2, one inode under many writers** (`writers.py`, `writers.out`). Three
writers each making 192 MB durable: into files of their own 1,180-1,280
MB/s; into reserved extents of one file 1,165-1,315; into extents written
before, by range with one flush, 1,240-1,666 in the open and 1,290-1,390
under a bound of 256 MiB. A ranged commit beside them waits tens of
milliseconds for their bytes in every shape. One inode costs the writers
nothing measured.

**X3, a mapping an extent** (`mapext.py`, `mapext.out`). 64 MB read cold
through a mapping of a file of its own and of an extent a gibibyte into a
2 GB file, through an open of its own: a scan in order 27-29 ms against
29-31, 4,000 pages at random 171-177 ms against 177-183, the same faults.
About 3% a cold fault for the extent in three reps of three, at the edge
of what the reps spread; read again in O2's bounded pairings.
