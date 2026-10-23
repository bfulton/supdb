# Handoff: out-of-core work on the compact form (zpages)

Branch `claude/recover-consolidate-branches-5rwvr5` of `bfulton/supdb`. The
last engine commit is `d5114b6`, pushed, and the full gate (`sh
scripts/check.sh`, every group) was green on it. This note is the only
thing committed after it.

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
- Commit trailer (no model identifier anywhere in commits, code or docs):
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>` and a
  `Claude-Session:` line for the session.

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

## This session's commits and verdicts

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

## E: what is known after this session

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

The script for the last pairing is `scratch/wave2/zcut-5m.sh`; copy it for a
new pair of arms. It:

- builds `bench` in release;
- unsets every instrument variable except those it wants, so both arms carry
  the same instruments;
- runs `bench ab --arms A,B` and then `B,A`, with
  `SUPDB_MIX_COUNTERS=1 SUPDB_MIX_KINDS=1 SUPDB_MIX_EVENTS=1 SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1`
  and `--passes read,read-threaded,scan,scan-threaded,mixes --pairs ycsb`.

It writes `NAME-ab-{1,2}.out` (the table) and `.err` (per-rep lines).

- Read the `.err` per arm by the arm name each line carries, never by parity.
- `mixcounters <arm> <mix> ... ph take/sync/settle/snap ... z reads/dec/groups/memo`
  and `mixkinds <arm> <mix> ... gets N ms scans N ms commits N ms total`.
- Each sitting takes about 9 minutes.
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
- Nothing is uncommitted.
