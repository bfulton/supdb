# w5: the dictionary by range — registered before the measurement

logshed's ask: partial reads of the key space, for the day a dictionary
is too large to fetch whole. `SparseBlob` keeps the key section's header
and fence and plans a range as a directory slice and then the records it
names; every plan is asserted exact in `tests/dict.rs`. What remains is
to price it on the shape it is for, the day index (`build_day`, term
order), against the whole-index open the browser does today.

## Predictions

- **P5.1 -- the sparse open fetches under 5% of what the whole open
  fetches**, page-rounded at 64 KiB: three pages or so (superblock,
  index header, block table, fence) against the whole key section.
- **P5.2 -- one field's range costs bytes proportional to its keys**: the
  two plans for a field, page-rounded, come to at most the field's share
  of the index plus two 64 KiB pages of slack (the fence stride at each
  end, rounded), for every field of the schema.
- **P5.3 -- exactness holds on the recorded reads**: every range's walk
  touches exactly its two plans and nothing else, on the day index, as it
  does on the test fixtures.
- **P5.4 -- ranking a field from a sparse reader is not slower than a
  tenth of a millisecond per key**: the walk decodes records out of a
  copied span, and the whole-index `scan_counts` was 4.5 ns a key; the
  copy is the difference, and it is bounded by the range.

## What would refute it

A field's plan much wider than its keys says the fence stride is too
coarse for this dictionary and the plan needs a second, finer level. An
open above 5% says the fence itself is not small on this shape.
