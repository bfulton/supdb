# A store in one file: decisions so far

The owner's proposal, in the session that priced freeing retired files
(`scratch/reap/notes.md`): a file abstraction of the store's own over a
single real file, for control of what file operations cost and for mobile.
Nothing here is built. The measurements each decision rests on are in
`notes.md`, with the probe that makes each.

## Decided

- **One file a store.** Its head holds the manifest's two slots, written in
  place as the two slot files are now, never the one holding the newest
  manifest known durable. Everything else is extents: segments, their
  ordered indexes, their form files, and the log.
- **The manifest names extents** (offset, length) where it names files. The
  free space is what no manifest entry covers, derived at the open; no
  allocator state is durable.
- **Space is reused, never freed in steady state.** An extent is free once
  the manifest that stopped naming it is durable and nothing holds its
  segment (the holder check of `reaper-v1.patch`, a `Weak<Seg>`). No
  automatic shrink; a trim a caller asks for.
- **The log is extents of the same file**, from the same allocator: a
  rotation takes the next, a retirement gives one back. A commit syncs its
  own range (an `O_DSYNC` write), never the file.
- **Growth reserves and does not write** (`fallocate`, in large steps). The
  manifest keeps the highest offset ever landed; a landing under it is made
  durable by its range's writeback and the slot's fdatasync as the one
  flush (Linux, ext4 and XFS), one over it by the file's own sync. Where a
  file system moves blocks it is written to, the file's own sync always.
- **A promotion is a manifest publish.** No link.
- **A checkpoint is a full copy** (the owner's decision): the live extents
  and a manifest written into a new file, a store of its own, as
  `Db::checkpoint` promises now and as its fallback across file systems
  already does. The extents being copied are held from reuse for the copy's
  length, in the process; no store refers to another's file. About 1.2-1.4
  s a gibibyte on this disk. Copied by the kernel's own call, so that a
  file system that can clone does.

Considered and set aside: the manifest as a file of its own so that a
second manifest is a store over shared extents (instant, but the origin
has to keep and release pins, and the checkpoint cannot be deleted, moved
or shipped by ordinary tools); a ring format for the log.

## Open

- Whether this is built on the encryption branch or as the line after it.
- Ordered ingest: a run appends into an extent and its commits put their
  marker in the log; today the run's file is its own log.
- Direct synchronous commits in the shared file: 0.13-0.16 ms a commit
  beside any background writer, which they starve (`login.py`). A lever on
  what a commit pays for background bytes, or a hazard; not designed.
- Extents contiguous on the file, or units mapped side by side: the readers
  lend from one mapping of a segment.
- What a torn write over an extent's previous contents leaves, by test.
- No benchmark opens a checkpoint: a timed pass for it comes before the
  change, so that the links have a number.
- The ceiling of the frees alone: a run with nothing freed until the close,
  at three million keys for the disk's sake.
