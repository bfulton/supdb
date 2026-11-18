# other-slot mutants

Runner: mut.py (each mutant applied to the kept fixed copy, `cargo test --profile checked --test durable
--no-fail-fast`, verdict written as it came, the fixed file put back and checked by sha256 after each).
Raw verdicts: mut-results.txt; per-mutant logs mut-M*.log. Run as root (the kpageflags tests ran).
Tree restored to keep/ sums (src_db.rs.fixed 3ec74464..., src_db_vault.rs.fixed 1913996d...).

| mutant | verdict | killed by |
|---|---|---|
| M1 the second sync deleted (the other slot's entry out of the loop) | killed | a_torn_publish_over_an_owed_slot_survives_a_power_cut_after_the_reopen (+cipher: the power-cut open refuses "names pcs-... which is missing"), an_open_makes_the_slot_it_takes_durable (+cipher, trace and page flags), the three pending-adoption tests (trace: both slots synced before the slot write) |
| M2 made after the sweep | killed | a_torn_publish... (+cipher): the open that fails the other slot's sync has removed the owed publish's segment, directory not as found |
| M3 made after open_refusals, before the sweep | SURVIVED (expected) | open_refusals removes and writes nothing, so a sync placed after it and before the sweep leaves every reachable state the same; no test can tell. The fix takes the stricter place (before open_refusals, beside the taken slot's sync). |
| M4 made only where the other slot read valid | killed | a_torn_publish... (+cipher): the torn other slot is left unsynced, the power cut hands the next open the owed record |
| M5 the taken slot synced twice (other index -> taken) | killed | a_torn_publish... (+cipher) (the seam's inode names the taken slot, and the power-cut open refuses), the kpageflags/trace tests, the pending tests |
| M6 its error swallowed (other slot's result dropped) | killed | a_torn_publish... (+cipher): the open answers Ok over the failed fdatasync |
| M7 skipped under a cipher | killed | a_torn_publish..._under_a_cipher, an_open_makes_the_slot_it_takes_durable_under_a_cipher |
| M8 the other slot opened with vault::open_slot | killed | an_open_makes_no_slot_file_it_does_not_find (+cipher): an open that refuses made manifest.alt; the pending tests (an open_slot record before the syncs) |
| M9 own: open_slot_found's NotFound arm deleted (the absent file an error) | killed | an_open_makes_no_slot_file_it_does_not_find (+cipher): a rename-shape store fails to open |
| M10 own: sync_slot's fdatasync deleted, the trace record kept | killed | an_open_makes_the_slot_it_takes_durable (+cipher), by the kernel's page flags only. Without root those tests read the trace alone, which this mutant keeps: under an unprivileged run it would survive. The trace is recorded by the call after its sync returns, so a lying call is what only the page flags catch. |
| M11 own: the seam's flag flipped (fails where unset) | killed | every test that opens a store (dozens) |
| M12 own: the order swapped (other slot synced first, taken second) | SURVIVED, equivalent | both syncs precede everything the open removes or writes; their order between themselves changes no state a crash can leave. |
| M13 own: `continue` on an absent slot file swapped to `break` | SURVIVED, unreachable | differs only where the slot the open just read is gone before it is opened for its sync (another process unlinking it); nothing in the engine reaches that, and no test can build it without a seam that deletes the file between the read and the open. The arm is the same `let ... else` that serves the other slot, which M9 holds. |

The report's and deferred.md's mutant lists: this defect (followup 48) is new since stage 0 and is in
neither; the brief's eight (M1-M8) are run above.

## Repair round 1: the reviewers' extra mutants

Repairs first: the seam moved into `vault::sync_slot` (`failed: Option<io::Error>` stands in for the
fdatasync's result inside the call that records the ones that return), and the torn test asserts
(a) the open whose other slot's sync the seam fails records the taken slot's `sync_slot` and not the
failed one's, (b) the first `open_slot` record after the reopen (the flush's publish), read by its real
path under a cipher too, follows both slots' `sync_slot`. (b) is asserted after the power-cut check, so
the unfixed engine still fails at the refusal (E7c).
Runner: mut2.py (applies each to keep/, full `durable` binary, puts the files back by sha256); raw
verdicts mut2-results.txt (the pass before the assertion reorder: mut2-results.first.txt), logs
mut2-E*.log.

| mutant | verdict | killed by / why |
|---|---|---|
| E1 = E6: `open_slot_found`'s `NotFound` arm widened to `Err(_) => Ok(None)` | SURVIVED, unreachable by an input; held by reading | `manifest_read_all` opens both slot files (`vault::open`) earlier in the same `Db::open` and returns every error but `NotFound` (src/db.rs, `Err(e) => return Err(e)` in its loop), so a slot that would fail `open_slot_found` with EACCES/EIO/ELOOP has already failed the open there. The arm differs only where the file changes between two opens of one call (another process) or the process runs out of descriptors between them. No test is added: a test as root cannot make EACCES, and an unprivileged chmod makes the earlier read fail first. The arm is kept: removing it would swallow the error the mutant swallows. |
| E2: `sync_slot`'s record made before the sync | KILLED | a_torn_publish_..._after_the_reopen (+cipher): the failed sync is recorded |
| E3: the seam answered after a sync that ran (`sync_slot(.., None)` then `return Err`) | KILLED | same tests, same assertion: a sync that ran is recorded, so the seam's failure is told from a sync that ran |
| E4: the `open_slot` record by its given path, not `real` | KILLED | a_torn_publish_..._under_a_cipher: the first slot write after the reopen is not found under the store's directory. (Before this round no cipher test read an `open_slot` record; the pending-adoption tests that do are plain only.) |
| E5: the other slot given a lying `sync_slot` record and no fdatasync | KILLED as root by an_open_makes_the_slot_it_takes_durable (+cipher, page flags); SURVIVED unprivileged (the durable binary run as uid 65534 via setpriv: 189 passed, page flags not read) | as the brief expected: a call that records a sync it did not make is caught only by the kernel's word, which needs root. |
| E7a (instrument): `b_synced` inverted, fixed engine | KILLED | a_torn_publish_... (+cipher): the device copy chosen from the record matters |
| E7b (instrument): the cut slot always torn, engine without the second sync | the torn tests pass the power-cut check (line 10394) and fail only at the trace assertion (10397, "the open fdatasynced manifest") | so the failing-first evidence of the power-cut check rests on the record choosing the owed whole image where no sync of that slot ran |
| E7c (control for E7b): test unchanged, engine without the second sync | KILLED at the power-cut check | "refused: the manifest slot manifest names pcs-...sup, which is missing from the directory" -- the before.log failure, still the first one |
| E8: the other slot synced only where the newest slot was skipped for pending segments | KILLED | a_torn_publish_... (+cipher) (skipped is false there), an_open_makes_the_slot_it_takes_durable (+cipher), the pending-adoption tests |

Survivors after this round: M3 (no test can tell; the stricter place taken), M12 (equivalent), M13
(unreachable), E1 (unreachable by input, held by reading), E5 unprivileged (root-only instrument).

## Repair round 2: the reviewers' extra mutants

Runner: mut3.py (applies each to keep/, the whole `durable` binary, puts the files back by sha256);
raw verdicts mut3-results.txt, logs mut3-R*.log. Before it, one test added (R1's reasoning below).

| mutant | verdict | killed by / why |
|---|---|---|
| R1: the other slot's sync skipped where the newest slot was `skipped` (passed over for pending segments not reading whole) | KILLED | a_torn_publish_over_an_owed_slot_survives_a_power_cut_after_the_reopen (its new plain-only section, `osl_skipped`): the power-cut open refuses "the manifest slot manifest.alt names pcs-...sup, which is missing from the directory". Not equivalent: the open writes the skipped slot (`write_shape`, the `had_pending` branch) only after its sweep (orphans removed, vault sweep), so a kill between the two leaves the device's B as it was -- an earlier owed publish whole, naming the segment the sweep removed. The test forges the cache's B as the owed publish's next sequence with its new segments flagged pending (`osl_pending_of`, checksum by `common::crc32c_from`), loses one of them, and stops the open between sweep and slot write with `Options::tear_open_slot_write`; premises on copies: with the segments whole the open takes the forged slot, with one lost it passes it over. Plain only: a sealed slot is not forged here; the cipher arm's sync is held by the cipher torn test, and the `skipped` branch is the same code in both modes. |
| R2 (instrument): the landing holds moved before the reopen through Options | NOT BUILDABLE as stated: no Option holds landings at open (Options has `zpages_hold_*` only). Premise checked instead: nothing publishes between `Db::open` and the holds (no seal without a commit at the threshold, no merge planned without a scan, `merge_plan_idle`), and the premise asserts (manifest_seq == a_seq after the commits, the torn slot unrewritten) held in 40 runs of both torn tests, four processes at a time beside the whole durable binary (r2-stress-load.log). |
| R3: the taken slot opened with `vault::open_slot` (create+write), the other with `open_slot_found` | KILLED | a_torn_publish... (+cipher): the reopen's `open_slot` record of the taken slot comes before both `sync_slot`s, so "fdatasynced before the first slot was written" fails |
| R4: the whole `if let Some(m) = &manifest` block (both syncs, `slot.landed`, `synced`) moved after open_refusals | SURVIVED, equivalent (as M3) | open_refusals removes and writes nothing; every name removal and slot write is after it, so no crash state differs. The fix keeps the stricter place. |
| R5: `sync_slot` with `sync_all` for `sync_data` | SURVIVED, invisible to the tests (cost mutant) | durability is the same (fsync is a superset); the difference is a metadata flush. Held by reading: `vault::sync_slot` calls `f.sync_data()`. |
| R6: the two seams' flags swapped in the loop's array | KILLED | a_torn_publish... (+cipher) (the refusal's words name "its manifest slot"), an_open_whose_slot_sync_fails_leaves_the_directory_as_it_found_it (+cipher) |
| R7: `1 - m.slot` written `m.slot ^ 0` (the taken slot synced twice) | KILLED | a_torn_publish... (+cipher) (the seam's inode and the power cut), an_open_makes_the_slot_it_takes_durable (+cipher), the three pending-adoption tests |

Survivors after round 2: M3/R4 (equivalent place), M12 (equivalent order), M13 (unreachable), E1 (held
by reading), E5 unprivileged (root-only instrument), R5 (cost mutant, held by reading).
