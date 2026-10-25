# Handoff: out-of-core work on the compact form (zpages)

Branch `claude/recover-consolidate-branches-5rwvr5` of `bfulton/supdb`. The
engine commits of this session are the block read through one cursor, the
memo's size counted and paired, the record coding (a form file's groups
holding their records one by one, now an arm) and the commit that made it
the arm (the section "This session" below; `git log -- src/db/zfile.rs`
names them), each pushed with the full gate (`sh scripts/check.sh`, every
group) green on its tree and the two fuzz ranges below green on its binary;
between them the probes of the per-record form (`bench/examples/zrec.rs`). The session before ended at
`92a5f35` (the page view read once), and the one before that at `d5114b6`;
their records are the sections after "Where the out-of-core comparison
stands".

## Standing instructions

- The user's direction: "fix correctness completely, then get back to
  out-of-core improvements and measurements", and for the levers: "keep it
  going, see what we can get; make sure we don't introduce penalties to the
  cells already won".
- The user's preferences: be logical, test locally, don't speculate, dive
  deep, no shortcuts.
- Commit only after a green `sh scripts/check.sh` (all of build, test, lint,
  wasm, bench), then push to the branch above. No PR unless asked.
- Read a pairing only as a whole table in both orders. Rates under about 10%
  are dominated by position effects. The deterministic per-mix counters are
  the evidence; two sittings of a phase column are the verdict when only that
  phase can move (`CLAUDE.md`, Profiling).
- Commit trailer: the two attribution lines the harness gives the session
  (`Co-Authored-By:` and `Claude-Session:`), and no model identifier
  anywhere else -- not in commits, code or docs.

## Environment (a fresh container)

- Run `sh scripts/deps.sh` first (wasm target, libclang).
- Compiles go one at a time with `CARGO_BUILD_JOBS=1`: the tool cgroup is
  about 13.4 GiB, and a second checked compile gets its `rustc` killed. The
  checked build of `tests/db.rs` takes about 25 minutes; iterate in the dev
  profile.
- Give every test or fuzz run its own `TMPDIR` and clean only that. A shared
  `TMPDIR` cleanup removed live stores from under the `db` tests and read as
  eight failures.
- The disk allowance is tight (about 5 GB free). Delete old bench binaries and
  `$TMPDIR/supdb-next-*` before reading a link failure as anything else.
- Never run a timing pairing beside a compile, a test or another pairing.

The gate chain used (it was in the scratchpad, recreate it):

```sh
export CARGO_BUILD_JOBS=1 TMPDIR=$SCRATCH/tmp-chain
for g in build test lint wasm bench; do
  sh scripts/check.sh $g > $SCRATCH/logs/gate-$g.log 2>&1 || { echo "$g RED"; exit 1; }
done; echo "chain green"
```

Afterwards, check `--list` or grep the test log for the new tests' names, so
the verdict covers them.

The stress used for races (four processes, one seed, counting failing runs):

```sh
# stress-seed.sh <fuzz binary> <seed> <procs> <iters>
for p in $(seq 1 $P); do ( d=$SCRATCH/tmp-stress-$p; mkdir -p $d; f=0
  for i in $(seq 1 $N); do rm -rf $d/supdb-next-*
    TMPDIR=$d ZFUZZ_ZPAGES=1 ZFUZZ_CACHE=all ZFUZZ_PROMOTE=1 ZFUZZ_SEEDS=$SEED $BIN > $d/out.log 2>&1
    grep -q "FAILURES [1-9]" $d/out.log && f=$((f+1)) && cp $d/out.log $d/fail-$i.log; done
  echo "proc $p: $f of $N" ) & done; wait
```

Build the fuzz binary with `cargo test --profile checked --test fuzz
--no-run` (about 1.5 minutes); it lands at `target/checked/deps/fuzz-*`.

Fuzz ranges to rerun after any change to the settle, the forms or the carry:

- `ZFUZZ_ZPAGES=1 ZFUZZ_CACHE=all ZFUZZ_OPTVARY=1 ZFUZZ_FROM=0 ZFUZZ_TO=300`
- `ZFUZZ_ZPAGES=1 ZFUZZ_CACHE=all ZFUZZ_PROMOTE=1 ZFUZZ_FROM=0 ZFUZZ_TO=120`

Both are green at `d5114b6`.

## Where the out-of-core comparison stands

These figures come from `scratch/wave2/stage4.md`, "Against the comparators
out of core" (5M keys under 640 MiB):

- **The forced compact form against LMDB (readahead off):** it wins every
  cell but E, which reads at 0.55x. E's loss is CPU, not faults: about
  12.5 µs an operation against LMDB's 7, with no faults on the form's side.
- **The controller (`ZPages::Auto`, what ships):** it buys at 23-32 s and
  installs at 56-71 s. The passes before the install run on the raw form and
  trail (reads about 0.52-0.67x, scans 0.26-0.64x). After the install it wins
  every cell but E.
- **RocksDB as tuned:** it is killed by the cgroup at 640 MiB. At 10M keys
  under 1 GiB the form takes every cell.

The open cell is E. A secondary one is the minute of raw reads before the
install (the "gear shift"). The per-record form, probed and built below as
the record coding, took the point reads' group decode away and not E's: a
scan's memo miss still decodes a group whole, and through its records that
costs more, so the coding is an arm. The profile at `3a885a9` (below) has
E's remaining cost spread over the decode (24%, at the cold decoder's
figure, which is its branches and not its lines), the start seek (20%, a
third of it decodes wasted on copy starts), the walks, the builds and the
settle, none above a quarter; the one lever of size left on the decode is
liblz4 (its high-compression mode and its decoder), a C dependency and the
user's call.

## This session: a block read through one cursor

- **Committed: `Options::zpages_block_cursor`** (on; the arm
  `supdb-cache64tofitautozpagesperrun` is the shape before). A block's
  build or walk over a covered partition reads the partition through one
  cursor held across the block (`db::PartReader` over `zfile::ZCursor`):
  the runs between the overlay's keys by the cursor's walk, each key's cut
  where the walk stops at the key, a sparse build's cuts by the cursor moved
  to each key from the group it holds (`ZCursor::skip_to`, with the
  last-key memory that answers an ascending batch from the position), and
  the record an overlay key replaces from the cursor standing on it. The
  shape before opened a cursor per run (`cache_scan`), sought every cut from
  the fence (`cuts_of`, `filed_cuts`, `cache_cut`) and located every
  replaced record (`cache_values`), each opening the key's group again --
  `2k + 5` opens a block of `k` overlay keys for the four or five the walk
  needs. Under the option `cuts_of` and `filed_cuts` resolve nothing for a
  covered partition (`Over::cut == u32::MAX` means "the build searches"),
  and the pieces-only sparse walk (`walk_deltas_over`, the
  `forms_pieces_only` arm's path) orders its two lists by key instead of
  by cut, which is the same order without a seek per turn; the arm keeps
  that order too. The reader's walk (`ZCursor::walk_run`) leaves a group
  it spent at its limit to the next read (`ZCursor::resume`): the first
  pairing read E's groups decoded up 0.7% for the next group opened at
  every run that ended on a group's last record. The overlay walk's
  `walk_until` keeps the cursor on the next group's first record, as its
  test holds and `scan_partition_cursor` reads it.
- **The counts** (`stage4.md`, "A block read through one cursor";
  `zblockcur-5m.sh`, `zblockcur2-5m.sh`): E's walks the counters see
  104,943 a pass before, 57,904 after; memo asks 316,780 before, 256,593
  after the first version and 254,358 after the lazy end; groups
  decoded 147,098 before, 148,162 and then 147,097; 24,007 blocks
  built in all. Every other mix's counts are identical, since only E
  builds blocks. The rates, by the mixes' own times over twelve pairs in
  both orders: E's scans 0.923 of the shape before in the first pairing
  (2 of 12 slower; 0.900 and 0.945 by sitting) and 0.977 in the second
  (5 of 12; 0.968 and 0.985), for the change in all four sittings; C,
  which builds nothing, 1.000 in the first and 1.003 in the second, the
  pairing's control; the update mixes' commits 0.93-0.94 in the first (a
  commit's fill of its batch's blocks is a build) and both ways in the
  second; the floors from identical code 0.96-1.08. The pairing reader is
  `scratch/wave2/pairing_read.py`.
- **Tests**: the cursor moved to a key against the seek a key (every
  record's key, the keys between, below the first, past the last; in key
  order with one memo ask per compressed group, and in reverse order); a
  cursor onto a damaged page falls back after the keys before it; a walk
  that ends at a group's last record opens no next group; the windows over
  a covered store under the arm; the pieces-only overlay model with the
  compact form (`tests/db.rs`), which is the one test that reaches the
  by-key merge's equal case (the windows tests and forty fuzz seeds let a
  mutant of it through); the fuzz turns the cursor off in some seeds under
  `ZFUZZ_OPTVARY`. Five mutants were run, four of the new code and one of
  the lazy end, each killed by a test in the gate.
- **The rule this session adds**: count a path's opens as a block pays
  them -- the walk's, the cuts' and the replaced records' together -- before
  pricing any one of them, since the profile named the cuts (5.9%) and the
  opens the walk itself repeated were the larger part; and a change that
  holds a group across a walk is read in the decodes as well as the asks,
  since an open made for the next read is a miss as often as not.
- **Committed: `Options::zpages_memo_bytes`** (eight mebibytes, as the
  memo was) with the arms `supdb-cache64tofitautozpagesmemo4`, `...memo16`
  and `...memo32` (`stage4.md`, "The memo's size, counted"; `zmemo-count-5m.sh`
  for the counts, `pair-5m.sh NAME A B` for any pairing). By count a
  sixteen-mebibyte memo decodes 8.3% fewer groups in E and 11.5% fewer in
  C, thirty-two 14.4% and 15.8% fewer (E's hit rate 42%, 47%, 50%), the
  point reads' major faults level. Paired, five pairs a sitting in both
  orders: sixteen moves neither E (0.997, the sign with the order) nor C
  (1.007) and reads D 1.065 slower in both orders (the uniform reads hit
  the memo once in eighty asks and pay its footprint); thirty-two reads C
  1.115 slower in both orders (the mix that saved the most decodes), D
  1.066, E level (1.026, the sign with the order); four
  (`...memo4`, added to read the other side) decodes 5.5% more in E and
  reads E 1.078 slower in both orders, C level. Eight is at the knee and
  stays. The reading: the memo pays for groups that come back while still
  warm in the last-level cache, the Zipfian head's, and a hit on a cold
  group costs the walk its thirty-two lines from memory, about what the
  decode would have, while the larger memo's footprint costs the point
  reads besides; a cycle counter could settle it and this guest has none
  (`stage4.md` names the micro-probe that would). The memo's size is not
  a lever on E.
- **The drafts under `next/`** are gone: the memo-size ones landed, the
  lever C ones were superseded. `next/README.md` says what the counts point
  at next and how a draft is applied (never dry-run by disarming its edit
  function).
- **Probed: the per-record form** (`bench/examples/zrec.rs`; `stage4.md`,
  "The per-record form, probed against the comparators"; the runs
  `zrec-incore-5m.log`, `zrec-bound-5m.log`, read by `zrec_summarize.py`).
  The user asked for a quick test that the faster format beats LMDB and
  RocksDB on a cell the engine loses. The probe writes the suite's records
  as groups of sixteen with the first key raw, the keys front-coded, each
  value LZ4-compressed on its own against a dictionary of the first values,
  and an offset table a group, and runs read-only mixes (uniform reads,
  Zipfian reads, Zipfian scans of fifty) against it, the forced group form,
  LMDB (readahead off) and RocksDB in one process, three rounds, every arm
  answering the same bytes. The form is the group form's size (362 MB at
  five million keys). Under 640 MiB it reads 1.80, 0.75 and 3.68 µs an op
  with no faults, against the group form's 5.83, 1.78 and 6.39, LMDB's
  15.41, 5.61 and 8.37 (0.19, 0.06 and 0.07 faults an op) and RocksDB's
  22.0, 11.3 and 36.5 at default options: 0.12x, 0.15x and 0.44x of LMDB's
  time, 0.31x, 0.42x and 0.58x of the group form's, in every round. In
  core it is level with LMDB on point reads (1.12x, 1.01x) and half its
  speed on scans (1.92x), fifty decodes a scan. Caveats, stated in the
  record: the group form already wins these read-only cells against LMDB
  under the bound (the probe reads it at 0.33-0.77x); the cells lost, E and
  the controller's pre-install passes, are mixes with inserts and unsealed
  keys the probe does not run; what the probe measures is the form's own
  cost per read and per scanned record, which the profile's shares (decode
  26.5%, the scans' start seeks 22%) name as E's lever on the form's side,
  and what it buys E is for the suite to measure once the form is an
  engine option. The build in the probe is slow (fifteen microseconds a
  record, the dictionary's table rebuilt per call) and that is the probe's.
- **Probed: the dictionary and the value coding** (`stage4.md`, "The
  dictionary and the value coding, probed"; `zrec-shapes-{suite,drift}-1m.log`).
  Seven shapes of the form at a million keys in core on the suite's payload
  and on a drifting one (`--payload drift`: a motif of its own every two
  hundred records). The segment dictionary (`zrec`, 72.3 B a record on the
  suite's payload) compresses nothing on the drifting payload (108.6, the
  raw size); the group's own first record as the dictionary (`zrec-local`)
  holds both at 80.5 and 75.6 against the group control's 72.3 and 70.1;
  the two together (`zrec-hybrid`) read 75.0 and 75.6. Point reads of every
  per-record shape are level with LMDB in core (0.9-1.1x) against the group
  control's 1.3-2x; a scan's time over LMDB's is its decodes alone (raw
  copies 1.13x, a hot hundred-byte dictionary 1.6x, a sixty-kilobyte one
  1.8-1.9x, whole groups 2.1x). Chosen for the build: LZ4 per record (raw
  values cost half again the bytes, which out of core are faults) with the
  group's first record as the dictionary, no segment dictionary needed; the
  segment dictionary appended to it is the arm, a trailer slot that an
  empty dictionary leaves as the local shape.
- **Built: the record coding** (`Options::zpages_record_coding`, on;
  `stage4.md`, "The record coding in the engine"): a group's records one by
  one, each decodable alone, the choice per group in the directory entry's
  spare bit, the trailer at version 2; a point read of a verified page seeks
  the heads and decodes one record (`ZFile::locate`), scans decode groups
  whole through the memo as before; the block coding is the arm
  (`supdb-cache64tofitautozpagesblock`, `supdb-nocachezpagesblock`). The
  sizing tool at 300k keys: the form 14% larger (87 against 76 B a key),
  the fill 1.5x slower, a group decoded whole through its records 2.4x the
  block's hot and 1.6x cold, a point read's one record about a sixth of a
  group. The plan's per-page coding was wrong (a 5,000-byte key the block
  compresses and a head never is) and so was the memo test's invariant for
  this coding; both are in the record.
- **Paired, twice, and made the arm** (`stage4.md`, "The record coding
  paired, and what E's loss was made of" and "... paired again, and made the
  arm"; `zcoding-ab-{1,2}.*`, `zcoding2-ab-{1,2}.*`, `pairing_read.py`). Over
  twelve pairs in both orders the record coding reads the point-read mixes'
  gets at 0.68-0.79 of the block coding's time, never slower in a pair, and
  E's scans at 1.24, slower in every pair; the write mixes' commits read
  7-33% slower in the first pairing and level in the second, once a page's
  first touch verified a coded group by its heads (`zpages::check_coded_group`,
  `zfile::first_checks`) instead of decoding it. E's loss is the memo
  misses' decode of a group through its records, 2.2x the block decode's
  instructions by callgrind and structural (a field parse, three copies and
  a decoder call a record against LZ4's bulk copies); a decoder and an
  encoder shaped for a record, held to the library byte for byte, were
  slower than the library and were removed. So `Options::zpages_record_coding`
  is off and the arms are `supdb-cache64tofitautozpagesrecord` and
  `supdb-nocachezpagesrecord`; the shipping default is the block coding as
  it was, under the per-group bit and the trailer's version 2. The shape
  that would serve both paths -- one block a group whose records reference
  only the first, an entry a record into it, the heads beside it -- is
  named in the record and not built: it would hold E level, not win it.
- **The quick row in core at `5059187`** (the suite's own gate, eight prior
  rows of the class, the newest at `5df834c` of October 20): passed, the host
  out of band (the memory floor at 0.73 of the window's lowest median) so the
  machine-bound quantities got no verdict; nothing regressed that is the
  engine's; three quantities better than every prior row, flagged for a look
  -- `supdb-ingest`'s E at ten thousand and a hundred thousand keys (1.03-1.48M
  ops/s against the window's at most 0.97M) and `supdb-nocache`'s threaded
  scan mix at thirty thousand -- which are the E levers that landed after the
  window's last row (the settle's cut left to the build, the block read
  through one cursor) showing in core, on a host slower than the window's,
  and not a broken measurement; the record coding is inert on the quick row,
  whose stores fit and build no form. The row is in `bench/runs-ci/`, not
  banked (the controls were out of band).
- **E profiled again at `3a885a9`** (`stage4.md`, "E profiled again, the
  decode's lines probed, and two levers on the lines a read touches";
  `scratch/wave2/prof/profile-e.sh`, the samples' text now made with
  `perf script --no-inline`, since the inline resolution ran at two
  kilobytes a second): the thread is on its core for all of E at 7.8 µs
  of CPU a scan, 1.6 of it the harness's; the decode 23.8% at exactly the
  cold figure (1.0 µs a group against 0.27 hot), the start seek 20% (a third
  of the decodes, wasted on the two thirds of scans that start in a copy),
  the block reader's walks 15%, the clean runs 12%, the builds 10%, the
  settle 7.5%. Nothing decisive is left in one place; the remaining levers
  are each three to ten percent.
- **Probed and refuted: the decode's lines asked for ahead**
  (`bench/examples/zlines.rs`, `zlines-probe-1m.log`,
  `zlines-probe-out-1m.log`): a prefetch of the group's payload at the view,
  of the page at the fence, of the output buffer -- 9% off a cold decode at
  most, the whole page slower. A cold decode's cost is the decoder's
  branches on tokens it has not learned, not its memory; `CLAUDE.md`
  (Profiling) carries the rule.
- **Probed, open for the user's decision: liblz4 in the engine**
  (`zdecode --hc LEVEL`, `zlines-hc-1m.log`): liblz4's high-compression
  mode at level 9 shrinks the form 4% and the cold decode 16.5% (fewer
  tokens), at 2.3x the build's CPU; liblz4's decoder is 0.84-0.86 of
  `lz4_flex`'s on cold input, and over HC payloads 0.71 of the engine's
  decode as it stands. Both are a C library the engine does not link (the
  bench's `lz4` crate, feature `lz4-c`, is how the probes have it); the
  compressor would be the builder's and the native build's alone. Decode is
  24% of E and 27% of C's CPU, so the pair is worth about 7% of E and more
  of the point reads in core. Not built: a dependency decision.
- **Built and priced: the fence's search tree (refuted, the arm) and the
  memo's victim hint (on)** (`stage4.md`, the same section; `zlines-fence-ab-{1,2}.*`,
  `zlines-memo-ab-{1,2}.*`, the arm pairs' profiles under
  `scratch/wave2/prof/` as `ycsb-E-<arm>.txt`). `Options::zpages_fence_tree`
  (`ZFence::index`): a breadth-first tree over the entries' words, one line
  a level prefetched three ahead, whole keys compared only where words tie
  -- built for the fence's search at 4.7% of E's thread, read as misses. The
  pairing read level everywhere with mixed signs and the arm-pair profile
  read the search's own samples up (4.2% against 3.4%): the store's fences
  are nineteen of 4,900 entries, L2-resident for the partitions a Zipfian
  reads, so the search was never miss-bound and the tree is instructions
  added. Off; `supdb-cache64tofitautozpagesfencetree` is the arm, on in a
  quarter of the fuzz's `ZFUZZ_OPTVARY` seeds, held to the search over the
  keys over every shape of fence and key. `Options::zpages_memo_ahead`
  (`Memo::prefetch_victim`): a decode asks for the line of the memo entry
  its put will displace; the arm-pair profile has `Memo::recycle` at 0.25%
  of the thread against 2.0% without it, the miss gone, and the pairing
  reads the memo-heavy mixes 3-6% for it in two thirds of the pairs and no
  cell against it (E's two sittings disagree in sign, the host's). On;
  `supdb-cache64tofitautozpagesmemonoahead` is the shape before. Two rules
  went to `CLAUDE.md`'s profiling notes: a cold decode's stall is its
  branches, and a self time is misses only once the structure's size is
  read against the cache per instance. Also: `perf record -p` attaches some
  200 ms after its mark, so the C mix's windows (130-200 ms) hold the tail
  of C or nothing; a point-read profile needs perf attached before the
  pass.
- **The settle counted, the span arm re-paired, the start cache refuted**
  (`stage4.md`, its last three sections). The per-mix events line carries
  the settle's counts (`settle_runs/keys/blocks/rebuilds`,
  `lazy_cuts_filed/resolved`; `Db::settle_density`): E's settle is 43 runs
  a pass filing the 49,000-key backlog the mixes before it leave, 1.1 µs a
  key, no piece over 2% of E, not a lever. `supdb-cache64tofitautozpagesspan`
  paired again in two sittings read E 0.990 (0.940/1.043): level, off. The
  start cache (`Options::scan_start_cache`, `db::StartCache`), a scan's
  start key kept with its block against the partition's blob identity,
  built for E's Zipfian starts (half among four thousand keys), paired
  level (E 0.981 in both sittings), and the counts and the arm-pair profile
  said why: 15,800 hits of 79,200 starts (63,000 puts churn 4,096
  direct-mapped slots), the decodes unchanged because every skipped seek
  was a memo hit, the seek's subtree down 4% of its samples and the scan's
  own up as much in the cache's lookups. Off;
  `supdb-cache64tofitautozpagesstartcache` is the arm, on in a quarter of
  the fuzz's `ZFUZZ_OPTVARY` seeds; `tests/zpages.rs` holds a store with it
  to one without, and `tests/fuzz.rs` draws it. The bound for the family: a
  cache by key reaches at most the 53% of E's starts that repeat, one by
  block range about 78%, and either saves the seek only where the block is
  a copy -- the hot blocks, whose seeks are the cheap ones.
- **The fuzz's red at seed 242, a bug older than the change, fixed**
  (`stage4.md`, "The fuzz's red on this tree"): the scan snapshot's arena
  answered a batched reservation of no bytes -- the empty key alone in the
  live table -- with its tail and no block, and the read of those bytes at
  the next partition's fence asked for the block: the upkeep thread
  panicked and the store's drop aborted the process, four runs in five at
  the seed alone. `SnapArena::reserve`, `write` and `slice` take no block
  for no bytes now; `db::snap_arena::no_bytes_take_no_block_and_read_as_none`
  and `tests/settle.rs`'s
  `a_snapshot_over_the_empty_key_alone_is_sought_at_a_partitions_fence`
  hold it, the second red on the guards removed. The attribution's method
  is the rule, now in `CLAUDE.md`: a draw added to the fuzz moves every
  seed's operations, so a failure that appears with a new option is
  attributed by running the commit before with the same draws (`$S/wt-head`,
  its fuzz patched to consume the draw, its own target), not by the
  option's name -- the start cache was off in the seed and the commit before
  failed four runs in six.
- **E against LMDB, the budget and the kinds** (`stage4.md`, "E against
  LMDB: the block cache's budget, refuted as the lever" and the kinds
  after it): `supdb-cache128tofitautozpages` and `supdb-cache256tofitautozpages`
  are the combined arm with the budget at 128 and 256 MB; paired at five
  million keys under 640 MiB the 256 MB arm reads E 0.975 over twelve pairs
  with every count identical to the default's. The events line carries the
  builds by kind, the covered blocks' second-touch copies and the sheds now
  (`builds_copy/sparse/wide/clean`, `recopies`, `sheds`; `Db::build_kinds`):
  E's 24,007 builds a pass are 14,789 clean, 8,890 sparse, 327 copies, one
  wide, 0 recopies, 0 sheds. The hot head is 327 copies walked 78,600 times
  a pass with no decode; the warm middle 8,890 sparse blocks built at first
  touch (7.7% of E) and walked 3.4 times a pass through the form; the cold
  tail 14,789 clean blocks walked 1.2 times. In core the same block cache
  reads E at 1.36-1.48x of LMDB (the full row of 29 September, 300k-3M
  keys), so the deficit under the bound is the form's decode under every
  walk that is not the head's, plus the sparse builds. The process holds
  77 MB anonymous beside 520-560 MB of file pages under the 671 MB limit;
  LMDB there is in core by minor faults. Next: a stored-values group coding
  (keys front-coded, about 530 MB at five million) as the lightest coding
  that fits the bound, the forced and the combined arms paired under it.
- **Where E's time is, by the arm pair's profile at the head**: the LZ4
  decode 25% of the thread (147,000 groups a pass, 2.5 a form walk, nearly
  every one a memo miss, since the form walks are the Zipfian's cold tail),
  the start seek 21% with its decodes inside it, the record walks about a
  quarter (`walk_run` 12%, the clean runs 12%), the builds 11%, the settle
  7%, the harness's generator and clock about a sixth. The levers left are
  on the decode. liblz4 is one (above, the user's call). The other is a form
  that compresses less: the suite's values are half motif and half entropy
  (`workload::Payload`), LZ4 takes them to about 68 bytes of 100 and the
  form files to 360 MB where the raw files are about 650, and a form with
  the keys front-coded and the values stored would be about 520 MB -- it
  fits the five-million rung under 640 MiB with little room and not the
  next, and its group decode is a copy, or a read in place with no memo.
  The controller chooses forms by their measured faults already, so a
  lighter form is a second form for it to choose when it fits, not a
  replacement. Not built: the user asked why compress at all, and this is
  the answer's shape.
- **Untouched**: the shuffled-load kill of the combined arm under 640 MiB
  and the gear shift, as the session before left them.

## The session before: E profiled, the page view read once (at `92a5f35`)

- **E profiled as it stands** (`stage4.md`, "E as it stands, profiled, and
  the page view read once"): on the combined arm at five million keys under
  640 MiB, E's main thread is the LZ4 decode 26.5%; the lookup machinery
  around it about a third (`view_checked` 8.5%, the fence's `entry_of` 5.2%,
  `open_group`'s own 4.7%, `group_seek` 2.2%, the memo's `recycle` 2%); the
  walk and emit a fifth; the harness's Zipfian generator and clock an eighth
  (LMDB pays the same); the one-off settle and builds a seventh. By caller
  `ZFile::seek` is 27% inclusive (the scans' start seeks 22%, the builds'
  cuts 5.4%), `materialize` 10.3% with `cuts_of` 5.9% -- a seek from the
  fence's top per snapshot key over a block. The tooling is committed:
  `prof/profile-e.sh` (perf attached to the harness at each mixes pass's
  start, detached at E's end; set `SCRATCH`) and `prof/aggregate.py` (the
  window cut by the phase marks, self and inclusive by symbol, callers by
  substring; `c++filt` demangles the v0 names). perf slows E about 1.3x, so
  read shares, not times. The aggregated profile is `prof/E-cache64-*.txt`.
- **Committed: `Options::zpages_trusted_views`** (on): a verified page's
  directory read trusted (`zpages::view_trusted`), one view per page open,
  the settle's lazy filing from the entry before (`ZFence::entry_of_from`);
  arms `supdb-cache64tofitautozpagescheckedviews` and the forced
  `supdb-nocachezpagescheckedviews`. **`Options::zpages_memo_first`** (off):
  the memo asked before any view, the entry carrying the group's first and
  count; arm `supdb-cache64tofitautozpagesmemofirst`. Exact count
  (`zviews-callgrind.txt`): a point read 5,018 against 5,315 instructions
  (-5.6%), the views 144 against 437; an E scan on the merge path 78,167
  against 80,119 (-2.4%). Pairings: both on against the shape before
  (`zviews-5m.sh`, `zviews2-5m.sh`, four sittings) read the counts identical
  and the rates 1-4% against on every mix, 44 of 72 pairs, inside a floor of
  a tenth; the trusted views alone (`zviews3-5m.sh`, five pairs a sitting,
  two sittings) read C and E for (0.98) and A, B, D, F against (1.05-1.07),
  37 of 60, the pairs swinging a quarter. The lean is left with the memo
  asked first, which reorders two misses; the trusted view reads the same
  bytes in the same order. If the host quiets, pair `supdb-cache64tofitautozpages`
  against `...checkedviews` again before building on it.
- **The rule this session adds** (`stage4.md`'s section ends with it): a perf
  share attributed to a function is partly the miss that function is the
  first to touch, so a share cut out by instruction is not a share of time
  saved until a pairing says so; and a change that reorders two loads is
  priced in time as well as by instruction.
- **Drafted, not applied** (`next/README.md`): lever C, a block's snapshot
  keys cut by one cursor moved forward key by key (`cuts_of` is 5.9% of E;
  the raw path already cuts this way); the memo's cap as an option with arms
  at 16 and 32 MiB, to count the decodes a larger memo saves before pricing
  the page cache it takes (E's memo hits 54% over four looks a scan; the
  static optimum for 8 MiB over this Zipfian is about 66%, for 32 MiB about
  76%); then an admission policy for the memo, which the design's 3.7 names
  as the arm if the hit rate lands under 0.45.
- **Untouched**: the shuffled-load kill of the combined arm under 640 MiB
  (`stage4.md`, "Against the comparators out of core") -- a census sampled
  through the load is the instrument it needs; and the gear shift, which the
  eager entry's sittings (`stage4.md`) left as it stands.

## The session before: its commits and verdicts (at `d5114b6`)

- **`dda952d` / `b5ea3ae`: the page as a container of groups.** A point read
  decodes one group. Kept, with the pairings in stage4.md.
- **`a02cc62`: the memo's slots hashed** (`Options::zpages_memo_hash`, on).
  Decodes fell 12-13% in C and B and 5% in E; the rates are unresolved.
  `supdb-cache64tofitautozpagesmemolinear` is the shape before.
- **Two levers refuted, kept as off arms in the same commit:**
  - `zpages_prebuild` (`supdb-cache64tofitautozpagesprebuild`): the form files
    built ahead once the store passes its budget. The install came 4-5 s
    earlier, but the raw read pass read 0.65x because the builds read from the
    device beside its faults.
  - `zpages_seek_span` (`...span`): a scan's block taken from the fence and
    the directory, skipping the rank seek. Level; the skipped seeks were memo
    hits.
- **`55d5592`: the settle's cuts left to the builds**
  (`Options::zpages_settle_span`, on; `...exactcut` is the shape before). E's
  settle went from 64-85 ms to 39-44 ms in all 16 reps, and group decodes fell
  12%. E's pass is unresolved (spread about 40 ms against 27 saved). No other
  mix runs the settle. The builds resolved none of the deferred cuts, because
  E walks the blocks wide and a wide walk treats `LAZY_CUT == u32::MAX` as the
  overlay's unknown cut and places the key by key.
- **`d5114b6`: a race fixed (correctness).** `rebase_tables` deferred the
  retraction of a carried form its table holds none for (it marked the block
  dirty), and the settle in the same look tripped "a block the writer holds no
  form for has none published as the block". The fuzz showed it under the
  promotion arm, about 4/100 under four-process load. With
  `commit_forms_build` off it is also a wrong answer for a handle: by code
  reading, not tested. The look now publishes the marker at once. The new test
  `a_form_dropped_under_a_landings_carry_is_retracted_at_the_writers_look`
  uses the seam `Reader::forget_form_unretracted` (plus
  `Reader::published_kind`); it fails on the old look and passes now. The
  stress went from 9/220 to 0/400. The cause is written into the existing
  `CLAUDE.md` shape entry "A form dropped after the copy stood as the block".

## E: what was known after the session before (at `d5114b6`)

The pairing: 5M keys, `--bound 671088640 --ops-div 10`, three reps after the
warmup.

- E's mix is about 500-590 ms: scans 460-535 ms, commits about 26-47 ms.
- The scans' phases (`mixcounters`): take about 2.6, sync about 5, settle
  about 40 (was about 68), snap about 8, install about 2.7 ms.
- Walks `78631/30410/18321` and starts `50978`; groups decoded about 147k (was
  about 168k). The memo hits are in the `memo` field.
- The blocks E builds are walked wide (`Cached::Wide`), by key.
- Earlier profile (stage4.md, "E's start seek skipped..."): the time is in the
  walks and the decodes, not in the seek.

Candidates for the next lever, none priced yet:

1. Profile E's scans as they stand now with `perf record -e cpu-clock`
   (`/usr/lib/linux-tools-6.8.0-31/perf`, after `apt-get install
   linux-tools-6.8.0-31`). Cut the samples to E's window from the
   `phasemark` lines (`SUPDB_LAG_MARK=1`, `perf record -k CLOCK_MONOTONIC`)
   and to the scanning thread. The stack-aggregation scripts were in the
   scratchpad and are gone; rewrite them from `perf script` output.
2. Whether a block E walks wide should become a copy sooner. The promotion
   arm (`zpages_promote_copies`) was refuted on rates; read why in stage4.md
   before retrying a variant.
3. The decode count in E (about 147k groups for 79k scans): the memo's
   capacity against E's Zipfian head, since the unscrambled Zipfian keeps the
   hot keys contiguous (`CLAUDE.md`, "A walk that reopened...").

## How the pairings were run

The script for the last pairings is `scratch/wave2/zblockcur-5m.sh`
(five pairs a sitting, both orders); copy it for a new pair of arms. It:

- builds `bench` in release;
- unsets every instrument variable except those it wants, so both arms carry
  the same instruments;
- runs `bench ab --arms A,B` and then `B,A`, with
  `SUPDB_MIX_COUNTERS=1 SUPDB_MIX_KINDS=1 SUPDB_MIX_EVENTS=1 SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1`
  and `--passes read,read-threaded,scan,scan-threaded,mixes --pairs ycsb`.

It writes `NAME-ab-{1,2}.out` (the table) and `.err` (per-rep lines).

- Read the `.err` per arm by the arm name each line carries, never by parity:
  `scratch/wave2/pairing_read.py A B NAME-ab-1.err NAME-ab-2.err` prints, per
  mix, the two arms' own times paired rep by rep (the geometric mean of B/A
  and the pairs B was slower in) and each arm's median counters, and names
  any count that varies across an arm's reps.
- `mixcounters <arm> <mix> ... ph take/sync/settle/snap ... z reads/dec/groups/memo`
  and `mixkinds <arm> <mix> ... gets N ms scans N ms commits N ms total`.
- Each sitting takes about 14 minutes at five reps.
- In the table, column B is the second arm named, so the arms' columns swap
  between the two sittings.

A new arm needs four things: a field on `Policy`/`Supdb` and its name in
`bench/src/engines.rs`, a roster line in `bench/DESIGN.md`, a test in
`engines.rs` holding the arm to its option, and any counter row it needs.

## Loose ends

- `commit_forms_build` off: `publish_dirty` returns before publishing dirty
  blocks and then moves `forms_at`. Other deferred-dirty paths would leave a
  stale published form there. The rebase's stale branch no longer defers; no
  other deferral was audited.
- The promotion arm stays off as shipped (refuted on rates); its race is
  fixed.
- The perf recordings and callgrind outputs of this session were in its
  scratchpad and are gone; the aggregated profile and the annotate summaries
  are under `prof/` and `zviews-callgrind.txt`.
- The drafts under `next/` for the memo's size: the engine script's `db.rs`
  half writes the file directly, so it cannot be dry-run by disarming its
  `edit` function (`next/README.md`).
- The overlay walk (`scan_partition_cursor`, the scans of a store with the
  block cache off) still opens the next group when a scan's limit falls on
  a group's last record (`walk_until`); `walk_run`'s lazy end is the lever
  there, unpaired, and its caller would read the key under the cursor only
  after a resume.
- The container restarted once during a gate's test group, at the checked
  compile of the engine, with nothing else running; the gate was run again
  from the start. `free` read 15.5 GB free of 16 after it.
- Nothing is uncommitted.
- The suite's `the_removal_arms_differ_from_their_default_in_the_one_option`
  overflows the test thread's stack in the dev profile at `3a885a9` already
  (`cargo test --lib` in `bench/`, before any change of this session); it
  passes under `checked`, which the gate runs, and with `RUST_MIN_STACK` at
  16 MiB in dev. The engine values are small (`Db` 2.5 KB, `Supdb` 2.8 KB),
  so the frame is the debug build's open chain, not the test's values;
  boxing the engines moved nothing. Not fixed here.
