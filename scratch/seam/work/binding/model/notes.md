# The lead's binding theory, checked by an executable model

Tree: /home/user/supdb at 3df85d22, read only; nothing built or run on the
engine. The model is `model.py` in this directory, with `probe.py` (one
history, one end state), `run_all.sh`, `run_deep.sh` and `summary.py`.
Results: `results.txt` and `summary.txt` (the matrix to depth 6, plus FULL and
FULLC at depth 8), `results_deep.txt` and `summary_deep.txt` (FULL and FULLC at
depth 9 split by their first two operations, the other variants at depth 8),
`results_deep2.txt` (plain run-file semantics at depth 8). Every run used
`nice -n 19 taskset -c 3`; the longest single run was 14.3 s of CPU.

Words: **held** means a prefix of the history of acknowledged commits, in
program order across the run's file and the WAL. **NEVER** means the open
answers a state that is not held. **ROLLBACK** means a held state shorter than
the history. **LOSTDUR** means a held state shorter than what a completed sync
had covered. **REFUSE** means the open fails, and **REFUSE_READ** means the
store is poisoned by the first read of the damaged bytes. P1 (no damage)
requires the open to succeed with a held state of at least the promised length.
P2 (damage) requires REFUSE, REFUSE_READ or a held state (LOSTDUR is allowed and
is counted).

## 0. Verdict

**The mechanism holds.** The binding counts only the run's *completed* syncs.
Under that count, a clean power loss never trips the check: LEAD has no false
refusal under either buffered sync order unless a merge is in the history, and
those refusals come from finding 1 below, not from the power loss. The two
mutants that break the counting are both caught under the buffered policy:
- M1, which counts unsynced commits, false-refuses in 8,101 states at depth 6;
- M2, which writes the binding before the close's sync, false-refuses in 854.

Nothing is paid per run commit.

**As stated, the theory is not enough.** The model finds four problems.

1. **The placement false-refuses healthy stores.** The next WAL record after the close has a sequence at or past the run's `end_seq`, because a plain run's segment is named at `self.wal.seq` at the close (db.rs:28919). The manifest that names the run therefore never covers the record, and every later open replays it.
   - Once the run's piece is gone from the live names, the open refuses. On the ordinary path this happens after landing, retirement and a merge: `RC WC SF MAN RT MG` → REFUSE `L1-missing`. A merge does not move the covered sequence, since its output takes `covered_seq` or the newest input's sequence (db.rs:38289-38294, 41569).
   - LEAD counts the run as present when a live segment carries its id. That reading still gives 109 false refusals at depth 6 under Always.
   - LEADT counts the run as present only through its temp. It gives 535, already on `RC WC SF MAN`, because the open removes the temp of a named run (db.rs:27728-27730).
   - **Fix:** frame the binding as its own WAL batch before `end_seq` is read (variant L1). The run's landing then covers it, and that path opens WHOLE.
2. **The recovering open breaks it again.** That open rebuilds an unnamed temp as a piece named at the covered sequence and leaves covered where it was (db.rs:27747-27770, the name at 27749, the slot at 27768). So the binding stays live.
   - **L1 alone.** After a merge it refuses: `RC RC WC CR MG` → REFUSE. That is 176 false refusals at depth 6.
   - **A settle frame that voids the binding** (L1S; the lifecycle reader's B and C) ends the refusals. It also leaves the slot that the recovering open wrote as the only record of the run. With that slot changed or deleted, the older slot is chosen, the piece is swept and the later WAL commits replay without the run: `RC RC WC CR CR2` + older manifest → `{W1}`, NEVER.
   - **Fix (L1P):** for each run it rebuilds, the reopen writes a binding that is moot once the chosen manifest's publish number is at least that of the slot that named the piece. With L1P the run→WAL direction (the lead's direction) has **0** NEVER under Always, to depth 8.
3. **It covers one boundary of three.**
   - **run→run:** `RC RC SE RC` + a changed byte in the first run → `{R1, R2}`.
   - **WAL→run:** `WC SE RC` + the WAL cut → `{R2}`.
   - **Through a reopen:** `RC RC WC SE RC CR` + the WAL cut → `{R1, R1, R3}`. The reopen deletes the temp that carried the WAL→run link (27770), while the WAL commit before the run lives only in the WAL, because its seal was lost at the crash.
   - **Fixes:**
     - **L2:** the previous run, carried in the next run's first marker.
     - **L3:** the WAL position at the run's open, carried in the run's first marker.
     - **A WAL floor** in the slot the reopen writes. The reopen also syncs the replayed WAL before it writes that slot, because `open_append` counts replayed bytes as synced without syncing them (db.rs:4355-4358).
4. **Under the buffered policy a plain store already opens never-held states after a clean power loss**, in all three directions: NONE has 4,892 at depth 6 (bufWR). The lead's binding neither causes this nor removes it, and the theory did not claim to. Two things remove it:
   - each link carries both the synced count s and the committed count k, and a recovered count r with s ≤ r < k ends the history at that boundary;
   - the reopen syncs the WAL before it writes the slot.

**FULL** is everything above together: L1 framed before `end_seq`, the reopen's binding by publish number, L2, L3, the slot's WAL floor and the reopen's WAL sync, and the end rule. It has **0 P1 failures and 0 NEVER** under all three policies:
- to depth 9, split runs, with power loss and damage combined to depth 6-7;
- to depth 8 with power loss and damage combined to depth 6-8;
- under both run-file damage semantics.

FULLC adds the chain (a second binding of (k, k) at the first sync after a close). It is also 0. Under buffered it turns some of the allowed rollbacks below what was promised into refusals: 28,056 LOSTDUR against 34,346 at depth 6.

**Price under Always (the default, db.rs:3835):**
- one unsynced WAL batch of about 54-70 B per run close;
- about 33-49 B in each run's first marker;
- nothing per run commit;
- no extra fdatasync.

A recovering open that rebuilds a run pays three more things: one binding frame per rebuilt run, one WAL fdatasync before its slot, and 8 B in that slot.

Under this model's buffered policy, FULL needs no extra sync. The engine's real `EveryN(n)` is not modelled here: per-file due syncs, a counter shared between run and WAL commits, and `Db::sync` not syncing the run. The today reader found sync-order requirements there.

## 1. The model

**Files** (abstract; one batch is one sequence):
- **Run files.** Commits with a synced count and a list of completed sync points. After SF the file is finished: it is the segment (a hard link, db.rs:28989 per the lifecycle reader, not read here), with every record durable (`seg.sync_all`, db.rs:29004).
- **WAL files.** Records of kind U (user batch), B (binding) and S (void settle). Each file has a synced count and a kind byte, CHAINED or LOOSE. The kind byte history is kept so that an older copy can be offered.
- **The manifest.** A list of slot images (publish number, covered, names, WAL floor). The newest image is the store. Slot writes are atomic.

**Operations.** The macro operations are listed in `model.py`'s docstring. Their enabling follows the engine:
- **RC** opens a run only over an empty memtable (db.rs:28327-28329).
- **WC** closes an open run first (`leave_direct`, db.rs:28354, which goes to `close_direct`, 28897, then `close_run`, 28917).
- **SE** closes a run or freezes a non-empty memtable, which is `seal()` (29100-29105).
- **RO** comes only right after a WAL commit (28513). The new file is LOOSE when the closed one holds unsynced records.
- **SF** and **MAN** land seals in order. MAN covers `max(covered, end)` (37770).
- **RT** retires temps and the WAL files a seal took at its hand (29021, 37897-37928).
- **MG** folds every named segment into one under a new id, with covered unchanged.
- **CR** is a process crash and a recovering open (27717-27770). It refuses, or rebuilds temps as pieces named at covered and continues the history. Its second half, **CR2**, is the settle or binding frames and the WAL sync, a step of its own so that a power loss can fall between.

**Policies.**
- **always:** every commit syncs its own file before it is acknowledged (28800-28807, 27055-27081).
- **bufWR / bufRW:** commits never sync. SY syncs every file in two steps, WAL then runs or runs then WAL, and it promises the commits before it when it completes.

**End states at every node.**
- A process crash: every write kept.
- Every power-loss landing: each file independently keeps its synced prefix and any prefix of its unsynced records. A run file never synced may be absent.

**Damage, one at a time, on top of either end state:**
- in a temp present: a changed byte at each commit, the file deleted, or an older copy at each shorter count;
- in each WAL file: a changed byte at each record, the file deleted, or an older copy at each shorter length with its kind at the time;
- in the manifest: an older copy, every earlier slot image. This covers the newest slot changed or deleted.

**Run-file damage semantics**, `--run-sem design` (design.md section 4):
- a byte at or below the sync point before the last → REFUSE_READ, since the open reads the run to rebuild it;
- a byte in the last synced span → falls back to every count from that earlier sync point up to the damage;
- a byte in the unsynced span → a torn tail;
- a finished file → REFUSE_READ.

`--run-sem plain` is `recover_direct`, which stops at the damage. WAL damage is today's sealed records: the walk stops at the damaged record (vault.rs:1824-1857, per the today reader).

An older copy of either kind of log enumerates every shorter prefix. So a Log-class WAL under S6, whose outcomes are a prefix at a synced point or a refusal, is subsumed by what was enumerated.

**The open** (`open_store`), in this order:
1. Every name the chosen slot lists must exist.
2. Named temps are ignored.
3. Replay from covered with continuity: a gap is refused, except at the first applied record of a LOOSE file that is not the first file, where the log ends (4609, 4651-4673).
4. Then the variant's link checks.
5. The state is the named segments, plus the temps' recovered prefixes, plus the replayed user batches, cut at the earliest END.

The verdict is computed before anything is deleted. That presumes the reordered open the today reader asks for.

**The link check** (`pred`). A predecessor P is moot in any of these cases:
- covered ≥ P's end sequence (a seq link);
- the chosen slot's publish number ≥ the one recorded (a publish link);
- (`by_id`) a live segment carries P's id.

Otherwise:
- if P's temp recovered r with r < s, the open refuses;
- if s ≤ r < k (the END variants), the history ends at P's r-th commit;
- if P's temp is absent and s > 0, the open refuses (the mutant M3 lets it pass);
- if P's temp is absent with s = 0 and k > 0, the history ends at P's start (END variants only).

L3 is a WAL position P with a synced position s3. Against the replay's reach and covered: below s3 the open refuses; at or above s3 and below P the history ends at the first missing WAL commit.

## 2. Variants and results (depth 6, every operation; damage after a power loss to depth 6)

For each variant the three policies are listed in the order always / bufWR / bufRW. Column "P1 failures (always)" gives the false refusals or never-held states without damage under Always. Column "P1 pl never (buf)" gives the never-held states after a power loss, with no damage, under bufWR / bufRW. Column "P2 NEVER" is all three policies.

| variant | what it is | P1 failures (always) | P1 pl never (buf) | P2 NEVER |
|---|---|---|---|---|
| NONE | no binding: plain parity, decision 1(a) | 0 | 4,892 / 4,532 | 12,502 / 50,058 / 49,435 |
| LEAD | lead, literal: the next WAL record carries (R, synced); comes back = named id or temp | 109 refusals at crash, 13 at a reopen, 109 after a power loss | 4,892 / 4,532 | 6,112 / 46,191 / 45,586 |
| LEADT | LEAD, comes back = temp only | 535 / 89 / 535 refusals | 4,892 / 4,532 | 6,074 / 46,189 / 45,584 |
| L1 | own batch before `end_seq`, moot by covered or by id | 176 / 23 / 180 refusals | 6,116 / 5,773 | 6,336 / 59,189 / 58,759 |
| L1S | L1 plus a void settle at a reopen | 0 | 5,996 / 5,634 | 8,321 / 59,919 / 59,420 |
| L1P | L1 plus a reopen binding moot by publish number | 0 | 5,996 / 5,634 | 5,439 / 53,488 / 52,955 (under Always: run→run 381 after a power loss, WAL→run 5,058, **run→WAL 0**) |
| L1SK | L1P plus k and the end rule | 0 | 1,665 / 1,772 (no run→WAL) | 5,439 / 22,389 / 22,619 |
| LEADK | the close syncs the run on the writer, then the binding carries k | 0 | 980 / 1,087 (WAL→run only) | 5,439 / 15,677 / 16,237 |
| FULLV | FULL with the void settle | 0 | 0 | 2,098 / 3,866 / 3,897 (run→WAL through the reopen's slot) |
| FULLN | FULL without the slot's WAL floor and the reopen's sync | 0 | 107 / 107 (the reopen window) | 1,427 / 2,479 / 2,482 (WAL→run) |
| **FULL** | L1, reopen binding by publish number, L2, L3, WAL floor and sync, END | **0** | **0** | **0 / 0 / 0** |
| FULLC | FULL plus the chain | 0 | 0 | 0 / 0 / 0 |

Deeper:
- FULL and FULLC at depth 8: always (82,212 nodes, damage after a power loss to depth 8), bufWR and bufRW (212,104 nodes each, damage after a power loss to depth 6). 0 P1 failures and 0 NEVER.
- FULL and FULLC at depth 9, split by the first two operations: 0 and 0, over 375,207 nodes under always and 1,076,819 under each buffered order (section 7).
- Under plain run-file semantics, FULL is 0 at depth 6 and 8.

The other variants at depth 8 show no shape that depth 6 does not.

## 3. Witnesses

Each was re-run through `probe.py`. The H column lists the history: Rn is a commit of run n, Wq a WAL batch at sequence q.

| variant | ops | damage / landing | H | opened | verdict |
|---|---|---|---|---|---|
| NONE | RC RC WC | byte in run 1, commit 2 | R1 R1 W0 | [0, 2] | **NEVER** (the lead's case) |
| LEAD | RC RC WC | same | | REFUSE L1-short | caught |
| LEAD | RC WC SF MAN RT MG | none | R1 W0 | REFUSE L1-missing | **false refusal** |
| LEADT | RC WC SF MAN RT | none | | REFUSE L1-missing | **false refusal** |
| L1 | RC WC SF MAN RT MG | none | R1 W1 | WHOLE | |
| L1 | RC RC WC CR MG | none | | REFUSE L1-missing | **false refusal** |
| L1S | RC RC WC CR CR2 MG | none | | WHOLE | |
| L1S | RC RC WC CR CR2 | older manifest (birth slot) | R1 R1 W1 | [2] | **NEVER** |
| L1P | same | same | | REFUSE L1-missing | caught |
| LEAD | RC RC SE RC | byte in run 1, commit 2 | R1 R1 R2 | [0, 2] | **NEVER** (run→run) |
| FULL | same | same | | REFUSE | caught |
| LEAD | WC SE RC | WAL file 0 cut at record 0 | W0 R2 | [1] | **NEVER** (WAL→run) |
| FULL | same | same | | REFUSE L3-short | caught |
| L1P, FULLN | RC RC WC SE RC CR CR2 | WAL cut at record 0 | R1 R1 W1 R3 | [0, 1, 3] | **NEVER** (the reopen dropped L3) |
| FULL | same | same | | REFUSE slot-wal-floor | caught |
| FULLN bufWR | WC SE RC CR, before CR2 | power loss: WAL keeps 0 | W0 R2 | [1] | **P1 NEVER** (the reopen named a piece over an unsynced WAL) |
| FULL bufWR | same | that landing does not exist (the WAL is synced before the slot) | | | |
| LEAD bufWR | RC RC WC | power loss: run 0, WAL 1 | R1 R1 W0 | [2] | **P1 NEVER** (the plain hole) |
| FULL bufWR | RC RC WC | power loss: run 0, WAL 2 | | [] | ROLLBACK (END at the binding) |
| L1SK bufWR | RC RC SE RC | power loss: runs 0/1, WAL 0 | R1 R1 R2 | [2] | P1 NEVER (no L2) |
| L1SK, LEADK bufWR | WC SE RC | power loss: run 1, WAL 0 | W0 R2 | [1] | P1 NEVER (no L3) |
| FULL bufWR | both | | | [] | ROLLBACK |

## 4. The mutants (each must fail, and does)

| mutant | of | caught where | witness |
|---|---|---|---|
| M1: counts unsynced commits (s := k) | FULL (and L1P as M1L) | bufWR 8,101 and bufRW 7,442 P1 REFUSE (M1L: 7,623 and 6,964) | `RC RC WC`, power loss with run 0 and WAL 1 (the binding) → REFUSE L1-short |
| M2: the binding is written before the close's sync, counting it | LEADK | bufWR and bufRW 854 P1 REFUSE | `RC RC WC` between the binding and CSY, power loss with run 0 and WAL 1 → REFUSE L1-short |
| M3: a missing run file passes | FULL (and L1P as M3L) | always: P2 NEVER 5,544, run→WAL 4,346 (M3L run→WAL 4,347 against L1P's 0); buf: P1 NEVER 1,946 / 1,751 | `RC RC WC` + temp deleted → [2]; bufWR with the run absent and WAL 2 → [2] |
| M4: a binding in a closed (rotated) WAL file is ignored | FULL (and L1P as M4L) | always: P2 NEVER 1,529, run→WAL 1,462 (M4L 1,552 against 0); buf: P1 NEVER 823 / 740 | `RC RC WC RO WC` + byte in run 1, commit 2 → [0, 2, 3]; bufWR `RC RC WC RO` with run 0 and WAL 2/0 → [2] |

**M1 and M2 are identical to the correct design under Always**, because every run commit is synced before the close (db.rs:28800-28807), so s = k. They must be held under a buffered policy.

**On the literal reading of M4**, where the binding's file has really been retired: retirement comes only behind a durable slot whose covered sequence passes the binding (37897-37928, with the seal that took the file handed after the rotation, 29021). A check that never sees a retired file's binding therefore loses nothing. Damage that rolls the manifest back over a retired file meets the WAL's own continuity check (4664, 4671), or it opens a held rollback (LOSTDUR, allowed by P2). Checked:
- prefixes `RC RC WC RO SE SF SF MAN MAN RT` and `RC RC WC RO WC SE SF SF MAN MAN RT`, plus four more operations, depth 14, damage after a power loss throughout;
- FULL: 0 NEVER;
- the older manifest after retirement opens `[]`, a ROLLBACK.

So the reading that bites is M4 as modelled: the file is closed and not yet retired.

## 5. What each fix costs and where it is written

| piece | where | bytes | syscalls |
|---|---|---|---|
| L1, the binding | its own WAL batch, framed in `close_run` before `end_seq` (28919) and written after `hand_seal` | (R, s), plus k under buffered. 54-62 B in the plain frame format: a frame of 8 + 9 + 16/24, then a commit frame of 21. The lifecycle reader's 62/70 B includes e, which is implicit at seq + 2. Under `LogCipher` add 32 B (lifecycle reader). | one write(2), no sync. Under Always it is synced by the next WAL commit; in an ordered-only load L2 carries it. |
| L2 | the next run's first marker, when no WAL user batch came between | (A, kind and value of mootness, s, k) ≈ 33 B | none: synced with that run's first commit |
| L3 | every run's first marker | (P, s3) = 16 B | none |
| the reopen's binding | the WAL, after `open_append`, one per rebuilt run, with the slot's publish number | ≈ 54-62 B | the reopen's WAL sync, which it pays anyway (below) |
| the reopen's WAL floor and sync | the slot written at 27768: a new field, so a new slot version | 8 B | one fdatasync of the WAL before the slot, only at an open that rebuilds |
| the end rule (buffered) | the open: cut the WAL at the binding and drop later temps | none | the cut and sync the open already makes (27929-27935) |

## 6. Code facts read for this (3df85d22)

- `direct_can_open` requires an empty memtable and an empty `wal.pending` (db.rs:28327-28329). So a run opens only after a seal or at a store's start, and two consecutive runs have no WAL user batch between them. That is the run→run case.
- `commit_direct`: due under Always means `d.w.sync()`, the run's file alone (28797-28807).
- `close_direct` calls `land_seal`, then `freeze`, then `close_run` (28897-28911). In the background mode `land_seal` waits only for room in the frozen list (30949-30987), so the pre-run seal can still be in flight at the run's close. That is the WAL→run case.
- `close_run`:
  - `end_seq = self.wal.seq` (28919);
  - `wals = take(closed_wals)` (29021);
  - the seal thread's `seg.sync_all()` (29004).
- `seal()`:
  - an ordered table is `commit_direct` then `close_direct` (29100-29105);
  - otherwise it syncs only when the policy says so (29111-29128);
  - its `end_seq = self.wal.seq` (29186).
- `sync_wal` syncs the loose files, chains, and syncs the live file. It never syncs a run's file (27055-27081).
- A rotation comes only after a WAL commit (28513), or in `log_run` (28882).
- **The open, in this order:**
  - the manifest slots (27631);
  - under a cipher, the vault sweep (27635-27637);
  - orphans removed (27688-27692);
  - temps: removed if named (27728-27730); otherwise rebuilt and named at `sealed` (27749), with the slot written (27768) and the temp removed (27770);
  - the replay (27880-27909);
  - the cut to `valid_len` and its sync (27929-27935);
  - `open_append` (4358), which counts the replayed bytes as synced, the comment saying they were "read back from the device".
- **The WAL replay:**
  - LOOSE counts only when the file is not the first (4609);
  - the gap refusal (4664, 4671);
  - the loose end (4651-4660).
- `manifest_read_all` returns the valid slots newest first and ignores a torn slot when another is valid (11890-11913).
- A landing covers the max of its names' `end_seq` (37770). A merge's output takes the covered sequence (41569), or for a tier the newest input's (38289-38294). So a merge never moves covered.
- `retire_covered` removes the closed WAL files and the temp names once a durable slot exists (37897-37928).

## 7. Deep runs

`run_deep.sh` produced 86 runs and no traceback. The longest single run took 14.3 s of CPU.

- **FULL and FULLC at depth 9.** Split by every reachable pair of first operations. WC,RC is unreachable, because a run opens only over an empty memtable. Damage after a power loss goes to depth 7 under always and depth 6 under buffered.
  - All 74 split runs: 0 P1 failures and 0 NEVER.
  - Nodes: 375,207 under always, 1,076,819 under bufWR and 1,076,819 under bufRW, for each variant.
- **The other variants at depth 8.** Always with damage after a power loss to depth 6; bufWR to depth 5.
  - Same shapes as depth 6, no new ones.
  - LEAD: 4,758 false refusals under always.
  - L1: 6,577.
  - L1P: run→WAL 0, with run→run 381 and WAL→run 87,242 remaining.
  - L1SK under bufWR: no run→WAL. Power-loss NEVER of run→run 19,349 and WAL→run 47,849.
  - LEADK under bufWR: power-loss NEVER of WAL→run only, 39,843.
- **Plain `recover_direct` semantics at depth 8** (`results_deep2.txt`):
  - FULL: 0 / 0 under always (82,212 nodes) and bufWR (212,104 nodes);
  - NONE: 173,270 NEVER under always.

## 8. Not modelled, or not verified

- **The engine's `EveryN(n)`** (per-file due syncs with a counter shared between run and WAL commits; `Db::sync` with a run open syncs neither the run nor anything else) is not modelled. The buffered policy here is the task's idealisation, a sync of every file. Under the real policy the today reader's sync-order rules are needed on top of FULL (their `LOOSE` variant). Not re-checked here.
- **Interleaving inside SF.** Seals finish in order (SF of the oldest only); the engine's seal threads may finish out of order. Under Always this cannot matter for a run's file, since it is synced. Under buffered, out-of-order finishes add no on-disk prefix that writeback did not already allow. Argued, not enumerated.
- **The manifest.**
  - Slot writes are atomic: a torn newest slot by power loss is not enumerated. It equals damage to the newest slot before retirement, which is enumerated.
  - Both slots deleted, which takes the pre-manifest scan (27656-27684), is not modelled.
  - `manifest_pending`, drains and the presync, `recycle_wal` and the spare, and `seal_rotates_wal` are not modelled.
- **Other files and damage.** Damage to segments other than the run's file, to ordered indexes and to form files is not modelled. Nor are the WAL kind byte (unauthenticated today; authenticated under the design), two damages at once, or what a refusal deletes before it refuses. The model assumes the reordered open.
- **Merges and runs.**
  - A merge folds every named segment; there is no partitioning, promotion or `pcs-` naming.
  - `leave_direct`'s re-logging of the uncommitted tail is folded into one atomic WC.
  - A run's keys are taken as disjoint from the WAL's, so S1 (a delete above `max_key`) is not modelled.
- **Directory entries.** Renames and unlinks are durable at once. Only a never-synced run file may vanish at a power loss. That a run file's directory entry is durable after its first fdatasync is assumed, as the engine assumes it; not verified on this filesystem.
- **Damage only at the last open.** Damage comes only before the final open. A damaged store that opens as a held rollback and then goes on taking writes and reopening is not enumerated. Neither is a reopen (CR) after damage or after a power loss: CR is a process crash with no damage.
- **Frame layouts and byte counts** are arithmetic from the frame format (FRAME_HEADER 8, a commit body of 13) as the lifecycle reader cites it; not checked by building.
- **The pub-mootness check** assumes slot publish numbers only grow and that a slot at or past P keeps the content of the piece P named, merges included. This holds by the model's construction. In the engine, `mseq` starts from the chosen slot's sequence and only grows within an open (27668, 27699, 27705, 27767, by reading). Two things were not traced: whether a later publish can reuse a number that a damaged newer slot had held, and the slot rewrite at an upgrade (27698-27710).
