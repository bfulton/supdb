# f46-segwrite: what would a purpose-built segment writer buy?

Registered before the run, as f38 through f45 were.

## Why

Phase accounting closed the commit path: f42's lazy-seal arm runs at
1,029,190 ops/s, past f39's raw+index floor of 1,014,003, so the WAL and
the memtable have no measured headroom left. EXT.22's 0.299x against LMDB
is therefore seal and partition work, and EXT.25 has already recovered
1.985x of it by policy (leaving partitioning to background compaction).

What remains is the seal itself. It writes each segment through
`Store::create` + `append` per value + `checkpoint` + `close` -- a general
put path with a hash table, a freelist, an arena and per-key bookkeeping,
for input that is already sorted, already immutable, and written exactly
once. A writer built for that shape would do two things and no others:
lay values into blocks in key order, and build the flat index once.

Both pieces already exist as public builders -- `flatindex::encode` takes
`(key, Extents)` pairs and produces the key section, `encode_blocks`
produces the block table -- so the question is not whether it can be
built. It is whether the general path costs enough to justify a second
writer in the format layer, with everything that implies for the seam
`tests/blob.rs` polices.

## Shape

Two arms interleaved over the same sorted input (1M keys, 100-byte values,
one value per key -- the seal's shape after its sort):

- **store-writer** — what a seal does today: `Store::create`, one `append`
  per value, `checkpoint`, `close`.
- **bulk-parts** — the two irreducible pieces of a purpose-built writer,
  measured without building one: the value bytes written sequentially to a
  file, and `flatindex::encode` over the same keys with the extents a
  sequential layout would produce. This is a FLOOR, not an
  implementation: it omits the block table, the checksums and the
  superblock, so a real writer lands above it.

## Predictions

- **P46.1 — the floor is at least 3x the general path.** Below that a
  second writer is not worth its risk: the seal would still dominate and
  the format layer would carry two ways to produce a segment. Above 5x it
  is clearly worth building.
- **P46.2 — the index build is the smaller half of the floor.** If
  `flatindex::encode` over 1M keys is most of the cost, a bespoke writer
  saves little, because that call is what a checkpoint already does and
  neither writer can skip it.

## What this decides

Build or do not build, on the same terms f45 used to decline the
inline-key format change: a floor measured before the work, with a
registered bar. Note what f45 taught -- its own diagnosis produced a
cheaper fix that closed the gap without the change it was pricing, and
the same outcome is possible here.
