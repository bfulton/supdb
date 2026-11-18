A segment writer's helper syncs through its own file description, so a commit beside its failed sync answers the failure

What a caller saw: the helper thread behind a segment's writer synced
through a duplicate of the writer's descriptor, one open file
description between them. Linux reports a writeback error once per
description, so a helper fdatasync that met the error took it for both;
a run commit whose own fdatasync returned in the instant before the
helper stored its error read the helper's word as clean, and the commit
was acknowledged over the hole.

The fix: where the helper is spawned the writer opens the file again
through /proc/self/fd/<n> (O_RDONLY|O_CLOEXEC, the path built on the
stack), and the helper syncs and reads through that description, so the
kernel reports an error to the writer's description as well, whichever
sync meets it first. Where that open fails the writer runs with no
helper and syncs and hashes alone, keeping its own checksum row. Db's
run open now ORs its fault byte into the writer's where it assigned it.
When nothing fails it adds one openat and one close per segment writer
that spawns a helper (a seal's output, a merge's piece, a run past
sync_ahead), in place of the dup it replaces; nothing a commit, a sync,
a flush or a read. A test seam (SegmentOptions::helper_fault,
HelperFault) fails the helper's sync by description, holds the helper
between its syscall and its store, refuses the reopen and can start the
helper only after the stop; it rides the fault byte the writer already
asks, and is free when unset.

Pricing, against the shape before kept behind a measuring field in the
measuring tree only (arms supdb-helperraceold, supdb-ingesthelperraceold):
  sh bench/scripts/pair.sh supdb supdb-helperraceold 10000 --passes load,lag
    nothing marked in either sitting; load 0.99 / 0.97, shuffled
    1.02 / 0.92. At 10k the file stays under sync_ahead and no helper
    exists.
  sh bench/scripts/pair.sh supdb-ingest supdb-ingesthelperraceold 10000
    nothing marked; load 1.04 / 0.99, shuffled 0.98 / 1.03.
  sh bench/scripts/pair.sh supdb supdb-helperraceold 300000 (run twice)
    first run: upkeep_ms new worse marked in one sitting, an update rate
    new faster marked in the other; second run nothing marked, upkeep_ms
    +3% unmarked in both: unresolved.
  sh bench/scripts/pair.sh supdb-ingest supdb-ingesthelperraceold 300000
    one cell marked, the shuffled load new faster 1.03 in one sitting;
    load 1.00 / 1.03.
  A second round of both pairs at both sizes after the spawn was
  restructured: no cell marked in both sittings.
  SUPDB_LOAD_PHASES=1 SUPDB_DRAIN_STEPS=1 sh bench/scripts/pair.sh <same
  pairs> 300000 --passes load: the closing sync, the drain's answer and
  the fsync after the readable landing are split between sittings in
  every phase (about 29-31 ms and 4.2-4.6 ms, either arm longer in turn).
  strace -f -c over bench/examples/load_only.rs <arm> 300000 1: the
  helper's F_DUPFD_CLOEXEC is replaced by one openat; the helper's
  fdatasyncs and the writer's hints are unchanged in count.
  Page-cache folio composition of every store file is the same.
Verdict: no cost resolved.

The tests (tests/durable.rs, section helper-race; two in src/db.rs)
hold the trace in which the helper's failed sync is held before its
store while a run commit syncs, plain and under a cipher, and at the
writer; the fallback with no /proc, marked and unmarked, at two strides;
the helper's descriptor at fd widths of two, three and four digits, its
own description, read-only, close-on-exec and closed at the finish; a
sync taken at the stop; a seal's output and a merge's piece whose helper
sync failed, named by no manifest and the reopen reading the model; and
a failed helper sync not retried. The run-commit tests and the writer
test fail on the shape before. Thirty-five mutants over four rounds:
all killed but two argued equivalent, and one check nothing could reach
was taken out.

Left open: the two-description report was read on ext4 over a loop
device on kernel 6.18 by a probe, not by the store's tests over a device
that fails writeback (the ROADMAP item for a seal's failed sync on such
a device); off Linux the helper still syncs through a duplicate, untested.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GEwcALZsCzTm28PHd4bQza
