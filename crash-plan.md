# c4: crash injection for the next engine — registered before the code

The next engine's durable-load number (`EXT.22`, 0.694x of LMDB) is only a
number if an acknowledged commit is actually on the device when `commit`
returns. So far that rests on `tests/next.rs`, which emulates one crash at a
time by tearing a WAL by hand, and on the fact that `commit` calls
`fdatasync`. Nothing here has killed the process while a seal, a promotion
and a merge were in flight and asked what the directory then opens to. `c3`
does that for the original store; the new engine has three more moving
parts -- rotating WALs, a manifest, and background threads renaming files
-- and every one of them is a place a crash can land.

## The experiment

A child process commits batches of self-describing puts and deletes (some
through `Txn`) under seals a few hundred operations wide, so that seals,
piece promotions, merges and manifest swaps are all in flight during the
run, and aborts at a random operation. It prints one line per acknowledged
commit and, at the abort, the state it died in (seal or merge in flight,
segment counts) and how many bytes of the live WAL were behind a barrier.

The parent then does what a power loss would do to the one file a barrier
governs: it truncates the live WAL to a random length between the last
synced byte and the end. Segments and the manifest are fsynced before they
are renamed and the directory after, so they need no emulation; the WAL is
the only file whose tail is legitimately unsynced. A process kill alone
cannot test this -- the page cache survives the process, so everything the
child ever wrote would be there at reopen and `Sync::EveryN` would look
exactly like `Sync::Always`. That is why the plain kill of `c3` is not
enough here.

The parent regenerates the child's operation stream from its seed, so it
knows the exact state after every batch, and asks which prefix of the
commit order the reopened store equals. Two arms, interleaved by trial:
`Sync::Always` and `Sync::EveryN(8)`.

## Predictions

- **P4.1 -- the store opens after every crash**, including the ones that
  land with a seal or a merge in flight. `open` reads the manifest, sweeps
  what it does not name, and replays the WAL to its last intact commit
  frame; no window should reach a state it refuses.
- **P4.2 -- under `Always`, every acknowledged batch survives.** The
  recovered prefix is never shorter than the last acked batch, and at most
  one longer (a batch whose fsync completed before the ack was printed).
- **P4.3 -- what survives is an exact prefix of the commit order.** No
  batch is half applied, no delete is lost while its neighbours' puts
  survive, `count` agrees with `read_all` and `scan` agrees with both.
- **P4.4 -- nothing is invented.** Every value read back is byte-for-byte
  one the child wrote.
- **P4.5 -- under `EveryN(8)`, at most seven acknowledged batches are lost,
  and only from the tail.** Seven, not eight: the eighth is the one that
  forces the barrier.

## What would refute it

A refused open is the finding the manifest and the orphan sweep exist to
prevent, so one is enough. A recovered state that matches no prefix says
replay applied a batch it should not have or skipped one it should have.
Under `Always` a prefix shorter than the ack count is the durability claim
being false, and the number `EXT.22` compares against LMDB is then not a
durable load at all.

## Where the precondition can fail

The parent classifies each crash by the state the child died in. If a
profile produces no crash with a seal in flight or none with a merge in
flight, P4.1 is `not_exercised` at that profile rather than held: a store
that always opens when nothing was happening has not been tested.
