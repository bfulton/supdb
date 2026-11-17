# The instruction counts of the eight fixes, 3df85d22 against HEAD 8b83dc03 (reading)

Status: complete. Method notes at the end.

## Completeness
All ten probes exited 0 (head.status, base.status: load_durable, load_buffered, work, read, merge, then "done").
Every .out, .self.txt, .incl.txt and .stdout exists for both sides.

## Control: point read (read_only supdb 100000 1 n-none, toggle on Engine::get, 200,000 gets)
Total 166,114,879 -> 166,115,088 (+209 over 200,000 reads). Every engine function has the same count on both
sides (Reader::enter, leave_looked, read_all_in, Blob::read_exts, pieces_over, may_hold, advise, parse_record_tail:
all +0). The +209 is glibc's _int_malloc (+197) and unlink_chunk (+12) inside the harness's consume closure: heap
state left by a different load before the toggle. The control did not move.

## agg.py and cmp.*.self.txt: not trusted
agg.py parses the auto-annotated .self.txt files, so it sums the per-function lines together with the
"=> callee (Nx)" call lines and the annotated source lines (cmp.read.self.txt has a row whose "function" is a
line of source). Its rows with "(Nx)" are inclusive call costs from one source line, not functions. The
reading below re-annotated each .out with `callgrind_annotate --auto=no` (rd/<side>.<probe>.self|incl) and,
for per-line and per-callee figures, parsed the .out files directly (rd/fnlines.py, rd/callees.py,
rd/fncmp.py: self cost of one function by source line, mapped to each side's own src/db.rs text).

## What each probe does at these arguments (from bench/examples)
- load_only supdb 100000 1 (durable, Always) and supdb-ingest (buffered, EveryN): 100,000 keys in key order,
  `Batch` flushes of 1,000 -> 100 commits of 1,000 keys through a direct run, then the bench's sync (a seal and drain).
  Syscalls: 100 run fdatasyncs (durable), 111 vectored writes of the run (durable), 12 (buffered).
- work_probe 100000 whole: 100,000 shuffled puts (101 commits), 4 rounds of 10,000 appends (44 commits) and 500
  scans, 3 syncs and settles, 100,000 read_all, 50,000 counts, 5,000 deletes (6 commits), 500 scans. 151 commits,
  all through the WAL (hashed table), each with a log write and an fdatasync (Always).
- read_only supdb 100000 1 n-none, collected only inside Engine::get: 200,000 point reads.
- merge_walk 100000 4 1: create; 100,000 appends in order (one direct run, one commit), flush; 4 chunks of 25,000
  sorted appends below the greatest key (WAL path), each a commit and a seal; settle; then 2 x (copy the store,
  Db::open, one append, commit, seal, settle). 9 manifest slot writes by publishes plus 2 opens.

## Totals
| probe | base 3df85d22 | HEAD 8b83dc03 | delta | per op |
|---|---:|---:|---:|---|
| load_durable | 340,385,259 | 340,560,651 | +175,392 | +1.75 a key; Db::commit +3,619 a commit |
| load_buffered | 339,945,359 | 340,212,629 | +267,270 | +2.7 a key, of which ~+100k is the zpages controller's timer ticks (one more z_tick, two more read_budget, +28 reads of cgroup files); Db::commit +3,555 a commit |
| work | 705,249,609 | 703,741,850 | -1,507,759 | inlining (push_chunk into main), see rows |
| read (control) | 166,114,879 | 166,115,088 | +209 | +0.001 a read; every engine function equal |
| merge | 1,681,203,430 | 1,681,782,260 | +578,830 | +4 a key in the direct run's commit, the rest below |

## Rows: functions that moved (self unless "incl"; per op = delta over the calls named)
| probe | function | base | HEAD | delta | per op | fix | kind |
|---|---|---:|---:|---:|---|---|---|
| load_durable | Db::commit (incl) | 28,634,630 | 28,996,496 | +361,866 | +3,619 / commit | run-errors, layout | sum of the rows below |
| load_durable | Db::commit_direct (self) | 12,046,933 | 12,444,373 | +397,440 | +3,974 / commit = +4 / key | run-errors (38378d1d) | MORE: per-key loop, source unchanged; see reading |
| load_buffered | Db::commit_direct (self) | 12,044,435 | 12,444,473 | +400,038 | +4 / key | run-errors | same |
| merge | Db::commit_direct (self) | 12,002,071 | 12,402,082 | +400,011 | +4 / key (100,000 keys, 1 commit) | run-errors | same |
| load_durable | writev_all -> pwritev_all (incl, at stream_flush) | 31,215 | 35,392 | +4,177 | +37.6 / write (111) | run-errors | MORE: from_iter of the IoSlice Vec out of line (10,229) where it was inlined; fault compare 2; the Vec and its malloc existed before |
| load_durable | AlignedWriter::stream_flush (self) | 21,135 | 21,679 | +544 | +4.9 / call | run-errors | error mapping (`owing`) on the write's return |
| load_durable | SegmentWriter::sync (new out of line; was inlined in commit_direct) | ~2,000 inline | 4,096 | ~+2,100 | ~+21 / run sync (100) | run-errors | MORE: a call; inside it `out.fault != 0` (load+branch) and synced_all's helper error load |
| load_durable | engines write_batch (Db::append inlined) | 15,504,791 | 15,304,794 | -199,997 | -2 / key | seal-sync (writes_go) | layout, cheaper |
| load_durable | AlignedWriter::stream_hash (self) | 873,222 | 852,474 | -20,748 | -207 / commit | none | CRC tail by alignment of the stream's blocks (block.rs:168 only) |
| load_durable | memcmp from commit_direct | 2,224,982 | 2,202,711 | -22,271 | | none | memcmp's page-cross path by address |
| work | Db::commit (incl) | 3,625,985 | 3,623,223 | -2,762 | -18 / commit (151) | log-errors, log-at-open, layout | cheaper net |
| work | Db::sync_wal (self) + wal_dir_barrier | 9,264 + inline | 9,968 + 1,510 | +2,214 | +14.7 / log sync (151) | log-errors (0aa41df7), log-at-open (a424903d) | MORE: wal_dir_barrier now out of line (10 / call); FAIL_SYNC_LIVE test of `fail_log_sync` (2); `wal_trace.is_some` (2) |
| work | Wal::write (self) | 4,666 | 4,956 | +290 | +2 / write (145) | log-errors | load+branch: `fail_next.is_some()` beside the write |
| work | Db::delete (inlined in main): max_key compare + memcmp | | | ~+90,000 | ~+18 / delete (5,000): memcmp +14, compare and slice ~4 | delete-max (f8d130a8) | MORE than a load+branch: a key compare a put already made, new on delete |
| work | MemTable::push_chunk (self) | 6,300,000 | 4,500,000 | -1,800,000 | 40,000 calls inlined into main (+705k in main) | none | LTO inlining moved by changed callers in main |
| work | Db::sync (self) | 297 | 294 | -3 | -1 / sync (3) | sync-run | level |
| merge | Db::append (self) | 33,201,520 | 33,401,520 | +200,000 | +1 / append (200,002) | seal-sync / delete-max | layout: per-append source unchanged |
| merge | Db::open (self + callees) | 96,894,650 | 96,899,340 | +4,690 | +2,345 / open, of which ~+8,100 new, offset by cgroup reads (resolve_auto_cache -9,982) | open-order (8b83dc03), slot-choice (2e8b173d) | MORE: new syscalls, see reading |
| merge | Db::open_refusals (new) | 0 | 13,239 incl | +13,239 | +6,620 / open; 5 statx / open | open-order | new work at open |
| merge | Db::unnamed_refusals (new) | 0 | 812 incl | +812 | +406 / open | open-order | |
| merge | vault::open_slot (out of line now) | inline | 504 self, 12 calls | | 42 / call | slot-choice / open-order | + IO_TRACE_ON load |
| merge | open: sync_data of the slot taken | 0 | 2 fdatasync | +2 | +1 fdatasync, +1 open, +1 close / open | slot-choice | new syscalls at open |
| merge | Maint::publish (self) | 840 | 1,144 | +304 | +38 / publish (8) | slot-choice | three seam loads (tear_slot, fail_slot_sync, slot_watching), slot choice word |
| merge | Maint::watch_slot (new, out of line) | 0 | 171 | +171 | +19 / slot write (9), plus the call | slot-choice | MORE: a call whose body is the seam's load |
| merge | manifest_write_slot(_unsynced) | 1,081 | 702 | -379 | | slot-choice | open_slot moved out of line |
| merge | slot-write chain (publish, write_slot, open_slot, watch_slot, sync_owed_slot) | 2,219 | 2,835 | +616 | ~+53 / slot write (10, less the open's 2 open_slot) | slot-choice | MORE: two out-of-line calls beside the slot's open/pwrite/fdatasync/close |
| merge | Maint::land_front (self) | 2,239 | 2,438 | +199 | +28 / landing (7) | seal-sync, slot-choice | retiring_wals, watch_slot call, slot write |
| load_durable | Maint::land_front (self) | 305 | 394 | +89 | +89 / landing (1) | same | |
| merge | Db::land_seal (self) | 508 | 568 | +60 | +9 / call (7) | seal-sync | |
| merge | Db::seal + seal_table + freeze (self) | 3,545 | 3,790 | +245 | +22 / seal call (11), +61 / table sealed (4) | seal-sync (hand_seal's awaits_writer flag), layout | freeze/seal_table inlined into seal |
| merge | Maint::collect (self) | 761 | 817 | +56 | +4 / poll (14) | seal-sync | `awaits_writer` test where it already asks `sealed` |
| all | vault::sync_dir | 35 / call | 36 / call | +1 | +1 / dir fsync | open-order | IO_TRACE_ON load beside the fsync |
| load_durable | Db::commit_staged | 19 / call | 19 / call | 0 (incl +9) | | | level |
| load_* / merge | Db::close_direct -> close_direct_with_room | 292,981 | 292,833 | -148 | | seal-sync | renamed, level |
| load_buffered | Maint::join_seal (self) | 430 | 1,332 | +902 | 1 call | none | poll loop's count of parks (timing): park_timeout inlined, Timespec::now, futex |
| all | Wal::frame, Wal::replay(_frames), finish_unsynced_with, SegmentWriter::layout, MemTable::entry/stream_bytes | | | 0 or < 0.01% | | | level (replay's closure renamed #17 -> #10, same count) |

## Syscalls by count (callgrind's call counts into the libc wrappers, rd/syscalls.py)
- load_durable: equal in every kind except writev 111 -> pwritev 111 and write 10 -> 5 / pwrite 4 -> 9 (the segment
  writer's buffered pieces now written at the offset it counts, `write_all_at`). fdatasync 103 = 103.
- load_buffered: the same swaps (writev 12 -> pwritev 12, write -> pwrite 4), and +4 open, +4 close, +4 statx, +28 read:
  the zpages controller read its cgroup files on two more ticks (z_tick 6 -> 7 calls, read_budget 5 -> 7). Timer-driven,
  not a durability path.
- work: equal in every kind (fdatasync 152 = 152, write 147 = 147); only the futex `syscall` count (702 -> 690) moves,
  which is thread wake timing.
- merge: write 79 -> pwrite (AlignedWriter::write_buf_with), writev 14 -> pwritev 14; publishes' fdatasync and close
  moved from manifest_write_slot into Maint::publish (8 = 8); and at the two opens: +2 fdatasync, +2 open, +2 close
  (the slot the open takes, opened and synced: slot-choice) and +10 statx (Db::open_refusals: each name the slot
  lists checked against the directory: open-order). Per open: one open, one fdatasync, one close, five statx.
So no commit, sync, flush, drain, seal, rotation or manifest publish made a syscall it did not make before; Db::open
makes eight more (rule 1 does not list the open, and the open's fdatasync and stats are the fixes themselves).

## Reading
The point read did not move: 200,000 reads, every engine function the same to the instruction, the +209 in glibc's
malloc inside the harness. Shared's new fields (followups item 42) moved no instruction on the read path; whether
they moved anything across a cache line callgrind cannot say, and this machine has no hardware counters.

The fixes cost a store nothing in syscalls on its success paths and a handful of instructions at each seam, with
four exceptions that are more than a load and a branch on a check the path already made:

1. A direct run's commit pays 4 instructions a key (+3,974 a commit of 1,000 keys, in all three probes that run
   one: durable, buffered, merge). The per-key loop in Db::commit_direct -- `put_encoded`, `key_of`,
   `stream_record_shape` -- has the same source on both sides; what changed is the function around it (run-errors,
   38378d1d): the "nothing new" branch now asks `attends()` and `wal.pending` and calls `run_attended` /
   `log_run_again`, the sync's error goes to `run_failed(e)`, and the open takes `run_fault_open`. By line, the loop
   reloads `keys_in_stream` (+2 a key), spends +2 a key at core/ptr/mod.rs:551 (drop glue) and +1 in the key compare's
   slice length handling (cmp.rs:360), against -1 at `self.last = 0` and -1 at `value_len == 0`: register pressure
   and placement, the shape CLAUDE.md names under "Naming the second walk inside the first cost the first". It is
   the largest cost of the eight and it is on a commit's success path. Which of the three changes caused it is not
   known without building each; the smallest form to try is the three new paths behind one cold out-of-line call
   taken from the branches that already exist (`committed == len` and `due`), counted again by function.
2. The run's writes pay 37.6 instructions a write (111 writes): `pwritev_all` (run-errors) collects the IoSlice
   Vec through an out-of-line `from_iter` (10,229 over 111 calls) where `writev_all` had it inlined, plus the
   `*fault >= RUN_FAULT_WRITE` compare (2). The allocation is not new (writev_all made the same Vec). Beside a
   syscall, but more than a load and a branch.
3. A run's sync (`SegmentWriter::sync`, 100 calls in the durable load) is now an out-of-line function, about 21
   instructions more than its inlined body was: the call, the `out.fault != 0` compare (the fault byte; a load and a
   branch) and synced_all's load of the helper's error (run-errors). The fdatasync count is unchanged.
4. A log sync (`Db::sync_wal`, every commit under Always in work) pays 14.7 instructions: `wal_dir_barrier` went out
   of line (10 a call) once its body grew the FAIL_SYNC_DIR seam and `keep_lost` (log-errors, 0aa41df7), plus the
   FAIL_SYNC_LIVE test of `fail_log_sync` (2, log-errors) and `wal_trace.is_some()` (2, log-at-open, a424903d). The
   two seam tests are loads and branches beside the fdatasync; the call is not. The hashed commit as a whole is
   still 18 instructions a commit cheaper than the base (Db::commit inclusive), because rotate_wal_by_size, seal_due,
   flush_merge_finished and MemTable::commit were inlined into it on HEAD -- an inlining result, not a saving any
   fix made, and not to be banked.

Smaller, each a load and a branch beside a syscall or a step the path already made: `Wal::write` +2 a write
(`fail_next.is_some()`); `vault::sync_dir` +1 (IO_TRACE_ON); `Maint::collect` +4 a poll (`awaits_writer`, seal-sync);
`Db::land_seal` +9 a landing; `Db::seal` +22 a call (+61 a table sealed), the hand-over's `awaits_writer` argument and
freeze/seal_table inlined into seal; `Maint::land_front` +28 a landing (retiring_wals, a slot write's watch).

The manifest publish pays about 53 instructions a slot write (slot-choice, 2e8b173d): three seam loads (tear_slot in
write_slot, fail_slot_sync in sync_slot, slot_watching in slot_synced), the durable-slot word (`1 - self.durable`,
`self.durable = slot`), and two out-of-line calls -- `Maint::watch_slot`, 19 instructions whose body is one Relaxed
load of `slot_watching`, and `vault::open_slot` (42 a call, with its IO_TRACE_ON load), which was inlined before.
Beside an open, a pwrite, an fdatasync and a close per slot it is small, but the watch is a call where the rule
allows a load: `slot_synced` already tests the same flag inline, and `watch_slot` can be gated the same way.

A delete pays about 18 instructions more (delete-max, f8d130a8): the `key > self.max_key` compare a put already
made, now on the delete, which is a memcmp call (14) and the slice setup (about 4); 5,000 deletes in work. It is the
fix (the greatest key must count a tombstone) and not a seam; it is more than a load and a branch, on a write's
path, which rule 1 does not list by name.

An open does new work by design (open-order, slot-choice): per open, five statx (each name the slot lists), one
open, one fdatasync and one close of the slot it takes, and about 8,100 instructions (open_refusals 6,620 in path
joins and metadata, unnamed_refusals 406, open_slot, a name lookup); the open's total moved only +2,345 because the
zpages cgroup read in resolve_auto_cache read 5,000 fewer instructions an open, which is the environment.

Moves that are the compiler's and no fix's work, read for what changed around them: the bench's write_batch
(Db::append inlined) -2 a key, after writes_go's slow path became a loop (seal-sync), its fast path unchanged;
Db::append +1 an append in merge, same cause, the per-append source unchanged; push_chunk inlined into work's main
(-1.8M self, main +0.7M), MemTable's source unchanged; stream_hash -207 a commit (one line, block.rs:168, the CRC's
tail by the arena blocks' alignment) and memcmp -22,271 (its page-cross path by address); Maint::join_seal +902 in
one call of load_buffered (the poll loop parked more times). No function whose source and callees are unchanged
moved except through inlining or data alignment.

## Not verified
- Which of the run-errors changes to commit_direct makes the +4 a key: not separable without builds of each, and
  this reading ran no build.
- Cache-line effects of Shared's new fields (item 42): invisible to callgrind; no PMU here.
- The per-delete figure (~18) is read from work_probe::main, into which Db::delete is inlined beside puts and
  appends: the memcmp count (+5,000 calls, +70,000 instructions) is exact, the ~4 around it is read off the lines
  that moved by exactly +5,000.
- agg.py's cmp.*.self.txt were not used (see above).

## Method
- Totals: `summary:` lines of the .out files. Self and inclusive by function: `callgrind_annotate --auto=no
  --inclusive=no|yes` (rd/<side>.<probe>.self|incl), summed over the files a function's inlined lines come from
  (rd/agg2.py).
- Per line, per callee and call counts: the .out files parsed directly (rd/fnlines.py, rd/callees.py, rd/fncmp.py,
  rd/ncalls.py, rd/callers.py, rd/syscalls.py), lines mapped to `git show 3df85d22:src/db.rs` (rd/base_db.rs) and
  HEAD's src/db.rs. No cargo, valgrind or build was run.
