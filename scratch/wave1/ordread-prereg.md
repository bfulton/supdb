# Stage 1b: `Options::point_via_ord` and the arm `supdb-nocacheordread`

Pre-registered before any pairing of it exists. The engine change is in place and its tests have run; no
throughput measurement of it has. The command lines for the pairing and for the fault record are in
`ordread-commands.sh`, written and not run.

Revised once after a review, still before any pairing: the verdict below now says what every combination of the two
sittings concludes (the first writing left several undefined), and the threaded handles' reads are counted, so the
threaded cells print their own evidence that they took the path. Nothing a measurement has shown changed anything.

## CELL

`supdb-nocache` against `supdb-nocacheordread`, the durable row, `bench ab --bound 1073741824` at ten million
keys (the 1 GiB cgroup, a page cache of about 0.94 GB), the read passes at `--ops-div 10` (1,000,000 uniform
point reads of the loaded set), on one thread and on the class's two and three. Quantities: `read
majfaults_per_op`, `read reads_per_s`, `read read_bytes_per_op`, and the same with `_2t`, `_3t`. The banked cell
for the baseline: 0.41-0.46 faults a read, 54k reads/s on one thread, 1,694-1,895 bytes a read, 0.36-0.43 faults
and 132k reads/s on three. `lmdb-nordahead`: 0.27 faults, 79-82k.

## DECOMPOSITION

A point read of the baseline touches two regions of one partition: the hash slot (8 B a slot, about 190 MB over
the store, touched by every read) and the record (about 1.3 GB, the value inline). Faults a read are the share of
those two touches that miss. With B = 937 MB of cache, `1 - (B - 190)/1300 = 0.425`, the banked 0.41-0.46. Through
the ordered index a read touches the heads (8 B a key, 80 MB, one 512-byte window of 64 heads), the directory
word (4 B a key, 40 MB) and the record: `1 - (B - 120)/1300 = 0.372`, and about 0.33 if the heads keep their pages
better than the hash did (M17's residual of 0.07; the probe read the hash file 91.0% resident after a pass, under
the 97-100% registered). The change hands 70 MB of always-touched cache to the records: 0.054 faults a read by the
model.

## CHANGE

`Options::point_via_ord` (default false). A point read of a partition whose ordered index holds whole keys
(`OrdIndex::uniform`) finds its rank with `OrdIndex::seek_exact`, checks the key against the record at the rank,
and reads the values with `Blob::values_at`. A key the heads place nowhere is answered absent without a record.
A rank whose record does not hold the key sends the read to the hash, and is counted. A partition whose keys are
not of one short length, the level-0 pieces, `count`, and every other path read as before. The arm
`supdb-nocacheordread` is the nocache policy with the option on, in the durable row, in no roster: it is chosen by
hand (`bench ab --arms supdb-nocache,supdb-nocacheordread`). `Reader::ord_point_reads()` says, for one handle, how
many reads the index answered and how many it sent back to the hash. The pairing prints it twice: `count
ord_point_reads` and `count ord_point_declined` are the writer's own handle (the single-thread read pass), and
`count ord_point_reads_threads` and `count ord_point_declined_threads` are the sum of the reader handles the
threaded passes hand their threads, each added to the store's tally when it is dropped, after its thread's clock
has stopped. Only the arm's handles sum; the baseline's are the bare handles it always had.

## PREDICTIONS (9.2, restated; a value outside a range is a refutation)

| quantity | `supdb-nocache` | `supdb-nocacheordread` | B/A |
|---|---|---|---|
| faults a read, 1 thread | 0.41-0.46 | 0.33-0.38 | 0.72-0.93 |
| reads/s, 1 thread | 52-58k (banked 54k) | 58-66k | 1.07-1.22 against 54k |
| bytes a read, 1 thread | 1,694-1,895 | 1,350-1,560 | 0.72-0.93 |
| `count ord_point_reads` | 0 | 1,000,000 | |
| `count ord_point_declined` | 0 | 0 | |
| `count ord_point_reads_threads` | 0 | 5,000,000: the passes run the single-thread count of reads on every thread, on this class's two and three threads (`run::thread_counts` of 4 cores) | |
| `count ord_point_declined_threads` | 0 | 0 | |
| faults a read, 2 and 3 threads (derived here: 9.2 registers the single-thread cell only) | as banked | below A | below 1 |

Derived here, not in the design, lower confidence: the faults of the ordered arm split between the new sites as
0.00-0.03 for the heads and the directory word together and 0.30-0.36 for the record. In memory (100k keys, the
read pass alone) the seek replaces a probe and B/A on `read reads_per_s` is 0.35-0.75, which is why the option is
not the default; that pairing is cheap (`--size 100000 --passes read`) and is not part of this stage's verdict.

## VERDICT

Twelve pairs a sitting, two sittings, the same command. `ab` prints, for each quantity, the median of A and of B,
the pairs with B above A out of those not tied, and the sign test (10 of 12 is p 0.039); "median" and "pairs"
below are those. The faults decide and the rate is read beside them.

**The gate: a pairing is not read at all unless the path was taken.** `count ord_point_reads` is the single-thread
read pass's reads on the arm (1,000,000) and 0 on the baseline; `count ord_point_declined` is 0 on both; `count
ord_point_reads_threads` is the threaded passes' reads on the arm (5,000,000) and 0 on the baseline; `count
ord_point_declined_threads` is 0 on both. A sitting that fails the gate is not a sitting: the cause is found and
the command is run again. A declined read is an index or a check that is wrong, and a count short of the reads is a
path that was not taken.

**One sitting is classified on `read majfaults_per_op`, one thread,** by d, the median of A minus the median of B
(positive when the arm faults less):

- MOVED: d is 0.02 or more and B is below A in at least 10 of 12 pairs.
- WORSE: d is -0.02 or less and B is above A in at least 10 of 12 pairs.
- LEVEL: d is under 0.02 in size, whatever the pairs say.
- MIXED: anything else (d of 0.02 or more in size with fewer than 10 of 12 pairs on its side).

**Two sittings are read by this table.**

| first | second | reading |
|---|---|---|
| MOVED | MOVED | the arm faults less: "how far", below |
| LEVEL | LEVEL | the STOP RULE |
| WORSE | WORSE | the arm faults more: the heads and the directory cost the cache more than the hash they replace; the STOP RULE's consequences hold, with the sign recorded |
| any other pair | | undecided: a position effect or the host's until a third sitting says, and no figure is banked from the two |

**A third sitting** is the same command. Two of the three in one class decide it, by the table read as if those two
were the sittings, and the odd one is reported beside the result. Three different classes decide nothing: the fault
record by region (below) is taken for the arm and the count that separates the sittings is found before another
sitting is run, as CLAUDE.md says of a pass that reads two shapes.

**How far, when both sittings are MOVED.** The arm's median faults a read in each sitting against the registered
0.33-0.38, and the baseline's against 0.41-0.46:

- inside the band in both: the prediction is met;
- below 0.33 in either: refuted low, the heads and the directory cost less cache than the model gave them;
- above 0.38 in either and still 0.02 or more under the baseline: refuted high, the lever is real and smaller than
  the model said; the measured d replaces the model's 0.054 as the coefficient and is banked as that, not as level;
- below 0.33 in one sitting and above 0.38 in the other: the sittings disagree about where the arm sits, which is
  undecided as above.

The rate beside it, `read reads_per_s`, B over A on one thread against the registered 58-66k (1.07-1.22 of 54k):
inside, met. At or below 1.00 with the faults moved, the seek's instructions cost what the faults saved: recorded
as a lever in faults and not in rate, and the option stays an arm chosen by hand for a store that does not fit.

**The threaded cells** (`read majfaults_per_op_2t` and `_3t`, `read reads_per_s_2t` and `_3t`) are derived here with
lower confidence, and are read only when the single-thread cell is MOVED in both sittings and the gate's threaded
counts held. Predicted: faults below the baseline's. A threaded cell that disagrees in sign with the single-thread
cell is reported as it came and not explained here.

## STOP RULE

The pair reads LEVEL on faults in both sittings, or in two of three after a third: the model's coefficient for
page-cache auxiliary structures is wrong, L1 is not a lever and is not the incompressible fallback, and z's scan
working set (R5) is re-priced. WORSE in both, or in two of three, stops it the same way with the sign recorded. A
gate that fails, and sittings left undecided, are not stops: they are a cause to find.

## DONE WHEN

The arm in place and off by default (done), and 9.2's ranges met or refuted by the two sittings, or by two of
three; or the sittings left undecided and the count that separates them found and written down.

## The untimed fault record (7.2b, "Faults by region")

Registered: of the baseline's read-cell faults, the hash-slot site takes 0.00-0.02 a read and the record site the
rest (0.40-0.45); refuted if the hash-slot site is over 0.04 a read (the hash is being evicted, M17's residual is
real, and holding it resident is worth up to that). The probe's residency (91%) makes the refutation more likely
than the registration's prior. The record is `perf record -e major-faults -c 1 -d` over `bench ab --passes read
--reps 1` with the phase marks on, bucketed by the code that took each fault and the file it landed in; the site
is the region, since a read reaches a region from its own line (hash slot: `rd_u64` under `with_lookup`; record:
`parse_record` or `record_key`; heads: `OrdIndex::head`; directory word: `rd_u32` under `key_at` or
`with_record_at`). It was exercised at 300k keys on files dropped from the cache: the baseline's path 0.45 faults a
read, 0.402 at the record site and 0.051 at the hash-slot site; the ordered path 0.416, 0.400 at the record site
and 0.014 at the directory word, none at the hash. That is a cold first touch, not the bound's steady state, and
checks the tooling, not a prediction.
