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
