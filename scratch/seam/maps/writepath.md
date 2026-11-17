## 1. Files the engine writes

**Sealed segment (SegmentWriter / AlignedWriter).** `SegmentWriter::create` opens with `vault::create_rw` (db.rs:6633) and writes `format::SUPER` zero bytes first (db.rs:6646). The body then goes through `AlignedWriter`: it buffers to the next `write_piece` boundary (default `1 << 20`, db.rs:7990), and two pieces go out as one `writev` (db.rs:6046). After each piece it calls `start_writeback`, which is `sync_file_range(SYNC_FILE_RANGE_WRITE)` (db.rs:6072-6085). A helper thread (`WriterHelper`, db.rs:6091-6130) runs `fdatasync` on a dup every `ahead` bytes and reads the key section back with `pread` to hash it (db.rs:6198, 6218). The finish takes the file back (db.rs:7725) and **patches the head in place**: the section header at `key_off` (db.rs:7728), the reserve, with the checksum row, fence and directory read back by `read_exact_at` and copied there (db.rs:7746-7771), and last the superblock page at offset 0, two slots plus the extension (db.rs:7818-7824). So this file is not append-only: page 0 is written as zeros and later rewritten. It is written to a temp name and then renamed (db.rs:12567, 29511). It is read back through `vault::map` → `MmapBytes::open` with mmap (db.rs:8308, bytes.rs:344).

**Direct run (ordered ingest).** It uses the same writer, records-first. At each commit it writes a 24-byte marker (`DIRECT_MARK_LEN`, db.rs:4076; `marker_bytes`, db.rs:6768) and calls `sync`, which runs `stream_flush(true)` to write a **partial piece** and then `sync_data` (db.rs:6917-6936). The tail is therefore synced and later extended. The run is **mapped while written** (`RunMap::open` via `vault::open_mapped`, db.rs:11461-11470), and the mapping is grown by remapping (db.rs:11447). The close hard-links the file under its piece name and leaves the temp name in place (db.rs:28989). Recovery uses `vault::read` and parses the whole file (db.rs:6948-6949). Under a cipher the log also holds the run (`Direct::logged`, db.rs:4746).

**WAL.** Created with a header (db.rs:4180-4183). Each commit is one `write_all` of a whole number of batches, sealed as one record under a cipher (db.rs:4436-4455), then `sync_data` (db.rs:4459). Every write appends and syncs a tail of arbitrary length. Other patterns:
- `prefill` writes zeros in 4 KiB pieces, then seeks back and writes over them (db.rs:4252-4262).
- `recycle` renames a spare into place and writes a new header over the old life's bytes (db.rs:4280-4319).
- `open_append` uses `O_APPEND` (vault.rs:1914-1920; db.rs:4325).
- The header kind byte is rewritten in place to mark it chained (db.rs:4493-4502).
- Replay truncates with `set_len` and syncs (db.rs:27932-27934).
- `drop_synced` calls `POSIX_FADV_DONTNEED` on synced pages (db.rs:4469-4477).

Rotation and retirement go through `rotate_wal` (db.rs:27015).

**Manifest slot.** `write_all_at(&out, 0)` rewrites the slot in place, never truncated, followed by `sync_data` (db.rs:11695-11720). The older shape writes a temp file, fsyncs it, optionally hard-links a spare, renames over the manifest and syncs the directory (db.rs:11765-11797). Read with `vault::open` (db.rs:11895).

**Ordered index.** Written once to `{name}.tmp` with `write_all`, synced only if `durable`, then renamed (db.rs:99-131). Rebuilt at open if missing (db.rs:27752-27764).

**Form file (zfile).** `FileSink::put` appends the piece with `write_all`, calls `start_writeback`, and optionally drops the previous range (zfile.rs:3852-3862). The file is written to `.tmp` and renamed (zfile.rs:4253, 4360). It is fdatasynced after rename (zfile.rs:4379-4380), or under a cipher `persist_form` gives it its envelope (zfile.rs:4373). Read through `vault::map`, `MADV_RANDOM` (zfile.rs:2007).

**Marker.** Not verified (vault.rs:417 `rewrap` rewrites it, "a hundred bytes" per CLAUDE.md).

## 2. The two interfaces

**Compression is not one seam.** There are two places:
- Value blocks are compressed inside `SegmentWriter`'s block emit (`block::compress`, db.rs:7378, or `write_chunked_sz`, db.rs:7371). The unit is one block, which then becomes bytes in the segment stream.
- The compact form's groups are LZ4'd into 4 KiB pages (zpages.rs:2, 99; compression at zpages.rs:1225). These pages leave through the `PieceSink` trait (zfile.rs:3851).

The key section, hash slots, inline records, WAL, manifest and ordered index are not compressed at all. So "after compression" would in practice mean "after the formatter".

**Opens go through one choke point, the `vault::` door** (vault.rs:1860-2229), and `passes()` routes the log, spare and manifest to the real directory (vault.rs:562). **The bytes themselves have no choke point.** The door hands back a raw `std::fs::File`, and everything after that goes straight to the kernel:
- `write_all`/`writev` in AlignedWriter (db.rs:6046)
- `write_all_at` in the finish and the manifest (db.rs:7728-7824, 11719)
- `read_exact_at` in the finish and helper (db.rs:5857, 6198, 7753)
- `sync_data`, `sync_file_range` and `fadvise` on the handle (db.rs:6072, 14911; vault.rs:2162)
- `RunMap`'s own mmap (db.rs:11469)

The only read abstraction is `Bytes` / `MmapBytes` (bytes.rs:57-79, 460-492).

## 3. What is hard for a page-granular encrypting layer

- **In-place page rewrites reuse a nonce unless each page write takes a fresh nonce or version.** Affected: the superblock page (db.rs:7824), the reserve and header patches (db.rs:7728-7771), manifest slots (db.rs:11719), the WAL header byte (db.rs:4502), WAL prefill then overwrite (db.rs:4252), and recycle over the old life (db.rs:4280). A per-page nonce therefore needs a per-page counter that is stored and authenticated somewhere, plus a way to stop rollback.
- **Partial tail pages are synced, then extended.** This happens on every WAL commit (db.rs:4436-4459) and every direct-run commit (db.rs:6932-6934). A layer must re-encrypt the tail page on each sync (a nonce per version), and a torn write of that page loses earlier committed bytes in it unless it uses a two-copy or journaled tail.
- **`O_APPEND`** (vault.rs:1920): the layer must own the file offset itself.
- **Files mmapped while still being written** (db.rs:11469) and segments mapped as plain pages (db.rs:8308): a kernel mmap of ciphertext is useless. Every mapping must become a userspace decrypted view, which is today's pager and `ASK_PIECES` machinery (bytes.rs:105, 492).
- **The finish reading its own unsynced bytes back** (db.rs:7753, 6198) requires read-after-write through the layer.
- **`rename` / `hard_link`** (db.rs:12567, 28989; zfile.rs:4360): fine if the page key is per inode or per file id and does not depend on the path. It breaks if the AAD binds the name.
- **`sync_file_range` / `fadvise` by plaintext offset** (db.rs:6072; zfile.rs:3932-3940): these must be translated to ciphertext offsets. With 4080-byte chunks per 4096-byte page (crypt.rs; CLAUDE.md), plaintext and storage pages are misaligned, so one plaintext page spans two chunks.
- **Exact lengths:** the WAL, the direct run's recovery walk (db.rs:6954) and `vault::len` all read the logical length, so the layer must store it. `set_len` truncation (db.rs:27933) must cut mid-page and re-seal that page.

## 4. What a plain store gets from the page cache

- **Zero-copy borrows from mappings.** `slice_at` / `lend_at` return pointers into the mapping (bytes.rs:468-490), and `Blob::zero_copy()` depends on this.
- **Residency checks with `mincore`**: `resident_share` (db.rs:14934-14950) and `paged_resident` (bytes.rs:415, 434, 577).
- **Prefetch with `MADV_WILLNEED`** (bytes.rs:139, 332; zfile.rs:4042).
- **Dropping pages with `MADV_DONTNEED` / `POSIX_FADV_DONTNEED`** (bytes.rs:557; zfile.rs:3940, 4013; db.rs:14911).
- **Pre-touching buffer pages with `MADV_POPULATE_WRITE`** (db.rs:9076-9079).
- **Folio sizing by write size**, which is why pieces are 1 MiB and the WAL is prefilled in 4 KiB pieces (db.rs:5464-5470, 4243-4247).
- **Writeback without a device flush via `sync_file_range`** (db.rs:6066-6085).
- **Kernel reclaim of clean pages.**

A layer below the formats must either reproduce all of these in userspace (a pager) or give up the out-of-core advice and residency accounting.

| file kind | write pattern | sync points | read-back | difficulty |
|---|---|---|---|---|
| sealed segment | write-once body in 1 MiB pieces, head and superblock patched in place at finish, tmp→rename | helper fdatasync every `ahead` bytes, final fsync | mmap; finish and helper pread | medium: page 0 and the reserve are rewritten once |
| direct run | append records plus a marker per commit, mapped while written, hard-linked | fdatasync per commit, partial tail | mmap while written; `vault::read` at recovery | hard: synced partial tail plus live mmap |
| WAL | append, `O_APPEND`, prefill then overwrite, recycle, header byte rewritten, `set_len` | fdatasync per commit or policy | `read` at replay | hard: tail re-seal, overwrites, truncation |
| manifest slot | whole-slot overwrite at offset 0, never truncated | fdatasync per publish | `open` + read | hard: nonce per overwrite, rollback risk |
| ordered index | write-once to tmp, rename | optional fsync | mmap (not verified) | easy |
| form file | append 4 KiB-page pieces, tmp→rename | sync_file_range per piece, fdatasync after rename | mmap, `MADV_RANDOM` | easy: already page-granular |
| marker | small rewrite at rewrap | not verified | not verified | medium: in-place rewrite |