# EXT.27: the next engine's load under shuffled arrival — registered before the run

Every durable-load number the next engine has against LMDB (`EXT.22`,
0.694x) comes from `ext-kv`, whose keys arrive in order, and f55 made that
shape special: ordered seals are promoted to partitions by rename and
nothing is merged. f55's own uniform arm put random arrival near 0.42x of
the ordered rate (F55.3), but that was an internal number with no LMDB
beside it in the same process. `ext-loadshape` already loads the same key
set both ways for `supdb-buffered` against `lmdb-nosync`; this adds the
matched durable pair, `next` against `lmdb`, to the same interleaved run.

## Predictions

- **P27.1 -- shuffled, the next engine loads at 0.35x to 0.55x of LMDB**,
  and `EXT.27` is recorded as failing. Every seal overlaps every partition,
  so each merge round rewrites the live set it lands in; LMDB pays page
  splits, which are cheaper than that.
- **P27.2 -- the next engine's own swing, ordered over shuffled, is between
  1.5x and 2.5x**; LMDB's is under 1.3x. The B-tree's order sensitivity is
  page splits; the next engine's is the difference between promotion and
  merge.
- **P27.3 -- the ordered pair in the same run lands within 0.6x to 0.8x**,
  bracketing `EXT.22`'s 0.694x from a different suite on the same host.

## What would change the plan

If P27.1 is refuted upward -- shuffled at or above 0.6x -- then merge
cost is not where the ingest goes under random keys and the next lever is
not the merge. If the swing is under 1.3x, promotion is not what makes
the ordered arm fast and F55.3 was misread.

## Outcome (full, one run, `results/ext-loadshape.full.json`)

P27.1 refuted, upward and by an order of magnitude: shuffled, `next` loads
284,938 ops/s against `lmdb`'s 48,041, **5.931x** (p=0.0022). P27.2 held
at the edge (swing 1.508x); P27.3 held (ordered pair 0.653x). The miss was
LMDB's swing, predicted under 1.3x and measured 13.7x: a durable commit of
a thousand random keys dirties about as many leaf pages and fsyncs them
all. The plan's own rule applies -- the merge is not where the ingest goes
*relative to LMDB* under random keys -- but the merge is still where the
next engine's own 0.66x against its ordered arm goes, and that is the
lever that remains. Recorded as `EXT.27`, holding.
