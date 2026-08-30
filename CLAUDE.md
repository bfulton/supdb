# Working in this repository

Supdb is an embedded key-multivalue store, and a benchmark suite whose job is
to try to falsify the claims made about it. The two are meant to stay together.

## The rule that matters most

**A finding is not a number, it is a statement with a recorded expected state.**
`claims.json` holds every statement the project makes about the engine,
including the ones that currently fail. `verify` checks it against `results/`
and CI fails in **both** directions:

- a limitation that gets worse turns the build red;
- a limitation that gets **fixed** also turns the build red.

The second is not a mistake. Either the engine improved and the claim is stale,
or the experiment stopped testing anything. Both need a person to decide which.
So when you fix something, update `claims.json` in the same change — that edit
is the record that the fix was intentional.

## Before adding a benchmark

Four rules, enforced in `src/bench/` rather than remembered:

1. **Nothing is measured once.** Use `Trial`, which runs configurations
   interleaved. Report a median with an interquartile range.
2. **A difference is not a difference until it clears `stats::compare`** — a
   Mann-Whitney U test *and* a minimum effect size. Do not hand-roll a
   comparison; the gate exists because the original design document reported a
   13.9% difference as a win against its own stated 15% noise floor, and
   `stats.rs` carries that case as a regression test.
3. **A finding whose precondition was not met is `Finding::not_exercised`,
   never `holds`.** This has already caught three false greens: an out-of-core
   experiment that compared warm against cold inside a dataset too big to be
   warm; a multi-process experiment that ran 8 readers against a 64-slot table;
   and a crash experiment that blamed the engine for crashing before any
   checkpoint existed. If a run cannot reach the condition, say so.
4. **Throughput never travels alone.** Latency distribution, peak RSS, and
   device-level write bytes come with it. Write amplification is measured from
   `/proc/self/io`, never inferred from file size — they are different
   quantities.

## Profiles

`ci` runs in seconds and is **never citable**; it proves the experiments still
run. `dev` is minutes. `full` is the only profile a published claim may cite,
and every record carries which it was.

Never run two timing benchmarks concurrently. Four cores measuring each other
is not a measurement.

## Layout

| path | what |
|---|---|
| `src/` | the engine, vendored from the design artifact **verbatim** |
| `src/bench/` | the measurement substrate — stats, histogram, plotting, env capture |
| `src/bytes.rs`, `src/blob.rs` | the read path over any byte source; compiles for wasm |
| `src/bin/internal.rs` | falsification suite (f1–f7) |
| `src/bin/correctness.rs` | damaged files, model oracle, crash injection (c1–c3) |
| `src/bin/logshed.rs` | day-index shape, size budget, browser-test fixture |
| `bench/external/` | Supdb inside other projects' evaluations (redb, LMDB, sled) |
| `web/` | the browser reader, its size control and its browser test |
| `results/` | committed measurements — the source of truth |
| `figures/` | generated from `results/`, never drawn by hand |
| `docs/architecture-review.md` | why every experiment here exists |

The engine modules carry scoped `#[allow(clippy::all, dead_code)]`. They were
vendored byte-for-byte from the design artifact and have since been changed
only to fix specific defects, each described in `claims.json`. **Do not
reformat them** — the architecture review cites line numbers in commit
`101a4e7`, and `results/baseline/` holds the measurements taken against that
revision. Everything in `src/bench/`, `src/bin/` and `bench/external/` holds to
`-D warnings`.

## Measuring a change to the engine

**Never compare two separate runs.** It was tried here and it does not work:
between a pre-fix and a post-fix run of the same suite, the three *unchanged*
comparators in the external benchmark moved by +20% to +43%. Almost all of the
apparent improvement was the machine.

To measure the cost of a change, put both arms behind a runtime flag and run
them **interleaved in one process**, as `f8-checksums` does for
`Options::checksums`. Space is the exception — file size is immune to drift and
can be compared across runs.

And use `--profile full`. The same checksum cost measured at `dev` came out
"+3.0%, not significant"; at `full`, with the variance tight enough to resolve
it, it is +8.5% and unambiguous. An underpowered measurement is not a free
lunch, it is a measurement that could not see.

## Comparing against another engine

**Match the guarantees before ranking, or do not rank.** `Features::unmatched`
decides whether a pair may be compared at all and `ordering_of` emits
`not_exercised` when it may not, naming the axes. This is enforced because it
was not: `engines.rs` carried three fairness rules and only two of them
equalized, the third merely *recorded* what each engine promises. Durability
was filed under the third, so `EXT.1` compared a Supdb that never reaches the
device against an LMDB that fsyncs every batch and called it a 1.28x win, with
the difference in a table two lines away. The checksum axis was unequalized the
other way for exactly as long, and cost Supdb its read lead.

Equalize in **both** directions where the engines allow it, so a reader gets
the comparison for the guarantee they care about rather than the one that
flatters: `supdb-durable` and `lmdb` both commit per batch, `supdb-buffered`
and `lmdb-nosync` neither do. Where an axis cannot be equalized -- LMDB cannot
stop being transactional -- say which way the residual leans and read the
result as a bound: a loss is at least that large, a win is not yet a win.

The matched scorecard against LMDB, `full`, all four failing:

| | Supdb | LMDB | |
|---|---|---|---|
| load, both durable (`EXT.9`) | 7,394/s | 674,858/s | **0.011x** |
| load, neither (`EXT.10`) | 964,147/s | 1,779,498/s | **0.542x** |
| read (`EXT.11`) | 1,139,523/s | 1,093,321/s | 1.042x, no difference |
| scan, cold (`EXT.12`) | 31.4M/s | 39.9M/s | **0.785x** |

Supdb beats redb on reads by 2.19x and is behind LMDB or level on everything
measured. That is the honest position and it is worse than what this file
claimed for months.

Rule 4 is why two of those numbers are legible at all. The suite reported
throughput, read latency and file size and neither of the other two the rule
names, until it did: the durable arm sends **29.9 GB to the block layer for
116 MB of data**, a write amplification of 270x against LMDB's 2.1x, and
leaves a 7.35 GB file. `checkpoint` being O(key count) was on the books as a
time cost and is a device cost of the same origin.

## The second reader

There are now two read paths. `store::Reader` maps a file; `blob::Blob<B>`
reads through a `Bytes` source and so runs where there is no file to map — a
browser, over an object fetched out of S3. Same format, same `flatindex`, same
`block` decoder. A second read path is a liability, because its failure mode is
not a crash but a browser quietly answering a different question from the
server, so `tests/blob.rs` opens a store written by `store.rs` and requires the
two to agree on every key, every value, every count and the checkpoint
identity. It has already caught two: `Blob` reporting the superblock's
generation where `Reader` reports the index section's, and a `value_bytes` that
counted the varint length prefixes it claimed to exclude.

Nothing in that path is asynchronous, and that is the constraint rather than an
accident. `flatindex::lookup` returns a borrow into the index section and a
borrow cannot survive an `await`, so the byte source is synchronous: JS
downloads the object into OPFS once, and every read after that is
`FileSystemSyncAccessHandle.read`. That is only viable because a day fits —
`w1-daysize` puts a 32 MB download at 912,522 log lines — and it is why that
was measured before any of it was built. `web/README.md` has the rest.

`Bytes` has two halves for one reason: `read_at` copies and every source can
answer it, `slice_at` lends and only a source backed by memory can. Native
takes the second for every access and copies nothing, which is the axis
`flatindex` exists to win and the one a byte-source abstraction is most likely
to lose. `Blob::zero_copy()` is asserted in the test, because a native reader
that started copying would still pass every correctness check.

**A count is not free, and the reason is the format.** `f28-count` runs four
arms interleaved. Resolving a key and stopping is 73 ns; counting its values by
walking their length prefixes is 2,421 ns; reading them all is 2,420. Counting
without decoding is *not* cheaper than reading — W2.1 is recorded as failing so
that premise cannot come back. An `Ext` is block, offset, byte length and the
offset of the last record, and none of those is a count. What is 28x faster is
`count_fixed`: a fixed-width value carries a fixed-width length prefix, so a
posting list's count is arithmetic on `Ext::len` with no block touched. That is
a property of the schema, not of the format. Adding a per-extent count to the
format would recover at most 6.7 ns of the gap between those two, for four
bytes on a 16-byte `Ext` paid by every store forever, so it was priced and
declined.

The same difference decides whether a browser can rank a dictionary at all.
`scan_counts` pays a `count` per key, so it is O(every posting in the range) —
for a day index, the whole file — and `scan_counts_fixed` is O(extents). Over
2,000 keys that is 1,308 ns/key against 5.0, a factor of 262, so a day's whole
term dictionary ranks in about 40 µs and nothing has to be precomputed at roll
time.

`count_fixed` claims a count only when two independent quantities agree: the
run is a whole number of strides, *and* `Ext::last` — the offset of the final
record, stored so that reading the newest value is O(1) — is exactly
`(n-1)*stride`. Divisibility alone is not enough and was not: a run of 17
variable-length values divided exactly by a stride of 4 and the first version
answered 23. Two quantities is still not a proof, so the contract is that the
caller knows its schema; `tests/blob.rs` carries the case either way.

**How the roll writes decides the file size, by 22.6x.** Appending a day's
postings in log-line order writes 831 MB where grouping them by term first
writes 36.7 — 44,629 inline merges against zero, which is F5.1's latency tail
showing up on the space axis. The ratio grows with the day, so the naive roll
degrades exactly where it matters. Any tool that builds an index here sorts by
key first.

## Known-failing on purpose

Do not "fix" these casually; each is load-bearing evidence and each is
described in `claims.json`:

- A reopened store declares history before the reopen broken. `Store::open`
  does not carry the reuse log across, so `history_from` is set to the
  generation opened and older snapshots are refused rather than served.
- Reader open grows with key count, though no longer in proportion to it: 20x
  for 100x the keys, and what remains is the block table rather than the key
  index. The index is 57 bytes per key in a mapped section readers share; it
  was 131 bytes per key, heap-resident and duplicated per process.
- Write throughput barely scales with writer threads.
- `checkpoint` is O(key count): it rewrites the whole key index rather than
  what changed. The durability *curve* now has a usable point on it -- a
  20,000-op window sustains ~199k ops/s with about 2MB at risk -- but that came
  from making writes faster, not from fixing the floor.

If you fix one, the corresponding claim must change from `fails` to `holds` in
the same commit, and the review in `docs/` should be updated to say so.

Fixed so far, with reproducers kept in `tests/known_bugs.rs`: delete
resurrection, the double-free that handed one slot to three blocks, decoder
panics on damaged input, silently-served corruption (now checksummed), a
checkpoint that appended three index sections and released none of them, and a
reader that fed a flat block-table section to the varint decoder and reported
the misparse as file corruption.

The largest one is `Store::read_all`. A writer can read its own sealed, staged
and pending state, so a read after a write no longer needs a checkpoint and a
fresh `Reader`; a scan refreshes with `publish` rather than `checkpoint`,
because it needs the writes visible and not durable. `EXT.3` moved from 13.5x
to 0.76x. It also moved the mixed YCSB workloads against LMDB from 0.07-0.14x
to 18.9x on A and 18.4x on F -- but do not read those as wins. They are
unmatched: LMDB commits durably on every batch there and Supdb does not, and
`ext-ycsb` emits no cross-engine finding, so nothing gated them. EXT.9 prices
that difference at 91x on a 1000-op batch, and YCSB batches at 100, so a
matched YCSB-A is not merely slower, it is currently unrunnable at `full` --
which is itself the finding. YCSB-E is the one still losing even unmatched, at
0.43x, because publishing rewrites index structure in proportion to the key
count rather than to what changed.

Two of the four above have moved from `fails` to `holds`. F2.2: reader open is
sub-linear in key count since the key index became a mapped section
(`src/flatindex.rs`, `Options::flat_index`). F4.2: a usable durability point
exists since block compression was turned off by default (`f12-compress`
prices that at 3.6x on reads, 30x on scans, 3.8x on writes, for 1.04x the
disk). F2.1 still fails — sub-linear is not independent — and so does F4.1, at
38x.

The compression change also took the size axis away: `EXT.6` moved from `holds`
to `fails`, since Supdb stores 168.6MB where LMDB stores 126.9. That was traded
knowingly, and scans are what it bought — `EXT.5` went from 4.7x slower than
LMDB to 0.96x of it, which is `no_difference` at p=0.37 rather than a lead.
Two earlier versions of that sentence were wrong in the same way twice: it
claimed 1.29x from a `ci` run, which is never citable, and then 0.65x from a
`full` run whose result file had since been regenerated underneath it. `verify`
compares the recorded verdict against `expect` and never reads the prose, so a
number quoted in a `because` can rot for as long as nobody re-derives it. When
you cite a figure here or in `claims.json`, read it out of `results/` first.

Scan is the one axis where Supdb and LMDB cannot be told apart *warm*, and it
took a methodology fix to see that. Cold they can: `EXT.12` scans at 0.785x
with checksums equalized. The two suites measure different things and both are
right -- `ext-sweep` builds one store per engine and sweeps it repeatedly, so
it walks a warm structure, while `ext-kv` loads a fresh store per repetition
and scans it once. Do not average them. `ext-sweep` used to decompose scan cost by fitting
`a + b*n` over lengths 1..400 and report both coefficients: `EXT.7` had Supdb
the faster walker and `EXT.8` had it paying the larger constant. The marginal
cost of an entry falls from about 89ns to 15 over that range before settling
near 20, and a straight line through it lands its intercept *above* the measured
cost of a one-entry scan — 952ns of "fixed cost" for a scan observed to finish
in 692, and the same for LMDB and redb. Both quantities are now measured rather
than fitted: the floor is the observed n=1 point, the per-entry cost is the
difference quotient between the top two lengths. Measured that way neither axis
separates the engines, `EXT.7` moved to `fails` and `EXT.8` to `holds`, and all
three scan measurements finally agree. A model is a claim about the data and
belongs under the same gate as everything else; `full_range_fit` stays in every
`ext-sweep` record so the refuted one is visible rather than deleted.

The external suite repeats and interleaves its engines, like everything in
`src/bench/`. It did not always: it ran each engine once, and `EXT.1` read
0.70x, 1.03x, 0.998x, 1.13x and 0.85x across five such runs, flipping between
holding and failing on margins as small as 0.2%. Seven repetitions settle it at
0.866x with p=0.0106. If you add an engine or a metric there, it repeats too.
