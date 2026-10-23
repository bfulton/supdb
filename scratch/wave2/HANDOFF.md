# Handoff: out-of-core work on the compact form (zpages)

Branch `claude/recover-consolidate-branches-5rwvr5` of `bfulton/supdb`. The
last engine commit is the one that added the section "This session: a block
read through one cursor" below (`git log -1 -- src/db/zfile.rs` names it),
pushed, with the full gate (`sh scripts/check.sh`, every group) green on
its tree and the two fuzz ranges below green on its binary. The session
before it ended at `92a5f35` (the page view read once), and the one before
that at `d5114b6`; their records are the sections after "Where the
out-of-core comparison stands".

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
install (the "gear shift").

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
- **The memo-size drafts** stay under `next/` (`next/README.md`: how to
  run them, and the dry-run trap). The lever C drafts there were superseded
  by this change and removed.
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
