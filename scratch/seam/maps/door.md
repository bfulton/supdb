## 1. Door functions (src/db/vault.rs)

Every free function checks `locate`/`under` first: `None` means a plain store and the std call. A name that `passes()` (vault.rs:562) goes to the std call on `real.join(name)`. Any other name is a **kept** file, handled by the `Vault` method listed.

- `create`/`create_rw` (1863/1874): kept → `make` (889), which creates a new memfd entry.
- `open` (1892): kept → `reopen_written` (1113). Only works while the memfd exists; a file that already has its envelope gets `Unsupported`.
- `open_write`/`open_append`/`open_slot` (1903/1914/1935): kept → `rewrite` (1090), refused once the file has a disk entry or a page. Append/slot fall back to `make` when the file is missing.
- `rename`/`hard_link` (1955/1990): kept → only the in-memory table is renamed or linked (1013/1022). Crossing between a store and somewhere else is `Unsupported`.
- `open_mapped` (1977): kept → `map_written` (1039), which returns a `Mapped` hold (627). That stops the pager giving back memfd pages under a writer's own mapping.
- `remove_file` (2009): kept → `remove` (981). The envelope is unlinked when the last name goes, and for `Kind::Seg` the `.z` beside it too (993-1007).
- `exists`/`is_file`/`len` (2019/2029/2039): kept → table lookup. `len` reads the plaintext length from the envelope `Header` when nothing has read the file yet (974-978).
- `names` (2050): the kept names plus the passing directory entries (1131).
- `read` (2065): kept → `page_of` plus `MmapBytes::paged` and a `read_at` under a hold, then `pager.check`.
- `sync_dir`/`open_dir` (2083/2105): act on the real directory, not the decoy.
- `real` (2096): the real path for passing names only.
- `persist`/`persist_one` (2118/2129): plain → no-op. Cipher → `Vault::persist_with` (1299), which writes and syncs the envelopes of segments plus their `.o`. `persist_one` derives the `.o` name with `Db::ord_name_for` (2132).
- `persist_form` (2146): plain → no-op. Cipher → `Vault::persist_form` (1425).
- `companion` (2188): plain → no-op. Cipher → registers an existing `x-<token>.z` (1489).
- `kept` (2197): whether a path is a kept file.
- `map` (2205): kept → `MmapBytes::paged(page_of)`.
- `check` (2217): `pager.check()`.
- `hold` (2227): `pager.hold()`.
- On the `Vault` itself: `seal_manifest`/`open_manifest` (1633/1657), `adopt_all` (1575), `sweep` (1593), `log_begin`/`log_resume` (1686/1694).

## 2. Form knowledge in the vault and the engine's use of it

- **`Kind::{Seg,Ord,Form}`** (468-484, suffixes "", `.o`, `.z`). The `.o` and `.z` files share their segment's token and key (1369-1371, 1450-1453). No manifest names the two companions, so they have to be found from the segment's token. Why: a plain store names its segment by fences (key bytes), and those names cannot go to the directory.
- **`passes()`** (562): the names `wal-`, `spare-` and `manifest` are written through to the directory. Why: the engine seals those bytes itself (each log record, each manifest slot), and they are written in place, appended or recycled, which a write-once envelope cannot be.
- **Manifest with tokens and keys**: `seal_manifest` (1633) produces `SUPDBMAE\x01|seq|len|nonce|ct|tag`, the plaintext being the manifest plus `(token|wrapped)*`. `open_manifest` (1657). `adopt_all` (1575) refuses a count mismatch and pairs each slot entry with the segment the manifest names. The engine calls these at db.rs:11669-11673 (`manifest_image`, which first does `v.persist(..., true)`) and db.rs:11680-11685 (`manifest_open`). Why: the manifest is the only place a token can be mapped to a fenced name. It also keeps the crash order intact, since "a manifest names only what is durable" (db.rs:11654-11660).
- **The log sealed by record**: `LogCipher` (1773-1858). The AAD is `id|at|len` (1791). A record is `len|nonce|ct|tag`. The engine side is `Wal::begin`/`resume` (db.rs:4203, 4220), the `SUPDBWE` header (4134) with the wrapped key behind it, `write` (4447-4450), `replay` (4557-4580, with the `recycles` exception), `recycle` (4281) and `open_append` (4336). Why: a plain log is appended and fdatasynced per commit, so it cannot be a write-once envelope.
- **`persist_form` and `companion`** (1425, 1489): the `.z` file is written under a temp name and renamed into place, because it may be rebuilt while a reader still holds the old view (1406-1411). Its `cold` flag drops the cached ciphertext (1458-1461), mirroring zfile's `drop_output`. Why: the compact form is rebuildable, appears after its segment, and nothing names it.
- **`MappedFile`/`Mapped`** (609-635): the ordered run's memfd, mapped by `RunMap::open` (db.rs:11462). Why: a run's file is read while it is still being written.
- **`sweep`** (1593): removes `x-*.tmp` files and any `x-` envelope no entry holds. A `.z` file counts as held by its segment (1612-1617). Why: an envelope written before a crash, with no manifest naming it.
- **`remove`** cascades a segment's removal to its `.z` (993).

## 3. Envelope and security design

- **Envelope** (crypt.rs:22-40, `Header` at 164-346): a 48-byte header (magic `supdbAE\0`, version, chunk shift, plaintext length, a 16-byte id, key_id), then zeroes up to `DATA_START=4096`. Each chunk is stored as `1<<shift` bytes (4096 by default, crypt.rs:120) holding `stored-16` bytes of plaintext followed by the tag (265-273). Nonce = `id[..8] || u32 chunk index` (340). The AAD is the 48-byte header (crypt.rs:44). An empty object still has one chunk, so the header always sits under a tag. The id is drawn at random per envelope, and the vault uses `EncryptOptions::default` (vault.rs:1166).
- **Key hierarchy**:
  - caller `master` → store key, wrapped in the marker `supdb.cipher` (vault.rs:362-449). The marker's 40-byte head is the AAD.
  - store key → one key per segment, wrapped with AAD `"file"|token` (880). The wrapped key lives in the manifest slot (169).
  - store key → one key per log file, wrapped with AAD `"log\0"|id` (1760). The wrapped key lives in the log header.
  - The `.o`/`.z` files use their segment's key (1366, 1447).
  - The store key's nonces are an 8-byte random prefix plus a counter, redrawn at each open (306-331).
  - `rewrap` (417) rotates the caller's key and nothing else.
- **Authenticated**: every byte of segments, indexes, form files, manifest slots and log records (vault.rs:92-107). A failed tag latches `Pager::check`.
- **Not authenticated**: rollback of the whole directory to an earlier state, or the log cut at a record boundary (109-111).
- **Not hidden**: the number of files and their sizes, the size of each commit, and the key_id (112-114). Plaintext in the pager's memory can reach swap or a core file (116-118).
- **Names**: segments become `x-<32 hex>`, with `.o`/`.z` beside them (505-513, 62-68). The fenced names exist only inside the sealed manifest.

## 4. Where the engine itself special-cases a cipher on write paths

- db.rs:11529 `cipher_controller`: under a cipher, `ZPages::Auto` becomes `Off` (called at 27305, 27604).
- db.rs:28219-28223: a direct run's table is made `.logged()` when `self.vault.is_some()`.
- db.rs:28834-28836 / 28862 `log_run`: a run's commits under a cipher are also written as log frames (`RUN_LOGGED`, 11520).
- db.rs:28999 (direct-run close) and 29525 (seal): `vault::persist` is called where a plain store fsyncs.
- db.rs:12574 `vault::persist_one` on each merge piece. db.rs:12865-12866 and 13033-13034: `vault::check` then `vault::persist` before a merge's outputs are named.
- db.rs:11669-11673: `manifest_image` writes the late envelopes and seals the slot.
- db.rs:27632-27636: `sweep` at open.
- db.rs:4203-4580: the WAL's cipher branches (begin, write, replay, recycle, open_append).
- db.rs:27058 `sync_wal` and db.rs:33930 run under `unlend`/`unlent`, which is pager-pin discipline.
- db.rs:31411 and 31455: `vault::companion` at form attach.
- zfile.rs:4204 and 4302-4310: `SegSrc::Kept` takes a separate build path that reads by the piece under a hold.
- zfile.rs:4373: `persist_form`.
- db.rs:40382: drop/willneed advice issued on envelope ranges.
- Not verified: other `under a cipher` sites (18015, 20640, 20771, 21701, 30808, 32436). From their comments these are read and rest paths, not write paths.

## Form knowledge a lower seam would delete

- `Kind::{Seg,Ord,Form}` and the token-sharing between a segment and its companions. `companion`, the `.z` cascade in `remove`, `sweep`'s rule that a segment holds its `.z`, and the temp-name-and-rename in `persist_form`.
- `with_ords`/`ord_name_for` threaded through `persist` (db.rs:27256-27267, vault.rs:2132).
- The per-site `persist`/`persist_one`/`persist_form` calls on the seal, run, merge and form paths, which exist only because a memfd's fsync does nothing.
- `RUN_LOGGED`/`log_run`, plus `Mapped`/`MappedFile` for the ordered run.
- `cipher_controller` and zfile's `SegSrc::Kept` build path.
- The "write once, rewrite refused" rule (1090-1102).
- The tokens and wrapped keys inside the manifest slot, if file keys and names moved below the format.

## Knowledge any seam would still need

- The marker and its key hierarchy (master → store key → per-file keys), nonce discipline, and rekeying.
- Hiding file names, because fence names leak key bytes. The fence-to-token map has to be authenticated somewhere.
- Some form of in-place or append writes for the log and the manifest slots: per-record sealing with offset in the AAD, a torn tail that reads as the log's end, and the recycled-spare header case.
- Durability ordering, so that nothing named is undurable, and a seam whose fsync actually makes the ciphertext durable.
- Tag-failure latching, since a reader's `Option` answers make a failed read look like an absent key (`Pager::check`). Also the plaintext cache with its pin/rest/evict discipline, if reads keep lending borrows.
- Rollback is not protected, and the size and count leaks remain unless they are padded.