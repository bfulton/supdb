# E4: the crash model, extended (design.md sec. 4, 9)

Directory: `scratchpad/seam/work/e4-model/`. No bench run, no build, nothing under /home/user/supdb touched.

| file | what |
|---|---|
| `orig/` | the existing model (`model.py`, `inflight.py`) copied unchanged, rerun |
| `pfm.py` | device, sealing, the Log Handle (sessions, header slots, ring with pins, CKPT, GROUP/HOME, re-home, OVR, TRUNC, finish/FINAL, retry), Log recovery, options for every fix and mutant |
| `logrun.py` | op-sequence driver for one Log file: proc/any checks, power-loss branching with resume |
| `e4.py` | Log families: base, inflight, resume, header, retry, trunc, finish, mix |
| `replace.py`, `cmp_replace.py` | Replace class under the engine's two-slot manifest; plain vs Pg comparison |
| `dirmodels.py` | recycle (spare -> WAL under a new file_id) and the table compaction (.pfcopy, rename-over) |
| `explain.py`, `dbg.py` | prints a counterexample's crash state object by object |

## The model

Device. Every object (data page, sealed tag page, ring frame slot, header slot, REC slot, a plain manifest slot) is several sectors written whole by one syscall. At a crash it holds its durable image, any image written to it since the barrier that covers it, or TORN (any mix: no mix authenticates, so all sector masks of one object collapse to TORN; TORN over a hole is the hole). fdatasync begun at tb and completed: objects dirty at tb hold their tb image or a later one; later-dirtied ones are free. Process crash: the page cache survives, every completed write call kept. Directory ops land in any in-order subset until sync_dir (`dir_mode=prefix` = ext4's ordered journal). Power-loss states are enumerated just before each sync completes and at each run's end: between two completed barriers the state set only grows and the legal set only grows, so every earlier state is covered.

Crypto. A sealing is an image id; it verifies at a location iff every sector holds it, it was sealed for that location, and under the key the reader derives (header table, ownership, session idx). Every (key, nonce) is logged: a second sealing is NONCE_REUSE (different plaintext) or NONCE_REPEAT.

Checks per crash state: recovery returns (never REFUSED); content is one the file held at or after the last completed sync (any) or exactly the last completed write call (proc, also between the syscalls of a call); every page below the recovered length verifies (UNREADABLE = the poison of sec. 4 Trust); LATENT (a closed group whose tag page is not on disk, readable only through frames); nonce log. Termination: recovery loops are bounded by the ring (<= R CKPTs, <= R frames a run); a 10,000-iteration assert never fired. That is all the model says about termination.

Scale: PB=4 bytes a page, G=2 pages a group (G=4 checked once), K=2-8 frames a CKPT, R=5-8 ring slots. Power-loss branches are sampled to `cap` per point where a run says so; the authoritative runs below are unsampled unless noted.

## Reproduction of the existing claims

`orig/model.py fix_caw 3`: prefix 1,776, offset 2,852, proc 444, any 1,583,118 states, 0 violations (16 s). `orig/inflight.py`: no pin 244,032 of 262,144 violate, pin 0. Matches design.md:280. (`model.py fix_caw 4` did not finish in 14 min and was stopped.)

New model, design as written, no new protocol: `e4.py base maxlen=5`: 9,330 runs, any 11,779,603, proc 61,673, 0. `e4.py inflight maxlen=8 R=8`: 1,312,998 any, 0; `pin_inflight=False`: 206,729 LOSS of 1,765,712.

## Findings (first counterexample, fix, rerun)

### F1. A resumed Handle's header overwrites the only durable header (Log; Replace when sessions are eager)

Design: "the header is never synced on its own" (design.md:239), new header into the slot that is not the newest valid one, newest hdr_seq wins (design.md:107).
`e4.py header maxlen=6`: 480 LOSS of 6,147,778. First: ops `1 S X 1 X 1`. Write 1 byte, sync (acked: header A durable). Process crash; reopen; write 1 byte: session s2, header B written (unsynced). Process crash; reopen: header B is newest *in the page cache*, so header A is overwritten (s3). Power loss: A TORN, B never written back. No valid header -> empty -> the acked byte is lost. Overwriting the newest slot instead (`hdr_same_slot`) fails sooner: 4,864 LOSS, ops `1 S X 1 X`.
Fix (`resume_sync`): a Handle that cannot show its newest header durable (every Handle after an open) issues one fdatasync before its first header write. Rerun: 0 of 501,334. The engine's WAL open issues no sync unless replay cuts (db.rs:27929-27934), so nothing else covers this. Cost: one fdatasync per resumed Log/Replace file per store open, at its first write; not measured here (CLAUDE.md's figure for a sync with nothing dirty is 0.03 ms). Not modelled: carrying new sessions in sealed ring frames instead of a header rewrite.

### F2. A sync covers groups whose re-home is still owed (Log, after any unclean open)

Design step 6 (design.md:278) re-homes the tag pages of groups closed in the replayed run; it does not say when, and says "Those tag pages were never durable", which is not always so. With re-home at the session's first write:
`e4.py resume maxlen=5 cap=150 resume_sync=True`: 24 UNREADABLE. First: `5 5 P S 1`. Gen 1: 10 bytes, group 0 closed, power loss with frames 0-2 on disk and tag page 0 not. Gen 2: `S` completes with nothing written and sets synced_home=2. Write 1: s2's header, re-home of tag page 0, a W frame and a CKPT with sh=2 naming the re-homed MAC. Crash: the CKPT lands and the re-homed tag page does not. Recovery picks the CKPT and verifies nothing below sh -> pages 0,1 unreadable (poison). The same shape after a process crash (`5 5 X S 1`, mix family): the old tag page t0 is valid and durable, and the CKPT names the re-home MAC that never landed.
Fix (`sh_rehome`): a sync's synced_home stops at the first group whose re-home is pending (alternative `eager_rehome`: re-home at the open, before any sync; it also passed at maxlen 5). Re-homing only tag pages that fail at recovery (`rehome_invalid_only`) avoids rewriting a durable tag page in place, but alone it fails: unsampled `resume maxlen=5` gives 20 UNREADABLE. Together with `sh_rehome` it passes.
Reruns with resume_sync + sh_rehome:
- `resume maxlen=5` unsampled: 0 of 2,603,214 any and 67,440 proc;
- `resume maxlen=6 cap=60`: 12,105 runs, 0 of 23,343,007;
- `resume maxlen=6` unsampled, time-limited: 11,359 of 12,105 runs, 0 of 39,449,074 any and 1,165,443 proc.
Own mutant `resume_trust` (the resumed Handle takes the recovered state as synced): 968 UNREADABLE.

### F3. A failed write's frame: the seq is spent (gap) or reused (nonce)

Design: ctr advances at every sealing attempt (design.md:161-162), but the frame nonce is `0x11 || seq || 0` (design.md:155) and has no ctr. The engine resends `pending` after a failed write (db.rs:4436-4455).
- `retry=R0` (seq spent, which the design's "seq never repeat" implies), `e4.py retry maxlen=4 K=8`: 1,584 violations (124 any, 1,460 proc LOSS). First: `('f',1,'frame') 1 S`. The failed call's frame seq 1 is TORN in the page cache. The retry is frame seq 2. Recovery's "longest contiguous run" stops at the gap, so the retry's acked bytes are lost, after a power loss and after a mere process crash. At K=2 the next CKPT bridged every gap within 4 ops (0 violations), so the failure needs a retry and its ack before the next CKPT; the design writes a CKPT every 256 KiB of frames.
- `R1` (seq reused): 782 NONCE_REUSE on ('f',1,0).
- Fix `R2`: frame nonce `0x11 || seq || attempt u24`. The retry reseals the same seq at attempt+1, and recovery takes, per seq, the highest attempt that verifies. Result: 0 at K=2 (919,003) and K=8 (384,263).
- Mutant "ctr reused on retry" (`ctr_rollback`): 552 NONCE_REUSE plus 900 NONCE_REPEAT (pages resealed at the same (p, ctr): identical ciphertext, which the design's hook would also flag).

### F4. TRUNC as written does not work. Today's engine cannot reach it

The design's TRUNC{len} (design.md:223-225), with the page containing len as the tail and re-seal only after a synced CKPT, fails three ways in turn (`e4.py trunc`).
(a) The session that cuts takes base = the old length's page. When the cut page fills again it goes home under that session at a page below its base. owner(p) is the older session, so the page never verifies. LOSS: `5 S X T3 S 1 S 1 S`, recovered 3 of 4 acked bytes.
(b) Create the session at the cut with base = the cut page (`trunc_base`). Then the header can land before the TRUNC frame, and the new session owns the old session's acked pages. LOSS: `5 S X T3 S`, recovered empty of 5 acked.
(c) Add TRUNC as a CKPT carrying the cut page's bytes (`trunc_ckpt`) and ownership by sessions present in the chosen run (`owner_by_run`). This still fails: the owed re-home under the new session puts it in the run before its TRUNC. 936 UNREADABLE: `5 S 5 P T3 S`.
The spec also leaves open:
- where the tags of pages below the cut live in a group the cut re-opens, when that group's tag page is named by synced CKPTs and "never rewritten" (my first recovery lost them: UNREADABLE);
- where a refilled page past the cut goes before a synced post-TRUNC CKPT (an OVR here, which breaks the "4 per file" bound).
Reachability, by reading and not verified by a test: replay cuts only `if len > valid_len` (db.rs:27929-27934). Under Pg every layer write is whole batches (`Wal::write` writes `pending` after `mark_commit`, db.rs:4436-4455) and a frame is atomic, so the recovered length is always a commit boundary and the cut is never below it.
Recommendation: refuse (debug-assert) a set_len below the recovered length on a Log file, or implement it as the compaction to a fresh file_id (F8's protocol, which passes).

### F5. finish: FINAL's scope, and the override pages written home

Design: finish writes override pages and the tail home at fresh ctrs, the last tag page, and FINAL; the open verifies "the pages finish wrote" and otherwise falls back to the ring (design.md:227-231). The engine's close runs the records' presync beside SegmentWriter::finish (trailer and head patches) and calls the layer's finish before `seg.sync_all()` (db.rs:28975-29062).
`e4.py finish` (1,404 sequences: durable and buffered policies; trailer, patches of pages 0/1, presync in flight or not):
- (a) LOSS, `5 S B O(P0) E Z S`. Page 0 is acked. The patch is an OVR frame, unsynced. finish writes page 0 home in place. Crash: page 0's new image lands, and neither the OVR frame nor FINAL does. The ring fallback needs page 0's old image, which is gone. Recovered empty of 5 acked.
- (b) UNREADABLE, FINAL trusted: `1 S B 5 E Z S`. The trailer's page 0 is written beside the presync and is never synced. FINAL and the pages finish wrote land, page 0 does not. FINAL verifies its own pages, so the file is trusted with a page that fails. That poisons the store, and `recover_direct` reads exactly this temp after such a crash (db.rs:6948).
- Totals: design 1,222,970 violations (time-limited at 1,344 runs); `final=verify_sh` 891,076; `final=deferred` 5,134 (class a); `final_ovr_rec` alone 1,024,682 (class b).
- Fixes that pass all 1,404 sequences:
  - **verify_sh + final_ovr_rec**: FINAL verifies from round_down(synced_home at FINAL) as well, and carries the <= 4 override images, so overridden pages never go home. 0 of 16,302,381.
  - **deferred + final_ovr_rec**: finish's writes and FINAL go after the final sync completes, unsynced, and FINAL carries the overrides. 0 of 5,510,928. A crash before FINAL lands reads the file as Log, which holds everything synced.
- `deferred + lazy_home_check` (skip the HOME check of pages a later OVR overrides) still fails: 5,134. The newest CKPT's own range verifies the overwritten page too.
- Mutants:
  - "FINAL trusted without verifying": 3,160 (deferred) and 206,526 (verify_sh, body 2) UNREADABLE;
  - "OVR written home before finish": 1,219,345 and 227,222 LOSS/UNREADABLE.

### F6. Replace: parity with plain; an engine hazard in both; Pg sessions must be lazy

`replace.py plain|pg` models the engine's protocol: slot by seq parity (db.rs:11598-11600), write_all_at then sync (11695-11721), the drain's owed sync superseded by the next synced publish (38851-38910), and the newest valid slot wins (11890-11913).
- Design defaults, maxlen 4: plain 1,112 violations, pg 874,526. `cmp_replace.py` shows **the same 924 failing sequences of 2,539 in both**, so the layer adds nothing.
- The failures are the engine's:
  - `W U W` LOSS: publish 1 synced in slot 1; publish 2 owed in slot 0; publish 3 overwrites slot 1 in place. Crash: slot 1 TORN, slot 0 still holds 0, so 0 < acked 1. A publish overwrote the newest durable slot while the other slot's write was not durable.
  - `U W`: NO_MANIFEST.
  - `W B W`: a sync in flight.
  - `W U X W`: after an unclean reopen, the newest slot found is in the page cache only.
- Fix (engine, outside the layer, both needed): `settle_other` (before a publish, wait for the other slot's in-flight sync and issue its owed one) plus `open_sync` (an open syncs both slots before its first publish). maxlen 5: plain 0 of 109,468; pg 0 of 1,513,393. Not verified against a real store.
- Pg-specific: sessions created eagerly at the Handle's open (`lazy=False`) give 314,520 NO_MANIFEST with ops `X` alone. owner(p) is the newest session with base <= p (design.md:257), so a session that has rewritten nothing owns the slot's durable pages and the slot reads damaged as soon as its header lands; resume_sync does not help. A session must be created at the slot's first write, which rewrites it whole.
- Session drop (`drop=after_sync`, `oldest cap=2`) and positional session idx (`idx=pos`): 0 violations each, so the model cannot see them. Each one is masked, because every session change comes with a whole rewrite while the other slot is durable. design.md:260's drop rule ("once a synced whole rewrite under a newer session exists") cannot be known after an open except through F1's sync.

### F7. Recycle: a legal crash state is refused

Design (design.md:264-268): the new file_id, flags.recycled + prev, header unsynced, "A file_id mismatch in any other case is refused".
`dirmodels.py recycle` (80 runs): 5,763 REFUSED of 205,108, with ops `()` (the recycle alone). The new header is written into slot A, as a create writes, over the spare's only header. The rename has not landed. The spare's entry (old fid) then sees a header of another fid and refuses the open. The same happens with `order=rename_first` and with `dir_mode=prefix` (ext4).
Fix, either one: the new header goes into the slot not holding the spare's newest header (`rslot=other`: 0 of 192,715), or a mismatch on an entry reads as an empty file (`mismatch=empty`: 0). The spare's content is never read (db.rs:27854-27862; retired WALs become spares, db.rs:37895-37935).
Mutants:
- `same_fid`: 3,980 WRONG; the last life's 5 bytes appear as wal-2's content.
- `no_prev`: 65,070 REFUSED, because the header lag no longer reads as the empty log.
"Header lags the rename -> empty log" passes.

### F8. Table compaction: passes

`dirmodels.py compact` (160 runs: four prefixes x posts of 0-3 ops; temp, fsync, rename to a higher name_seq, sync_dir, unlink, appends after): 0 of 186,512 any and 1,048 proc. Mutants:
- no fsync of the copy: 3,268,580 LOSS;
- no sync_dir: 249,588 (subset) and 6,760 (prefix) LOSS, the second from acks into the copy before its name is durable;
- keep lower name_seq: 3,380;
- copy's name_seq lower: 3,380;
- unlink before rename: 249,748 and 86,736.

## The design's named mutants

| mutant | result |
|---|---|
| no in-flight pin | fails: 206,729 LOSS (inflight, R=8); 37,452 (R=6) |
| verify from synced_home without rounding | **does not fail** (base maxlen 5 at G=2 and G=4, resume maxlen 5). The model cannot see the rule. A sealed tag page verifies whole, and the page at synced_home lies in its own group, so any group closed since the sync is already checked. The original model's failure came from per-sector (unsealed) tag entries. design.md:276's "load-bearing" does not hold for the design's sealed tag pages. |
| no re-home of tag pages | fails: LATENT 176 (base maxlen 3); resume: 18,908 LATENT, plus UNREADABLE and proc LOSS |
| old-session frames accepted after a restart | the continue half fails: 287 WRONG (`1 1 P 1`: a crashed session's stale frame past the next session's first_seq is appended). The restart half fails only if a restart keeps the older entries in its table (`restart_keeps`: 2,744 WRONG). The design's restarts start a new table (create) or a new file_id (recycle), so that half cannot fire and "after the newest restart" is dead code. |
| ctr reused on retry | fails: 552 NONCE_REUSE, 900 NONCE_REPEAT |
| FINAL trusted without verifying | fails (above) |
| OVR written home before finish | fails (above) |

Own mutants that did not fail, so the model cannot see them:
- `owner_check=False` in the resume family: first_seq windows already reject the stale frames that would name stale pages;
- Replace `drop`, `idx=pos`, `own=rec`: masked as in F6.

## Combined rerun with every Log fix

`e4.py mix maxlen=5` with resume_sync, sh_rehome, R2 and K=3 (failed writes, X, P and in-flight syncs together), unsampled: 60,185 runs, 0 of 19,467,707 any and 760,337 proc (212 s). The same family on the design as written (cap 40): 19,307 violations (2,714 any LOSS, 40 any UNREADABLE, 16,553 proc LOSS).

## Not modelled / caveats

- The Once class from birth (seals) was not modelled. By analogy with F5(b), a FINAL that lands before its barrier with a torn page is trusted. That matters only where an unnamed file is read: `pending_whole` under `manifest_pending`, and recover_direct.
- Not modelled: the tag tier, the pager, names' AEAD, `.pfcopy` sweep, the ring punch after finish, preallocate, rename-over of logical names other than compaction, multiple files of one store besides the manifest pair, the WAL's chain byte as a TAIL at an older offset (patches inside the tail are modelled generically).
- Sampling: `cap` < the number of power-loss branches sampled them evenly. A sampled run passing proved nothing about F2 (mix cap 40 passed `rehome_invalid_only`, which fails unsampled). Every pass cited as a result above is unsampled, except `resume maxlen=6 cap=60` and the trunc runs.
- TRUNC's held-page and re-opened-group handling and LATENT counts are my inventions to make the spec runnable; its totals include LATENT flags of those inventions. Only the first counterexamples (a)-(c) are claimed.
- Scale is tiny (4-byte pages, 2-page groups, rings of 5-8). Counts are of model states, not of anything real.
- The engine-side Replace finding (F6) and F4's unreachability are by reading the cited code, not verified by a test.
