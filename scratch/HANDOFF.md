# Handoff: encryption branch, stopped 2026-11-22 about 22:20 UTC for quota

Branch `claude/inspiring-brahmagupta-d7y8n7`. No pull request exists and none was asked for.
Everything is on the remote and the working tree is clean. Nothing is running and no check-in is
scheduled. The handoffs before this one (18 and 20 Nov) are in history; this file replaces them,
and its older sections below are kept because they are still true of what they describe.

`scratch/` is session notes kept in the repo so they survive the container. **Remove it before
any merge or pull request.**

## The encrypted comparators exist now (23 Nov, `b12bd36`)

The second half of the merge bar ("beating competitors on added encryption benchmarks") has
its instrument. `bench/src/enc.rs`, behind the suite's feature `enc`, in a build of its own:

```bash
cd bench && cargo build --release --no-default-features --features enc --target-dir target-enc --bin bench
```

```bash
sh scripts/check.sh enc
```

- **Arms**: `lmdb3` and `lmdb3-enc` (LMDB's 1.0 line, plain and behind its page hook under
  AES-256-GCM), `rocksdb-tuned-enc` (RocksDB's encrypted Env under AES-256-CTR, which
  authenticates nothing), each with a `-nosync` twin. `bench run --arms enc` is the roster:
  `supdb`, `supdb-cipher` and those, both guarantee groups. In that build the `lmdb` arms are
  refused by name (LMDB's two lines define the same C symbols); every other arm is as it is.
- **Rows of it go under `scratch/enc/`** (`--out`), never under `bench/runs`: they would share
  the standing series' class. Pairings: `BIN=bench/target-enc/release/bench OUT=scratch/enc/pairs
  bench/scripts/pair.sh supdb-cipher rocksdb-tuned-enc SIZE [options]`.
- **Encrypted LMDB stops at 300,000 keys.** Past that its shuffled load fails inside LMDB
  (`MDB_PROBLEM` at a commit): a write transaction that reaches across about two thousand
  decrypted chunks, on LMDB's unreleased line, reproduced on its C API alone
  (`a_write_transaction_over_two_thousand_chunks`, ignored). The arm is not reshaped to pass.
  Past the quick scale's top the encrypted comparator is RocksDB's. Its ordered load and reads
  do run at any size (`--passes read` loads in order), so a bounded read pairing against it is
  possible.
- **Why the LMDB arm is on the C API**: through `heed3` an encrypted environment fails under
  reader threads (`MDB_PAGE_NOTFOUND`), the wrapper's hook writing ciphertext into a buffer
  another transaction reads. `bench/CLAUDE.md` has the whole of it ("A comparator's wrapper is
  not the comparator").
- **OpenSSL's headers** are needed for that build (`sh scripts/deps.sh openssl`); this
  container has them.

### What the comparators say (the row and four bounded pairings, 23 Nov, at `b12bd36`)

Everything here is under `scratch/enc/`: the row `quick/20261123T014642Z-b12bd36.json`
(`bench run --scale quick --arms enc`, five reps, four rungs, twelve arms), read cell by cell by
`python3 scratch/enc/encrow.py ROW.json` (a cell is resolved when every rep of one arm lies on
one side of every rep of the other), and `pairs/` (`pair.sh`, both orders, two pairs a sitting).

**In core, 10,000 to 300,000 keys: ahead in about 120 of 145 cells against each encrypted
comparator, in both guarantee groups.** Every read, scan, shuffled load, lag point and mix.
Behind in three families:

| family | cipher store | its plain twin | LMDB encrypted | RocksDB encrypted | whose to win |
|---|---|---|---|---|---|
| ordered load, durable, k ops/s by rung | 690, 974, 927, 1007 | 921, 1345, 1237, 1351 | 1028, 1078, 1111, 1069 | 1038, 1177, 1096, 1163 | the layer's from 30,000 keys (it needs 0.85-0.9x of plain); at 10,000 **the plain store trails both too** |
| ordered load, buffered | 1206, 1528, 1473, 1716 | 1821, 2383, 2650, 3214 | 1868, 2095, 2265, 2300 | 1602, 1630, 1794, 1832 | the layer's (it needs 0.72-0.88x of plain); at 10,000 plain trails LMDB's too |
| bytes on disk a byte stored | 2.5 | 1.3-1.4 | 1.15-1.2 | 1.03-1.09 | the double write is the layer's; **the plain format's 1.3 is above both comparators whatever the layer does** |
| open after a crash, ms by rung | 5, 14, 53, 169 | 4, 7, 22, 65 | 0.9 at every size | 13, 28, 79, 224 | ahead of RocksDB's; LMDB recovers nothing and **the plain store is 4 to 70 times its open too**; the cipher's 2.6x over plain is this branch's (task 13) |

The ordered load is 0.72-0.75x of plain durable and 0.53-0.66x buffered, the known double
write and the envelope at the close; the design's S4 predicts 0.85-0.95x and 0.78-0.90x, which
is ahead of both comparators from 100,000 keys and about level at 30,000.

**Out of core, five million keys under 640 MiB (`--bound 671088640 --passes read,scan
--ops-div 10`): behind.**

| cell | `supdb-cipher` | RocksDB encrypted | LMDB encrypted | `supdb` | `rocksdb-tuned` |
|---|---|---|---|---|---|
| point reads a second | 38-42 k | 54-55 k | 9 k | 630-760 k | 63-74 k |
| scan entries a second | 0.42-0.47 M | 1.58-1.62 M | 0.59-0.60 M | 27-36 M | 1.7-1.8 M |
| major faults a read | 0.47-0.50 | (0.42-0.44 refaults) | 0.73 | 0.004-0.008 | |

- Encrypted RocksDB reads 1.3-1.4x faster and scans 3.5x faster than `supdb-cipher`; encrypted
  LMDB scans 1.3x faster and reads 4x slower.
- **The plain store is not out of core at this bound any more**: numbered keys take the stride
  form upstream made the default, 100 bytes a key, and five million keys of it fit 640 MiB. So
  `supdb-cipher` is at a seventeenth of `supdb` on reads and a seventieth on scans here, where
  the design's table (written before the stride form shipped) says 0.29x.
- That is the layer's largest target and it reorders the plan: what loses these cells is that a
  store under a cipher is kept out of the compact forms (the stride form declined, the
  controller off because a form page straddles two 4080-byte chunks). Pages stored whole (S3),
  the controller on (S5) and the stride form read by the piece (the fifth between-stage change)
  are what put the cipher store in the regime its plain twin is in.

**For the owner to rule on** (asked in the session's report of 23 Nov): three of the cells
above are lost by the *plain* store to the same comparators (the smallest rung's ordered load,
bytes on disk, the open against LMDB). No encryption work wins those. The reading taken until
told otherwise: on such a cell the bar is that the store under a cipher does no worse against
the encrypted comparator than the plain store does against the plain one.

## Upstream merged again, and what was held over it (23 Nov, `b35f519`)

Upstream's head `558606d` (eight commits: the raw rewarm, `scan_forms_off_counts_built`, the
forms-by-partition and drop-reason probes, a full row of ids) is merged and pushed. The branch
is current with upstream as of that commit. What the merge took and what was checked:

- **One conflict**, the generated `docs/figures/full/index.html`, taken as upstream's.
- **Three sites of upstream's new code read what this branch moved behind the seam** and did not
  compile: `raw_rewarm_ask` (`MmapBytes::open`, the mapping's lent bytes), and
  `Reader::forms_by_partition` and `Maint::z_raw_settle` (`resident_share` over a whole borrow).
  They go through `vault::map`, `map_len`, `map_resident_share` and `OrdIndex::resident_share`
  now. Expect the same at every merge that adds a file mapped by path or a residency read:
  those are the two things upstream writes that the seam renamed.
- **The gates, answered in their own terms**: the door's count of `std::thread::` in `db.rs`
  103 to 104 (`z_raw_settle` sleeps between looks), and three new public probes on lines of
  `tests/data/cost-entries.txt`; four raises declared in `scripts/raises.txt`. The lists pass
  against `main` (where every list is new) and against the branch's tip before the merge
  (`SUPDB_BASE=cb9236a sh scripts/check.sh lists`), where the raises are held.
- **Under a cipher**: the raw rewarm asks for the envelope behind each file and decrypts
  nothing; `a_settles_raw_rewarm_under_a_cipher_asks_for_the_envelopes_and_decrypts_nothing`
  (`tests/zpages.rs`) holds it, and three mutants fail it (no ask, mapped past the door, no
  wait). Upstream's other two options are bookkeeping of the writer's scans and read no file.
- **`sh scripts/check.sh` green** on the merged tree, all seven groups.
- **The fuzz beyond the gate's range**, all with `ZFUZZ_CACHE=all ZFUZZ_OPTVARY=1`, no failure:
  seeds 1000 to 3000 under a cipher with a 16 KiB cache (3.5 million cache units given back),
  1000 to 3000 in the clear, 1000 to 2000 under a cipher with the compact form forced (4.5
  million reads through a form).
- **Every arm opened once at ten thousand keys**: the roster's thirteen, the bounded roster's
  three more, the seven cipher arms, the six segment arms, upstream's three new arms. No
  refusal. The two `dirkey` arms were not opened: no fscrypt filesystem was up.
- **The count against upstream's head** (`sh scripts/cost.sh
  origin/claude/e-recode-stride-selector-g5f6hl`, at `b35f519`): every one of the fifteen probes
  under upstream by its total -- read -39 an operation, read_z -31, scan -383, scan_z -596,
  the loads -8 to -9, the commits -8, work -23, merge -467, land -346, seal -1, scan_at -373.
  The script still exits red by function, as at the two merges before: the same gainers
  (`Db::op`, `WriterOp::drop`, `Reader::leave_looked`, `Blob::key_at` and the rest), each a
  function upstream's build inlines where ours calls it, beside the caller that lost more.
  Nothing new gained with these eight commits.

Still owed from before this merge: `sh scripts/cost-red.sh` since `join_seal`'s bound moved to
0.04 (`594fe3ca`), and that commit's review -- read on 22 Nov by this session and found sound
(`OrdIndex::whole_key_at`'s plain half indexes `top_eager` within its length for a mapped file,
`vault::persist`'s pairs are made only under a cipher, the seal's closure borrows), but read by
one reader only.

## Correction from the owner, after the note below was written (22 Nov)

The note below reads the owner's "Stay wholly off of upstream" as: never fetch, merge, count
against or build upstream. **That reading was too wide.** The owner, asked again, in full: "It's
fine to fetch the upstream and even to merge the upstream here, just don't modify the upstream
or main branches. Our goal is to stay current and eventually have an encryption feature we can
merge." So:

- Fetching `origin/claude/e-recode-stride-selector-g5f6hl` and `origin/main`, merging either
  into this branch, building them and counting against them are all fine.
- **Never push to, or otherwise modify, upstream's branch or `main`.** Push to
  `claude/inspiring-brahmagupta-d7y8n7` only.
- The goal has two halves: stay current with upstream, and end with an encryption feature that
  can be merged. The owner is worried by the branch's size and has lost its thread; say plainly
  where things stand, in few words, whenever reporting.

Wherever the text below says upstream is not fetched or merged, this section replaces it.

Measured at that moment (`b05e881`):

- `main` is `5aad09d`. Upstream is 323 commits ahead of it, with no pull request open; no pull
  request is open in the repository at all. This branch is 160 commits on top of upstream (475
  ahead of `main`), so nothing here reaches `main` before upstream's work does.
- Upstream is 8 commits ahead of this branch (head `558606d`: rewarm options, probes, a full
  row). A trial merge: **one conflict, in a generated file** (`docs/figures/full/index.html`,
  redrawn by `sh bench/scripts/figures.sh`). Not merged yet.
- This branch against upstream (`git diff 7c712c1f HEAD`), lines added:

  | part | lines | what |
  |---|---|---|
  | `scratch/` | 123,000 | session notes; deleted before any merge |
  | bench rows and figures | 29,000 | regenerated files |
  | `tests/` | 31,600 | `durable.rs` 16,300 (crash states), `source.rs` and its lexer 8,300 (the gates), `cipher.rs` 2,400 |
  | `src/` | 18,500 | the four encryption modules 8,400; `db.rs` 6,800; the readers 3,400 |
  | docs and notes | 4,100 | `CLAUDE.md` 2,600 |
  | scripts and CI | 3,000 | the cost count, the lists, the door, the seal |
  | bench code, web | 1,900 | arms, probes, `web/crypt.mjs` |

  About 59,000 lines a reviewer would read, of which about 16,000 is one crash-test file and
  about 12,000 is gate machinery.
- Put to the owner and **answered (22 Nov): "We're not going to merge partial."** The page layer
  is built on this branch before anything merges, upstream conflicts and all: "I encourage you
  to have the cleanest design possible to that end." And the bar for the merge, in the owner's
  words: "this is a performance first project, so our eventual bar for merging requires no
  impact for unencrypted runs, and beating competitors on added encryption benchmarks."

### The merge bar, and what it needs that does not exist yet

1. **No impact for unencrypted runs.** Held by instruction count, change by change
   (`scripts/cost.sh`, the `cost` group). Owed beside it before a merge is proposed: the timing
   rows (quick, full, ooc) at the branch's head against upstream's at the commit merged, since a
   count sees no stall.
2. **Beating competitors on added encryption benchmarks.** At the time of writing the suite has
   **no encrypted comparator arm**: `bench/src/engines.rs` runs LMDB (`heed` 0.20) and RocksDB
   (`rocksdb` 0.23) in the clear only, and every encryption pairing so far is supdb against
   itself. What the dependencies offer, read off cargo's registry on 22 Nov:
   - RocksDB ships `include/rocksdb/env_encryption.h` (`NewEncryptedEnv`, an
     `EncryptionProvider`, a CTR stream over a block cipher of which only ROT13 is built in),
     but neither the C API (`c.h`) nor the `rocksdb` crate exposes it: an encrypted RocksDB arm
     needs a small C++ shim with a real block cipher. CTR authenticates nothing, so the arm's
     guarantee is weaker than `Encryption::Cipher`'s and the row says so.
   - `heed` 0.20 and `lmdb-master-sys` 0.2.6 have no encryption. LMDB's page encryption
     (`mdb_env_set_encrypt`) is on its `mdb.master3` line, which the `heed3` crate wraps with a
     caller's AEAD: a second LMDB dependency beside the first.
   - Both comparators on a directory the kernel encrypts stand beside `supdb-dirkey`
     (`scripts/fscrypt.sh`); this guest's kernel has no AES instructions in its cipher
     (`CLAUDE.md`), so its write cells there are the kernel's.
   Order this implies: the encrypted comparator arms and one row of them come **before** the
   page layer's stages, since the cells supdb loses under a cipher are what the layer has to be
   designed to win (in its cache `supdb-cipher` reads 0.8-0.92x of `supdb`; out of its cache a
   miss is four microseconds and the scans are 0.11-0.39x of the store in its cache).

## Start here (written 22 Nov, for a session with no memory of this one)

**The owner's instructions that bind everything** (the full list is under "Standing instructions
from the owner" below; these are the ones a new session gets wrong first):
- "Use your best judgement. The goal is an encryption feature which is reliable, performant, and
  not fragile to maintain." A store with no encryption pays nothing per key, entry, block or
  step (`scripts/cost.sh`, by function, against the commit before a change). Nothing is switched
  off or limited under a cipher as a design property. Fix bugs and flaky tests; no shortcuts; do
  not speculate.
- **Never modify upstream's branch or `main`** (the owner, 22 Nov; see the correction above:
  fetching, merging into this branch, building and counting against upstream are fine).
- An engine default changes only after benchmarks confirm it is better and `sh scripts/check.sh`
  has passed with it. Push whenever there is a commit. No pull request unless asked.
- The owner (22 Nov, on the bound of `Maint::join_seal` in `scripts/cost-hot.txt`): "You can do
  whatever you think is right on that threshold." It is 0.04 now, by the file's own rule.

**Where the work is.** The page layer (one module behind the `vault::` door that isolates
encryption from the forms) is being built in stages. Stage G1, its first gates on today's code,
is **closed** (eight units, the full gate green, a critic's account in
`scratch/plan/build/close/g1-built.md`). Five changes stood between G1 and stage G2; on 22 Nov:

| # | change | state |
|---|---|---|
| 1 | the seal's words renamed, its source test rebuilt (T21) | landed `89da086f` |
| 2 | the `cross-arm` CI group's crash inside QEMU (T22) | landed `f1cbced5` |
| 3 | the cipher test that failed now and then (task 14), and a mutant it uncovered (T25) | landed `620e4c5a`, `3c2b64d2` |
| - | a flake of the stride group's concurrent fuzz, met on the way (T23) | landed `bc9cdd84` |
| - | a flake of the pager's residency unit test, met on the way | landed `a711e083` |
| 4 | the merge probe's functions that gained instructions (T26) | **landed `594fe3ca`, built, counted and gated, but reviewed by nobody; see below** |
| 5 | the stride form and the heads' copy made to work by the piece under a cipher (task 23) | not started |

Then stage G2 (`scratch/plan/g2.md`, ten units, a conformance group), its plan first brought up
to this tree (the critic's section "For G2" lists what moved), then the layer itself
(`scratch/seam/design.md`, stages S1 to S7). Section 1 under "What is left" has every one of the
above in detail, newest first near its end.

**Change 4 (T26), exactly where it stopped.** Its builder finished and its work is committed as
`594fe3ca` by the launcher, after the run was stopped for quota. What stands behind it: the instruction count against the commit it began at
(`3c2b64d2`) green over eight probes, each reshaped function down (merge probe: `whole_key_at`
-32.0 instructions an operation, `Emitter::enter` -21.0, `Db::seal_table`'s closure -12.0, the
probe -268; `Db::append` unchanged, its +4 shown not to be the cipher's by a floor build); ten
mutants killed; the builder's fuzz under a cipher in a small cache over seeds beyond the gate's
range, eight workers, each ending green (`fuzz.summary.log`; the launcher read their last lines
and no more); and the launcher's own gate (build, test, stride, lint, the lists against
`3c2b64d2`, wasm: green, after one red of an unrelated unit test fixed in `a711e083`). What
does **not** stand behind it: **no reviewer read it** (the workflow's two review lenses and
their repair rounds never ran; the launcher read the diff's shape and not every line), and
`scripts/cost-red.sh` was not run again after `join_seal`'s bound moved to 0.04. First
task of the next session: read `scratch/plan/build/T26/` (notes.md, change.diff, the listings)
and review that commit as the workflow's reviewers would (the brief is
`scratch/plan/build/T26/brief.txt`, its section "How reviewers read it"), or run its review
rounds by launching `scratch/plan/build/change-v1.js` with a brief that says the build is done.

**How this session worked, so the next can go on the same way.** Each change is one run of the
workflow script `scratch/plan/build/change-v1.js` (build, up to three review rounds each with a
repair, a gate, one local commit; the launcher pushes), launched with a brief kept as a file
(`scratch/plan/build/<key>/brief.txt` and `launch-args.json`; T21, T22 and T26 are the models).
A brief says what is known and no more, what must be shown red first, the mutants, what must
not be done, how an agent waits, the commit it begins at, and the disk. Small test-only fixes
the launcher did itself, with the same discipline (T20, T23, T14, T25: a trace of the state
first, then the state forced red, then the fix, then green under the same forcing, then a
mutant). The scripts live in the scratchpad of the session that ran them and in
`scratch/plan/build/`; a new session copies them from the repository. One cargo, one valgrind,
one emulator at a time; every run its own canonical `TMPDIR`, removed by its literal path.

**Lessons of 22 Nov that are also in `CLAUDE.md`:** trace the state before forcing a schedule
(two guessed schedules cost a build and a sweep each and reproduced nothing); a count's
baseline is read behind the join of every worker that moves it; a comment that says what a
test holds is held to the mutant; an instrument's crash is traced to the calls in flight before
it is read as any test's.

**Open with the owner (none blocks work):** whether the lists land on the main line ahead of
this branch (today they hold pushes only); whether the door's libc test keeps reading cargo's
registry. The cost job's base in CI is the main line (answered by "stay wholly off of
upstream"); against the main line it counts everything this branch merged from upstream until
the main line holds that work, so until then the plain-store rule is held change by change.

**Known to fail now and then, each to be read as its own before it is read as a change's:**
the checked `zpages` binary's wall-clock waits under load (listed further down). Fixed on 22 Nov
and worth knowing by name if they come back: the stride group's `concurrent_range` with
conversions ("three settled asks ... (1, 3)"), `tests/cipher.rs`'s "the builds gave nothing
back", and `db::pager::tests::a_view_says_what_is_in_memory_and_drops_and_asks_for_what_stands_behind_it`
(its wait was for one page and its assertion for the whole file's share; red once in the unit
binary and once in six runs alone, forty green after the fix; the race was read off the two
lines and not forced by a seam).

## The story in one paragraph

The branch adds encryption to supdb (`Encryption::Cipher`, `Encryption::Kernel`, the `crypt`
envelope) and, found along the way, a long list of crash-safety fixes to the plain store. It
works and is gated. The owner's problem is that merging upstream hurts, because the cipher's
code is threaded through the engine. The owner's direction (19 Nov): **finish our work here so
that we integrate with upstream once**, scope "everything doable here", order "count-back, then
build the layer". The count-back is done. The page layer (the redesign that isolates
encryption) is designed and planned and **not started**. That is the next piece of work.

## Standing instructions from the owner

- Fix bugs and flaky tests. A performance cost is not "accepted": understand it, do not regress.
- A store with no encryption pays nothing per operation for the feature. `scripts/cost.sh` is
  the check (below).
- A directory fsync that fails: make the names durable another way (guards); if that fails too,
  the store stays open for reads and its writes answer the failure. Implemented (`f364cf4`,
  `9a668be`).
- Push to `claude/inspiring-brahmagupta-d7y8n7` and to no other branch
  (`git push origin HEAD:claude/inspiring-brahmagupta-d7y8n7`, up to 4 tries), whenever there is
  a commit to push. An earlier copy of this note said "only after `sh scripts/check.sh` is green
  on the tree"; the owner corrected that on 20 Nov: "The standing rule should be to change the
  engine default only after benchmarks confirm it's better, why would I care about pushing
  commits?" So pushing waits for nothing, and the gate is for what a commit claims, not for the
  push.
- An engine default changes only after benchmarks confirm it is better (the owner, 20 Nov), and
  after `sh scripts/check.sh` has passed with the new default in place (`CLAUDE.md`).
- No pull request unless asked. No model ids in commits. Commit trailer: the `Co-Authored-By`
  and `Claude-Session` lines the session gives.
- Preferences: be logical, test locally, do not speculate, no shortcuts.
- **Superseded by the correction at the top of this file: fetching and merging upstream are
  fine, modifying upstream or `main` is not.** What follows is the first reading, kept as it
  was written. Stay off upstream (the owner, 22 Nov, in full: "On main vs upstream. Stay wholly
  off of upstream."). It answered a report that had asked what the lists' and the cost job's base
  should be while upstream's work is not on the main line. Read here as: from that message on,
  nothing is done with `origin/claude/e-recode-stride-selector-g5f6hl`. It is not fetched,
  merged, pushed to, counted against or named as any gate's or CI job's base; a base is the main
  line (what `scripts/base.sh` takes from the event) or the commit a change began at. This
  replaces "Merging upstream is fine" (20 Nov) and "May want to merge upstream changes again"
  (21 Nov) for everything after it. What the branch already holds of upstream (five merges, the
  last a row at `7c712c1f`, merged as `60921870` an hour before the message) is left as it is:
  taking it out was not asked and would be a rewrite of the branch. The reading is the
  launcher's, said back to the owner the same hour; if the owner meant less (the bases alone)
  or more, this bullet is what changes.

## Where the branch stands

| commit | what | checked how |
|---|---|---|
| tip | this note and the records under `scratch/plan/build/` | not gated: `scratch/` only |
| `a711e083` | the pager's residency unit test waits for what it asserts | forty runs alone, lint |
| `594fe3ca` | change 4 (T26): three merge functions under their earlier cost, `join_seal`'s bound 0.04 | count green against `3c2b64d2`; launcher's gate: build test stride lint, lists, wasm. **Not reviewed** |
| `3c2b64d2`, `620e4c5a` | a form build under a cipher held to resting; the cipher test's flake | cipher binary under the test and stride environments, lint; forced state and mutants |
| `f1cbced5` | change 2 (T22): the `cross-arm` group | its unit gate, `cross-arm` three times running |
| `bc9cdd84` | the concurrent fuzz's convert-steer flake | fuzz under every gate environment, lint |
| `89da086f` | change 1 (T21): the seal's names | its unit gate, count green, wasm size unchanged |
| `60921870` | the last merge of upstream's rows (before the owner's instruction) | figures redrawn, bench group |
| `31b33339` | stage G1's last commit | **the whole default gate in one run** (build test stride lint wasm bench lists), 22 Nov |

No whole default gate has run since `31b33339`: each commit after it was held by its own unit
gate. `bench/target/checked` was removed for disk by change 4's builder, so the next
`sh scripts/check.sh` rebuilds it (a few minutes, 3.5 GB). A whole gate, the `cost` group and
`cross-arm` once more are a reasonable first hour of the next session.

Upstream (`origin/claude/e-recode-stride-selector-g5f6hl`) was merged five times, the last a row
at `7c712c1f` (`60921870`, 22 Nov). **That was the last: the owner's instruction of 22 Nov is to
stay wholly off upstream.** (Corrected the same day, at the top of this file: fetch and merge
are fine; upstream is 8 commits ahead and its merge is owed.)

The history holds "In progress:" snapshot commits from 18 Nov (`7421505`, `ccc2d7b`, `e0badc1`).
Their code has passed the full gate several times since; the main line merges by squash.

## What was done since the 18 Nov handoff

All pushed, each behind a green gate. `git log --first-parent a4f3764..HEAD` is the list.

1. **Upstream merged three times** (`eecfdc0`, `fddc12a`/`6dd58a6`, `6365fdf`): the stride form,
   run sequence numbers, the store lock, the pre-written log. Fallout fixed after each.
2. **The two set-aside durability holes, closed at no measured cost.**
   - run-dir: a run opens on a spare temp name a directory fsync the store already makes has
     covered (`d6734a9`, `Options::run_spare`; arm `supdb-runnospare`). Two directory fsyncs a
     10k durable load against three; throughput level.
   - run-close: covered by the run sequence numbers the upstream merge brought (a sync covers
     the runs that closed loose) plus `21ee2bd`.
   - a failed directory barrier is made by guards, a hard link per name (`f364cf4`); a kept
     failure leaves a `dir-lost` mark the next open acts on (`9a668be`).
3. **Correctness items off the roadmap** (19 Nov evening):
   - `8b19c8e` a panic on the segment work's thread stops the seal queue with the panic as its
     cause; the writer no longer hangs (`Db::panic_segment_work` is the test seam).
   - `29460a2` a manifest slot names the log file it needs (`Manifest::needs_log`, behind the
     format word, old readers ignore it); an open refuses a slot whose log is gone.
   - `9ecc5cf` an unnamed run's piece past the slot with its temp name gone is refused.
   - `09b6a78` the same fall back held under a cipher.
   - `f4e055f` a log whose lost first sector hides frames that verify is set aside
     (`wal-N.aside`), not cut. Plain stores only.
   - `a4cef33` the open orders a delete from before deletes raised the greatest key against the
     run after it (a cov file records the run's first key).
   - `5107a65` under a cipher a form file's failed chunk takes the form out of service, not the
     store (`pager::Paged::regenerable`).
   - `324a395` fuzz seed 1179: a copy's claim to be its block's records names its partition.
4. **Flaky tests.** The wide-values builders test (`e0badc1`), the recycled-log test's detection
   (`840fd25`), several zpages waits moved from tick counts to `tick_until_built` (`1d05892`,
   `e1de138`). The compact-form cipher flake ("the builds gave nothing back") did not recur and
   was **not** diagnosed.
5. **The 10k ordered-load cost of `10898a94`**: explained, no regression against the
   comparators; nothing changed.
6. **Count-back** (`af56aac`, `5ca5a84`, `3f65bfb`), next section.

## The count-back, and how to hold it

`sh scripts/cost.sh BASE [probe ...]` builds six bench probes at BASE (in a worktree, from this
tree's example sources) and at the tree, runs each under callgrind, prints every function's own
instructions side by side, and fails where a function listed in `scripts/cost-hot.txt` gains
more than `COST_SLACK` (0.5) instructions an operation. About 45 minutes. It builds a second
copy of the bench workspace while it runs and removes it at the end, so read `df` first; the
machine need not be quiet, since it counts instructions. Run as:

```bash
COST_DIR=/tmp/claude-0/cost/w sh scripts/cost.sh origin/claude/e-recode-stride-selector-g5f6hl
```

Against upstream head `4358827`, at `3f65bfb` (negative is fewer than upstream):

| probe | per operation |
|---|---|
| read (point read, 200k) | -7.0 |
| scan (1000 scans of 100) | -52.6 |
| load_durable (100k keys) | -7.8 |
| load_buffered | -5.8 |
| work (writer's round, 300k) | -12.4 |
| merge (200k) | -207 |

The one real finding: `forms_off_scan` and `scan_seek_seeded`, upstream's, were inlined in
upstream's build and called out of line in ours (+8.4 a scan). Each keeps its first test inline
now. Two false alarms are written up in `CLAUDE.md` beside the count ("`sh scripts/cost.sh BASE`
is the count made one command").

**Not covered by `cost.sh` yet** (the plan's G1-3 wanted them): reads and scans through the
compact form (`supdb-nocachezpages`), a scan over `SCAN_REST` (1024), a seal probe
(`SegmentWriter::create_with`), and `Blob::scan_at` through the public API. Upstream's 6 new
commits have not been counted against.

## The zpages stress: upstream's tests are fragile under load, not ours

Running the checked `zpages` test binary four copies at once, four threads each (16 tests on 4
cores), three rounds: ours failed 10 runs of 12; upstream's own build (`4358827`) failed 12 of
12, with as many failures or more, in the same tests. So it is not this branch's regression.
The gate (one copy, four threads) passes. The failures are wall-clock waits and controller
states never reached: `scans_after_a_quiet_stretch_have_the_cold_form_files_asked_for` (most
often; its `drive_to` never reaches COMPACT, 3 of 7 form files built, 4 builds started),
`the_trial_judges_the_refilled_state`, the hot-raw policy tests, `the_sample_starts_between_...`,
`the_reading_at_the_crossing_...`, `the_selector_builds_the_raw_tails_...`,
`readers_read_through_a_partitions_conversions_both_ways`,
`handles_on_threads_read_stride_files_while_merges_retire_them` (`builds_collected` waits 30 s).
The pattern that fixed the ones already done: `drive_to` (a count of rounds) to `drive_to_built`
(rounds while a build is in flight, a minute of wall), `tick_until` to `tick_until_built`. A
bulk replacement broke tick-exact tests once; do it test by test. Planned for the integration.

## What is left, in the owner's order

### 1. The page layer (task 12). Stage G1 is closed: eight units, the full gate green on `31b33339`, the critic's account in `scratch/plan/build/close/g1-built.md`. Of five changes that stood between it and G2 four have landed (the seal's names, `89da086f`; the `cross-arm` group, `f1cbced5`; the cipher test's flake, `620e4c5a`; the merge probe's gainers, `594fe3ca`, which no reviewer has read); the fifth (the stride form and the heads' copy under a cipher) is not started.

**As of 22 Nov 11:24 UTC.** The stage's workflow script is `scratch/plan/build/g1-build-v2.js`,
launched one unit a launch: `Workflow {scriptPath, args: {base:
"ea6352181f342b7bffd75ec8c88df92bc7b78ff6", only: ["G1-<n>"], tail: false, since: <note>}}`,
the same base for every unit; after G1-8 a launch with `only: ["none"]` and no `tail: false`
runs the full gate and the critic. `since` (from G1-4 on) is the launcher's note of what landed
after the plan was written, read by every step of the launch; G1-4's is in
`scratch/plan/build/G1-4/launch-args.json` and the later units take the same text with what
has landed added. From G1-4 on a unit's gate also runs the `lists` group, and `wasm` for a
unit that changes compiled engine code. A unit is a build, up to three rounds of two reviews and a repair, then its gate
and one local commit, which whoever launched it pushes. A container restart kills the run and
leaves its files: resume with `resumeFromRunId` and the same args, plus `args.resume` (a
build) or `args.notes` (any other step, by its label) to tell the killed step what the tree
holds; a note for a step changes no earlier prompt, so the steps before it replay.

- **G1-1 `wall` landed: `e7d3fe71`.** `tests/source.rs` over a lexer in `tests/lex/`, the
  couplings in `tests/data/couplings.txt`, `.github/CODEOWNERS` naming the owner for that file
  (the owner was asked on 20 Nov and decided against it: see "The owner's decision" below). Its notes, mutants and red
  inputs are in `scratch/plan/build/G1-1/`. Its third review's three findings were repaired and
  not reviewed again; the stage's closing critic reads the whole.
- Its gate went red once on a cipher test that bet on the segment work's timing; that test is
  fixed (`ac512887`).
- **G1-2 `door` landed: `fa7e9356`.** The library's file, mapping, advice, sync and
  process-read calls are barred by symbol outside the door's modules (vault, bytes, pager,
  crypt, dirkey, blocks): a clippy run of its own over the library (`scripts/door/run.sh`, in
  the lint group, config `scripts/door/clippy.toml`), its red input (`scripts/door/red.sh`),
  and what clippy cannot see held by text in `tests/source.rs` against `tests/data/door.txt`
  and `tests/data/door-libc.txt`. No compiled engine code changed (lint attributes, one test
  moved out of `vault.rs`). Its notes and mutants are in `scratch/plan/build/G1-2/`. Open, for
  the closing critic and the owner: all three review rounds asked for repairs and the third
  repair was not reviewed again; the unit is 1,845 lines of `tests/source.rs`, and its libc
  class test reads the libc source Cargo.lock resolves out of cargo's registry, so a libc bump
  that adds a string-taking call fails it until the call is barred or listed, and a vendored
  or offline layout fails it outright -- a maintenance cost to weigh against what it holds;
  the config's names were resolved on x86_64 only; `CLAUDE.md` still names the deleted
  `the_engine_names_no_file_but_through_the_door` and says no module carries a blanket lint
  allowance, both G1-8's to correct.
- **G1-3 `cost` landed: `c1546dc1`.** `sh scripts/check.sh cost` (outside the default set, and
  a CI job) runs `scripts/cost-red.sh`, the gate's red input, and then `scripts/cost.sh BASE`:
  fifteen probes built at BASE from this tree's example sources and at the tree, each under
  callgrind, every function's own instructions compared by canonical name
  (`scripts/cost-diff.py`, the list and the bounds in `scripts/cost-hot.txt`), and
  `tests/source.rs` holds every public entry point of `Reader`, `Db` and `Blob` to a probe or
  a reason in `tests/data/cost-entries.txt`. No engine code changed. A container restart at
  12:30 UTC killed its first build sixteen minutes in; the run was resumed with a note. Its
  notes, its mutants (reviewers' and its own, four passes) and the two whole runs it rests on
  are in `scratch/plan/build/G1-3/` (`against-upstream-9bfc4362.log`, `against-itself.log`).
  All three review rounds asked for repairs; the third repair (the `~` lines given bounds, the
  `land`, commit and long compact-form scan probes, type aliases and blanket trait impls in the
  entry-point test) was read by the launcher and by no reviewer: the closing critic's.
- **What the cost gate says of this branch against upstream `9bfc4362`** (the upstream head
  last merged), since task 17 landed as change `T17`, **`c584cdc3`**: every one of the fifteen
  probes is at or under upstream by its total, and the gate still exits red by function, on
  moves. Before the change two probes were over: `scan_at`, the public `Blob::scan_at` with a
  caller's own callback, +239 instructions a scan of a hundred entries, and `land`, +274 a key
  rewritten. What was found and done, each by function and by instruction
  (`scratch/plan/build/T17/notes.md`):
  - `scan_at`, now 373 a scan under: the public walk moves its rank once where its loop ends,
    `Blob::seek`'s arm by the piece is out of line, and `FlatIndex::search` resolves its two
    regions once before its loop (the engine's own scans read about 300 a scan lower for that
    too).
  - `land`, now 55 to 62 a key under: the cause was `324a395a`'s search by key in
    `Reader::splice_run` wherever a copy's claim names another partition (the reading of G1-3's
    one run, verified). The fix stays; the patch takes the entry its rank names only where a
    key compare (`key_eq`) proves that entry is the key in hand, and searches otherwise, so the
    shape is sound by a check it makes. `tests/settle.rs` holds it over keys of 4, 16, 17 and
    19 bytes and a whole block, red with the compare removed, with `324a395a` taken out, and
    with the search dropped.
  - `Reader::settle_each`, whose source is upstream's, read 37 a key over: the ordered index's
    cold arms that return (`head_absent`, `top_late`) were inlined into it and kept the query's
    head on the stack across the calls. Both arms are out of line (`plan_seek_late`,
    `seek_planned_by_piece`) and the stride's probes read unchecked over the slice the plan
    cuts; it reads 9 a key under, with 8 of codegen growth still inside it, outweighed and not
    removed. `CLAUDE.md` has the rule (a cold path that returns into a hot path is priced in
    the caller).
  - The whole run and the pairs: `scratch/plan/build/T17/r1/cost-whole-R1e.log` and
    `pairs-whole-R1e.txt` (probe, gained, lost, net). Every gainer in the other thirteen probes
    is paired with its loser but two, neither this change's and both small: `read_z`'s
    `zpages::view_with` +3 a read, and `merge`'s small gainers inside that probe's slack. The
    pairs for `scan_z`, `scan_long_z` and `merge` were read off function totals, not by
    instruction.
  - Under a cipher, by the cipher's twins of the probes: the scan by the piece 199 a scan
    lower, the patch into a carried copy 231 a key lower in all, and inside that the settle's
    planned seek 67 a key higher, since an index read by the piece takes the two arms out of
    line now. Two shapes that took the 67 back each cost the plain store and were refused; it
    is the page layer's to take back (its index is not read by the piece through a cold arm).
    The cipher twin of `land` was a scratch copy of the probe
    (`scratch/plan/build/T17/r1/land_cipher_scratch.rs`), not a probe of the gate.
  - Two review rounds; the second passed in both lenses with items that a second repair then
    worked (ten more mutants, a test of the walk by the piece across several windows, a
    checked-build assertion that a plan is in order). That repair's diff was read by the
    launcher and by no reviewer: one `debug_assert!` and two tests.
  - **Owed, none taken:** timing. `sh scripts/check.sh quick` at the tree for the scan pass
    and the lag sweep's bursts, and `bench/scripts/pair.sh supdb-cipher supdb 300000 --passes
    lag` beside the same pairing at `c7a0c933` for the planned seek under a cipher. The hints
    ahead of a patch and a walk take any claim, as upstream's did, so a carried copy is
    prefetched as upstream prefetched it.
  - **Open:** the scan's start over a carried copy (`blk.identity == seg.blob.id()`,
    `324a395a`'s line) is held by no test: a mutant that takes any claim there survives the
    tests and 2,950 fuzz seeds, which reach a stale claim at a scan's start 1,808 times and
    every time at the block's first entry, where the rank and the search agree. `key_eq`'s
    arm for keys under sixteen bytes compares two empty slices by a call of no bytes, which
    this file's own rule says to guard (the empty key in a carried copy, one key a store).
    The new tests in `tests/blob.rs` hand closures straight to `Blob::scan_at`, where
    `tests/common`'s `kv` would compile one copy.
- **What the cost gate cannot do yet, for the closing critic and the integration:** a move
  between two functions reads red with nothing to say it is a move, so a pull request against
  the main line fails its cost job as the branch stands, with every total under upstream; the
  way through is undecided, and `T17`'s paired table is its input (a declared group of
  functions held to its net, valid against one base, is the candidate, shaped like the
  lists' declared raises below). Also open: the `~`
  bounds rest on two to four self-checks a probe; `land` runs the segment work inline, not
  on its own thread; the CI job and valgrind's noise on a hosted runner are unmeasured;
  `sh scripts/check.sh cost` with its default base has not been run; a whole run wants
  several gigabytes free; libc's and the allocator's functions are held only loosely; `Txn`,
  `RunWalk` and the `SparseBlob` readers are outside the entry-point list.
- **The owner's decision (20 Nov 17:38 UTC), task 18, landed: `4ceda21e`.** A list's "only goes
  down" is held by a check, not by `.github/CODEOWNERS` (removed), and every workflow that
  runs on a change checks only through `sh scripts/check.sh <group>`. `scripts/base.sh` is the
  one definition of the commit a change is made on (an override, a pull request's base, a
  push's before, else the merge-base with `origin/main`; an unresolvable base fails, never a
  fallback), and the cost group takes its base from it. The `lists` group (in the default
  set) runs `scripts/ratchet-red.sh` and then `scripts/ratchet.py` through `ratchet.sh`: each
  registered list (`couplings.txt`, `door.txt`, `door-libc.txt`, `cost-entries.txt`'s reasons
  in place of a probe) at the merge-base against the tree, growth read by the list's own key
  and number, the registry itself held to the base's; an intended raise is a line of
  `scripts/raises.txt` (list, key, from, to, reason) matched in all four, and a line the base
  already has is spent. `cross-arm` and `machine` (the Apple Silicon capture, which
  `apple-silicon.yml` checked by hand) are groups outside the default set. `tests/source.rs`
  reads every file under `.github/workflows`: a workflow on a change's event is read key by
  key (an `if`, a `continue-on-error`, an `env` that sets a base or narrows a group, a check
  outside `check.sh` are red; a group no job runs is red), the measurement workflows are
  listed with why and held to their triggers, and any other file is red. Run as change `T18`
  with `scratch/plan/build/change-v1.js` (the stage's script with the unit taken from its
  args); its notes, mutants and launch arguments are in `scratch/plan/build/T18/`. All three
  review rounds asked for repairs; the third (every workflow read, the `machine` group, the
  set-up scripts' writes to the job's environment) was read by the launcher and by no
  reviewer. Open: the `machine` group has not run on a Mac; what a set-up's binary or an
  action writes to a job's environment is not read; the cost gate's declared moves are not
  built (`raises.txt` is shaped to take them); the launcher's choice that a raise is declared
  rather than impossible has not been answered by the owner.
- **`19d078d1`: the pager's residency test compiled out under `supdb_emulated`.** The change
  above installed the cross toolchain and qemu-user here and ran `cross-arm` for the first
  time on this branch: one test red, five runs of five, the pager's ask ahead that qemu-user
  passes to no kernel. Green after (519 passed). No hosted runner has run any job of this
  branch: the first pull request is where `cross-arm`, `machine`, `cost` and `lists` first run
  there.
- **G1-4 `offsets` landed: `97412a31`.** `Blob::willneed_ranks`, `willneed_dir` and
  `record_offset` answer for a reader by the piece as for a whole one (they answered 0 or
  `None`), and `record_region_place` for both kinds. Each dispatches on `Blob::flat_lent` (a
  borrow through `Sec::lend`, the `lend_at` test with no error made) to a
  `#[cold] #[inline(never)]` `*_by_piece` arm over the same `Sect`-generic helper; the offset is
  `Sect::start_in`. `tests/blob.rs` holds the two kinds of reader to each other and to the
  file through a source that records every ask (`Advised`), and holds the piece reader to
  asking for under a quarter of the record region; `tests/fences.rs` holds where a merge's
  drops behind it begin (the base's mechanism, held by no test before). The plain store's
  count: `sh scripts/cost.sh 869a4fee read read_z scan scan_long scan_z scan_long_z merge
  land` green; `record_offset` 51 instructions a call where the base had 55, and kept
  `#[inline(never)]` because inlined into `compact_run` the merge row read RED (+4.0 an
  operation; `r2/red9.sh` is that red input by script). Two review rounds: the first asked
  for a repair (a callee-saved register kept across `not_lent`'s call cost the slice arm two
  instructions a call), the second passed with no blocking item, and its repair added tests
  only, read by the launcher and by no reviewer. Notes, mutants, the scratch probes
  (`g14_count.rs`, `g14_drop.rs`, `g14_gate.rs`), the listings and the count logs are in
  `scratch/plan/build/G1-4/`. Open, each owed to a named place:
  - No `cost.sh` probe reaches the arms that call these a key or a block at a time: one with
    the gate set away from the raw form (`Db::zgate_set`, a toggle on the main thread's scans)
    and one merging under `merge_drop_behind` and `merge_read_ahead` are owed to the cost job.
    Until then three cost-only mutants (the attributes off the piece arms, `flat_lent` made
    through `flat`) are held by the unit's scratch counts and by no gate.
  - What the pager does with the advice it is now given (`Paging::willneed`, `forget`) was
    not run: no store under a cipher with `ZPages::Always` had its asks counted. S5 or G1-6.
  - `compact_run` takes `record_offset(0)` of every input of every merge, dropping or not
    (`src/db.rs`, where `dropped_to` is made): under a cipher that is now a directory word
    read through the pager an input and then ignored. Take it only where the input drops.
  - `Sect::start_in` is a new required method of a public trait (no implementor outside the
    crate's own two); `record_region_place` answers the file's offset for a whole reader
    now, which G1-6, where `record_region_span` goes, has to know.
  - The attributes off `willneed_dir_by_piece` alone read about 12,000 instructions fewer
    over the gate probe's run: a cheaper shape for whoever counts those rows next.
  - Owed to S5, no timing taken: `bench/scripts/pair.sh supdb-cipherauto supdb-cipher 5000000
    --bound 671088640 --passes read` at this commit and the one before.
- **G1-5 `authenticated` landed: `ce095eb9`.** `Bytes::authenticated` (default false) says
  whether a source has checked every byte it lends. `Blob::open_with` skipped the key
  section's checksum row for any section held by the piece; it now sends such a section to
  `verify_row_by_piece` (`#[cold] #[inline(never)]`), which skips the row only over an
  authenticated source and otherwise verifies it through `flatindex::verify_pieces_in`, the
  row asked for once and each piece on its own. `MmapBytes` answers true for a paged mapping
  and false for a mapped file, so a store under a cipher opens as it did (the open bound in
  `tests/cipher.rs` is what fails when a paged mapping answers false) and the change is for
  an outside source that lends by the piece. `tests/blob.rs` flips the last byte of every
  piece, the short first one and the one ending at the row among them, and forges a piece
  whose CRC moves in its top byte alone (a one-byte flip cannot tell a three-byte compare
  from a four-byte one). The plain store's count: `sh scripts/cost.sh ea0962c2 load_durable
  load_buffered commit_durable commit_buffered seal scan_at merge land` green;
  `Blob::open_with` reads three instructions more a segment opened in the two loads (265 to
  268) and the same in the merge and the landing, nothing a key. Three review rounds; the
  first repair was cut short (its agent ended its turn to wait for a background run, and the
  harness took its answer as it stood), the second finished what it owed (every mutant, the
  full test run, lint, wasm, the bench group, the recount), and the third added the per-piece
  flips, read by the launcher and by no reviewer. Notes, mutants, the mutant scripts, results
  and count logs are in `scratch/plan/build/G1-5/`. Open, each owed to a named place:
  - The three instructions a segment opened: a reviewer's shape that may cost nothing (the
    old `if .. && key.pieces(&src).is_none() { whole arm }` kept, with an `else if let
    Some(p)` to the cold function) was not counted. G1-7 counts row 6 and edits the same
    dispatch.
  - `MmapBytes::authenticated` is `self.map().is_none()`, spelled so that `bytes.rs` gains no
    wall token; it is exact while `Own` has two variants and `map()` matches both with no
    wildcard. A third variant that answers `None` there would answer authenticated: say so
    at `map()` and at `Own` (G1-7, which edits both).
  - Docs that do not yet say what the code does: `BlobOptions::verify_index` (the row is
    skipped over an authenticated piece source and verified a piece an ask over any other),
    `BlobOptions::read_ahead` (the walk by the piece ignores it), `Paging`'s contract (an
    implementor lends only bytes it has checked; `InMemory` in `src/ordindex.rs`'s tests
    checks nothing and answers authenticated through `MmapBytes::paged`),
    `Bytes::authenticated`'s "a reader" (`OrdIndex::open` skips its CRC on `piecewise`
    alone, which is right only because its sources come from `vault::map`), and
    `verify_row_by_piece`'s "whole" (it means every byte). G1-8.
  - `flatindex::verify_pieces_in` is a new public function, and it trusts `Sect::range` to
    return exactly the length asked for. Not verified: that no outside crate implements the
    doc-hidden public `Paging` (G1-7 makes it `pub(crate)`).
- **For every launch from G1-6 on:** an agent never ends its turn to wait for a background
  command; it waits in a foreground command with a long timeout. The launch note says so.
- **G1-6 `advice` landed: `eab987d1`.** A store's residency and advice are asked of its byte
  source by offset. One span question, `Bytes::resident_share(off, len) -> Option<f64>`,
  answered for a mapped file (by `mincore`) and a paged mapping (by the pager) alike, in place
  of `MmapBytes::resident_pages`, `paged_resident` and `resident_count`, `db::resident_share`
  over a borrowed slice, `OrdIndex::mapped` and `Blob::record_region_span`, all gone with
  every caller moved. `Blob::record_region_place` and `record_dir_place` give a region's
  place in the file for both kinds of reader; the plain build's asks and drops
  (`ask_ahead`, `drop_behind`, the block form's and `write_stride`'s) go by offset through
  the mapping, Linux-only as they were; the three descriptor sites go through
  `Seg::records_by_descriptor`; `warm_file` measures `map.len()` and asks through
  `advise_willneed`. Sites the plan did not name were found by the unit's own list and
  handled (`ZFile::pages_resident`, `Seg::redrop_ord_pages`, `read_back_residency`,
  `heads_all_resident`, `raw_read_span`). Under a cipher, behind `ZPages::Always` or
  `merge_read_ahead` and on no default's path: the warm asks for the envelope and decrypts
  nothing, the form file's and the ordered index's residency are asked of the pager where
  they had decrypted the whole file to `mincore` it, a merge asks ahead for its inputs'
  ciphertext, and a paged reader's records are dropped. The plain store: `sh scripts/cost.sh
  f9e83900 read read_z scan scan_long scan_z scan_long_z merge land` green, and the plain
  build's `madvise`/`fadvise`/`mincore` call shapes held to the base's by strace, made a gate
  with an exit status over the one test whose shapes are stable
  (`scratch/plan/build/G1-6/strace/gate.sh`). Two review rounds: the first found the plain
  build issuing `madvise(WILLNEED)` off Linux where the base issued nothing (the Linux gate
  put back) and the cipher warm's ask held by no assertion (a test reads the envelope's
  residency after the warm); the second passed, and its repair added tests only, read by the
  launcher and by no reviewer. Notes, mutants, scripts, results, the strace scripts and count
  logs are in `scratch/plan/build/G1-6/`. Open, each owed to a named place:
  - The pager keeps no count of the advice it is given, so what the crossing and the build
    ask of it under a cipher is held by outcomes only. S5.
  - `warm_file` touches pages through `Bytes::lend_at`, and a new test
    (`a_drop_behind_unmaps_the_window_and_no_page_past_it`) reads the mapping's address
    through `MmapBytes::as_slice()`: G1-7 seals the first and deletes the second, and has to
    give each another way.
  - The item allow on `ask_file` (kept for its `posix_fadvise`) covers any barred call added
    to the same function: a mutant that opens by `File::open` there survives. Move the
    advice into a small allowed helper, as `db.rs` does, so the function carries no allow
    (S1, or the door's next edit).
  - The stride form's plain build is not under the strace (no test builds one there); the
    strace's merge test read four new shapes in one head run of three, put down to which
    partition a background build took and not confirmed by more base runs.
  - The merge row's libc `memcpy` read +3.6 and then +12.9 an operation over the base in two
    runs of the same code, green under the libc rule and above the base both times: count
    the tree against itself twice before it is called noise.
  - Not built for macOS or aarch64; `cross-arm` not run. `tests/blob.rs`'s
    `holds_to_the_file` now compares `record_region_place` of two readers where one side was
    taken by pointer subtraction; the independent check left is `record_offset(0)`.
  - Owed to S5, no timing taken: `bench/scripts/pair.sh supdb-cipher64 supdb-cipher 1000000
    --reps 4` with an arm that sets `merge_read_ahead`, and `pair.sh supdb-cipherauto
    supdb-cipher 5000000 --bound 671088640 --passes read`.
- **Upstream merged again: `22dcd115` (a change called M1), with `2a0d750a` after it.** The
  owner asked (21 Nov: "May want to merge upstream changes again."); it went in before G1-7,
  which edits the files upstream is editing. Merged: `origin/claude/e-recode-stride-selector-g5f6hl`
  at `7c62153e`, 31 commits past `9bfc4362`. It ran through `scratch/plan/build/change-v1.js`
  from the brief `scratch/plan/build/M1/brief.txt`, three review rounds. What stands:
  - **One shape of the copy's identity.** Both sides had fixed a copy's claim about a
    partition's records carried across its rewrite. The merged tree has upstream's
    representation (`identity: bool`, `identity_of`, `CachedBlock::identity_for`) with this
    branch's key compare kept at the patch (`Reader::splice_run` takes the rank's entry where
    the claim is of the table's partition or one key compare at that entry proves it, and
    searches otherwise), chosen on the land probe's count. Both sides' `tests/settle.rs` tests
    stay, each red with the fix out. `CLAUDE.md`'s two entries are one.
  - **Text-clean and wrong, twice.** Upstream's `OrdIndex::head_bracket` read `self.top`, a
    field this branch made a method (`top()`, which is also what is right for an index by
    the piece; a new ordindex test holds the bracket by the piece to the mapped one). Both
    sides added `tests/settle.rs` tests whose stores were named `copy-identity-rewrite`: one
    directory for two tests of one process, red 15 runs of 15 until this branch's were
    renamed. Then every function both sides touched was listed and its merged body held to
    the base plus both sides' changes (`both/both.py`, `union.py`), and every changed file
    to the exact per-line sum (`whole3.py`: 74 of 84, the other ten the hand-resolved ones).
  - **Two engine edits beyond the merge, for the count.** Upstream's read-mode note
    (`ModeNote`) is a cell a field where it was one cell copied whole in and out a read:
    `Reader::scan` reads 21 instructions a scan under upstream where the plain merge read 11
    over. `OrdIndex::seek_in_exact` searches the heads held in memory unchecked with the
    window bounded once (a mapping by the piece holds none and takes the head-at-a-time
    loop): `Reader::cut_raw` under upstream. Both read by two reviewers and by the launcher.
  - **This branch's gates over upstream's code.** The workflows test admits the quick job's
    `KEYS` beside `ARMS`. Two cost entries for upstream's doc-hidden test seams
    (`Reader::block_cache_identity`, `Reader::piece_bounds`), their raises declared in
    `scripts/raises.txt`, the first raises this branch declared. The wall needed none; the
    door is clean; upstream's added code calls nothing G1-6 removed. `docs/figures` was
    behind upstream's own ooc rows and the bench group red on it: redrawn by
    `sh bench/scripts/figures.sh`.
  - **Under a cipher.** Each new upstream mechanism read for what bytes it reads: the bounds
    walk (the index's top level, a record by the piece on a tie), the landing that leaves
    bounds unread and the quiet third merge (atomics), the fence-stepping cursor (the fence in
    memory, the page by `slice_at`), the sparse walk's key step (a field). The differential
    fuzz beyond the gate: seeds 1000 to 4000 under a cipher with a 16 KiB cache and in the
    clear, block cache on, options varied, and 1000 to 2000 with the compact form forced: no
    failure; the counters show the cipher runs draw upstream's options. Every arm of the
    roster and every cipher arm opened once at ten thousand keys, no refusal.
  - **The count against `7c62153e`.** All fifteen probes under upstream by their totals
    (`cost-r3.log`; the four that run the bench code again in `cost-r4.log`): read -38 an
    operation, scan -382, merge -215, land -344, the loads -6 to -8. The gate still exits red
    by function, as it did against `9bfc4362`: each gainer is paired with its loser in the
    notes, is once a store, or is on the list below.
  - **Two of upstream's tests were flaky, each red on upstream's head alone, each fixed in
    the test.** `a_bounds_walk_places_a_boundary_by_its_head_and_reads_its_record_only_on_a_tie`:
    every store walks the keys twice (the upkeep thread's fill, the landing's walk), and a
    seal that freezes the burst before the lent fill runs leaves one store a walk short; red
    5 of 5 with the upkeep thread held; the test fills inline and counts the walks since the
    burst was filed. The stride group's conversion steer in the fuzz: a seed's asks, each
    replaced by the next and taken only at the segment work's turn, can all go untaken beside
    seals and merges (16 of 24 red on upstream's head six at once); the test asks the first
    served partition three times after the writer's last settle, settling each, and holds a
    move each way. A durable test that read a stopped queue's flag and then a commit's answer
    waits for the answer it reads.
  - **The lists' check answered by its base** (`2a0d750a`, the launcher's, after the merge):
    the two raises were let through against the branch's tip before the merge and read red
    against the main line, which predates every list. A declaration for a list the base does
    not have holds nothing now and is said so; three cases in `scripts/ratchet-red.sh`, three
    mutants red. `sh scripts/check.sh lists` is green against both bases.
  Notes, mutants, scripts, results and count logs: `scratch/plan/build/M1/` (and `T19/` under
  it for the lists' fix). Open, each owed to a named place:
  - **The cost gate against upstream cannot fail usefully**: its verdict is red before and
    after any change, so a new regression reads like the reconciled list. A baseline of
    accepted gainers, or a declared group held to its net, that `scripts/cost-diff.py` reads
    (`scripts/raises.txt` is shaped to carry it). And four gainers in the merge probe have no
    loser named since T17: `OrdIndex::whole_key_at` +10.9 an operation,
    `Db::seal_table`'s closure +5, `Db::append` +4, `Emitter::enter` +3; `Maint::join_seal`
    reads +0.034 against its bound of 0.010 in the land probe (0.027 at T17). A change of its
    own, before the integration.
  - `tests/cipher.rs` `the_compact_form_is_built_and_read_within_a_cache_the_store_does_not_fit`
    ("the builds gave nothing back") went red once more, in the stride group, and is still
    undiagnosed: `scratch/plan/build/M1/cipherflake-red.txt` has its counts. Task 14.
  - `heads owned` is never reached under a cipher (the heads' copy is a mapped file's):
    an option that silently does nothing there. The page layer's, or a change of its own.
  - Upstream's new defaults (`bounds_by_heads`, `land_bounds_read`,
    `merge_parallel_quiet: 3`) were priced on plain stores; the cipher arms inherit them
    unpriced, and no cipher arm with them off exists. Owed to S5: each cipher arm against
    the same arm with the option off, at 300,000 keys and, for the bounds options, under a
    bound at ten million.
  - The bounds test no longer runs the walk on the lent-fill path, and its fills' room was
    not driven closed by real refaults (a forced fill-off stands in). The fuzz's three-ask
    shape ran over the gate's seeds only. The suite's `Supdb::with_policy` costs about 9,000
    instructions more once a store than upstream's (an `Options` that holds `Arc`s, built by
    struct update): the suite's open, not the engine's.
  - The walk's start under a stale claim is still reached by no store test (T17's M4b); a
    unit test of `identity_for` holds the predicate.
- **G1-7 `sealed` landed: `41a07cf6`.** The read path's whole borrows are sealed to their
  readers by the compiler. `Bytes::lend_at` takes a zero-sized `blob::lend::Token` whose
  constructor is `pub(in crate::blob)`, so outside the crate it can be neither called nor
  overridden. `MmapBytes::as_slice` is gone; `MmapBytes::lent<T: whole::Reader>(&self, _: T)`
  takes the token of the reader that may hold a mapped file whole, the ordered index's
  (`OrdIndex::over`, `mapped_heads`) or the stride form's (four functions of
  `src/db/zfile.rs`, upstream code the plan predates), the trait sealed in a private module
  of `src/bytes.rs`. `Paging` is crate-private and `Paging::base` takes `bytes::Base(())`.
  `Blob::record_regions` and `record_pieces` are one crate-private `Blob::records()`
  returning `Records::{Whole, Pieces}`, every caller naming both arms. `warm_file` touches
  through `MmapBytes::touch` (nothing of a paged mapping). Red inputs: compile-fail doctests
  in `src/lib.rs`, each beside a twin that compiles; `scripts/seal-red.sh` in the lint group
  (a copy of the crate in which each barred call compiles at home and fails at its own line
  abroad), with `scripts/seal-red-red.sh` as its own red input; and a source test
  (`tests/source.rs`, `seal_check`) for what the compiler cannot see: a token's module
  growing a second way to make one, a token made apart from its call, a new caller inside a
  reader's module. The plain store: `sh scripts/cost.sh e9a6abff` over all fifteen probes
  green, no hot function moved. The wall's count of the seam's paths out goes 0 to 3, one
  `use` a token, declared in `scripts/raises.txt`. Three review rounds: the first held the
  unit to a token on `lent` (built without one to save the raise), the second and third to
  spellings the source test missed (a renamed import, a call by type, a name handed to a
  macro); the third repair, the check alone, read by the launcher and by no reviewer. Notes,
  mutants, scripts and count logs: `scratch/plan/build/G1-7/`. Open, each owed to a named
  place:
  - **The source test's weight, for the closing critic.** It is some nine hundred lines of
    `tests/source.rs`: a reading of identifiers off the lexer, three lists of allowed sites
    (`SEAL_DECLS`, `SEAL_CALLS`, `SEAL_MACRO_WORDS`) and a rule that refuses the words `lent`
    and `lend` inside any macro's tokens, with four test assertions excepted by file, scope
    and text. `lent` is a common word in this engine (the upkeep is lent), so an ordinary
    `assert!` that names such a local trips it, and a reformatted excepted line does too. It
    guards against a colleague routing round the seal on purpose. Two ways to lighten it,
    neither built: give the seal's method and token modules names nothing else uses, so the
    macro rule needs no exceptions and the reading by position could become the word
    anywhere; or cut the text half back to the construction sites. Decide with the critic's
    read, as a change of its own.
  - **The stride form does not work under a cipher.** Its four readers take the mapped file
    whole (`lent`), which is nothing for a paged mapping, so under `ZPages::Always` with a
    cipher `attach_stride` and `check_stride` refuse silently and the partition is served in
    the block form. From before this unit (through `as_slice`). Owed to the page layer's
    stage for the compact form (S5), with a test of today's behaviour first.
  - Two shapes counted and refused are in the notes for whoever tries them again: a lendable
    length on `Bytes` with the lend a function of `blob` (`Blob::scan_whole` and the public
    `scan_at` over), and G1-5's reviewer's dispatch in `Blob::open_with` (over in the merge
    and land probes), so G1-5's three instructions a segment opened stay.
  - `scripts/seal-red.sh` and `seal-red-red.sh` add `cargo check` builds of a copy of the
    crate to every lint run; their time is in the closing gate's log.
  - Docs for G1-8: the prose of `lend_at`, `lent`, `Paging`, `touch` and the crate docs'
    section on what the read path does not hand out (one sentence of it is history);
    `docs/zpages-design.md` still names `record_regions`.
  - Not built for macOS or aarch64; `cross-arm` not run.
- **Two more durable helpers answered the segment work's failure by the stop's flag
  (`067d9fa8`, the launcher's).** `ss_no_room_for_the_run` and
  `ss_handed_over_a_stopped_queue` waited for the stopped queue's flag and then read commits
  and seals on the bet that the first commit would answer the failure; one went red once in
  G1-7's whole run. Both take the failure with `ss_commit_answer` now. Shown by a scratch
  spin of a set width between the flag and the failure's leaving (in the engine for the hunt,
  taken out): the tests as they stood red 24 runs of 25 at a millisecond, the fixed ones
  never at eleven widths, the whole durable binary under three widths with no test red.
  `scratch/plan/build/T20/` has the sweep.
- **G1-8 `docs` landed: `581e11cc`.** Comments and docs alone (the diff of `src` and
  `bench/src` with comments aside is empty). `CLAUDE.md` says what holds the wall, the door,
  the seal, the lists and the count as the gates are now, and its Layout table has the
  stage's scripts and lists; the docs the earlier units left untrue or with history in them
  are corrected (`BlobOptions::verify_index` and `read_ahead`, `Bytes::authenticated` and
  `resident_share`, `Paging`, `lend_at`, `lent`, `touch`, `verify_row_by_piece`, `ModeNote`,
  the crate docs' section on what the read path does not hand out, `docs/zpages-design.md`'s
  header). `README.md` and the crate docs say what a store under a cipher does, each from
  the code: the controller leaves it in the raw form unless `zpages_under_cipher` is set
  and `ZPages::Always` builds the compact form; the stride form is not built there (a build
  that asks for it builds the block form and counts the decline, `zfile::StrideNo::Kept`);
  the heads' copy (`Options::heads_owned`) is not made. `ROADMAP.md` has an entry for each
  of the last two with what would make it work. Its red input is a scratch script
  (`scratch/plan/build/G1-8/docs_red.py`, sixteen checks that hold a doc sentence to a line
  of code or a test's name; 62 findings on the tree before, none after), not a gate in the
  tree. Two review rounds. Open: no test holds the stride form's decline or the missing
  heads' copy under a cipher (the docs say both from the code; owed to S5); the look that
  would copy the heads still asks the kernel for their ciphertext before it finds nothing
  to copy, where the compact form is on under a cipher (in `ROADMAP.md`).
- **The stage's closing gate and critic (22 Nov, on `31b33339`).** `sh scripts/check.sh`
  green, every default group in one run (build test stride lint wasm bench lists); the
  lists group against the stage's base green; the count against upstream's head `7c62153e`
  with every probe's total under upstream (read -40 an operation, scan -382, merge -198,
  land -337, the loads and commits -6 to -8) and no flagged function new beside the merge's
  logs but one of 140 instructions over the seal probe's whole run
  (`std::fs::OpenOptions::_open`, unexplained). The critic read the tree and the units'
  notes and wrote `scratch/plan/build/close/g1-built.md`: what each gate holds, the red
  input that showed it, and a way to write the barred thing that passes it. Read it before
  touching a gate. What it found, by weight, and what each calls for:
  1. **Three gates give no verdict on a pull request into the main line.** The cost group
     exits red by function against any base (moves) and against the main line counts weeks
     of upstream's work; the lists hold nothing against a base that predates them, which the
     main line does until this branch lands; and `cross-arm` died in two parallel runs of
     five with a crash inside QEMU ("QEMU internal SIGSEGV", never a test's failure; green
     one thread at a time, 530 tests; green five of five at `19d078d1` with 519).
  2. **The seal's source test trips on ordinary code.** Any `lent` or `lend` inside a macro
     is red (the engine has `to.lent`), its exceptions and `SEAL_DECLS` lines are keyed by
     exact text that rustfmt can rewrap, and any `.base(x)` call anywhere in `src` reads as
     the seal's. Recommended: rename the seal's words to names nothing else uses, drop the
     exceptions and read the word anywhere.
  3. **`OrdIndex::whole_key_at` is probably a real cost a key in a merge** (+10.9
     instructions an operation in the merge probe since T17): it reads `top()` and `head()`,
     each with a cold arm that returns, where upstream read a field and asserted. Three
     smaller gainers beside it are undecomposed.
  4. **Three mechanisms do nothing under a cipher**, which the owner's rule does not allow
     as a property: the stride form (declined at the build), the heads' copy (not made), the
     quiet third merge (runs only where the compact form is installed). `README.md`'s
     "every option a store in the clear runs with, a store under a cipher runs with" is true
     of what is accepted and false of what acts. G2's conformance run needs the first two
     fixed, or it lists them as seams.
  5. **A whole borrow the seal does not see**: a new walk inside `src/blob.rs` over `flat()`,
     `Sec::get` or `flat_lent`, or a new caller of `ZFile::stride_bytes`, compiles, passes
     every gate and answers nothing under a cipher. G2's conformance pass is what holds it,
     at run time.
  6. Smaller: red inputs kept out of the tree run nowhere (G1-6's strace gate, G1-8's
     `docs_red.py`, G1-4's two probes); the door's libc test reads cargo's registry and
     fails offline; `compact_run` reads a directory word an input of every merge under a
     cipher and throws it away; the ordered index's CRC skip keys on `piecewise` where
     `Bytes::authenticated` now exists.
  It found nothing wrong in the commits it read (the merge's two engine edits, the lists'
  fix, the durable helpers' fix, the merge's bench test); it did not read the last repairs
  of G1-1, G1-2, G1-3, T18, T17 and G1-5 line by line.
- **The seal's words renamed, its source test rebuilt: `89da086f` (change T21, 22 Nov).**
  The critic's second finding. `MmapBytes::lent` is `lend_whole_map`, the three token modules
  `lend` are `lend_seal`, the readers' trait's module `bytes::whole` is `whole_map_readers`,
  `Paging::base` is `base_address` and `bytes::Base` is `BaseAddressToken`; `Bytes::lend_at`
  and the three re-exports (`LendToken`, `OrdLendToken`, `StrideLendToken`) were unique
  already and stay. `lent`, `lend`, `base` and `whole` are ordinary words again. The check
  reads every seal word as an identifier wherever it stands, macros included with no rule
  for them, at sites named by file and scope and never by a line's text; a new test of
  ordinary code and rewrapped listed lines was red on the old check and is green. Two review
  rounds found ways through the first rebuild (a top-level line at a declaration row, a type
  alias hand-out, a token taken from another call's row), each closed with a red case first.
  No engine behaviour changed: the src diff is the renames alone (compared with comments and
  whitespace stripped), the count against `60921870` over six probes is green with no engine
  function moved, the wasm module is the base's size to the byte. The check and its helpers
  went from 552 lines to 457; the seal's whole section of `tests/source.rs` grew, 970 lines
  to about 1,280 (the file 7,133 to 7,441), because the red and green cases grew: the
  critic expected two hundred lines fewer and that was not reached. Left open, in the
  commit's words: a closure in a listed scope taken as a function value passes (it passed
  the old check); a second non-test function with a listed scope's name in its home file
  reads as listed; a name a macro builds from pieces is not read. Records:
  `scratch/plan/build/T21/`.
- **A flake of the stride group found in that change's gate and fixed (T23, 22 Nov).** The
  concurrent fuzz with conversions on asks a partition to change form three times and holds
  the moves to three; it read four, "(1, 3)", once, and passed fifty-three times after. Its
  baseline was read behind a plain `settle`, which joins no form build and takes no
  conversion ask: where the loop's last ask was untaken with the builder free, the segment
  work's turn behind that settle started it and it was counted inside the first settled ask.
  Two forced schedules guessed before a trace made nothing red; the trace named the state,
  the state built was red in every seed of four runs of four, and with `zpages_settle()`
  before the baseline green in six of six. Test only; `CLAUDE.md` has the case under "Two
  counts compared across a worker's race". Records: `scratch/plan/build/T23/`.
- **The `cross-arm` group's crash found and the group made to give a verdict: `f1cbced5`
  (change T22, 22 Nov).** The critic's third concern. The crash is the emulator's own and no
  test's: qemu-user 8.2.2 fakes `/proc/self/maps` from its own record of the guest's
  mappings and crashes reading it while another thread maps or unmaps; a panic's backtrace is
  symbolized by reading that file; this shell has `RUST_BACKTRACE=1`, so the crypt test that
  panics on purpose read it beside the crypt tests' loop of `mmap` and `munmap` of their own
  ranges. Evidence: qemu's `-strace` of a dead run ends in the open of `/proc/self/maps`
  beside a `munmap` of the address the `mmap` before it returned; the crypt tests died in 8
  runs of 30 with backtraces on, 0 of 30 with them off and 0 of 30 with the one panicking
  test skipped; a C program of four threads and no engine (`scratch/plan/build/T22/repro/
  maps_race.c`) dies 10 runs of 10 with the same message, and lives with its reader on
  `/proc/self/status`, with no mapper thread, and built natively. The whole binary: 15 dead
  of 32 with backtraces on, 42 clean of 42 with them off. The fix is in the group's case of
  `scripts/check.sh`: `RUST_BACKTRACE=0` for the emulated run. No test is compiled out,
  skipped or loosened. Both reviewers passed it; their mutants then found two ways round the
  variable, each closed in the repair round, which **no reviewer read and the launcher did**:
  the variable written after `--lib` is a test-name filter (the group ran no test and exited
  0), so the case now captures its output and requires a `test result: ok` line with tests
  passed and `0 filtered out`; and a unit test that formats `Backtrace::force_capture()`
  killed one run in twelve under the fix, so `tests/source.rs` refuses any line of `src/`,
  unit tests included, that reads `/proc/self/maps`, names `Backtrace` or names
  `RUST_BACKTRACE` (`the_library_reads_no_maps_and_makes_no_backtrace`, with a fixture test).
  The group green three times running under the caller's `RUST_BACKTRACE=1`, 530 tests each.
  Lost: a failing test under the emulator prints no backtrace, and the job's log shows
  nothing until the run ends. Not verified: whether a hosted runner exports `RUST_BACKTRACE`
  (so whether CI would have met the crash at all; the case holds either way), whether QEMU
  has since fixed the race, and that no dependency of the library reads the file at run
  time. Records: `scratch/plan/build/T22/`.
- **The cipher test that failed now and then, diagnosed and fixed: `620e4c5a` (task 14,
  22 Nov), and a mutant it uncovered held: `3c2b64d2`.** Both by the launcher, test only.
  `the_compact_form_is_built_and_read_within_a_cache_the_store_does_not_fit` required the
  cache's evictions to grow between its fill and the form's settle ("the builds gave nothing
  back"; red twice, the second time in a stride group). A store under `ZPages::Always`
  builds as partitions land (`Options::zpages_eager`): a print at the first reading showed
  30 to 40 of 93 builds already run in six runs of six, and with the test's thread made four
  seconds late to that reading all 93 had, red three runs of three with the gate's message.
  The test runs twice now: builds left to the settle (the count exact, a build before the
  reading a failure of its own) and builds begun at landing (held to having been made).
  Its mutant check then found that **a form build under a cipher that never rests
  (`Held::passed` without `self.hold.rest()`) passed every cipher test**, this one's comment
  notwithstanding: its partitions are an eighth of its cache. The new
  `a_form_build_holds_its_cache_and_not_its_partition` builds one ordered run's segment of
  27 MiB under a cache of 2 MiB from a reopened store (a file being written is in memory
  whole until its envelope, so only a reopened store's peak is its builds' alone): peak
  2.2 MiB as the engine is, 23 MiB and red three runs of three with the rest taken out.
  `CLAUDE.md` has both cases. Records: `scratch/plan/build/T14/`, `T25/`.
- **The merge probe's gainers: `594fe3ca` (change T26, 22 Nov), built, counted, gated, NOT
  reviewed.** Stopped for quota after its build step; the launcher committed the builder's
  finished copy (its `keep/v3`, by sha256) after running the gate itself. Method, per function:
  the instruction listing laid to source lines, then a floor from a scratch copy with the
  cipher's arm taken out as text (no other tree built), which reproduced the earlier counts to
  the instruction for three of the four. `OrdIndex::whole_key_at`: one limit test in the plain
  half (the limit is zero for an index read by the piece), the old body out of line as
  `whole_key_at_by_piece`; kept `#[inline(never)]` because inlined it read as a caller's gain.
  `Emitter::enter`: a path clone a rank gone. `Db::seal_table`'s closure: the table bound once.
  `vault::persist` takes its pairs as a closure called only under a cipher. `Db::append`:
  unchanged, its floor is its count. Merge probe -268 instructions an operation.
  `Maint::join_seal`'s bound is 0.04 (five counts of one source spread by 1,328 over the run;
  the owner left it to judgement). Owed: the two review lenses over the diff; `sh
  scripts/check.sh cost` once (its red input after the bound moved). Records:
  `scratch/plan/build/T26/` (`notes.md`, `change.diff`, `cost.log`, `results.txt`, the
  listings' scripts, `brief.txt`).
- **Next, each a change of its own through `scratch/plan/build/change-v1.js`, in this
  order:** (1), (2) and (3) done, above; (4) landed unreviewed, above: review it first. As planned, (4) was `whole_key_at` and the three small
  gainers decomposed and brought down, each held by the count against the commit before it
  (task 20; the baseline of accepted moves against a named upstream commit that this note
  planned is not built, since no base is upstream's now: the cost job's base is the event's,
  the main line for a pull request, and against the main line it counts everything this
  branch merged from upstream until the main line holds that work, so until then the rule
  "a plain store pays nothing" is held change by change, against the commit before); (5) the
  stride form and the heads' copy made to work under a cipher by the piece, with tests, and
  the docs' sentence made true. Then G2 (`scratch/plan/g2.md`), its plan first brought up
  to this tree: the critic's section "For G2" lists what moved. Upstream may be fetched and
  merged again (the correction at the top of this file).
- **The owner's to decide, none of it blocking:** whether the lists should land on the
  main line ahead of this branch so that a pull request is held by them (today they hold
  pushes only); what the cost job's base is in CI before upstream itself is on the main
  line (answered 22 Nov, as read here: the main line and never upstream, which is what
  `scripts/base.sh` already takes for a pull request); whether the door's libc
  test should keep reading cargo's registry.
  Every unit from here is launched with a build note: a CI job it adds is
  `sh scripts/check.sh <group>` with nothing in the YAML but event values, a list it adds is
  registered in `scripts/ratchet.py`, and a move between functions the cost group reads red is
  said in its notes with callers and callees, the gate not weakened.

What follows was written before the stage began.

Design: `scratch/seam/design.md` (final), decisions in `scratch/seam/decisions.md`, the contract
in `scratch/seam/forms.md`. One module, `src/db/pagefile.rs`, behind the `vault::` door, that
knows pages, lengths, versions, sync points, sealed names and a durability class a file, and no
format. Plaintext page p is one stored 4096-byte page with its tag kept apart, which also fixes
the "form page is two chunks" loss that keeps the controller off under a cipher.

Plan: `scratch/plan/g1.md` (eight units: wall, door, cost, offsets, authenticated, advice,
sealed, docs), then `scratch/plan/g2.md` (ten units, a conformance group), then the layer.
`scratch/plan/build/g1-build.js` is a workflow script for G1, written and never launched.

Read before starting:
- **The plans' line numbers are stale.** They cite export `682b2c09`, from before three upstream
  merges (about 250 commits) and everything in "what was done" above. Every `db.rs:NNNN` has to
  be found again by symbol.
- **G1-1's inputs are in the repo now** (`scratch/plan/wall`, `door`, `borrows`, `conform`,
  `wallcheck`: the scouts' notes and checks, `couplings.txt`, the token lists, `lex.py`,
  `gen.py`), copied on 20 Nov from the scratchpad of the session that made them. They are of
  the tree at `682b2c09`: the counts will differ on today's tree. One file over a megabyte was
  left out (`wall/db.reg`, which `gen.py` writes again). `scratch/plan/build/g1-build-v2.js` is
  the G1 script brought up to today's tree.
- **G1-3 is partly built**: `scripts/cost.sh` and `scripts/cost-hot.txt`. What it lacks is
  listed under the count-back.
- The process each unit inherits is at the top of `g1.md`: red input first, the change, mutants
  against its tests, two reviews, gate, commit; nothing switched off under a cipher to make a
  unit easier; no instruction added to a plain store's operations.
- A workflow (many subagents) needs the owner's say-so in the new session.
- Whether to merge upstream's 6 commits before starting is the owner's call: "integrate once"
  says no, but the layer will be built against a `zfile.rs` upstream has since changed by 1,700
  lines. Raise it.

### 2. Performance items (task 13). Not started. `ROADMAP.md` has each.

- The controller under a cipher: its model held to a measurement, and pricing the store's own
  cache as well as the kernel's. Likely folds into the layer (whole-page chunks).
- An open after a crash, priced by its parts (+160-180 ms at 3M keys where a merge's output is
  unnamed).
- A log file retired off the drain (an `unlink` of 14 ms on a cipher's ordered drain).
- Also open under "Engine" in `ROADMAP.md`: an envelope written in one pass / beside its
  segment, an ordered run under a cipher as its own log, a miss at less than a fault, the read
  under a cipher at the plain store's rate, a read of one key larger than the cache.

### 3. Correctness items still open in `ROADMAP.md`

- A fall back held by a segment's last key: one way left, a merge output that folded in a run's
  piece (media damage alone; wants the segment to say it is a merge's output).
- What a guard does not cover: the segment work's directory fsyncs stop the seal queue instead
  of guarding.
- The items "held on a device that makes them" (log errors, power cuts, a torn slot, a seal's
  failed sync): need a device that really fails, which this container has not got.
- From the 18 Nov note, not re-checked: the last repairs of five pricing units landed with no
  review after them (followups 82, 102, 116, 123, 132); the tests-tidy unit's two reviews were
  never run; the engine-tidy wording items (followups 113, 115, 124, 127, 129); a test that
  holds about a thousand descriptors and two that use `kcmp`, against a CI runner's limits.
  `scratch/stage0/fix/followups.md` numbers every item to 134 and has not been updated since
  18 Nov, so check an item against the tree before working it.

### 4. Integrate once (task 14)

(Written 20 Nov. The owner's word since, at the top of this file: stay current with upstream
by merging it here, never modify upstream or `main`, and end with a feature that can be merged.
The steps below stand.) Merge upstream's head (9 hunks at the stop), fix the fragile zpages tests above, run
`scripts/cost.sh` against the new upstream head, run the gate, then a stress of the `zpages`,
`durable` and `cipher` binaries, then the quick row (`sh scripts/check.sh quick`, a timing run,
machine to itself; after a default changes, open the whole roster at the smallest rung first).
Remove `scratch/`. Then the owner decides about a pull request.

## Working here

- The gate is `sh scripts/check.sh` with its own `TMPDIR` (`TMPDIR=/tmp/claude-0/tg`), about 20
  minutes with the build caches warm and longer from cold; run it in the background and wait on
  a marker line in its log. Do not edit the tree
  while it runs. Do not run a stress beside it.
- Clean test stores by literal path (`rm -rf /tmp/claude-0/tg/supdb-*`); a glob through a
  variable was blocked by a safety check.
- Disk: 11 GB free at the stop. `bench/target` is 12 GB and `target` 4.7 GB.
- The container restarted once on 20 Nov mid-stress and three times on 18 Nov. Background jobs
  die with it; commits survive. Commit before long runs.
- All of `db.rs`'s file operations go through `vault::`; a unit test reads the source for it.
  `-D warnings` everywhere.
- Scripted edits (python `assert s.count(old)==1`) failed without writing several times when the
  assert sat after an early return; read the result back.
- A stress of `zpages` four at a time is 5-9 minutes a round; of `durable`, about 4.4 minutes a
  run.
- Leftovers that can go: worktrees under the session scratchpad (`mg`, `rc`, `rd`, branches
  `rc-work`, `rd-work` with one local WIP commit `b2e4143`, a run-close shape that was
  superseded), and `/tmp/claude-0/*.fix`, `*.bak`, `z*.out`.

## Where things are

| path | what |
|---|---|
| `CLAUDE.md` | the rules and every lesson, by failure; the count-back is under "A live store under the caller's cipher" |
| `ROADMAP.md` | open and closed items; "Engine" holds the cipher and durability ones |
| `scripts/cost.sh`, `scripts/cost-hot.txt` | the plain store's cost by function |
| `scratch/seam/` | the redesign: `design.md`, `decisions.md`, `forms.md`, maps of the read and write paths |
| `scratch/plan/` | `g1.md`, `g2.md`, the G1 script, the 17-18 Nov session log |
| `scratch/stage0/` | the durability fixes: per-fix notes, the pricing run, `fix/followups.md` |

Session task list at the stop: 12 the page layer (pending), 13 the performance items (pending),
14 integrate once (pending); 1-11 done.
