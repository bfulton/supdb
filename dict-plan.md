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

## Outcome (full, `results/w5-dict.full.json`)

P5.3 and P5.4 held: nine ranges read exactly their two plans and agreed
with the whole reader on every row; ranking a 210-key field costs 10 ns a
key. P5.1 and P5.2 were refuted, both by page geometry rather than bytes.
The sparse open is 23,808 bytes -- 3.5% of the index -- but four 64 KiB
pages, 279,856 against the whole open's 869,680 (32.2%); it crosses 5%
at about 5.6 MB of index, the shape this reader is for. A field's plans
are well under its share of the index un-paged (country 9,860 against
18,467), but a range is two plans in two regions, each straddling a page
boundary at both ends: four pages of slack, not two, and `country` read
1.31 of the two-page bound. What the numbers say together: the reader
does what it was built to do and the page size is the unit that matters
at the dictionary sizes logshed has today; the page size is `cache.mjs`'s
to tune, and a 16 KiB page for the index region would take the sparse
open under 5% of this fixture's whole open.
