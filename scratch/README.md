# The state at the consolidation of 2026-10-19

The session before this one was cut off with its work spread over eleven
`claude/wip-*` branches beside the mainline; this one recovered the work from
their commits (its own transcript was not available) and brought
everything live onto the one branch this file is on. The mainline it
extends is `claude/performance-dimensions-analysis-gijutv` at `4b112f1`,
476 commits past `main` (`c89f9d3`, 2026-09-06), with no pull request open
or ever opened for it. Below: the workstreams and where each stands, what
the gate says on this branch, what each branch was and what became of it,
and where to start.

## The workstreams

**The second form (z-pages).** `design/zpages.md` is the pre-registered
design; its section 8 is the staged plan and its section 7 the registered
predictions and the verdict rule. On the mainline:

- Stages 0a and 0b: the layout half, `src/db/zpages.rs`, and
  `bench/examples/zsize.rs` over the engine's own bytes (merged `4e19958`).
- Stage 1b: `Options::point_via_ord`, the point read through the ordered
  index, arm `supdb-nocacheordread`, off by default (merged `022d38c`). Its
  pre-registration is `wave1/ordread-prereg.md` with the pairing's command
  lines in `ordread-commands.sh`; no pairing of it is recorded anywhere.
- Stage 1d: the selector's pure core, `src/db/zselect.rs`, wired to
  nothing (merged `71916db`).
- Stage 2a: the forced arm for point reads and counts: `Options::zpages`,
  `ZPages::Always`, the sidecar `zpg-<id>-<end_seq>.zpg` beside each
  partition, arm `supdb-nocachezpages`, `Db::zpages_settle` (merged
  `cd4e774`). Measured in one sitting at ten million keys under a 1 GiB
  bound and written up in `docs/engine.md`, "Out of core: the second form"
  (`4b112f1`, the last commit before the cut-off). The design's verdict
  (7.6, 7.8 a) needs a second sitting with the arms' order swapped; it has
  not been run. In memory the forced form reads at a fifth of the raw
  form's rate, so nothing ships without the selector.
- Not started: 2b (lifecycle: retirement through the epoch, attach at
  promotion, the crash windows, the filter, the warm job), 3 (scans through
  the form), 4a-4c (`ZPages::Auto`: the selector wired to the engine, the
  census), 5 and 6 (the default flip, which 7.8 b gates on the forms-memory
  lead).

**The forms-memory lead** (`ooc/forms-lead.md`): `Options::forms_to_fit`
with its three parts as arms of their own (merged `b8437bd`), and the
commit's maintenance files nothing far from a scan. The condition in 7.8 b
for `Auto` as a default stays open.

**The bounded scale** (`bench run --scale ooc`, `bench/src/bound.rs`,
`bench/scripts/ooc.sh`): merged `63b7a0a`. Four rows were taken at
`ca738ec`, rungs 1 and 2 at three and ten million keys, the arms
`supdb-nocache`, `lmdb` and `lmdb-nordahead` with the floors (`ooc/rows/`);
none is banked under `bench/runs/ooc/`, since committing a row is a
person's decision.

**Also merged on the way**, each with its arm: the drain stamped under both
policies and the presync's error answered by the flush (`fb5a45a`); a
scan's seek hinting its stride window's directory, `Options::seek_prefetch_dir`
(`ca738ec`); the arena's first reservation making its block whatever its
size, the empty-key panic (`e2e1301`).

**Hunt I: a Snapshot handle reads a write made after it.** With the block
cache on, a `Snapshot` handle's first scans answered a write committed
after the snapshot, about one run in two hundred; and the upkeep thread's
`cut_known` debug assertion fires in about 3% of the oracle's runs under
`checked`, after which the lent upkeep's assertion aborts the process. The
cause is not known. The z-pages windows test runs its snapshot half only
with the cache off and says so (`tests/zpages.rs`, "The cause is not known
here"). The brief is `hunt/ctx.txt`; the tooling (forced schedules,
minimal forms over the public surface, an event ring in the publisher, a
reader of failing traces, and `cross.sh`, the two-by-two of two scratch
fixes against the forced schedules) is `hunt/patches/`, as patch series
of the branches it ran on. Whether `cross.sh` was run before the cut-off,
and what it said, is not recorded anywhere.

**Hunt II: the engine fails a differential fuzz against a model.**
`tests/fuzz.rs` (merged from `wip-hunt2-base`; brief `hunt2/ctx.txt`). On
this branch, built in the dev profile and run one process a seed with the
block cache off (the default draw):

| seeds 0..60 | count |
|---|---|
| pass | 53 |
| `compaction would write a key outside its fence` at a commit, settle or close, after which the segment work has stopped (seeds 2, 11, 34, 46, 48, 58) | 6 |
| a raw scan with a value duplicated (seed 36, big values) | 1 |
| hang | 0 |

Seed 2 is in the fuzz's default range of four, fails ten runs of ten, at
steps 241 to 274 of 725, and fails the same way on the fuzz's own base
branch (`7024b5d`, before the second form was merged): the defect is the
engine's and older than this branch. The brief lists two more classes the
reviewer saw: the tiling assertion in `publish_segs_with` killing the
maintenance thread with the writer then parked in `land_seal`, and, with
the cache on, a `copy from the form disagrees with the build from the
sources` panic. Neither showed in these sixty seeds.

## The gate on this branch

`sh scripts/deps.sh` first (`all deps ready`), then the groups one at a
time:

| group | result |
|---|---|
| `build` | green |
| `lint` | green |
| `wasm` | green: the module links, 128,266 bytes |
| `test` | running when this was committed; the result is in the commit that follows |
| `bench` | queued behind `test`; the result is in the commit that follows |

## The branches, and what became of each

Nineteen remote branches. Everything live is reachable from this branch;
the lines marked *contained* add no commit this branch lacks, the lines
marked *superseded* carry an earlier form of a commit the mainline holds
(checked by patch-id and by diffing the branch's files against the
mainline's: four lines each, the mainline's own edits), and the four
August branches are the engine before the rewrite that `main`'s five pull
requests squashed in, with the initial commit as their only common
ancestor.

| branch | last commit | what it is | here |
|---|---|---|---|
| `claude/performance-dimensions-analysis-gijutv` | 2026-10-19 | the mainline, 476 commits past `main` | the base of this branch |
| `claude/wip-wave2-zproof` | 2026-10-18 | stage 2a's branch | contained (`cd4e774`) |
| `claude/wip-wave1-merge` | 2026-10-18 | wave 1's merge: layout, selector, ordread | contained |
| `claude/wip-fix-emptykey` | 2026-10-18 | the arena fix | contained (`1328c74`) |
| `claude/performance-assessment-yy86vx` | 2026-09-16 | the row that measured `d6b1e4d` | contained |
| `main` | 2026-09-06 | | contained |
| `claude/wip-scratch` | 2026-10-19 | the design, briefs, scripts and rows | merged (`5fa9940`): `scratch/` |
| `claude/supdb-perf-blog-angle-y3ywv0` | 2026-09-29 | the C ABI experiment, borrowed against copied, with its results | merged (`667f7e5`): `experiments/ffi/` |
| `claude/wip-hunt2-base` | 2026-10-18 | the differential fuzz | merged (`a95fec8`): `tests/fuzz.rs` |
| `claude/wip-hunt-stress0` | 2026-10-18 | hunt I, ten commits: hooks in `src/db.rs` and loop-target tests | `hunt/patches/wip-hunt-stress0/` (`fd3cf19`) |
| `claude/wip-hunt-bisect0` | 2026-10-18 | hunt I's bisect line, six commits | `hunt/patches/wip-hunt-bisect0/` |
| `claude/wip-hunt-stress0-fixexp` | 2026-10-18 | five of stress0's commits and two scratch experiments | `hunt/patches/wip-hunt-stress0-fixexp/` (the two) |
| `claude/wip-ooc-suite` | 2026-10-17 | the bounded scale, first form | superseded by `63b7a0a` |
| `claude/wip-scan-dirprefetch` | 2026-10-17 | the seek's directory hint, first form | superseded by `ca738ec` |
| `claude/wip-drain-step0` | 2026-10-17 | the drain stamped, a WIP backup | superseded by `fb5a45a` (same patch) |
| `claude/supdb-ext-analytics` | 2026-08-30 | the previous engine and suite | old: not merged |
| `claude/supdb-cached-bytes` | 2026-08-30 | the previous engine and suite | old: not merged |
| `claude/supdb-wal-values` | 2026-08-30 | the previous engine: a log carrying values | old: not merged |
| `claude/supdb-wasm-reader` | 2026-08-30 | the previous engine and suite | old: not merged |

The hunt branches were not merged as code because their author marked the
engine hooks "for a hunt branch and not for a merge" and their tests are
loop targets that fail by design when the defect fires (one in a few
hundred runs, and the oracle aborts the process in a few percent), which
would make the gate red at random. Each series was applied on its base and
the tree compared with the branch's: identical (`hunt/patches/README.md`).

Deleting the fourteen branches other than `main` and the August four
loses nothing this branch does not hold; the mainline's own name,
`claude/performance-dimensions-analysis-gijutv`, is among them, and the
briefs under `scratch/` name it as the base a worktree is cut from, so a
reader of them should know it is gone. Deleting the four August branches
loses the pre-rewrite history of the previous engine (its experiments: the
value-carrying log, the cached byte source, the analytics scorecard), which
`main`'s squashed pull requests do not carry; a tag before deletion keeps
it without a branch:

    for b in supdb-ext-analytics supdb-cached-bytes supdb-wal-values supdb-wasm-reader; do
      git push origin origin/claude/$b:refs/tags/archive/$b
    done
    git push origin --delete \
      claude/performance-dimensions-analysis-gijutv claude/performance-assessment-yy86vx \
      claude/supdb-perf-blog-angle-y3ywv0 claude/wip-scratch claude/wip-hunt2-base \
      claude/wip-hunt-stress0 claude/wip-hunt-bisect0 claude/wip-hunt-stress0-fixexp \
      claude/wip-wave2-zproof claude/wip-wave1-merge claude/wip-fix-emptykey \
      claude/wip-ooc-suite claude/wip-scan-dirprefetch claude/wip-drain-step0
    git push origin --delete claude/supdb-ext-analytics claude/supdb-cached-bytes \
      claude/supdb-wal-values claude/supdb-wasm-reader

Neither command was run by the consolidating session: deleting a remote
branch is the owner's call.

## Where to start

1. Hunt II's fence defect: `ZFUZZ_SEEDS=2 cargo test --profile checked
   --test fuzz fuzz_range -- --exact --nocapture` fails every run on a
   store of 4 KiB seals, 1 KiB partitions, `l0_trigger` 4, the upkeep at
   `Background(2)`, the segment work on a thread, `sync: Always`. A store
   whose segment work has stopped is the worst thing the engine can ship,
   and the brief's rules for the fix apply: a regression test that fails
   deterministically on the unfixed head, the cluster's seeds and a fresh
   range at a control's rate. Seed 36's duplicated value is the second.
2. Hunt I: resume from `hunt/patches/`, `cross.sh` first, since the two
   scratch fixes were prepared and never adjudicated.
3. The second form: the second sitting of the forced arm's pairing
   (`wave2/zproof-commands.sh`), then stage 2b.

Every timing run needs the machine to itself (`bench/CLAUDE.md`), and the
checked compile of `tests/db.rs` needs the memory to itself (`CLAUDE.md`,
"Running the checks").
