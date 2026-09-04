# f45-scanfloor: what does resolving a key cost a scan, and would inline keys recover it?

Registered before the run, as f38 through f44 were.

## Why

EXT.24 stands at 0.769x of LMDB (0.818x on two dev runs), up from 0.040x
when first recorded. Three implementation faults accounted for that climb
and none of them was compaction policy. What remains is structural: LMDB
advances a cursor inside a leaf page that holds keys and values together,
which is a pointer bump and consults nothing, where a segment keeps its
keys only in the index section and must resolve each one to find its
values.

The proposed fix is a format change -- keys written inline in the data
blocks, so an ordered scan sweeps the data and never touches the index.
It costs roughly 16% more space at this suite's 16-byte keys and 100-byte
values, on top of an index that still holds the keys for point lookups.
It also risks a second read path, and this project has twice been bitten
by exactly that (a `Blob` reporting one generation where `Reader` reported
another; a `value_bytes` that counted prefixes it excluded).

So the change gets priced before it is built. The question is narrow:
**how much of a scan is key resolution, and how fast would a sweep be?**

## Shape

Five arms interleaved over one 1M-key store (100-byte values, the ext-kv
shape), each answering the same 10,000 ranges of 100 keys:

- **scan** — `Db::scan` as it stands, the baseline EXT.24 measures.
- **index-walk** — `Blob::key_at` per rank and nothing else: what walking
  the index costs with no values read at all.
- **values** — `Blob::values_at` per rank and nothing else: resolution
  plus block read, with no key returned.
- **inline-sweep** — a synthetic file holding `klen|key|vlen|value` in key
  order, swept linearly from a precomputed start offset. This is the
  ceiling the format change could reach: no index, no resolution, one
  sequential pass. The start offset is precomputed and NOT timed, because
  a real implementation would find it with one index lookup amortised over
  the whole range.
- **inline-sweep-cold** — the same, with the page cache dropped between
  reps where the host permits, so the sweep is not credited for being warm
  when the baseline is not.

## Predictions

- **P45.1 — the inline sweep is at least 2x the current scan.** Below
  1.3x the format change is not worth its space or its second layout and
  should not be built; between 1.3x and 2x it is a judgement call that
  wants the space number beside it.
- **P45.2 — key resolution, not block reading, is the larger half.** The
  index-walk arm accounts for ≥ 40% of the baseline's per-entry time.
  Refuted means the cost is in reading value bytes, which an inline
  layout does not avoid, and the whole premise is wrong.
- **P45.3 — the sweep beats LMDB's recorded 16.98M entries/s on this
  host.** If the ceiling itself does not clear the comparator, the format
  change cannot close EXT.24 and something else must.

## What this decides

Build or do not build, on a number rather than on the appeal of the idea.
If P45.1 and P45.3 hold, the format change is justified and the follow-up
question is the one this project already knows to ask: one layout for
everyone (pay the space always, keep a single read path) or two (save the
space, reopen the seam that has produced two silent bugs here).
