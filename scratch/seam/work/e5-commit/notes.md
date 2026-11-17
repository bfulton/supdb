# E5: the device cost of a Log commit (design section 9), as a C probe

Tree 3df85d22. Kernel 6.18.44 Firecracker guest, 4 vCPUs (aes, vaes, vpclmulqdq), OpenSSL 3.0.13.
Store device /dev/vda: virtio, write-back cache, ext4 mounted `discard,delalloc,barrier` with
**no journal** (`/sys/fs/ext4/vda/journal_task` is `<none>`, no `/proc/fs/jbd2/vda-*`).
Idle background writes on vda: 16-328 sectors per 10 s (raw/idle.txt), under 1% of any run.

## Files
- `e5.c` / `e5`: the probe. `e5 <A|Aw|A0|Ap|B|Cf|Cp|D> <commit bytes> <commits> <dir> [label]`
  - A: the design's Log commit. Plaintext offsets map to stored offsets as in section 3 (data page p at
    1 MiB + 2 MiB + p*4096 in extent 0, tag page of group g at 1 MiB + g*4096, ring 8 KiB..1 MiB).
    Per door write call: full pages sealed (AES-256-GCM, nonce 0x10|p|ctr, AAD file_id|p|ctr|len) into a
    reused 2 MiB stage and sent home in one pwritev; a completed group's tag page (128 x 32 B entries)
    sealed and pwritten; one frame (16 B clear header, sealed body, 16 B tag) with TAIL (bytes since the
    previous frame that are not home), HOME (version+tag per page sent home), GROUP (g, version, MAC),
    and a CKPT (len, synced_home, home, tail image, open-group tags, group MACs) behind it once 256 KiB of
    frames have passed, pwritten at the ring head (skip-allocated, pinned span checked). Then fdatasync.
    Header A (sealed under K_f derived from K_s) written once at create, unsynced. Sealed spelling of 253
    characters. Write-through: memcpy of every appended byte into a MAP_SHARED memfd mapping.
  - Aw: A with the write-through as pwrite into the memfd's descriptor.  A0: A without write-through.
  - Ap: A over a file prefilled with zeros at every stored offset it will write (ring, tags, data), in
    4 KiB writes + fsync, before the window (the design's `vault::preallocate`, section 4).
  - B: the plain run's commit (db.rs:6870-6893 mark_stream -> stream_flush(false) at db.rs:5707; sync at
    db.rs:6917-6936): records + 24-byte marker (db.rs:4076); the whole 1 MiB pieces completed written
    with a writeback hint (sync_file_range) each, the rest in one writev, then fdatasync. The first
    commit carries the 4288-byte prefix (format::SUPER + flatindex::HEADER).
  - Cf: plain WAL commit (Wal::write db.rs:4436, sync, drop_synced db.rs:4469, sync_wal db.rs:27055):
    one write of the batch + 21-byte commit frame, fdatasync, POSIX_FADV_DONTNEED per MiB synced
    (wal_drop_bytes, db.rs:4051), fresh file with its 8-byte header.  Cp: Cf over a prefilled file
    (prefill db.rs:4252: 4 KiB zero writes + fsync, outside the window).
  - D (extra): today's cipher run commit: B's writes into a memfd + its fdatasync, plus the batch sealed
    as one log record (vault.rs:1801 seal_into: len4|nonce12|ct|tag16) written + fdatasynced (log_run,
    db.rs:28862). The envelope at the run's close is not in the window.
  - Window: create + headers + every commit, bracketed by /sys/block/vda/stat (fields 5,7,14,16) and
    /proc/self/io write_bytes; sync() + 300 ms before; a sync() after gives the late metadata.
- `run.sh`: one process per run, fresh files, shapes rotated each rep. `summarize.py`, `paired.py`
  (per-rep ratios), `pooled.py` (all sittings pooled).
- `iocost.c`: the device's price by parts (k separate page writes / k contiguous / k appended + fdatasync).
- `trace.sh`, `tracecount.py`: block_rq_issue/complete via tracefs over a short run; IOs per commit by kind.

## Raw outputs (raw/)
- main.txt (sitting 1: 6 shapes x 4 sizes x 5 reps), main2.txt (sitting 2: 8 shapes x 4 sizes x 5),
  main3.txt (sitting 3: 8 shapes x {100,4096,133000} x 10), main4.txt (sitting 4: 8 shapes x 13300 x 10),
  each with *-summary.txt; pooled.txt (all four); paired.txt; long.txt (10,000 commits, 3 reps);
  jext4.txt (journaled ext4 on a direct-io loop device, 300 commits x 3 reps: counts reliable,
  timings indicative only); iocost.txt; tracecount.txt and trace-*.txt; strace/c-*.txt (strace -c,
  200 commits); strace/engine-load.txt (strace -f -y of `bench ab --arms supdb,supdb --size 30000
  --passes load --reps 1`, rebuilt bench at 3df85d22); lat/*.lat (per-commit ns of the last sitting
  at each size: total, write part, sync part).

## Reproduce
    gcc -O2 -o e5 e5.c -lcrypto && gcc -O2 -o iocost iocost.c
    ./run.sh $PWD/raw/mainX.txt 10 100 4096 13300 133000 && python3 pooled.py raw/main*.txt
