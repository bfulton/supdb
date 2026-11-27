# Baseline at C0

What the baseline holds, the command behind every figure, and the command for everything not yet run. Written 2026-11-27 for the coordinator, the measurement agent and the owner. Commands run from the repository root unless a `cd` says otherwise.

Every figure is of `b3a043dc` (tree `7f58d776`). `release/STATUS.md` names that commit C0, and `origin/release-candidate` stands there (`git rev-parse origin/release-candidate 'origin/release-candidate^{tree}'`). A checkout whose HEAD differs from it under `release/` alone is still that tree for every count here, and two lines of the counting tool say so:

```sh
sh release/tools/static_counts.sh | grep -E '^(tree_less_release|worktree_dirty_paths_outside_release)'
# 3851202654a874daf76c1bc5ab8714ef26231037 and 0: the counts are b3a043dc's
git ls-tree -r b3a043dc | git hash-object --stdin     # the same fingerprint, from the commit itself
```

No timing run and no callgrind count has been made. The only build behind a figure is section 2's `--build` run. "Waits for C0" means a clean checkout of C0 on a machine with nothing else running.

## 1. State

| # | baseline item | state | section |
|---|---|---|---|
| 1 | quick row | commands ready, not run: waits for C0 | 4.2 |
| 2 | full rows at 7 reps, numbered keys and ids | commands ready, not run: waits for C0; the banked shape (section 7, item 1) | 4.2 |
| 3 | out-of-core rows under 1 GiB and 1.25 GiB, numbered keys and ids | commands ready, not run: waits for C0 | 4.2 |
| 4 | noise floor: an arm paired with itself | commands ready, not run: waits for C0; the reader that turns the tables into a floor a cell is not written | 4.4 |
| 5 | callgrind instruction counts by function | commands ready, not run: waits for C0 | 3.2 |
| 6 | the pinned hot-function list | agreed by default (section 7, item 12); C0 counted against itself is running | 3.1 |
| 7 | wasm size | measured today at b3a043dc (tree 7f58d776), by one `--build` run | 2 |
| 8 | test count | measured today at b3a043dc (tree 7f58d776) | 2 |
| 9 | `Options` field count, `#[doc(hidden)]` count, lines mentioning `unsafe`, engine lines with and without tests, byte sizes of the docs | measured today at b3a043dc (tree 7f58d776) | 2 |
| 10 | wall time of each gate group and test binary | commands ready, not run: waits for C0 | 5 |
| 11 | options ledger, mechanical columns | measured today at b3a043dc (tree 7f58d776); the judgment columns are phase 1's | 6 |
| 12 | encrypted roster's row (not in the plan's list; encryption is in the tag) | one quick-scale row (section 7, item 3) | 4.2 |

The plan's exit has three parts. Reproducible: every figure below carries its command. Noise floor recorded: not yet. Hot-function list agreed: by default.

## 2. Static counts

`sh release/tools/static_counts.sh` prints 184 `name<TAB>value` lines and builds nothing; its header comment defines each line. Two runs print the same bytes. The command for a row below is `sh release/tools/static_counts.sh | grep '^<line>'`, with the line the row names. The plan's figures were taken at `8025423`; the bracket is that commit by the command under the table.

| quantity | value | plan | line |
|---|---|---|---|
| fields of `pub struct Options` (`src/db.rs`) | 423 | | `options_fields` |
| of them under `#[doc(hidden)]` | 158 | | `options_doc_hidden` |
| of them with EXPERIMENT in the rustdoc; both; neither | 115; 37; 187 (423 - 158 - 115 + 37) | | `options_experiment`, `options_doc_hidden_and_experiment` |
| fields of `SegmentOptions`, `BlobOptions`, `StoreCipher`, `EncryptOptions`, `ReadOptions` | 14 (4 hidden), 4, 9 (private), 2, 3 | | `fields:<Type>`, `fields_doc_hidden:<Type>` |
| `#[doc(hidden)]` lines in `src/` | 417 | | `doc_hidden_lines_src` |
| lines of `src/` that mention `unsafe`, code or comment | 470 | | `unsafe_lines_src` |
| `unsafe {`; `unsafe fn`; `unsafe impl` | 399; 41; 28 | | `unsafe_blocks_src`, `unsafe_fn_src`, `unsafe_impl_src` |
| lines holding `SAFETY`; `# Safety` rustdoc headings | 388; 36 | | `safety_comment_lines_src`, `safety_doc_sections_src` |
| engine lines: `src/`, 21 files | 149,584 | | `src_lines`, `src_files` |
| of them inside test modules (49 modules) | 35,138 | | `src_lines_in_test_mods`, `src_test_mods` |
| of them outside test modules | 114,446 | | `src_lines_outside_test_mods` |
| `src/db.rs` lines; of them in its 27 test modules | 74,788; 5,106 | 65K (65,159) | `src_lines:src/db.rs`, `src_test_mod_lines:src/db.rs` |
| `#[test]` lines: `src/`; `tests/`; engine total | 645; 1,189; 1,834 | 531; 680; 1,211 (the same) | `test_attrs:src`, `test_attrs:tests` |
| `#[ignore]` lines: `src/`; `tests/` | 0; 6 | | `ignore_attrs:src`, `ignore_attrs:tests` |
| test files directly under `tests/`, each a test binary | 30 | 23 (23) | `test_files_tests_top` |
| `#[test]` lines of the suite: `bench/src`; `bench/tests` | 164; 12 | | `test_attrs:bench/src`, `test_attrs:bench/tests` |
| rustdoc code blocks in `src/`; of them `text` | 19; 2 | | `rustdoc_code_blocks_src`, `rustdoc_text_blocks_src` |
| lines of `tests/` (32 `.rs` files); `bench/src` (18); `bench/examples` (32) | 102,330; 45,797; 8,875 | | `lines:<dir>`, `files:<dir>` |
| arms of `open` (`bench/src/engines.rs`); `supdb` arms; others; distinct arms in a roster | 660; 644; 16; 27 | | `bench_arms_open`, `bench_arms_open_supdb`, `bench_arms_open_other`, `bench_roster_arms_distinct` |
| doc bytes, eleven files | 1,775,734 | | `doc_bytes_total` |
| rustdoc comment lines in `src/` | 32,794 | | `rustdoc_lines_src` |
| `Cargo.toml` dependencies; dev-dependencies | 3 (libc, lz4_flex, memmap2); 1 (ring) | 3 (3, the same names) | `deps:Cargo.toml:dependencies`, `deps:Cargo.toml:dev-dependencies`, `dep_names:<...>` |
| packages of `Cargo.lock`; `bench/Cargo.lock` | 23; 120 | | `lock_packages:<file>` |
| workflows; their jobs | 10; 21 | | `workflows`, `workflow_jobs_total` |
| banked rows: full, ooc, quick, killed | 8, 34, 86, 2: 130 | | `bench_rows:<dir>`, `bench_rows_total` |
| tracked files; bytes | 2,630; 92,948,487 | | `git ls-tree -r -l b3a043dc \| awk '$2 == "blob" { n++; b += $4 } END { print n, b }'` (the tool's `tracked_files` and `tracked_bytes` follow HEAD) |

Doc bytes by file (`doc_bytes:<file>`): `README.md` 18,724; `CLAUDE.md` 24,748 (the plan: 521 KB; 521,282 at its commit, and 843,723 at `e40d6283`, the commit before #37 cut it); `bench/CLAUDE.md` 80,204; `ROADMAP.md` 41,173; `CONTRIBUTING.md` 2,706; `bench/DESIGN.md` 95,804; `web/README.md` 16,718; `docs/engine.md` 1,033,568; `docs/index-theory.md` 21,550; `docs/runs-form-design.md` 151,718; `docs/zpages-design.md` 288,821. Five tracked Markdown files are outside the eleven, 19,755 bytes together: `git ls-files '*.md' | grep -v '^bench/runs\|^release/'` lists all sixteen.

Engine lines with and without tests. A test module is an inline `mod name {` with `#[cfg(test)]` or `#[cfg(all(test, ...))]` directly above it; its lines run from that attribute to its closing brace. The tool bounds the modules a second way, by brace depth through a lexer that skips strings and comments, and prints the modules the two ways bound differently: 0 (`src_test_split_disagreements`). All 645 `#[test]` lines fall inside the modules (`src_test_attrs_outside_test_mods` 0). 70 test-only items that are not modules count as outside (`src_cfg_test_items_not_mods`). By file: `src_lines:<file>` and `src_test_mod_lines:<file>`.

`StoreCipher`'s nine fields are private; its knobs are seven setters (`grep -c '^StoreCipher,' release/options.csv`).

The plan's figures at its own commit, or at any other:

```sh
r=8025423
git show $r:src/db.rs | wc -l                                                                   # 65159
git grep -c -E '^[[:space:]]*#\[test\]' $r -- 'src/*.rs' | awk -F: '{s+=$NF} END {print s}'     # 531
git grep -c -E '^[[:space:]]*#\[test\]' $r -- 'tests/*.rs' | awk -F: '{s+=$NF} END {print s}'   # 680
git ls-tree --name-only $r tests/ | grep -c '^tests/[^/]*\.rs$'                                 # 23
git show $r:Cargo.toml | awk '/^\[.*dependencies\]/ {on=1; print; next} /^\[/ {on=0} on && /^[a-z0-9_-]+ *=/ {print "  " $1}'
git cat-file -s $r:CLAUDE.md                                                                    # 521282
```

Main is five pull requests past the plan's commit (`git log --oneline 8025423..b3a043dc`): #33 to #37.

### Counts that need a build

```sh
sh scripts/deps.sh                                                # gate on its last line: all deps ready: ...
TMPDIR=<a directory of its own> sh release/tools/static_counts.sh --build
```

It prints the plain lines, then builds the wasm module (`web/build.sh`) and the engine's tests (`cargo test --profile checked --no-run`), and lists each test binary without running a test. Taken once, on a checkout that was `b3a043dc` plus `release/STATUS.md`; not taken again since. The first `--build` on the C0 checkout is the check.

| line | value |
|---|---|
| `rustc` | rustc 1.100.0 (0ef1bc386 2026-11-09) |
| `wasm_bytes`; `wasm_gzip` (`gzip -9`) | 131,556; 53,550 |
| `floor_bytes`; `floor_gzip`, the empty cdylib of `web/floor` | 30,719; 12,971 |
| `tests_built_binaries`; `tests_built_total`; `tests_built_ignored_total`; `tests_built_list_failures` | 31; 1,834; 6; 0 |

The built count equals the textual count in each of the 31 binaries (`tests_built:<target>` against `grep -cE '^\s*#\[test\]' tests/*.rs tests/lex/mod.rs`): `src/lib.rs` 645, `tests/db.rs` 303, `tests/durable.rs` 283, `tests/zpages.rs` 251 are the largest. The six ignored tests are four in `tests/dirkey.rs`, one in `tests/crypt.rs` and one in `tests/alloc_peaks.rs`. The repository holds the wasm size to no number: `web/README.md` ("Size") says "nothing here decides whether a size is acceptable". The builds write ignored paths only (`target/`, `web/supdb.wasm`, `web/floor/target/`).

## 3. The hot-function list and the callgrind gate

### 3.1 The proposal

The tree already has a count by function, compared against a base with identical probe source: the `cost` group. `scripts/cost.sh` builds the probes at a base commit (in a worktree) and at the tree, runs each under callgrind and compares every function's own instructions; `scripts/cost-diff.py` holds the slacks; `scripts/cost-hot.txt` is the list; `scripts/cost-red.sh` is the red input; `tests/data/cost-entries.txt` maps every public entry point of `Reader`, `Db` and `Blob` to a probe or a reason. The group is not in the default gate and its workflow runs by hand (`.github/workflows/cost.yml`).

Proposed: the pinned list is `scripts/cost-hot.txt` as C0 holds it, with every probe, used unchanged as the gate of phases 1 and 2. It has 88 hot lines and 14 bound lines. Under a toggle every function built from source that the toggle collects is held, listed or not, so the list is the least of what is held.

| probes | binary under `bench/examples/` | toggle | operations |
|---|---|---|---|
| `read`, `read_z`, `read_s` | `read_only` | the suite's `get` | 200,000 |
| `scan`, `scan_long`, `scan_z`, `scan_long_z`, `scan_s`, `scan_long_s` | `lag0_probe` | `supdb_bench::engines::supdb_range` | 1,000 |
| `commit_durable`, `commit_buffered` | `load_only` | the suite's `write_batch` | 100,000 |
| `load_durable`, `load_buffered` | `load_only` | none: the whole program | 100,000 |
| `work` | `work_probe` | none | 300,000 |
| `merge` | `merge_walk` | none | 200,000 |
| `land` | `land_probe` | `land_probe::rounds` | 100,000 |
| `seal` | `seal_probe` | `seal_probe::seal` | 100,000 |
| `scan_at` | `scan_at_probe` | `scan_at_probe::scans` | 1,000 |

18 probes, 8 binaries, 14 toggled, 4 whole. Slack (`grep -n 'SLACK = \|LIB_REL = ' scripts/cost-diff.py`): under a toggle 64 instructions a function over the run; with none 0.01 an operation, which is 1,000 instructions a function for the loads, 3,000 for `work` and 2,000 for `merge`; libc and the allocator under a toggle fail past 2 an operation and 5% of their own count, both.

```sh
sed -n '/^probe() {/,/^}/p' scripts/cost.sh | grep ') echo "'                    # the probe table: operations|toggle|binary|arguments
sed -n 's/^PROBES=${\*:-\(.*\)}$/\1/p' scripts/cost.sh | wc -w                   # 18
grep -vc '^#\|^~\|^!\|^$' scripts/cost-hot.txt; grep -c '^~' scripts/cost-hot.txt  # 88; 14
F=tests/data/cost-entries.txt; grep -v '^#' $F | grep -vc '^$'; grep -v '^#' $F | grep -v '^$' | awk -F'\t' '$2!="-"' | wc -l; grep -v '^#' $F | awk -F'\t' '$2=="-"' | grep -c gap   # 408 entries; 14 name a probe; 9 reasons say "gap"
```

Not covered by any probe: reads through a handle (`Db::reader`) and `Latest` or `Snapshot` isolation; a scan through a form file with unsealed keys over it; open and recovery; checkpoint; the held log (`SyncPolicy::Explicit`); handles that write a shared table (`Reader::put`, `append`, `delete`, `rmw`, which the entries call "the gap, owed a probe"); `SparseBlob`; `Txn`; and every path under a cipher, which the job leaves out on purpose (it counts "a store in the clear"). In whole mode a fixed cost under the slack reads green: `merge` opens its store twice, so a cost an open under 1,000 instructions a function is not seen; `work` makes 151 commits, so a cost a commit under about 20 is not seen.

Candidate probe lines over binaries the tree has. None has been counted. Each is admitted only if two counts of C0 against itself read it exact, or its movers can be bounded as the `~` lines of `scripts/cost-hot.txt` are.

```sh
commit_log_durable) echo "100000|$WB|load_only|supdb 100000 1 shuffled" ;;          # A: the commit through the log, toggled
commit_log_buffered) echo "100000|$WB|load_only|supdb-ingest 100000 1 shuffled" ;;
get_h) echo "16666|shared_ops::phase_gets|shared_ops|supdb-heldnoframes 100000 1" ;;  # B: handles that write a shared table
put_h) echo "16666|shared_ops::phase_puts|shared_ops|supdb-heldnoframes 100000 1" ;;
rmw_h) echo "16666|shared_ops::phase_rmws|shared_ops|supdb-heldnoframes 100000 1" ;;
read_h) echo "16666|shared_ops::phase_gets|shared_ops|supdb 100000 1" ;;              # C: reads through a handle; needs SHARED_OPS_FRESH=1
```

Each name also goes into the `SUITE` case of `scripts/cost.sh`; B adds the suite's `SupdbSharedWriter` methods `get`, `update_batch` and `rmw` to its `ITEMS` and moves four entries of `tests/data/cost-entries.txt` from a reason to a probe. A probe line cannot carry C's variable, so C is run by hand or gets a source of its own.

Proposed rules, agreed by default (section 7, item 12). Phase 1, each pull request against the commit it is based on: `cost: green` passes; `cost: RED` passes only where every `HOT GAINED` and `LIBRARY GAINED` line is accounted for in the pull request as inlining that moved (caller and callee in one probe, net at or under zero), as padding (shown by a sum by mnemonic), or as a sideways move declared before the count because a counted arm or probe pinned a non-default value of the deleted option; no slack, bound or hot line changes but for functions that are gone; one family of options a pull request; at the phase's last commit the two self-counts are taken again and kept. Phase 2 is count-neutral: the pull request carries its rename table and updates `scripts/cost-hot.txt` and `scripts/cost-red.sh` in the same commit; with the base's names mapped every toggled probe reads `0 of them moved` outside the bounded functions; a toggled function that moves, down as well as up, is restored and not explained, unless a sum by mnemonic shows padding alone; the first pull request is a small move, one `impl` block and one free function, to confirm how names read.

What phase 2 does to the gate itself. The comparator is by name. 67 of the list's 102 lines name items of `src/db.rs` itself, 30 of them methods of `Reader` or `Db` and 37 naming 26 other types and free functions; 10 of the 14 bound lines are among the 67. A type or free function that moves modules gets a new name and reads `HOT GAINED` at its whole count. `scripts/cost-red.sh` makes 5 edits of `src/db.rs` by exact text, so the `lint` group fails until the script follows a split in the same commit.

```sh
L=$(grep -v '^#\|^$' scripts/cost-hot.txt | sed -e 's/^[~!]//' -e 's/^[a-z_,]*: \{1,\}//' -e 's/ .*$//'); S='^supdb::db::\(zfile\|zpages\|zlz4\|zselect\|blocks\|vault\|pager\|onefile\|dirkey\)::'
echo "$L" | wc -l; echo "$L" | grep '^supdb::db::' | grep -vc "$S"; echo "$L" | grep -c '^supdb::db::\(Reader\|Db\)::'    # 102; 67; 30
grep '^~' scripts/cost-hot.txt | sed -e 's/^~//' -e 's/^[a-z_,]*: \{1,\}//' | grep '^supdb::db::' | grep -vc '^supdb::db::\(zfile\|zselect\)::'   # 10
grep -c 'edit "$R/h/src/db.rs"\|>>"$R/h/src/db.rs"' scripts/cost-red.sh                                                 # 5
```

### 3.2 The commands at C0

The job adds a git worktree to the checkout and builds the tree as it stands: the checkout is clean, at the commit counted, with no other cargo running and no edit until the last line.

```sh
sh scripts/deps.sh && sh scripts/deps.sh profile     # valgrind; the count needs nothing else of the second
git status --short; git rev-parse HEAD               # nothing; C0
KEEP=<a directory outside the checkout>; mkdir -p "$KEEP"
export COST_DIR="$KEEP/w" COST_BASE_TARGET="$KEEP/base-target"    # COST_DIR is emptied at the start of every run
COST_BASE=HEAD sh scripts/check.sh cost >"$KEEP/self-1.log" 2>&1; echo "exit $?"    # the red input, then the count
mkdir "$KEEP/self-1" && cp "$COST_DIR"/out/*.self "$KEEP/self-1/"
sh scripts/cost.sh HEAD >"$KEEP/self-2.log" 2>&1; echo "exit $?"                    # the count alone
mkdir "$KEEP/self-2" && cp "$COST_DIR"/out/*.self "$KEEP/self-2/"
grep -E '^== |hot functions ran|^  bound |HOT GAINED|LIBRARY GAINED|EMPTY|PARTIAL|NO HOT|^cost' "$KEEP"/self-[12].log
```

`$COST_DIR/out/head.<probe>.self` is the absolute count: one line a function with its own instructions, and `PROGRAM TOTALS`. One function: `grep -F '<supdb::db::Reader>::read_all' "$KEEP/self-1/head.read.self"`; a count an operation is that over the probe's operations. Run to run on one binary:

```sh
python3 -B scripts/cost-diff.py "$KEEP/self-1/head.read.self" "$KEEP/self-2/head.read.self" 200000 scripts/cost-hot.txt read toggle 'supdb::,supdb_bench::,read_only::'
```

C0 against itself shows that the job runs end to end on the host, which functions move between two builds of one source, that the 14 bounds hold, and which hot lines no probe reaches. It says nothing about a change. The kept listings are a record and not a base: every gate run counts its base again with the head's probes.

The gate for a pull request, on its head with a clean tree. Name the base: with none, `scripts/base.sh` takes the merge-base of HEAD and `origin/main`, which for a pull request into `release-candidate` is the commit where that branch left main.

```sh
COST_BASE=<the commit the pull request is based on> sh scripts/check.sh cost >"$KEEP/pr-N.log" 2>&1; echo "exit $?"
grep -n '^cost\|^base: ' "$KEEP/pr-N.log" | tail -3
```

Durations and disk, as files state them: the group is "most of an hour" (`scripts/check.sh`, the `cost` group's comment; `.github/workflows/cost.yml`, whose `timeout-minutes` is 150); a probe binary is "a third of a gigabyte" (`scripts/cost.sh`, header), 8 a side. No file states a total for the three target directories a run uses.

### 3.3 Reading the verdict

| outcome | `sh scripts/check.sh cost` | `sh scripts/cost.sh BASE` |
|---|---|---|
| green | exit 0; last line `all checks passed: cost`, with `cost: green` above it | exit 0; last line `cost: green` |
| red: a hot function gained past its slack, or a probe collected nothing | exit 1; last line `cost: RED` | exit 1; last line `cost: RED` |
| no count: the base names no commit, a build or a probe failed, or the suite's per-operation code differs between the sides | exit 1; the last line is the refusal, which begins `cost: ` | exit 2; the same line |
| the red input failed | exit 1 before any count; no `cost-red: ok` line | not run |
| the base cannot be decided | exit 1; the last line begins `base: ` | the base is its argument |

Through the group exit 1 is four things, so the log is read, not the status. A red is read by function first, then by instruction, then by bisect:

```sh
grep -E '^== |hot functions ran|^  bound |HOT GAINED|LIBRARY GAINED|EMPTY|PARTIAL|NO HOT' "$KEEP/pr-N.log"
sed -n '/^== land /,/^== /p' "$KEEP/pr-N.log"        # one probe: its 25 largest moves, signed
T='<supdb_bench::engines::Supdb as supdb_bench::engines::Engine>::get'
valgrind --tool=callgrind --dump-instr=yes --callgrind-out-file="$KEEP/head.read.instr.out" --collect-atstart=no "--toggle-collect=$T" bench/target/release/examples/read_only supdb 100000 1 n-none
valgrind --tool=callgrind --dump-instr=yes --callgrind-out-file="$KEEP/base.read.instr.out" --collect-atstart=no "--toggle-collect=$T" "$COST_BASE_TARGET/release/examples/read_only" supdb 100000 1 n-none
sh scripts/cost.sh <the commit before> <probe>       # the bisect, a commit at a time; then a build with each hunk out
```

- A caller that gained what a callee lost in the same block is inlining that moved. `+N` on a new name beside `-N` on an old one is a rename.
- A gain with no source change is summed by mnemonic in both builds before it is read by line: padding counts as instructions and the gate cannot tell. No script of the tree joins counts to the disassembly; it is by hand.
- A change to the toolchain, a lockfile, a profile or `bench/.cargo/config.toml` is a new compiler: a pull request of its own, not gated by the count, with the self-counts taken again after it.
- A struct that grows or shrinks moves functions that do not name it; phase 1 removes fields, so expect it and bisect by hunk.
- `LIBRARY GAINED` with no engine function beside it is not chased: libc and the allocator move between two runs of one build.
- A pull request that edits a probe, or one of the suite's per-operation items, changes what both sides run or is refused.
- An arm a probe names (`supdb`, `supdb-ingest`, `supdb-nocachezpages`, `supdb-nocachezstride`), once renamed or removed, fails that side's probe: exit 2 from the job, 1 from the group.

## 4. The rows and the noise floor

### 4.1 Set-up, once

`C0` is the release-candidate commit. `SRC` is a detached checkout of it with no tracked change, which nothing edits or commits to until the last run has ended: the binary's stamp is made at build time (`sha` in `bench/build.rs`), a later build restamps it, and a resume refuses a stamp that differs. `BASE` is a directory outside every checkout, on the machine's disk, for rows, tables, logs and each run's `TMPDIR`.

```sh
cd "$SRC" && git status --porcelain --untracked-files=no     # must print nothing
sh scripts/deps.sh                                           # gate on: all deps ready: rust node libclang faster
cd bench && cargo build --release --bin bench                # from bench/, where .cargo/config.toml is
./target/release/bench stamp                                 # first field: C0's full hash, no -dirty
./target/release/bench machine                               # last line: class: ...
mkdir -p "$BASE/rows" "$BASE/pairs" "$BASE/logs" "$BASE/tmp"
sh scripts/ooc.sh --prove-red                                # a smoke: the gate fired on a bound of 67108864 bytes
./target/release/bench run --scale quick --top 10000 --bottom 10000 --reps 1 --out "$BASE/smoke"    # a smoke: every arm opened once
```

`scripts/deps.sh` fetches FASTER's sources at the pin in `bench/vendor/faster/PIN` where `bench/vendor/faster/src` is absent, as in a new checkout. Where the fetch is refused, copy `bench/vendor/faster/{src,LICENSE,COMMIT}` from `/home/user/supdb`, which has them at the pin (`cat bench/vendor/faster/PIN bench/vendor/faster/COMMIT` prints one hash twice).

Before every run:

```sh
T="$BASE/tmp/<run-name>"; mkdir -p "$T"; export TMPDIR="$(cd "$T" && pwd -P)"; df -h "$TMPDIR"
env | grep -E '^(SUPDB_|GLIBC_TUNABLES|MALLOC_ARENA_MAX|RUSTUP_TOOLCHAIN)'     # must print nothing
```

### 4.2 The rows

From `$SRC/bench` unless said. Keep both streams of each: `> "$BASE/logs/<run>.out" 2> "$BASE/logs/<run>.err"`.

| row | command | lands at |
|---|---|---|
| quick, numbered | `cd "$SRC" && sh scripts/check.sh quick` | `$SRC/bench/runs-ci/quick/<utc>-<sha7>.json` |
| quick, ids (section 7, item 4) | `cd "$SRC" && KEYS=ids sh scripts/check.sh quick` | the same directory |
| full, numbered | `./target/release/bench run --scale full --reps 7 [SHAPE] --out "$BASE/rows"` | `$BASE/rows/full/` |
| full, ids | the same with `--keys ids` | `$BASE/rows/full/` |
| ooc, 1.25 GiB, numbered | `sh scripts/ooc.sh 1342177280 --out "$BASE/rows"` | `$BASE/rows/ooc/` |
| ooc, 1 GiB, numbered | `sh scripts/ooc.sh 1073741824 --out "$BASE/rows"` | `$BASE/rows/ooc/` |
| ooc, 1.25 GiB, ids | `sh scripts/ooc.sh 1342177280 --keys ids --out "$BASE/rows"` | `$BASE/rows/ooc/` |
| ooc, 1 GiB, ids | `sh scripts/ooc.sh 1073741824 --keys ids --out "$BASE/rows"` | `$BASE/rows/ooc/` |
| encrypted roster (section 7, item 3) | `cd "$SRC" && sh scripts/deps.sh openssl && cd bench && cargo build --release --no-default-features --features enc --target-dir target-enc --bin bench && ./target-enc/release/bench run --scale quick --arms enc --out "$BASE/rows-enc"`, then `python3 scripts/encrow.py "$BASE/rows-enc/quick/<row>.json"` | `$BASE/rows-enc/quick/` |

- **quick.** The group builds, removes `bench/runs-ci`, runs the row, gates it against `bench/runs/` and draws its figures. Verdict line: `all checks passed: quick`. Copy the row out at once, since the group's next run deletes it: `mkdir -p "$BASE/rows/quick" && cp bench/runs-ci/quick/*.json "$BASE/rows/quick/"`.
- **full.** `--reps 7` is the scale's default (`Scale::reps`, `bench/src/lib.rs`: quick 5, full 7, ooc 3). `[SHAPE]` is the banked shape (section 7, item 1). The two banked full rows at `a728c4c` are `--top 3000000 --arms supdb,lmdb,rocksdb-tuned,supdb-ingest,lmdb-nosync,rocksdb-tuned-nosync`. With no `--top` the ladder on this host is ten rungs to 300,000,000 keys (4.5).
- **ooc.** `bench/scripts/ooc.sh BYTES [options]` refuses a binary whose stamp ends `-dirty`, makes room (`bench/scripts/room.sh`) and runs `bench run --scale ooc --bound BYTES`: rungs 3,000,000 and 10,000,000, 3 reps, read operations divided by 10 (`run::OOC_OPS_DIV`), the bounded roster, each pass in a process and a memory group of its own. Its stdout holds the run's two streams and the row's path; its stderr holds the verdict, `ooc.sh: bounded run under N bytes banked`. Exit 137 is a pass killed under the bound, and the row is then under `$BASE/rows/killed/ooc/`. Without `--out` the row is written into the checkout's `bench/runs/ooc/`.
- **Rosters.** Default 15 arms, bounded 10, `--arms enc` 12: `awk '/^macro_rules! roster/ { on = 1 } on { print } /^pub const ENC_BUFFERED_ARMS/ { last = 1 } last && /^];/ { exit }' bench/src/engines.rs`.

### 4.3 A run that was cut, the gate, banking

A run writes a ledger as it goes, `<out>/inflight/<utc>-<sha7>/`, and removes it at its end (`bench/src/ledger.rs`). A resume takes its plan from the ledger, keeps every whole rep and refuses `--arms`, `--top`, `--bottom`, `--ops-div`, `--keys`, `--fresh-process` and `--one-process` (`cmd_resume`, `bench/src/main.rs`); it is refused where the binary's stamp or the machine's class differs from the ledger's.

```sh
./target/release/bench run --resume "$BASE/rows/inflight/<utc>-<sha7>"       # a full row
sh scripts/ooc.sh 1342177280 --resume "$BASE/rows/inflight/<utc>-<sha7>"     # a bounded row: BYTES as the ledger's
./target/release/bench gate "$BASE/rows/<scale>/<row>.json" --runs runs      # exit 1: worse than every row of its window
```

The quick row's ledger is `$SRC/bench/runs-ci/inflight/<utc>-<sha7>`, since the group runs with `--out runs-ci`. The group's next run removes it, and `bench run --resume` on it runs neither the group's gate nor its figures. The gate reads the last ten rows of the row's class and scale under `bench/runs/` and says "no band yet" under three; a row of ids is a class of its own. Banking is a signed commit and a person's decision (`bench/DESIGN.md`, "Rows"): `cp` the row into `bench/runs/<scale>/` (a killed row into `bench/runs/killed/<scale>/`), then `sh bench/scripts/figures.sh && sh bench/scripts/figures.sh --check`. It changes tracked files, so not in `SRC` while a run is owed (section 7, item 7).

### 4.4 The noise floor

`bench ab --arms supdb,supdb` through `bench/scripts/pair.sh` is two sittings of one arm against itself; each sitting gives two series of identical code, alternated rep by rep. In core, for the quick and full rows' cells:

```sh
cd "$SRC/bench"
for K in numbered ids; do for SIZE in 10000 30000 100000 300000 1000000 3000000; do
  OUT="$BASE/pairs" NAME="floor-supdb-core-$K" sh scripts/pair.sh supdb supdb "$SIZE" --reps 7 --keys "$K" \
    --passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled,lag,recover --pairs ' '
done; done
```

Under each bound, for the bounded rows' cells:

```sh
for B in 1073741824 1342177280; do for K in numbered ids; do for SIZE in 3000000 10000000; do
  OUT="$BASE/pairs" NAME="floor-supdb-b$((B >> 20))m-$K" sh scripts/pair.sh supdb supdb "$SIZE" --bound "$B" --ops-div 10 --reps 3 --keys "$K" \
    --passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled,lag --pairs ' '
done; done; done
```

Each call writes `$BASE/pairs/$NAME-$SIZE-{1,2}.{out,err}` and `$NAME-$SIZE-head.txt`. One call at a time. `RESUME=1` keeps a sitting whose table is whole. A sitting with a killed rep exits 137 and `pair.sh` stops before the next. The same loops with another arm in both places give that arm's floor (section 7, item 2).

Not written yet, and the floor is not recorded until they are: a reader of the pairing tables (no script of the repository reads one; `pair.sh` only looks for the ` quantities: ` line); a comparer of two rows of one class, cell by cell, with the floor beside each cell and each quantity's direction taken from `gate::higher_is_better` (`bench/src/gate.rs`); a queue that runs the pairings one at a time and skips a table that is whole. They belong under `release/tools/` with a self-test.

The floor of a cell (arm, key shape, bound, rung, workload, quantity) is the band of the medians identical code read: over S sittings, the least and the greatest of the 2S medians, and their ratio W. At phase 6 the loops are run with the final binary and with C0's binary again in the same visit (`BIN=<C0's binary> sh scripts/pair.sh ...`), so C0's `bench/target` and `bench/target-enc` are kept as built. A cell is a regression above the floor where every median of the final binary is worse than every median of C0's binary taken in that visit and the nearest two differ by more than W; the same on the better side is flagged; anything else is not decided. `CLAUDE.md` ("Measuring") adds that a cell under a tenth is not resolved by one pairing and a verdict needs a second sitting. This cannot decide a regression smaller than W, a cell that runs in two shapes by a decision the engine makes once a rep, a cell with no self-pairing, or a difference between a phase 0 row and a phase 6 row as such, which are two runs on two days.

### 4.5 What each run needs, and this host

| need | this host, when read | command |
|---|---|---|
| disk for a rung: three times its records at 116 bytes a record (`run::run`, `bench/src/run.rs`) | 3M keys ask for 1.04 GB, 10M 3.48 GB, 100M 34.8 GB, 300M 104.4 GB; 52.5 GB free, which other work moves | `df -B1 --output=avail /tmp`; the block below |
| a cgroup v1 memory mount, writable, and uid 0 (`Bound::enter`, `bench/src/bound.rs`) | present; uid 0 | `grep ' cgroup .*memory' /proc/mounts; id -u` |
| the shell's own memory group above the bound | limit 14,338,371,584 bytes (13.35 GiB), hierarchical | `g=$(awk -F: '$2=="memory"{print $3}' /proc/self/cgroup); cat /sys/fs/cgroup/memory$g/memory.limit_in_bytes /sys/fs/cgroup/memory$g/memory.use_hierarchy` |
| stores on a disk, no swap, a page cache `room.sh` can drop | `/dev/vda ext4`; no swap; writable | `findmnt -n -o SOURCE,FSTYPE -T /tmp; cat /proc/swaps; test -w /proc/sys/vm/drop_caches && echo writable` |
| the suite's machine level, twelve CPU features (`LEVELS`, `bench/build.rs`) | all twelve | `for f in avx avx2 bmi1 bmi2 f16c fma abm movbe pclmulqdq popcnt sse4_2 xsave; do grep -m1 '^flags' /proc/cpuinfo \| grep -c -w $f; done` |
| toolchain 1.100.0, valgrind | installed | `ls ~/.rustup/toolchains; command -v valgrind` |
| a built suite | none here: no `bench/target` | `ls -d bench/target bench/target-enc` |

```sh
python3 -I -c "
import math
kb = int(open('/proc/meminfo').readline().split()[1]); top = math.ceil(kb * 1024 * 1.5 / 116)     # run::full_top
l = [10000]
while l[-1] < top: l.append(l[-1] * 3 if len(l) % 2 else l[-1] * 10 // 3)                          # ladder, bench/src/lib.rs
print(top, len(l), l[-1]); [print(r, round(116 * r / 1e9, 2), round(3 * 116 * r / 1e9, 2)) for r in l[5:]]"
# 218129426 10 300000000, then keys, records in GB, GB asked for
```

This host can run every command above: it meets each requirement of the bound, and the banked shapes fit its disk. The full scale with no `--top` does not: its last rung is refused for disk, and `bench/DESIGN.md` ("Scale") records a pass at 100 million keys that "ran 27.5 hours without supdb finishing it". What the host lacks is quiet: a timing run needs the machine to itself, one run at a time.

The host's class decides which series a row joins. This container is `Intel(R) Xeon(R) Processor @ 2.30GHz`, 4 CPUs, 16,473,316 kB, kernel `6.18.44-fc-v880` (`grep -m1 'model name' /proc/cpuinfo; nproc; grep MemTotal /proc/meminfo; uname -r`); `bench machine` at C0 prints the class. One line a banked row, and the rows by class:

```sh
rows() { python3 -I -c "
import json, glob
for f in sorted(glob.glob('bench/runs/*/*.json') + glob.glob('bench/runs/killed/*/*.json')):
    r = json.load(open(f)); m = r['machine']; ms = [x for x in r['measurements'] if x.get('size') is not None]
    print(*f[11:-5].rsplit('/', 1), str(r.get('keys') or 'numbered').lower(), (m.get('mem_limit_bytes') or 0) >> 20, m['cpu_model'].split()[-1], max(x['size'] for x in ms), len({x['arm'] for x in ms if x.get('arm')}), len(r.get('killed', [])), len(r.get('resumptions', [])))"; }
rows                                                    # scale, row, keys, bound in MiB, CPU, top rung, arms, kills, later sittings
rows | awk '{ print $1, $3, $4, $5 }' | sort | uniq -c
```

| series | rows on a 2.30GHz host | rows on a 2.10GHz host | the gate, for a row taken here |
|---|---|---|---|
| quick, numbered | 16 | 29 | a band |
| ooc under 1 GiB, numbered | 4 | 4 | a band |
| ooc under 1.25 GiB, numbered | 8 | 15 | a band |
| full, numbered; full, ids | 0; 0 | 3; 1 | no band yet |
| quick, ids; ooc under 1.25 GiB, ids; ooc under 1 GiB, ids | 0; 0; 0 | 1; 1; 0 and one killed | no band yet |

No banked row is at `b3a043dc`. The newest are at `33bc663` (quick; ooc under 1 GiB), `5fea7c0` (ooc under 1.25 GiB) and `a728c4c` (full, and every row of ids). From each to `b3a043dc`, `git diff --shortstat <commit> b3a043dc -- src` prints 15 files changed, with 28,283 insertions for the first two and 42,784 for the third (section 7, item 6).

### 4.6 Durations as recorded, and the sitting plan

A row's name carries its start. The interval to the next row's start, or to the commit that banked it, is an upper bound on the run.

```sh
gap() { python3 -c "import datetime as d,sys; t=lambda s: d.datetime.strptime(s,'%Y%m%dT%H%M%SZ').replace(tzinfo=d.timezone.utc) if s.endswith('Z') else d.datetime.fromisoformat(s); print(t(sys.argv[2])-t(sys.argv[1]))" "$1" "$2"; }
gap 20261122T050933Z 20261122T100903Z                         # two rows' starts: 4:59:30
gap 20261127T001319Z "$(git log -1 --format=%aI ec6d9e81)"    # a row's start to its banking commit: 0:17:06
```

| run | recorded | source |
|---|---|---|
| quick, 15 arms, a 2.30GHz host | 17 m 06 s to its banking commit; 18 m 16 s to the next row's start | row `20261127T001319Z`; commit `ec6d9e81`; row `20261127T003135Z` |
| full, six arms, 7 reps, to 3M keys, numbered, a 2.10GHz host | at most 4 h 59 m 30 s | rows `20261122T050933Z`, `20261122T100903Z` |
| the same, ids | at most 5 h 20 m 15 s | row `20261122T100903Z`; commit `f017c79e` |
| ooc, 10 arms, 1.25 GiB | at most 3 h 21 m 34 s | rows `20261126T125550Z`, `20261126T161724Z` |
| ooc, 10 arms, 1 GiB | at most 4 h 12 m 09 s | row `20261127T003135Z`; commit `5f591089` |
| ooc, 10 arms, 1.25 GiB, in three sittings | 5 h 50 m 30 s, start to banking commit | row `20261127T050109Z`; commit `0b2699d5` |
| the comparators' build | "a ten-minute C++ build the first time" | `scripts/check.sh`, header |
| a pairing against a comparator at 10M keys | "half an hour" a sitting | `bench/scripts/pair.sh`, header |

No duration is recorded for a self-pairing, a full row of fifteen arms, a bounded row of ids with ten arms, or the encrypted roster's row. The plan below is an estimate from the records above, not a measurement.

| item | sittings | estimate | from |
|---|---|---|---|
| build, smokes, quick row | 1 | 0.6 h | build 10 m, quick 18 m |
| full, numbered, six arms | 3 | 5 h | 4 h 59 m 30 s |
| full, ids, six arms | 3 | 5.3 h | 5 h 20 m 15 s |
| ooc, 1.25 GiB, numbered | 3 to 4 | 3.4 h | 3 h 21 m 34 s |
| ooc, 1 GiB, numbered | 3 to 4 | 4.2 h | 4 h 12 m 09 s |
| ooc, 1.25 GiB, ids; ooc, 1 GiB, ids | 3 to 4 each | 3.4 h; 4.2 h | no record with ten arms; taken as the numbered rows |
| floor in core, both key shapes | 4 to 6 | 6.7 h | the full rows scaled by 32 arm-reps a rung against a six-arm row's 48 |
| floor under both bounds, both key shapes | 4 to 8 | 6.1 h | the bounded rows scaled by 12 arm-reps a rung against a ten-arm row's 30 |
| encrypted build and row, if in | 1 | 0.5 h | no record: an allowance |

About 39 hours in 28 to 38 sittings, with nothing else on the machine. A full row over the fifteen-arm roster would be about two and a half times the six-arm one. Phase 6 repeats the set.

Rules that have cut runs here (`bench/CLAUDE.md` unless said): sittings of under two hours with the ledger between them, each ending at a whole rep and resumed with `--resume` or `RESUME=1`; one timing run at a time, no build or test beside it; no `SUPDB_` variable in a row's environment (`pair.sh` unsets them, `bench run` does not); nothing a run in flight reads is edited (its binary, `ooc.sh`, `pair.sh`, `room.sh`); `room.sh` before a bounded run, never beside one; after a killed pairing remove `$TMPDIR/supdb-ab-<pid>` by name; read the verdict line, not a pipe's status; `pair.sh` writes to `pairs/` under the current directory unless `OUT` is set; a restart can move the session to a host of another class, which a resume refuses.

## 5. Timings not taken

Each wants the machine to itself and one cargo at a time. `sh scripts/deps.sh` first.

```sh
# wall time of each gate group (the default set of scripts/check.sh)
for g in build test stride onefile lint wasm bench lists; do
  s=$(date +%s); sh scripts/check.sh "$g" >"gate-$g.log" 2>&1; echo "$g rc=$? $(( $(date +%s) - s ))s"
done
# wall time of each engine test binary: libtest prints "finished in <s>" on each "test result:" line,
# and the "Running" line above it names the binary; then the gate's other two modes
cargo test --profile checked --no-fail-fast 2>&1 | grep -E '^ +Running |^test result:'
SUPDB_TEST_STRIDE=1 cargo test --profile checked --no-fail-fast 2>&1 | grep -E '^ +Running |^test result:'
TMPDIR=$(mktemp -d) SUPDB_TEST_ONEFILE=1 cargo test --profile checked --no-fail-fast 2>&1 | grep -E '^ +Running |^test result:'
# compile time from nothing: cargo prints "Finished ... in <t>" and writes cargo-timings/ under the target directory
CARGO_TARGET_DIR=$(mktemp -d) cargo test --profile checked --no-run --timings
CARGO_TARGET_DIR=$(mktemp -d) cargo build --release --timings
(cd bench && CARGO_TARGET_DIR=$(mktemp -d) cargo build --release --timings)
```

Also not taken: how long the `cost` group, each row and each pairing runs here (sections 3.2 and 4.6 quote what files record). Two counts that need a build were not taken either:

```sh
(cd bench && cargo test --profile checked --lib --bins --tests -- --list 2>/dev/null | grep -c ': test$')   # the suite's tests as built; textual 176
cargo test --profile checked --doc -- --list 2>/dev/null | grep -c ': test$'                                # the engine's doctests; textual 17
```

## 6. The options ledger

`release/options.csv` is the output of `release/tools/options_ledger.py` (python3, standard library, no build) over `b3a043dc`: one row an option, 453 rows and 30 columns. The mechanical columns are filled. `category`, `decision` and `note` are empty in every row: they are phase 1's. The tool's header defines each column and says what each count over- and under-counts.

```sh
python3 -B release/tools/options_ledger.py | cmp - release/options.csv                 # silent: the ledger is the tool's output for the tree
python3 -B release/tools/options_ledger.py --summary release/options.csv               # every tally below (by_type and cross lines)
python3 -B release/tools/options_ledger.py --keep release/options.csv 2>/dev/null | cmp - release/options.csv   # the check once judgment is filled
python3 -B release/tools/options_ledger.py --sites Type.name 2>&1 >/dev/null | grep ' site '                    # the sites counted for one row
```

| type | rows | `doc(hidden)` | says EXPERIMENT | says refuted | an arm sets it | no arm sets it | nothing reads it | only tests set it | nothing sets it | fuzz draws it |
|---|---|---|---|---|---|---|---|---|---|---|
| `Options` | 423 | 158 | 115 | 24 | 379 | 44 | 0 | 34 | 7 | 61 |
| `SegmentOptions` | 14 | 4 | 3 | 0 | 6 | 8 | 0 | 3 | 1 | 0 |
| `BlobOptions` | 4 | 0 | 0 | 0 | 0 | 4 | 0 | 0 | 0 | 0 |
| `StoreCipher` (setters) | 7 | 5 | 0 | 0 | 5 | 2 | 0 | 1 | 0 | 0 |
| `EncryptOptions` | 2 | 0 | 0 | 0 | 0 | 2 | 0 | 0 | 0 | 0 |
| `ReadOptions` | 3 | 0 | 0 | 1 | 1 | 2 | 0 | 0 | 0 | 0 |
| all | 453 | 167 | 118 | 25 | 391 | 62 | 0 | 38 | 8 | 61 |

- Of `Options`' 423 fields 332 are `bool`: 212 default to `true`, 120 to `false`. 226 are read at exactly one site.
- 115 say EXPERIMENT: 37 hidden, 78 in the public rustdoc. 236 fields are EXPERIMENT or hidden or both; 187 are neither.
- No row is unread, so the plan's `delete-unread` category has no candidate by text. The nearest mechanical classes are `never_set` (8) and `tests_only` (38); no category of the plan names a test seam.
- The suite's `with_policy` leaves 41 `Options` fields unset (`bench_mapped` 382), and 11 fields are set from a variable of the process environment (`env_dependent`).

## 7. Decided

The coordinator took the default for each question the sections raised;
the owner may overrule any of them in `release/STATUS.md`.

1. **The full rows** are the banked shape: six arms (`supdb`, `lmdb`,
   `rocksdb-tuned`, `supdb-ingest`, `lmdb-nosync`, `rocksdb-tuned-nosync`)
   to 3,000,000 keys, in both key shapes. The held arms are measured in the
   quick and bounded rows, whose rosters already hold them.
2. **The floor** is `supdb` paired with itself. Other arms' cells are read
   against it, and a cell with no floor of its own is reported as such.
3. **The encrypted roster** gets one quick-scale `--arms enc` row at C0 and
   one at the final commit, kept beside the baseline; no floor.
4. **The quick row** is numbered keys only, the series that has a band.
5. **The 1 GiB bounded rows** take both rungs, as the newest rows do.
6. **Every row is retaken at C0.** All existing rows predate #35.
7. **Banking.** The baseline rows are committed on a measurement branch,
   `release-measure`, cut from `release-candidate`, and not to main. They
   and phase 6's rows go into `release-candidate` together after the last
   code commit, which the plan allows.
8. **The gate's band.** Where a class has no prior rows the gate reads "no
   band yet", and the pairings carry the verdict.
9. **No row-shaped control.** The self-pairings are the floor.
10. **A killed run** is taken once more; a second kill stands as the killed
    row, recorded.
11. **C0's binaries are kept.** If phase 6 lands on another host class,
    C0's binary runs there beside the final one.
12. **The hot-function list** is `scripts/cost-hot.txt` with every probe of
    the `cost` group as it stands, under the phase 1 and phase 2 rules of
    3.1.
13. **New probes.** Candidates A, B and C are counted at C0 after the
    self-count and admitted where they read exact.
14. **Paths no probe counts** stay with the tests and phase 6's rows.
15. **Names in phase 2.** A renames file read by `scripts/cost-diff.py`,
    with a red input of its own, lands in phase 2's first pull request,
    before anything moves.
16. **The wasm size** at C0 (131,556 bytes, 53,550 under gzip) is the figure
    phases 1 and 2 may not exceed without a reason in the pull request.

## 8. Open defects and what is not verified

- **`release/tools/options_ledger.py`, the guard behind `unread`.** The header says a destructuring pattern of an option type in non-test `src/` makes the tool exit 1, so a 0 in `read_sites_src` is no textual read. The tool takes `Type { .. }` for a pattern only where its closing brace is followed by `=` or `=>`. A pattern in a function or closure parameter, inside a tuple or `Some(..)`, in `matches!`, in a guarded `match` arm, in a `for`, as `Self { .. }` or through a type alias is not caught: the option reads as `unread` with exit 0, and in six of those positions the pattern is also counted as a set. No cell of `release/options.csv` is wrong by it today: no row is flagged `unread`, and non-test `src/` holds no such pattern. It matters for every later run of the tool over a tree phases 1 and 2 have changed. Open.
- The `--build` lines of section 2 were taken once and not again.
- No command of sections 3.2, 4 and 5 has been run to its end. Their flags, scripts, defaults and paths were checked against the sources.
- In the ledger, `bench_arms_setting` is checked by a second evaluation for the rows mapped `x`, `!x` or `?x`, and for 12 of the 86 rows whose mapping is `fn(..)`, `lit(..)` or `direct(..)`. The columns `doc_markers`, `arms_in_doc`, `fuzz_how`, `docs_mentions`, `rs_mentions` and `gate_mentions` were not checked independently.
- `doc_bytes_total` counts the eleven files the tool's header lists; five tracked Markdown files are outside it (section 2).
