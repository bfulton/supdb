# run-dir (pricing unit): a run's temp name has no directory barrier before its first ack

Base HEAD 6f2e8dd3 (0c6748bb + other-slot, rename-arm, due-log; run-close did not land). Tree clean at start.

## Step 2 (in progress): tests + seams in, before any fix
- Seam: vault::DirSync::Run kind; Db::watch_dir_syncs / dir_syncs_watched / fail_next_run_dir_sync (Shared::dir_syncs, fail_run_dir_sync), hook rides IO_SEAMS.
- Tests: tests/durable.rs section "run-dir" (7 tests, rd_ prefix).
- TMPDIR: stage0/tmp/run-dir
- before.log: 5 of 7 fail at their barrier/answer assertion (always, everyn, sync, again, fail); cipher and flush pass before (holding tests, not evidence).

## Step 3 (done): fix = lazy shape. Direct.log_owed:bool -> owed:u8 (RUN_OWES_LOG|RUN_OWES_DIR), DIR set at open in the clear;
  due branch `d.owed != 0` -> run_due_first (log, run sync, run_dir_barrier); run_again and sync_run (Db::sync) pay run_dir_barrier after the run's sync.
  run_dir_barrier: vault::sync_dir_as(dir, DirSync::Run), failure keep_lost.
  Measuring fields: Options::price_run_dir_old (no barrier), price_run_dir_eager (sync_dir after create, before first batch).
  after-unit.log: 7/7 pass. keep/ = fixed tree with the price fields (no bench arms): src_db.rs, src_db_vault.rs, tests_durable.rs.
## Step 4: neighbours neigh.sh -> n-<t>.log, neigh.status (running)
## Step 5 prep: bench arms in bench/src/engines.rs (from due-log's patch): supdb-rundirold, supdb-ingestrundirold (price 1), supdb-rundireager, supdb-ingestrundireager (price 2). Twins checked by reading.
## Step 5: pairs.sh list1.txt -> pairs/ (status pairs/status.txt); strace in strace/
- Step 4 done: neighbours n-durable 250/250, n-db 226, n-cipher 24, n-fuzz 5, n-segwriter 19, n-lib 404 (first lib run failed
  the_engine_names_no_file_but_through_the_door: the seam listed with std::fs::read_dir; it lists through vault::names now, lib/durable/cipher rerun green).
  after.log: whole suite --no-fail-fast exit 0, 20 test results ok.
- bench built (bench-build.log exit 0).

## Pricing (A = new shape, B = old = no barrier; "old/new" > 1 means old faster; pairs/ has every table, head.txt)
Counts (strace -f -c over load_only <arm> SIZE 1, strace/load_only-*.c), fsyncs:
  durable supdb (lazy): 4 @10k / 8 @300k; rundirold 3 / 6; rundireager 4 / 8   -> one dir fsync a run (1 run @10k, 2 @300k)
  buffered supdb-ingest (lazy): 3 / 6 = ingestrundirold 3 / 6; ingestrundireager 4 / 8 -> lazy pays nothing (the load's run closes at a flush, whose landing's dir fsync names it)
Timing (strace -tt -T -y, writer thread): lazy dir fsync after the run's first fdatasync 0.13 ms @10k, 0.15-0.36 ms @300k;
  eager (before any data) 0.52 ms @10k, 0.26-0.52 @300k. So the lazy order does share the commit, partly.
Rates, pair.sh NEW OLD SIZE --passes load (tag m), ordered load old/new s1/s2, all unmarked unless noted:
  supdb vs rundirold 10k 1.019/1.030; 300k 1.032/1.040; shuffled 300k 0.994/1.054* (shuffled has no run)
  supdb-ingest vs ingestrundirold 10k 0.879/0.987; 300k 0.981*/0.955 (new faster, split)
  rundireager vs rundirold 10k 1.042/0.984; 300k 0.966/1.064
  ingestrundireager vs ingestrundirold 10k 1.072*/1.118; 300k 1.009/1.042
  -> no rate cost resolved (the one eager 10k mark is one sitting only; unresolved)
Phase (SUPDB_LOAD_PHASES=1 SUPDB_DRAIN_STEPS=1, tag st/st2, phases.py medians of loadphases "first" = first commit, ms, new vs old):
  lazy durable 10k st: 1.21 vs 1.02, 1.23 vs 1.08 (+0.19/+0.15; sittings differ 0.02-0.06) ; st2: 1.15 vs 1.16, 1.19 vs 1.17 (level)
  lazy durable 300k st: 1.53 vs 1.32, 1.39 vs 1.23 (+0.21/+0.16; sittings differ 0.09-0.14); st2: 1.49 vs 1.20, 1.44 vs 1.19 (+0.29/+0.25; differ 0.01-0.05)
  lazy buffered 10k 0.48/0.44, 0.43/0.46; 300k 0.51/0.50, 0.49/0.54 -> level (nothing paid)
  eager durable 10k 1.28/1.12, 1.50/1.23 (+0.16/+0.27); 300k 1.62/1.25, 1.63/1.39 (+0.37/+0.24)
  eager buffered 10k 0.84/0.41, 0.78/0.45 (+0.43/+0.33); 300k 0.84/0.49, 1.01/0.44 (+0.35/+0.57)
Verdict (rule 13e/f): lazy: resolved cost on the durable ordered load's first commit at 300k (two stamped pairings, both sittings,
  excess > sittings' difference), +0.15-0.29 ms, one dir fsync a run; level on the buffered arm. Eager: resolved cost on both arms,
  larger. Every shape the brief allows costs -> the unit does not land. Per key: one dir fsync (0.13-0.36 ms) a run, i.e. ~13-20 ns
  a key for the 10k run, ~1-2 ns a key for a run of ~150k keys. The suite has no many-short-runs workload priced here (not searched
  beyond the load pass).
## 13f: not-landed.patch = git diff of the measuring tree (fix, tests, seams, price fields, bench arms); tree reverted with git apply -R.
## Final
Wrong: a plain run's temp file (direct-*.tmp) is created and its commits acknowledged by fdatasyncs of the file alone; no
directory fsync covers its name before the first ack (Always, EveryN at the due commit, Db::sync, and the owed-batch resync).
The open finds a run by that name alone.
Built (in not-landed.patch): Direct.owed:u8 (RUN_OWES_LOG | RUN_OWES_DIR) replacing log_owed; DIR set at a plain run's open;
paid after the run file's sync in run_due_first (the due branch's existing one-byte check), run_again and sync_run;
run_dir_barrier = vault::sync_dir_as(dir, DirSync::Run), failure kept (keep_lost). Seams: Db::watch_dir_syncs,
dir_syncs_watched, fail_next_run_dir_sync (hook on IO_SEAMS). 7 tests in tests/durable.rs "run-dir" (5 fail before, 2 holding).
Not landed: both allowed shapes cost on the first commit (above). Mutants and docs not done (13f: stop).
Not verified: a lost name by a real power cut (the loss is filesystem-dependent; ext4 here); the eager shape against the
tests; the refs arm (direct_table_refs) skipping its conversion tail at every run's first due commit under the lazy shape
(memory only, by reading); no many-short-runs workload priced; no callgrind count. The bench binary under bench/target was
built from the measuring tree and is stale after the revert.
