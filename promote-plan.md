# f55: piece promotion — registered before the code

f54 found where the merge's bytes go under ordered keys: every seal lands
in the last partition, whose fence is open above, so each merge round
rewrites it and re-splits it, and ordered keys wrote more device bytes than
random ones (F54.2). Selecting ranges cannot help; a range is being
rewritten to receive data that overlaps none of its keys.

## The change

Before a range merge, look at the range's pieces against its partition's
last key (`key_at(keys - 1)`): if every piece's first key lies above it and
the pieces are mutually disjoint in key order, no merge is needed. The
partition keeps its data and its fence closes at the first piece's first
key; each piece becomes a partition by rename, its fence running from its
first key to the next piece's (the last one inheriting the range's upper
fence). A piece and a partition are the same file format at the same
level of the same writer; only the name and the level differ, so promotion
is renames and one manifest write, and nothing is rewritten. Uniform keys
never qualify -- every piece spans the whole space -- and are untouched.

## Predictions

- **P55.1 — with sequential keys at 16 MB seals, device bytes fall to at
  most 0.5x of f54's range-flush arm** (662.6 MB). Data is written once
  to the WAL and once to a seal; the partition rewrites go away.
- **P55.2 — sequential ingest-to-routed rises by at least 1.3x** over the
  same arm; the merge phase all but disappears.
- **P55.3 — uniform keys are unchanged** (device bytes within 1.05x,
  ingest a tie): nothing qualifies.
- **P55.4 — reads after the drain do not differ** under either order:
  promoted pieces are partitions, fence-routed, with no Bloom to consult.

f55 runs the four arms of f54 with promotion on and off, interleaved.
