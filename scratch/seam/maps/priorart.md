**Basis:** WebSearch worked. The search results confirmed the RocksDB prefix and CTR stream, the TiKV key hierarchy, WiredTiger's sizing callback, the LMDB callbacks and their bugs, SQLCipher's reserve bytes and overhead, dm-integrity's journal cost, and the UMap result. Everything else below is from memory and is marked *(mem)*. I did not open source files to check those.

**RocksDB.** The seam is the `Env`/`FileSystem` file layer, below the SST block format, so compression happens above it. `EncryptedEnv` wraps every file (SST, WAL, MANIFEST). A prefix block, 4 KiB by default, holds the per-file counter/IV and is hidden from the file's reported size. The body is CTR via `BlockAccessCipherStream`, which allows random access at any offset ([env_encryption.h](https://git.nextgraph.org/NextGraph/rust-rocksdb/raw/commit/90a3378b2d429e8105bdf56c27281450a40ff36c/include/rocksdb/env_encryption.h), [env_encryption_ctr.h](https://sources.debian.org/src/rocksdb/9.11.2-1/env/env_encryption_ctr.h)). The format above it knows nothing about encryption.
- **Authentication:** none. Integrity rests on the block CRCs, which are computed over plaintext. CTR is malleable, so a flipped ciphertext bit flips the same plaintext bit.
- **mmap:** *(mem, fairly sure)* the encrypted `RandomAccessFile` refuses `use_mmap_reads`. Reads go through `pread` and are decrypted into a buffer, and the block cache holds plaintext.
- **TiKV:** built a key manager on top because RocksDB only takes a provider. A master key wraps data keys, data keys rotate weekly, and rotation never rewrites files: compaction migrates data to the new key. A per-file dictionary keyed by absolute path records each file's key and method ([PingCAP docs](https://docs.pingcap.com/tidb/dev/encryption-at-rest/)). The cost of that design is that renames and path changes become key-management events.

**WiredTiger.** The seam is per block, after compression and before the block manager writes. The `WT_ENCRYPTOR` interface has `encrypt`, `decrypt` and `sizing`. `sizing` returns a constant worst-case expansion, and the destination buffer must hold source length plus that ([docs](https://source.wiredtiger.com/11.3.1/struct_w_t___e_n_c_r_y_p_t_o_r.html)). The libsodium example spends its expansion on a nonce and a tag. *(mem)* The block header stays in the clear, and the checksum covers the on-disk ciphertext, so damage is caught before decrypt. The btree knows encryption only as "blocks may grow by N bytes". WiredTiger does not mmap encrypted files, since its cache holds plaintext pages.

**SQLite SEE / SQLCipher.** The seam is the pager codec, per page. Each page reserves bytes at its tail for a random IV and an HMAC over ciphertext, IV and page number: 16, 48 or 80 bytes by version ([SQLite3MC notes](https://utelle.github.io/SQLite3MultipleCiphers/docs/ciphers/cipher_sqlcipher), [design](https://www.zetetic.net/sqlcipher/design)).
- **Format impact:** the btree only sees a smaller usable page.
- **WAL:** frames are the same encrypted pages *(mem)*.
- **mmap:** off. *(mem)* SQLite disables mmap fetch when a codec is attached. Wrappers report "sqlite3_key() expects PRAGMA mmap_size=0" ([forum](https://synopse.info/forum/viewtopic.php?id=2659)).
- **Cost:** Zetetic reports 5–15% overhead, and turning HMAC off is the documented speed lever ([perf](https://www.zetetic.net/sqlcipher/performance)).

**LMDB 1.0.** The seam is page I/O. `mdb_env_set_encrypt(env, func, key, size)` takes a callback that maps a page-multiple input to an equal-size output, and the call forces `MDB_REMAP_CHUNKS` ([announcement](https://lists.openldap.org/hyperkitty/list/openldap-devel@openldap.org/thread/RBJ7IY7USM7524ZVDCMB4PIKITGX45FE)). Readers no longer borrow from the file map. They borrow from decrypted chunk copies cached in a second pool *(mem: the exact eviction scheme)*.
- **Known faults:** the first IV was fixed for the life of the environment, and Chu wanted one per write. A MAC size not subtracted from usable page space caused `MDB_PAGE_FULL` ([bug](https://lists.openldap.org/hyperkitty/list/openldap-bugs@openldap.org/message/Z4HX6QTP2FUKY7SZI57PWAKL35BYPK5A/)). Encryption also interfered with checksums.
- **Numbers:** none published that I found.
- **What it gave up:** zero-copy from the page cache.

**InnoDB / MariaDB / Postgres TDE** *(mem)*.
- **MySQL InnoDB:** a tablespace key is wrapped by a master key and stored in page 0, and pages are AES-encrypted on write.
- **MariaDB:** AES-CTR, with the IV built from space id, page number and LSN. The key version sits in the page header, which stays in the clear, and the checksum covers ciphertext.
- **Postgres TDE proposals:** CTR or XTS keyed on page LSN plus page number. They hit pages rewritten without a new LSN (hint bits), which reuse the nonce, so `wal_log_hints` became mandatory.
- **Authentication:** none of the three has page-level authentication.

**fscrypt / dm-crypt / dm-integrity.** XTS is length-preserving, so a sector has no room for an IV or tag, and the result is confidentiality only (NIST SP 800-38E). dm-integrity stores tags in interleaved metadata sectors. Writing data and tag separately is not atomic, so it journals both and copies them home. That costs extra reads and read-modify-write, up to 50% of write throughput, and 50–80% CPU against 30% for XTS alone ([Hazel](https://arxiv.org/pdf/2510.18756), [kernel doc](https://docs.kernel.org/6.14/admin-guide/device-mapper/dm-integrity.html)). Broz on dm-devel: "do not expect super-fast performance" ([dm-devel](https://listman.redhat.com/archives/dm-devel/2018-November/036666.html)). Without the journal, a crash can leave data and tag mismatched.

**userfaultfd pagers.** Production users include QEMU post-copy, CRIU lazy restore and LLNL UMap; UMap reports 1.25–2.5x over its system-service baseline ([OSTI](https://www.osti.gov/servlets/purl/1574628)). One CRaC post measured `UFFDIO_COPY` at about 5 µs per 4 KiB page, units inferred ([crac-dev](https://mail.openjdk.org/pipermail/crac-dev/2023-May/000867.html)). Each fault also pays a wake/handoff round trip on top. Pitfalls, all *(mem)*:
- **Privilege:** `vm.unprivileged_userfaultfd=0` by default since 5.11, which leaves the `UFFD_USER_MODE_ONLY` flag or `/dev/userfaultfd` (6.1+).
- **Kernel-mode faults:** with user-mode-only, a syscall touching the region (`write(fd, mapped_ptr)`) gets `EFAULT` instead of blocking.
- **Fork:** needs `EVENT_FORK`, and it must be handled.
- **THP:** registration complicates it.
- **Handler thread:** it must never fault on the region itself.
- **Debugging:** gdb reads of the region hang.

I found no database that uses userfaultfd as a decrypting pager.

**Nonces** *(mem)*. Reusing a GCM nonce under one key leaks the XOR of the two plaintexts and the GHASH key, which enables forgery. With random 96-bit nonces, NIST SP 800-38D limits a key to 2^32 encryptions. AES-GCM-SIV (RFC 8452) degrades to leaking only equality when a nonce repeats. Storage practice is either a fresh key per immutable file with the counter as nonce, or a per-page write counter or LSN in the nonce.

**Lessons for an mmap engine**
1. Every system that encrypts in user space gave up mmap of the file (SQLite, RocksDB, WiredTiger, LMDB). Zero-copy survives only by lending from a plaintext cache, which is LMDB's path.
2. Put the seam per file or per page, below the format. The only thing the layers above should know is a constant expansion and a usable size (WiredTiger's `sizing`, SQLCipher's reserve).
3. Immutable files make nonces trivial: one key per file and the chunk index as nonce. Rewritable pages need a counter that is persisted.
4. Keep a key hierarchy (master key wraps file keys) so rotation is a rewrap and compaction migrates the data, as TiKV does.
5. Keep the tag with the chunk, in the same write as the data, so there is no data/tag atomicity problem to journal.

**Traps**
1. CTR or XTS with plaintext CRCs is not authentication: CTR is bit-malleable, and a CRC an attacker can recompute catches nothing.
2. A MAC or IV not subtracted from page capacity breaks the layer above (LMDB's `MDB_PAGE_FULL`).
3. Rewriting a page without bumping its nonce reuses the nonce (Postgres hint bits, LMDB's fixed IV).