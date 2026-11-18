A store under the manifest's rename shape survives a power cut at every crash state built for the slots, and across a crossing to the slots and back

What a caller saw, under Options::manifest_slots off (the rename shape,
supdb-ingestmanifestrename), or crossing between it and the slots: a
seal whose publish failed after its rename was counted out with the
seal queue running, so the next landing's directory fsync named its
segments durable and retired the log over the entries the failed one
was for; an open that read a manifest a kill had left renamed and
unsynced swept what that manifest stopped naming, and a power cut could
keep the sweep and lose the rename; an open of a store that ran under
manifest_pending took its pending names, read from the page cache,
without fsyncing them; and a store moved slots, rename, slots, killed
between a rename and its directory fsync, found both slot files on the
way back, made nothing, fsynced no directory and swept. Each ended, at
the next open, in a refusal over acknowledged writes ("the manifest
slot manifest names par-..., which is missing from the directory").

What the fix does. Under the rename shape, a failed durable landing
stops the seal queue (land_front's error branch). An open that read a
manifest fsyncs any pending segment it takes (in the clear), then the
directory, before it removes anything; where it finds manifest.alt, the
slot file a life under the slots left, it writes the manifest read as
`manifest` where that file held it, then removes it and answers a
removal that fails. Under the slots, the open makes its slot files
before the cipher's sweep of envelopes, and where a slot file is
missing it fsyncs the directory before making it as well as after, so
the file that says the crossing is made appears only once the rename
life's entries are durable. The seams (fail_open_dir_sync,
fail_open_alt_removal, the publish's directory-fsync hook) ride the word
vault::sync_dir_as already loads.

When nothing fails: a store under the slots pays nothing on any success
path; the pre-make fsync runs only on an open that finds a slot file
missing, and Db::create is unchanged. Under the rename shape an open
makes one directory fsync before its removals, plus the pending fsyncs
where it takes pending names, plus on a crossing one unlink and, where
manifest.alt held the manifest, one manifest write. Publish adds
nothing. Not priced: the added work is on the open of a shape kept for
pricing.

tests/durable.rs gains a section that builds the crash states on disk,
plain and under a cipher: failed rename publishes before and after the
rename, a kill between a rename and its directory fsync with a power
cut after the reopen, pending stores reopened under the rename shape
with and without the option (the open's record of what it fsynced held
to the traced calls), crossings both ways, a crossing slots open whose
directory fsync fails (the trace holds one fsync before the make), and
the slots open after such an open, committed to and cut. Each new
reproduction failed before its fix. The tests were held to about
seventy mutants over four rounds; survivors are argued in the unit's
notes: the stop placed after the count-out, the had_manifest gate, the
alt removal before the open's fsync, the hook after the fsync, the
manifest written at alt's sequence, the open's directory and pending
fsyncs swapped (orders and values no state tells apart), and three
that name gaps below. CLAUDE.md's crash-discipline paragraph and its
entry "A path only one arm exercises" say what is true now.

Left open: no test fails a merge's or a promotion's rename publish (a
mutant that stops the queue on every rename publish error survives the
whole durable binary); no seam fails the slots open's directory fsync
before the make, so ignoring its error survives; a power cut during
the rename open itself is stood in for by the seam; and a direct run's
temp name removed by a rename open after a kill is not built.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GEwcALZsCzTm28PHd4bQza
