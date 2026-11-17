# The fix candidates in the crash model (ADJ, JRP + Stream, and decision 2)

Directory: `scratchpad/seam/work/fix/model/`. Tree `3df85d22`. Nothing under `/home/user/supdb` was
touched and no git command was run. No bench process ran during any of these runs (checked before
each; `batch.sh` loops on `pgrep -f 'bench ab|pair.sh'`). Every run was under `nice -n 19`, one at a
time, each under 4 minutes but one (`jrp infl2 ... RPMAX=2 cont=1`, 233 s). One run that was too
large for the budget (`adj infl2 cap=20000`) was stopped by hand at 10 minutes and rerun smaller.

| file | what |
|---|---|
| E4's files, copied unchanged | `pfm.py`, `logrun.py`, `e4.py`, `replace.py`, `dirmodels.py`, `orig/` (E4's own notes stay at `work/e4-model/notes.md`) |
| `cand.py` | device, world, oracle, tally, the no-resume driver |
| `adj.py` | ADJ: cases 0-5, P, chained tag pages, its recovery |
| `jrp.py` | JRP: byte-packed ring, homings, RPs, pins P1-P3, its recovery |
| `stream.py` | the WAL: JRP's Stream class, ADJ's Journal class, today's LogCipher + the engine's open-time cut; resumes after crashes |
| `run.py`, `batch.sh`, `list_*.txt`, `explain.py` | families, the batch runner, the run lists, a one-sequence tracer |
| `results/*.txt` | every run's output, first counterexamples included |

Reproduce: `sh batch.sh list_adj.txt out.txt` (and `list_jrp.txt`, `list_stream.txt`,
`list_extra.txt`); one sequence: `python3 explain.py adj "(1,'S',3,'S',4)"`.

## The model

What it keeps from E4: an object at a crash holds its durable image, any image written since the
barrier that covers it, or a mix; fdatasync begun at tb and completed makes every object dirty at
tb hold its tb image or later; a process crash keeps the page cache; every (key, nonce) sealed is
logged. What it changes:

- **Two kinds of location.** A *page* (data page, RP slot, ADJ record page, tag page, header) is
  written whole by a call; a mix of two different images authenticates as nothing and is collapsed
  to TORN, added only where no option already reads as nothing, and not for an identical rewrite
  (E4's `Dev.any_views` added TORN there too). A *sector* (JRP's ring, the Stream file) is atomic and
  holds units, so a byte-packed frame or record is torn at every sector boundary, and a sector a
  pinned frame shares with a new one holds old or new.
- **A process crash can cut a syscall**: the page cache then holds a prefix of its objects in file
  order. Checked at every such point of the last call of every sequence.
- **The run temp is never written after a crash** (db.rs:27727-27770, read: `recover_direct`, a
  rebuilt segment, the temp removed), so ADJ and JRP are judged without resume: every op sequence
  up to the length given, judged at its end (every prefix is itself enumerated, so this covers every
  instant) and inside its last call. **The WAL is resumed** (db.rs:27924-27936, read: replay, the
  cut, `open_append`), so `stream.py` branches at a power loss over every view and writes on.
- **Checks**: LOSS (a state before the last completed sync, or before the last completed call after
  a process crash), WRONG (a state the file never held), UNREADABLE (a page below the recovered
  length that does not verify), NONCE_REUSE / NONCE_REPEAT, and PARTIAL (a byte prefix of a later
  call, past the acked state: reported, not one of the four checks; it contradicts JRP's "a prefix
  of the write calls").
- **Exhaustive or sampled.** A sequence whose views exceed `cap` is stride-sampled over its
  product, and the run says how many sequences were (`sampled`). Every family below has an
  exhaustive run beside its sampled one; the claims are the exhaustive runs' unless noted.

Scale: PB = 2 bytes a page, G = 2 pages a group; ADJ M = 2 (q_max 6), 8 ring slots of 2 pages,
a piece of 2 pages, a record of 1 page plus 1 per carried image; JRP T_d = 2 pages, T_h = 3 pages,
a ring of 16 units (and 8) in sectors of 2, a frame 1 + b units, an RP of 1 slot (2 past 2 pages);
Stream sectors of 3 units, a record 2 + b units. Write sizes 1-7 bytes: inside a page, completing
the tail page, completing several. Families: `dur` (every call synced, the last unsynced or in
flight), `infl` and `infl2` (the helper's sync in flight across synced and unsynced calls, two at
once), `fail` (a syscall of a call fails, its objects torn in the page cache, the engine resends:
E4's retry model), `buf` (whole-piece calls, the rest, syncs in flight), `any` (every interleaving
of writes, syncs and syncs in flight).

## ADJ (design-adjacent/design.md)

### A1. Case 1 writes a whole piece over a protected record. Counterexample, closed, rerun.

Case 1's condition (t = 0 at a piece boundary, the call whole pieces, Q empty) does not ask P,
and nothing else stops it: a flush (case 2) that ends exactly on a piece boundary leaves its record
at home(T), the first page of the next piece, and the next whole-piece call is case 1.

`explain.py adj "(1,'S',3,'S',4)"`: call 1 (1 byte) is case 0, a ring record. S. Call 2 (3 bytes,
L = 4, T = 2, t = 0) is case 2: homes 0 and 1, its record at home(2), tag page 0. S: that record is
A_sync and A_pc. Call 3 (4 bytes, a whole piece at L = 4) is case 1: homes 2 and 3, the first over
the record. A power loss that keeps the new home(2) (or tears it) and not call 3's ring record
leaves call 1's record as the newest that verifies: 1 byte recovered of 4 acked. And with no sync
at all, `(1, 3, 4)`: a process crash between call 3's homes and its ring record loses call 2, a
completed call.

| run | sequences | any states | proc states | violations |
|---|---|---|---|---|
| `adj dur n=4` sizes 1-5 (first run) | 2,340 | 76,022 | 6,015 | 360 (336 any LOSS, 24 proc LOSS) |
| `adj dur n=5` sizes 1-5 | 11,715 | 493,733 | 29,639 | 3,318 (3,132 any LOSS, 186 proc LOSS) |
| `adj buf maxlen=5 cap=2000` | 4,897 (759 sampled) | 2,100,231 | 16,174 | 2,694 (2,523 any LOSS, 171 proc LOSS) |

**Smallest change**: case 1 falls to case 4 when a home of H is in P (`case1_checkP`). Rerun: 0
violations in every family below.

Reachability in the engine, by reading `stream_flush` (db.rs:5707-5753: one write per piece, never
longer than a piece, starting at `written`): in the buffered run every mark writes a whole piece
at a boundary (case 1) until a sync writes the rest mid-piece; the next mark's first write runs
from there to the boundary, and if it completes 1..M pages it is case 2, its record at the next
piece's first page with t = 0, and the mark's next whole piece is case 1. That is about M/256 of
the syncs (one in 32 at M = 8; arithmetic, not counted on a run). Under the durable policy a rest
must end exactly on a 1 MiB boundary first, which is rare. Not verified on the engine.

### A2-A4. Three rules the spec has to state (the model implements them; the literal readings fail)

- **A2. synced_home** is the home frontier of the record registered at the sync's issue, counting
  its pending pages: min(A_sync.Q ∪ {A_sync.T}). Taken as A_sync.F (pending ignored, `sh=F`):
  1,034 UNREADABLE (`adj dur n=4`), first `2 S 2 S 2`: a page pending at the sync goes home after
  it, unsynced, and is below the bound recovery verifies from. Taken at the sync's completion
  (`sh=completion`): 5,291 UNREADABLE in `any`, first `B 5 E 1`, and 20,889 in `infl2`.
- **A3. Case 2 needs H contiguous.** Case 4 leaves a blocked page in Q (the old tail under a
  record still in P), so the next H has a gap; this happens in the durable family (event
  `case2_noncontig` 3,804 times in `dur n=5`). The model sends it to case 4 (0 violations). "One
  write over [home(min H), home(T)+r-1]" re-seals the home pages in the gap (`lit_interval`):
  61,860 violations (61,480 any LOSS), first `2 S 4 S 2`: call 1 flushes (record at home(1));
  call 2 completes pages 1 and 2 with m > m̂, so case 4 sends page 2 home and keeps page 1 (under
  the record) in Q; call 3's H is {1, 3}, and the interval write re-seals page 2, which the synced
  ring record names.
- **A4. Case 3's placement can meet P** (`carry_meets_P`: 11,386 times in `infl n=5`, 1,109 in
  `infl2`); the spec has no rule. The model falls to case 4: 0 violations.

### ADJ with A1's change: every family

| run | sequences (sampled) | any states | proc states | violations |
|---|---|---|---|---|
| `dur n=5` sizes 1,2,3,4,5 | 11,715 | 497,619 | 29,672 | 0 |
| `dur n=7` sizes 1,3,5 | 9,837 | 421,039 | 24,055 | 0 |
| `infl n=5` sizes 1-5 | 43,955 | 1,451,836 | 79,892 | 0 |
| `infl2` sizes 1,3,5, cap 1500 | 3,900 (2,164) | 3,858,861 | 11,731 | 0 |
| `fail maxlen=5`, cap 2000 | 19,244 (8,270) | 19,799,104 | 54,228 | 0 |
| `buf maxlen=6`, cap 5000 | 26,441 (3,959) | 26,886,153 | 89,130 | 0 |
| `any maxlen=4`, exhaustive | 3,063 | 28,937,470 | 12,473 | 0 |
| `any maxlen=5`, cap 3000 | 22,483 (7,366) | 26,318,270 | 92,416 | 0 |

No run was Blocked: a free ring run always existed with 8 slots and up to two syncs in flight.

**T** (ADJ with case 4 for every m ≥ 1, `M=0`): 0 violations in `dur n=5` (165,185 any), `infl n=5`
(156,317), `infl2` (3,002,727), `fail maxlen=5` (20,141,046, 8,365 sampled), `any maxlen=5`
(24,604,076, 6,445 sampled). Its records never sit in the data area, so A1 cannot arise in it.

### ADJ mutants

| mutant | run | result |
|---|---|---|
| the one-commit delay removed (`nocarry`: the flush written over the protected record) | `dur n=4` | 9,038 (8,116 any LOSS, 922 proc LOSS), first `2 S 2` |
| nothing protected (`P_none`) | `dur n=4` | 20,631 LOSS |
| the record written over the newest durable copy (`P_no_sync`: A_sync not in P) | `any maxlen=5`; `buf maxlen=6` | 784,321 any LOSS, first `1 S 1 1`; 986,280 any LOSS |
| recovery taking the newest record unchecked (`rec_nocheck`) | `dur n=4` | 30,040 UNREADABLE |
| a version counter reused (`m_ctr`: seq, attempt and ctr given back on a failure) | `fail maxlen=4` | 576 NONCE_REUSE (`('f',1,0) 1`, the record at (seq, attempt) again) + 1,872 NONCE_REPEAT |
| no in-flight pin (`P_no_inflight`) | `infl2`; `infl n=4`; `any maxlen=4` | 829,124 any LOSS; 0; 0 (needs two unsynced calls inside the sync, which only `infl2` builds) |
| synced_home as F; at completion | above (A2) | fail |
| no rounding of synced_home (`noround`) | `dur n=5`, `any maxlen=5` | 0: not seen, as E4 found for sealed tag pages |

## JRP (design-free/design.md sec. 5), the run

### As written: no violation found

| run | sequences (sampled) | any states | proc states | violations |
|---|---|---|---|---|
| `dur n=6` sizes 1,2,3,5 | 16,380 | 228,164 | 39,787 | 0 |
| `dur n=5` sizes 1,2,3,4,5,7 | 27,990 | 510,746 | 70,982 | 0 |
| `infl n=5` | 15,028 | 60,952 | 25,069 | 0 |
| `infl2` sizes 1,3,5, cap 1500 | 3,900 (1,320) | 2,849,234 | 10,331 | 0 |
| `fail maxlen=5`, cap 3000 | 18,243 (9,239) | 31,379,060 | 60,021 | 0 |
| `any maxlen=5`, cap 5000 | 11,098 (1,762) | 13,607,142 | 40,604 | 0 |
| `any maxlen=6`, cap 400 | 70,851 (28,167) | 13,401,431 | 259,380 | 0 |
| ring of 8 units: `dur n=6`, `infl n=5`, `infl2`, `fail maxlen=5`, `any maxlen=5` | | 218,784; 60,568; 2,857,230; 30,994,528; 12,915,456 | | 0 each |
| ring of 8 units, `any maxlen=4`, exhaustive | 1,744 | 3,732,734 | 6,308 | 0 |

The ring-full path (a homing instead of a frame) ran 925 times in `infl2` at 8 units.

### J1. A call homed as several [pages][RP] in one pwritev: the intermediate RP is an end state

Sec. 5.1: "An RP covers at most 1024 pages; a larger call is several [pages][RP] in the same
pwritev." An RP's body is the state after the call with a tail of at most one page, so an
intermediate RP can only name the state at its own chunk's end. Recovery takes the newest RP that
verifies with its chain, and frames journaled before the call carry bytes past that chunk's end.
Modelled with at most 2 pages an RP (`RPMAX=2`): `explain.py jrp "(1,'S',3,'S',1,'S',1)" RPMAX=2`:
three journaled frames reach L = 5, synced; the fourth call homes pages 0-2 as [0,1][RP_a][2][RP_b];
a power loss (or a process crash inside the pwritev) that keeps the slots through RP_a returns
RP_a's state, 4 bytes of 5 acked, and the frame holding the fifth byte is older than RP_a and never
applied. `dur n=5`: 31,205 violations (2,716 any LOSS, 6,596 any WRONG, 182 proc LOSS, 442 proc
WRONG, 21,269 PARTIAL); `any maxlen=5`: 456,644.

**Smallest change**: every RP of a call but its last is marked CONT and recovery never ends at a
CONT RP (it still has to verify, as part of the chain). Rerun (`cont=1`): 0 in `dur n=5` (326,101
any), `any maxlen=5` (13,741,694, 1,813 sampled), `infl2` sizes 1,3,5,7 (18,066,312, 10,301
sampled), PARTIAL gone too. One RP per call, its slots growing with the tags, is the other way.

Reachability: today no run write call exceeds a piece (db.rs:5707-5753, read), and T_h = 128 KiB,
so a homing is at most about 288 pages, under the 1024-page cap: not reachable through
`stream_flush`. Whether the close's writes before `finish` (the trailer, the key section) can be
larger calls was not checked.

### JRP mutants

| mutant | run | result |
|---|---|---|
| a homing that starts at the newest RP's slot (the record written over the newest durable copy) | `dur n=5` | 78,182 (72,105 any LOSS, 6,077 proc LOSS) |
| the partial tail written home, not carried in the RP (JRP has no one-commit delay; this is its counterpart) | `dur n=5` | 41,906 LOSS |
| an RP trusted without its pages (recovery taking the newer unchecked) | `dur n=5`; `any maxlen=4` ring 8 | 10,574; 550,966 UNREADABLE |
| an RP trusted without its predecessors | `any maxlen=4` ring 8 (after fixing a model crash on the broken chain) | 96,759 UNREADABLE, first `5 5`; `dur`: 0 (needs two unsynced homings) |
| P1 dropped once a newer unsynced RP exists | `any maxlen=5` ring 8; `infl2` ring 8 | 35,554; 201,936 LOSS; ring 16: 0 |
| no in-flight pin (P2) | `infl2` ring 8 | 105,403 LOSS; ring 16 and `infl`: 0 |
| no P3 | `any maxlen=5` ring 8 | 431 (424 proc LOSS); ring 16: 0 |
| a version counter reused (seq, attempt, ctr given back) | `fail maxlen=4` | 240 NONCE_REUSE + 1,224 NONCE_REPEAT |
| frames applied across a seq gap | `any maxlen=5`, rings 16 and 8 | **0: not seen.** Each TAIL must begin at the current length, which says everything seq contiguity says while every frame appends; a patch frame (a TAIL at an older offset, sec. 5.3 step 1) breaks that, and patches are not modelled |

The pins are caught only with the ring of 8 units: at 16 the ring does not wrap inside a pinned
span within five or six calls.

## The WAL: Stream (JRP's decision (c)), Journal (ADJ sec. 10) and today (decision (b))

### W1. A record a power loss left behind a lost one is adopted after the next open

Both new classes read records "to the first that fails" and say nothing of what the stored file
holds past that point, nor what length the layer answers. Modelled with the length the plaintext
replay returns (neither design says otherwise), the engine's cut (`if len > valid_len`,
db.rs:27929-27935) never fires. A power loss can lose record k and keep
record k+1 (unsynced writes land in any subset). The next open appends at record k's offset; when
the new record has record k's length, the stale k+1 sits exactly where the next record is read, its
AAD (offset, length) matches, and its key is the file's (Stream: derived per file_id; Journal: the
old session's key, which the reader holds).

`stream.py base maxlen=6`, first: `(1, 1, P, 1)`: two records, nothing synced; the loss keeps the
second record and neither the header nor the first, so the open recovers an empty log (legal); the
header is written again and record 1' lands at unit 1 with record 1's length, and replay then reads
1' and the old record 2: a state the file never held, with a write the first recovery had rolled
back. The Journal's first counterexample is the same ops with the header kept: the old record 2 is
under session 0's key, seq 1, right after 1' at seq 0 under session 1's.

| run | any | proc | violations |
|---|---|---|---|
| Stream as written, `base maxlen=6` (31,698 sequences, exhaustive) | 1,386,458 | 287,715 | 4,483 WRONG |
| Stream as written, `fail maxlen=5` | 1,219,464 | 154,215 | 188 WRONG |
| Journal as written, `base maxlen=6` | 1,386,974 | 287,813 | 2,637 WRONG |

The engine as it stands is not exposed: under today's LogCipher replay answers the stored end of
the last whole record (db.rs:4573-4581, read) and the open cuts the file there and syncs before
appending (db.rs:27924-27935, read; its comment names this hazard for a batch). That is decision
(b) as ADJ keeps it.

**Smallest changes**, each rerun over `base maxlen=6`, `fail maxlen=5` and `twoP maxlen=6` (two
power losses): 0 violations each.
- Stream and Journal: the record's AAD carries the previous record's tag, the first record's the
  header's (a header written again starts a new chain) (`chain=1`). No I/O.
- Stream: the layer reports a length past the plaintext whenever the stored file holds bytes past
  the last whole record, so the engine's cut fires, and the cut is synced (`cut=sync`, today's
  behaviour). The cut without its sync (`cut=nosync`) fails: 195 WRONG, first `2 2 1 P 2`.
- Journal: replay refuses a record of an older session after a newer one (E4's first_seq window,
  `sess_order=1`).

### WAL mutants (decision (b), `cut=sync`, unless said)

| mutant | result |
|---|---|
| the one-commit delay removed | not applicable: an appended log has no delay |
| the record written over the newest durable copy (`over_last`: a write re-seals the last record with the new bytes, in place) | 9,007 (5,375 any LOSS, 3,632 proc LOSS); the same with `chain=1` |
| replay taking a record whose head is there, tag unchecked (`nocheck`) | 33,207 WRONG; with `chain=1` 33,511 |
| a version counter reused: the count not advanced on a failed attempt | 75,015 NONCE_REUSE; Journal with `chain=1`: 77,312 |
| a version counter reused: the prefix per file, not per open | 6,976 NONCE_REUSE |
| a retry appended after the failed attempt's bytes (JRP's mutant 11) | 13,691 LOSS (11,075 proc) |
| the cut not synced | 195 WRONG |

## Per candidate, in one line each

- **ADJ cases 0-5**: unsound as written (A1); sound in the model with case 1 asking P, given A2-A4
  stated as the model has them.
- **ADJ's T** (two writes, self-sufficient ring records): no violation found.
- **ADJ's decision (b)**: today's LogCipher and cut, no violation found. ADJ's optional Journal class
  as written: W1; closed by the chain or the session order.
- **JRP's run**: no violation found as written for calls homed under one RP; the split into several
  RPs a call fails (J1) and is closed by CONT; not reachable through today's `stream_flush`.
- **JRP's Stream WAL**: W1 as written; closed by the chain, or by a length that makes the engine
  cut, with the cut's sync.

## Not modelled, and what the model cannot see

- `finish` and FINAL, patches and OVR (so the seq-gap mutant is blind), `set_len` other than the
  WAL's open-time cut, recycle (E4 F7), header slots and resume beyond one header (E4 F1 is
  assumed fixed: the Journal's reader knows every session's key), the extent boundary, the tag tier,
  the pager, names.
- ext4: recovery reads every block written; a stale i_size can only hide more. Allocation and
  whether a homing or a flush is one request are E5b's question, not this model's.
- A failed call: the failing syscall's objects torn, earlier ones kept (E4's model); a write that
  fails after its bytes landed is not modelled.
- ADJ: m̂ is the maximum m of the last four completed calls (the spec says synced calls); r is one
  page for a flush record; overrides are not carried; case 1's piece is 2 pages.
- Sampling: the `any`, `fail`, `buf` and `infl2` runs sampled their largest sequences (counts in
  the tables). Exhaustive runs beside them: ADJ `any maxlen=4` (28.9 M states), JRP `any maxlen=4`
  at 8 units (3.7 M), every `dur` and `infl` run, every Stream run.
- Scale: 2-byte pages, 2-page groups, rings of 8 or 16 units; the counts are model states.
- Not verified on the engine: A1's frequency in the buffered run, and the size of the close's write
  calls (J1's reachability).
