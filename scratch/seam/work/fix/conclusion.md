# The fix, concluded: a Log commit in one device request, and decision 2

Tree `3df85d22`. This note puts together:
- the two candidate designs: `fix/design-adjacent/design.md` (ADJ) and `fix/design-free/design.md` (JRP + Stream);
- the crash model, `fix/model/notes.md`;
- the device probe, `fix/probe/notes.md`, `raw/s1-s3.txt` and `raw/lat/`;
- the two skeptics' reports.

New here:
- `fix/conclude/tables.py`, `matrix.py` and `lapsplit.py`, which compute every candidate's paired ratio from the probe's raw runs (`tables.md`, `matrix.txt`);
- one rerun of the model skeptic's counterexample.

Nothing under `/home/user/supdb` was edited and no git command was run. The only computation was that one model sequence, 0.0 s, run after `pgrep` and `ps` showed nothing busy. Ratios are paired per rep (same sitting, rep and size), written as median [min-max] k/30, where k is how many pairs have the first shape slower.

## 0. The answer

**Yes, the fix was tested first, in the model and on the device.** The answer is: do not build it.

**Fixed homes cannot give one request without carrying bytes.** Both designs prove this separately:
- ADJ's exhaustive search: 1.5m+r pages a commit against m+r for two writes;
- design-free §2: the extra bytes are at least the next commit's size.

**The only way to one request is moving homes (JRP), and it was built.**
- It is one data request a commit at every size: 1.00 / 1.01 / 1.00 / 1.16 (tracefs).
- It is sound in the model once one sync rule is written down.
- **It buys no time at the load's 133 KB batch,** which is the one synced commit the run temp is known to see:
  - against the plain run (R0/B): 1.25 [1.04-1.63], 30/30 slower;
  - against the design (R0/A0): 1.02 [0.79-1.44], 17/30;
  - against today's cipher run (R0/D): 1.10 [0.85-1.45], 25/30.
- Its wins at 4 KiB and 13.3 KB come from preallocation, not from the single request. After the ring's first lap it costs what a prefilled plain WAL costs (R0/Cp 1.09 and 0.97). Inside that lap it is 1.28x and 1.37x the plain run.

**The design's commit stays as it is:** two writes, delta frames and CKPTs.

**Decision 2 is (b) on the measurement:**
- the WAL stays a log of sealed records, at the plain WAL's cost (Ds/Cf 0.93-1.01);
- the same records can move behind the door with the chain fix, at the same cost (S/Ds and J/Ds 0.99-1.05), if the WAL's name must be hidden;
- design.md's paged-Log WAL loses: 1.20-1.26x Ds at 4 KiB and up.

## 1. Which candidates are sound

| candidate | crash model | model skeptic | device | verdict |
|---|---|---|---|---|
| **ADJ cases 0-5 (O)**: one write, carry and flush | Unsound as written. A1, case 1 writing over a protected record: `1 S 3 S 4`, 3,318 LOSS (`dur n=5`). Zero violations with A1 fixed and three unstated rules A2-A4 added. | Two holes the model cannot reach. (i) The ring runs out at M=8 with 8 slots: `16 S 17 S 40 40` blocks with no rule. (ii) Case 3 placed across an extent boundary lands on extent e+1's tag pages, which case 5 writes without asking P (from the layout arithmetic, not run). | O0/A0 0.82 [0.56-1.11] at 4 KiB, but transient: 0.87 [0.78-1.22] after commit 240 (lapsplit) and 0.94 after commit 500 (skeptic). 1.07 at 13.3 KB: the extent leaf is written each sync (meta 1.79 a commit) and sectors are 1.40x. It fails ADJ's own rule (< 0.9 in every sitting). | **Rejected** |
| **ADJ T**: case 4 always, two writes, self-sufficient ring records | 0 violations in every family. | Stands. | T0/A0 1.05 / 1.14 / 1.15 / 1.07. The difference is discards (0.96 against 0.42 a commit) with equal write requests, so it is this mount's extent behaviour, not the record format. | **Sound, not adopted.** By the rule fixed beforehand, A's frames stay. |
| **JRP run**, one record page per call | 0 violations as modelled. | **Unsound as written.** design-free §5.4 sets `synced_seq := issued_seq` at completion. A newer sync that completes first is then undone by an older one completing later: `1 B 1 B N E 5 1 1 1`, 124 LOSS on an 8-unit ring, which I reran here (`ooo.py ../model jrp assign one RU=8`). The family has 980,404 LOSS. With `synced_seq := max(synced_seq, issued_seq)` there are 0. The fix model's runner only ever completed the oldest sync, so it could not see this. | One request achieved. 133 KB: 1.25x B, 1.02x A0, 1.10x D, failing all three of its own kill rules (≤ 1.15 B, ≤ 0.90 A0, ≤ 0.92 D). Small commits: the win is preallocation (§2). | **Sound with the max rule. Rejected on its own rule;** the fallback JF is A's shape at 133 KB. |
| JRP, a call split into several record pages (CONT) | 31,205 violations as written; 0 with CONT. | Stands. | Not reachable: no call exceeds 1024 pages (asserted in the probe). | n/a |
| **(b)**: today's LogCipher with the open-time cut and its sync (db.rs:4436-4456, 4573-4580, 27929-27935, read) | 0 violations. Mutants fail as expected (the cut without its sync: 195 WRONG). | Stands for LOSS, WRONG and nonces. One difference: the model restarts on a lost header, where today's open refuses a log of header length or more whose header does not parse (db.rs:4220-4235 sealed, 4594-4607 plain, read). This is reachable by reading, because `Wal::create` writes the header unsynced (db.rs:4180-4183, read). It is an engine hazard in both modes, common to every WAL option, so it does not separate them. Not verified on the engine. | Ds/Cf 1.00 / 1.01 / 0.93 / 1.01; Dsp/Cp 0.96 / 1.00 / 0.94 / 1.00. | **Sound, recommended** |
| **Stream (S) and Journal (J)**: the records behind the door | Unsound as written (W1): `1 1 P 1` adopts a stale record after a reopen (Stream 4,483 WRONG, Journal 2,637). 0 with the record's AAD chained to the previous tag, or with a length that makes the engine's synced cut fire (or, for J, a session-order check). | Stands. Same header question as (b). | S/Ds 0.99 / 1.04 / 1.05 / 1.03; J/Ds 1.02 / 1.00 / 1.03 / 1.02. The chain costs no I/O. | **Sound with the chain fix.** Level with (b). |
| **design.md's paged-Log WAL** (A0 fresh, Ap0 prefilled) | Modelled in E4 with its fixes F1-F8. | — | A0/Ds 0.74 / 1.26 / 1.21 / 1.20; Ap0/Dsp 0.99 / 1.13 / 1.12 / 1.07. | **Rejected** for the WAL |

**The design's own A (§4) is safe from the out-of-order completion only through its sequencer.** "A sync that finds one in flight waits for it" (design.md:233). E4's driver also only completes the oldest sync in flight (`logrun.py` op `E`; `pfm.py` `sync_end` assigns `synced` outright), so the model never checked this.

## 2. What each candidate costs per commit

These are medians of run medians in µs, then paired ratios. Sizes are 100 B, 4096 B, 13,300 B (the mixes' batch) and 133,000 B (the ordered load's batch). Write-through applies only to run shapes (`w` = pwrite into the memfd, `0` = none); the WAL is not mapped. Full tables: `fix/conclude/tables.md`.

### 2.1 The run temp

Against the plain run B, today's cipher run D, and the design A. The A column compares like with like: X0/A0 and Xw/Aw.

| size | shape | µs | /B | /D | /A |
|---|---|---|---|---|---|
| 100 | R0 | 112 | 0.73 [0.47-1.07] 1/30 | 0.74 [0.38-1.22] 4/30 | 0.97 [0.67-1.99] 13/30 |
| 100 | Rw | 108 | 0.76 [0.47-0.97] 0/30 | 0.73 [0.40-1.04] 1/30 | 1.00 [0.60-1.38] 15/30 |
| 100 | T0 / Tw | 115 / 116 | 0.76 / 0.79 | 0.76 / 0.75 | 1.05 / 1.03 |
| 100 | A0 / Aw | 110 / 116 | 0.75 [0.42-1.11] / 0.76 | 0.73 / 0.73 | - |
| 4096 | R0 | 131 | 0.67 [0.44-1.13] 2/30 | 0.62 [0.47-1.13] 1/30 | 0.54 [0.35-0.80] 0/30 |
| 4096 | Rw | 127 | 0.66 [0.43-1.23] 1/30 | 0.63 [0.43-1.01] 1/30 | 0.52 [0.34-0.84] 0/30 |
| 4096 | T0 / Tw | 265 / 287 | 1.43 / 1.46 | 1.31 / 1.44 | 1.14 [0.82-1.47] / 1.15 |
| 4096 | O0 / Ow | 192 / 195 | 0.95 [0.59-1.43] / 0.98 | 0.94 / 0.99 | 0.82 [0.56-1.11] / 0.81 |
| 4096 | A0 / Aw | 237 / 242 | 1.25 [0.60-1.71] 26/30 / 1.27 | 1.22 / 1.26 | - |
| 13300 | R0 | 126 | 0.59 [0.43-0.86] 0/30 | 0.54 [0.39-0.84] 0/30 | 0.51 [0.37-0.72] 0/30 |
| 13300 | Rw | 147 | 0.66 [0.43-0.98] 0/30 | 0.62 [0.47-0.84] 0/30 | 0.53 [0.39-0.74] 0/30 |
| 13300 | T0 / Tw | 296 / 306 | 1.37 / 1.37 | 1.27 / 1.26 | 1.15 / 1.10 |
| 13300 | O0 / Ow | 276 / 272 | 1.27 [0.85-1.65] 28/30 / 1.22 | 1.14 / 1.21 | 1.07 [0.71-1.47] / 1.00 |
| 13300 | A0 / Aw | 260 / 274 | 1.16 [0.82-1.66] 27/30 / 1.25 | 1.15 / 1.15 | - |
| 133000 | **R0** | 474 | **1.25 [1.04-1.63] 30/30** | **1.10 [0.85-1.45] 25/30** | **1.02 [0.79-1.44] 17/30** |
| 133000 | **Rw** | 556 | 1.53 [0.97-1.92] 29/30 | 1.30 [0.84-1.90] 27/30 | 1.06 [0.73-1.58] 20/30 |
| 133000 | T0 / Tw | 491 / 534 | 1.34 [1.02-1.62] / 1.46 | 1.18 / 1.28 | 1.07 [0.75-1.30] / 1.06 |
| 133000 | O0 / Ow | 488 / 540 | 1.33 [1.06-1.56] / 1.43 | 1.20 / 1.26 | 1.03 [0.87-1.49] / 1.04 |
| 133000 | **A0 / Aw** | 464 / 508 | **1.27 [1.00-1.48] 30/30 / 1.42 [1.10-1.66] 30/30** | **1.10 [0.90-1.45] 24/30 / 1.22 [0.98-1.58] 29/30** | - |

B's absolute costs are 148 / 200 / 226 / 365 µs, and D's are 154 / 199 / 227 / 412 µs. The design with memcpy write-through, A, is 1.52x B at 133 KB.

The same run shapes priced against the WAL's shapes (`tables.md`):
- at 133 KB, R0 against the plain WAL is Cf 1.23, Cp 1.44, Ds 1.25 and Dsp 1.43; T0, O0 and A0 are within 1.20-1.49 of these;
- at 4 KiB and 13.3 KB, R0 against Cp is 1.12 and 0.94, and against Dsp 1.10 and 1.02. JRP's small commits cost what a prefilled record WAL costs.

**Write-through by pwrite, w/0:**
- Rw/R0: 1.03 / 0.97 / 1.13 / 1.17, which is +84 µs median at 133 KB;
- Aw/A0: 0.99 / 1.02 / 1.04 / 1.09, which is +47 µs.

Why Rw pays more than Aw at 133 KB is not explained.

### 2.2 The regimes the pooled figures mix

`lapsplit.py` splits each per-commit dump at JRP's ring lap. It uses reps 1-2 of each sitting, 6 pairs a cell. Each cell gives the ratio over the early commits, then the ratio over the rest:

| size (split at) | R0/B | R0/Cp | R0/A0 | A0/B | O0/A0 |
|---|---|---|---|---|---|
| 4 KiB (commit 240) | 1.28 [0.97-1.37] → 0.63 [0.50-0.89] | 2.26 → 1.09 [1.01-1.13] | 0.76 → 0.54 | 1.50 → 1.15 | 0.62 → 0.87 [0.78-1.22] |
| 13.3 KB (77) | 1.37 [1.04-1.61] → 0.55 [0.46-0.78] | 2.19 → 0.97 [0.89-1.17] | 1.08 → 0.54 | 1.35 → 1.01 | 1.08 → 1.17 |
| 133 KB (500) | 1.28 → 1.23 | 1.46 → 1.49 | 1.06 → 1.13 | 1.20 → 1.08 | 1.07 → 1.16 |

**JRP's small-commit gain is the gain of writing into allocated blocks.** Ap0, the design prefilled, gets the same: Ap0/B 0.63 at 4 KiB and 0.67 at 13.3 KB. Inside the fallocated ring's first lap (about 1 MiB of journal) JRP loses to B, and a run temp is a fresh file every run.

**A durable ordered load of 300 commits at 133 KB lies inside A's ring's first lap.** The ring is about 370 commits of 2.75 KB frames. So the A0 figure that applies to the engine is the early column, about 1.20x B.

### 2.3 The WAL

All shapes against the plain WAL Cf/Cp, the sealed WAL today Ds/Dsp, and B (the probe's notes put the 1.06-1.07 cells on 21-24 of 30 pairs):

| shape | 100 B | 4 KiB | 13.3 KB | 133 KB |
|---|---|---|---|---|
| Ds/Cf | 1.00 [0.57-1.34] 14/30 | 1.01 [0.73-1.53] 16/30 | 0.93 [0.55-1.26] 10/30 | 1.01 [0.82-1.26] 17/30 |
| Dsp/Cp | 0.96 [0.70-1.72] | 1.00 [0.55-1.33] | 0.94 [0.71-1.25] | 1.00 [0.77-1.41] |
| S/Ds, J/Ds | 0.99, 1.02 | 1.04, 1.00 | 1.05, 1.03 | 1.03, 1.02 |
| Sp/Dsp, Jp/Dsp | 1.00, 1.00 | 1.01, 1.05 | 0.99, 1.00 | 1.06 [0.85-1.36], 1.06 [0.88-1.37] |
| A0/Ds (paged Log, fresh) | **0.74 [0.45-1.33] 2/30** | 1.26 [0.61-1.75] 24/30 | 1.21 [0.73-1.85] 27/30 | 1.20 [1.02-1.55] 30/30 |
| Ap0/Dsp (paged Log, prefilled) | 0.99 | 1.13 [0.65-1.59] 22/30 | 1.12 [0.70-1.51] 22/30 | 1.07 [0.77-1.64] 22/30 |
| Dsp/Ds (what `recycle_wal` buys the record WAL) | 0.71 | 0.55 | 0.61 | 0.89 |
| Ds/B | 1.01 | 0.98 | 0.99 | 1.05 [0.84-1.25] |

The record WALs' absolute costs (Ds / S / J, µs) are 156 / 151 / 155, 188 / 193 / 190, 213 / 227 / 218 and 384 / 398 / 393. The plain WAL Cf costs 157, 189, 227 and 372.

## 3. Why one request bought nothing at 133 KB

At the load's batch, JRP takes A0 from 2.38 data requests to 1.16, and the time does not move (R0/A0 1.02).

**What separates the paged shapes from B there:**
- **CPU:** sealing and staging, write part 67 µs against B's 33.
- **The sync part:** 401 µs against 329.
  - Outside the discard mode, R0 still pays +49 µs.
  - The measurement skeptic ties that to one more block request a commit on the file's extent-tree leaf block: 4.11-4.16 write requests a commit against B's 3.13. That block is written as metadata in some runs and discarded in others, at block 32005840 in the traces.
  - Every paged shape pays it at 133 KB, and B mostly does not.

**One separate request here costs 10-40 µs** (E5's iocost). A 133 KB commit is 360-470 µs of bytes, flush, growth and CPU, so the request is inside the noise.

**The lever the probe points at is the stored file's extent behaviour on ext4.**
- `filefrag` gives A0, T0 and O0 exactly 4 extents, which is the inode's in-place capacity.
- The hypothesis is that the tree changes depth each commit. That is not verified.
- An extent-stable arm is owed (E5's follow-up list). It is not designed and not measured.

## 4. What it does to the design

### §3 layout

- **Unchanged:** fixed homes, tags apart, data page p at `E(e)+2 MiB+(p&0xFFFF)·4096`, the 1 MiB-aligned pieces and the prefix ring.
- **Not adopted:**
  - JRP's interleaved address mode, which fails its own rule at 133 KB and whose small-commit gain is preallocation's;
  - inline tag pages, which save at most 0.25 requests a commit at 133 KB and turn 127 of 128 pieces into folio ladders (ADJ §8);
  - ADJ's records in the data area.
- **One spec fix.** A CKPT's "group MACs since the last restart" must carry each tag page's version, as `GROUP{g, version, mac}` does. A tag page's nonce is (g, ctr), so a MAC without its ctr cannot be verified. The probe found the gap in E5's A (e5.c:467-470) and in ADJ's chain, and fixed both at no device cost.
- **Open:** the stored file's extent count during the Log phase (§3 above). This is the cost the probe actually saw on this mount.

### §4 protocols

**The Log commit stays A's two writes,** with TAIL / HOME / GROUP / OVR / CKPT frames, the pins, the sequencer and finish with E4 F5's fixes.

**Write down the sync rule:** synced = the newest sync, by issue, among those completed.
- The sequencer's wait (design.md:233) gives it today.
- A later change that lets syncs overlap, as JRP's §5.4 did, loses acknowledged data without it.
- A model op that completes the newest sync in flight is owed in E4's driver.

**If decision 2 is (b), the WAL leaves the Log class.** The run temp is then the only Log file, and it is never resumed (recover_direct then removal, db.rs:27725-27770; the start read here, the rest cited from the model's notes). So these go from the Log class:
- `continue` sessions and base-page ownership for Log;
- TRUNC (E4 F4);
- the resumed-header sync (F1, for Log);
- the re-home after an unclean open (F2);
- the 48-session compaction (F8).

Recycle (F7) stays with whatever holds the WAL. Replace keeps lazy sessions (F6). This is design-free's simplification, taken without its interleaving. The line count is not counted.

### §8 ordered-load row

**The fix leaves it where E5 put it.** Over 300 commits of 133 KB, from the paired median differences times 300:

| shape | over plain |
|---|---|
| today's D | +17 ms [−25, +40] |
| A0 | +31 ms [0, +49] |
| Aw | +43 ms [+12, +74] |
| R0 | +28 ms [+5, +68] |
| Rw | +58 ms [−4, +97] |

E5 had +25 (D) to +38 (Aw).
- The durable 0.80-0.90x stays as arithmetic: the drain's envelope of 43-55 ms and the unlink of 14 ms are still CLAUDE.md's stamps, not re-measured.
- The buffered row (1 MiB pieces, syncs only at the flush) is measured by nobody.

**At the commit, the Log class is a little dearer than today's double write.**
- A0/D is 1.10 [0.90-1.45] 24/30 here; E5 had 1.00 [0.75-1.24].
- So S4 (the Log class for the run temp) stays gated as experiments.md has it. What it buys is device bytes (about 1.04x plain per commit, against today's double write) and the 14 ms unlink, not commit time.
- Decision 1's option (c), keeping `log_run`, is not priced by D. D's run half goes to a memfd. Under S3 those pages reach the device as well, and the WAL's flush pays for whatever is written back before it (0.85 ms/MB, CLAUDE.md). Not measured.

### Stages

- **S4:** unchanged by the fix, and still conditional on S3's load pairing.
- **S6:** drop "the WAL under the layer as a Log file". The WAL and the spare become (b), or the Stream/Journal class with the chain fix. The manifest slots (Replace, E4 F6) were not part of this probe.
- **S0 gains the lost-header question for both modes:** a log whose header sector is lost while its records are kept is refused at open, db.rs:4220-4235 and 4594-4607. Reachable by reading only.

## 5. Decision 2, asked again with the numbers

**(a) the WAL under the layer as a paged Log file, against (b) records sealed by the engine.** (b) wins on this device wherever the suite commits to the WAL:
- 13.3 KB, the mixes' batch, behind the A and F cells: A0/Ds 1.21 [0.73-1.85] 27/30;
- 133 KB, the shuffled load's batch: A0/Ds 1.20 [1.02-1.55] 30/30;
- 4 KiB: A0/Ds 1.26.

(a) wins only at 100 B on fresh files (0.74), and that is growth: `recycle_wal` gives (b) the same (Dsp/Ds 0.71, Ap0/Dsp 0.99).

`recycle_wal` is off by default (db.rs:3938, read), and no bench arm sets it: `grep -rn recycle_wal bench/` finds nothing, and `recycle` in bench/src/engines.rs is only `direct_recycle`. It works under a cipher today (CLAUDE.md).

**The variant (a′): the same records behind the door** as a Stream or Journal class. It costs the same as (b): within 0.99-1.05, and 1.06 at 133 KB prefilled, which is about +19 µs, nearly resolved. It hides the `wal-` name and removes Wal's cipher branches. It needs the chain fix (W1) and a stated length rule. This is a choice about code and names, not about cost.

**Recommendation: (b).** Take (a′) if decision 4's "every name hidden" holds for the WAL. Drop design.md's paged-Log WAL in either case.

### What would change the answer

**A device that overlaps requests** (NVMe with a deep queue, a cache that does not serialize). This is reasoning only; no such device was available here.
- (a)'s extra ring request would get cheaper.
- Its extra CPU would not: cpu 123 against 95 µs a commit at 133 KB, 54 against 45 at 4 KiB.
- Nor would its tail sent twice, about 2 KB a commit, or its extent behaviour.
- So (a) would come closer from above and would not cross (b) at 4 KiB and up. At 100 B its win is growth, which is not about requests.

**A journaled filesystem.** Growth costs a journal commit there. The jext4 control (a loop device, indicative only) reads A0/Ds 0.38 at 100 B and 1.06 / 0.97 / 1.02 above it.
- With `recycle_wal` off, the paged WAL wins tiny synced commits by about 2.6x there. The plain store's own WAL (Cf) pays the same growth, so this is a property of the engine in both modes.
- With `recycle_wal` on, the record WAL is level with the paged shapes at 100 B on jext4, and (b) is no worse anywhere.
- So on a journaled filesystem the answer stays (b) **provided the WAL is recycled or preallocated.** Otherwise tiny synced commits favour (a), but the better fix is `recycle_wal`, for plain and cipher stores alike.

**`recycle_wal` itself.** With it, (b) is level with or ahead of every paged shape at every size on vda: Ap0/Dsp 0.99 / 1.13 / 1.12 / 1.07, and R0/Dsp 1.05 / 1.10 / 1.02 / 1.43. Without it, the one regime (b) loses is tiny synced commits on fresh WAL files.

So the answer flips to (a) only if three things all hold:
- tiny synced commits dominate;
- recycling or preallocating the WAL is not available;
- a journaled filesystem makes growth expensive.

The suite produces none of these. Whether any caller does is not known.

## 6. Not verified and owed

- **Not measured:** the buffered policy (1 MiB pieces), syncs in flight, recovery and finish cost, the read path (JRP's dead table and lost alignment), JF, and the T_d / T_h / M sweeps.
- **Not attributed:** the extent and discard mechanism, R0's +49 µs outside the mode, and Rw's +84 µs write-through at 133 KB.
- **Not checked:**
  - that a run temp never gets synced commits of 4-13 KB. The mixes go to the WAL (design-adjacent §1), but the mix driver was not read;
  - that the run temp is never written after a crash, beyond db.rs:27725-27732 read here.
- **The lost-header refusal** (db.rs:4220-4235, 4594-4607): reachable by reading, not built on the engine. It belongs in S0.
- **The probe's limits:**
  - the controls are 3 reps of 300 commits on a loop device, so their timings are indicative;
  - the traces of the prefilled shapes include the prefill;
  - the rotation is cyclic, so each shape's predecessor is fixed;
  - verify.py's 88/88 left no kept log.
- **Owed before any variant that lets syncs overlap:** an E4 driver op that completes the newest sync in flight.

## 7. Files

| file | what |
|---|---|
| `work/fix/conclude/tables.py`, `tables.md` | §2's tables from `probe/raw/s1-s3.txt` |
| `work/fix/conclude/matrix.py`, `matrix.txt` | every candidate against every reference |
| `work/fix/conclude/lapsplit.py` | §2.2, from `probe/raw/lat/` |

Reproduce:

    python3 -I tables.py ../probe/raw/s{1,2,3}.txt
    python3 -I lapsplit.py ../probe/raw/lat

Rerun of the out-of-order sync counterexample:

    echo "(1,'B',1,'B','N','E',5,1,1,1)" | python3 -I ../skeptic/ooo.py ../model jrp assign one RU=8

It reports 124 LOSS with `assign` and 0 with `max`.
