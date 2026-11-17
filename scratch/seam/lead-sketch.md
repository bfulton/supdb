# Lead's sketch: a paged sealed file under the door

Seam: a file layer under the `vault::` door. Above it every formatter (SegmentWriter, the direct run,
the ordered index, the form file builder) writes plaintext at plaintext offsets through a door file
handle, as a plain store does. Below it, one ciphertext file per plaintext file. The layer knows pages
and nothing of the forms. (The owner's "after compression, before disk", translated: this engine has no
single compressed block every byte passes through -- value blocks and form pages are compressed, the key
section, hash slots, inline records and ordered index are read in place -- so the universal unit is the
file's page.)

1. On disk. A header page: magic, version, a file id, the file's own key wrapped under the store key
   (AAD = file id), and two commit slots. Then data pages 1:1 with plaintext pages, 4096 to 4096, with a
   tag block interleaved every G pages (an entry is a 16-byte tag and a 4-byte version). Nonce = page
   index and version, under the per-file key. No length in any page's AAD.
2. Commit record. Two alternating slots in the header area, sealed, version-numbered: the durable
   plaintext length and the bytes of the partial tail page. home(k) is written exactly once, when page k
   is full; a partial tail lives only in the commit record. Invariant: a flush never changes a byte an
   earlier flush made durable, except an explicit in-place patch (page version + 1), which the engine
   makes only to a file nothing relies on yet (the segment finish's head). One fdatasync of one file per
   engine sync.
3. Write-through. Pages are sealed as the engine writes them, piece by piece, into the kernel's page
   cache, so what survives a process crash is what a plain store's does; sync = fdatasync of the
   ciphertext file; sync_file_range and fadvise are forwarded at translated offsets.
4. Consequences. The direct run is its own log again (no RUN_LOGGED / log_run double write); no envelope
   written whole at a close; a file being written need not stay in memory whole; the persist /
   persist_one / persist_form call sites become the plain store's own sync sites.
5. Names. Directory name = "x-" + base64url(nonce || AEAD_storekey(plaintext name || file id)). The
   vault's name table is rebuilt from readdir at open; the directory is the only truth, so rename,
   hard_link, remove and sync_dir are the kernel's operations with the plain store's crash semantics. The
   manifest carries no tokens or keys; Kind / companion / adopt_all / the sweep by table go; the engine's
   own sweeps work through names().
6. Read side. The existing explicit-ask pager (pager.rs) with chunk = one page, units any whole number of
   pages; a form page no longer lies across two chunks. Unsealed pages are pinned; sealed pages can be
   given back at once.
7. WAL, manifest slots, marker: as they are (a sealed record stream, a sealed slot, names that pass) --
   the layer's second primitive. An append-only log written through is naturally a record stream; putting
   it on paged files needs truncate / generation / recycle semantics for no measured gain.
8. A transparent fault-driven mapping (userfaultfd): measured here (SIGBUS mode 4.5 us a miss, a handler
   thread 31-43 us), tentatively rejected: the syscall is blocked by default container seccomp profiles
   (to be verified), a kernel-mode access to a missing page is EFAULT in the unprivileged mode (a lent
   value a caller passes to write()), and a process-wide signal handler would run the caller's cipher.

Open questions about this sketch: tag-block residency out of core (a cold read is a data page and its
tag block); the torn-write assumption (a sector is old or new); whether the segment writer's helper
fdatasyncs should write commit records; one file rolled back to an earlier commit record; the bound on a
sealed name's length; whether process-crash parity needs a commit record at every unsynced flush of a
run; whether plaintext is kept in the cache after a write (a copy against cold reads after a landing);
how a closed file's partial last page is served (from the commit record, for good?); how the in-core
read gets to the plain rate (339 instructions over 818 today).
