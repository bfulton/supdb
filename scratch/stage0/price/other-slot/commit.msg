An open fdatasyncs both manifest slots, the one it takes and the other where its file is there, before it removes or writes anything

A caller saw a store that would not open over writes it had
acknowledged. A drain writes its slot with the fdatasync owed; the next
publish into the same slot -- the other holding the newest manifest
known durable -- killed part way leaves the page cache holding that
slot torn, while the device may hold the owed publish in it whole, its
writeback having run before the torn write. The reopen passed over the
torn slot, took the other, synced only that one, swept the owed
publish's segment and acknowledged commits; a power cut before anything
wrote the torn image back handed the next open the owed record as the
newest slot, and every open after refused with "the manifest slot
manifest names pcs-...sup, which is missing from the directory". The
same holds under a cipher, and where the newest slot is passed over for
pending segments: the open writes over that slot itself, but only after
its sweep, so a kill between the two leaves the same device.

Db::open now opens each slot file it finds, read-only and never making
one (vault::open_slot_found), and fdatasyncs the slot it takes and the
other, whatever the other read as, through vault::sync_slot, which
records the sync only once it has returned. Both run before
open_refusals and before anything is removed or written; a failure of
either is the open's answer, with every file as found. A seam,
Options::fail_open_other_slot_sync, stands in for the other slot's
fdatasync result and names the inode it failed for. When nothing fails
the added work is the open's alone: one open(2), one fdatasync of a
slot with nothing dirty and one close(2) where the other slot file
exists, one failed open(2) where it does not (a store under the rename
shape); the taken slot is opened read-only where it was opened for
write and create, the same count of calls. Nothing is added to a
commit, sync, flush, drain, seal, rotation, publish or read. The open
was not timed.

tests/durable.rs builds the torn-over-owed state, plain and under a
cipher, and cuts the power after the reopen; a plain section forges a
newest slot passed over for pending segments and stops the open between
its sweep and its slot write (tear_open_slot_write). Two tests hold
that an open makes no slot file it does not find (they pass before the
fix and are not counted as evidence of the defect).
an_open_makes_the_slot_it_takes_durable and the three pending-adoption
tests are restated to both slots, by the sync_slot trace, which runs
without root, and by page flags where root has them. The tests were
held to twenty-five mutants; the survivors are a sync placed after
open_refusals and the two syncs' order (equivalent), a break for a
continue on a slot file another process unlinked and open_slot_found
swallowing errors other than NotFound (no input reaches either), a
sync_all for the sync_data (a cost no test can see), and a sync record
that lies, caught only by the page-flag tests as root. CLAUDE.md names
both slots in the crash-discipline paragraph and adds the corollary to
"A slot chosen by its sequence"; ROADMAP.md says both slots are synced.

Left open: an open that reads no manifest at all, both slots unreadable,
syncs neither; the skipped-slot section runs in the clear only, since a
sealed slot cannot be forged in the test (the branch is the same code
under a cipher); and the power cut is built by hand, as every slot
test's is.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GEwcALZsCzTm28PHd4bQza
