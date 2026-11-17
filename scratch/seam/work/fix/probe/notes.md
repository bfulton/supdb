# The fix, tested: the candidates' Log commits and the WAL's options, on the device

Tree `3df85d22`. Nothing under `/home/user/supdb` was touched and no git command was run. Every timed thing ran alone: before each sitting, the traces and the controls, `pgrep -f 'bench ab|pair.sh'` found nothing and `ps` showed nothing busy. Machine: E5's (Firecracker guest, 4 vCPUs, virtio vda with a write-back cache, `/` ext4 `discard,delalloc`, no journal; `/proc/fs/ext4/vda/options` read again here).

## The answer

**One device request per commit is reachable, and on this device it pays only for small synced commits. At the load's batch (133 KB) no candidate beats the design's A without write-through, and every one costs about 1.25-1.33x the plain run.**

| | 100 B | 4 KiB | 13.3 KB | 133 KB |
|---|---|---|---|---|
| JRP (R0) / plain run B | 0.73 | **0.67** (mean 0.86, p90 1.17) | **0.59** (mean 0.74, p90 1.36) | 1.25 (30/30 slower) |
| JRP (R0) / A0 | 0.97 | 0.54 | 0.51 | 1.02 (17/30) |
| ADJ one write (O0) / A0 | 1.02 | 0.82 | 1.07 | 1.03 |
| ADJ two writes (T0) / A0 | 1.05 | 1.14 | 1.15 | 1.07 |

Paired medians over 30 pairs on vda; the mean and p90 columns are the same pairs' ratios of mean and p90 per commit.

- **JRP** (design-free) wins where its journal path runs: commits under 64 KiB. Each such commit is a ring frame in preallocated blocks, so it is one request with no file growth. Its homing every 128 KiB of journal is the spike in its p90.
- **JRP fails its own kill rules at 133 KB.** Against B it is 1.25, where the rule is ≤ 1.15. Against A0 it is 1.02, where the rule is ≤ 0.90. Against D it is 1.10, where the rule is ≤ 0.92. Its fallback rule says JF, since it is not 25 µs ahead of A0 (+7 µs median). JF was not measured: by construction it is R's journal path at ≤ 64 KiB and A0's shape above.
- **ADJ's one write (O)** gets to 1.01-1.05 data requests a commit, but loses at 13.3 KB:
  - the extent tree's leaf block is written at every sync (probe/meta 1.79 a commit);
  - the carried pages put it at 1.40x A0's sectors.
- **ADJ's self-sufficient ring records (T)** are 1.05-1.15x A0 at every size, which fails ADJ's rule for replacing A's delta frames.

**Decision 2 on the measurement: (b), or the same records behind the door.**
- Today's sealed WAL commit (Ds: one `len|nonce|ct|tag` record and an fdatasync) costs what the plain WAL costs at every size: Ds/Cf 1.00 / 1.01 / 0.93 / 1.01, and Dsp/Cp 0.96 / 1.00 / 0.94 / 1.00.
- ADJ's Journal class (J) and JRP's Stream class (S), each with the model's chain fix, cost the same as Ds within 0-6%.
- A WAL as design.md's paged Log file (A0) costs 1.20-1.26x Ds at 4 KiB and up, and 0.74x at 100 B, because its ring never grows.
- JRP as a WAL beats a *fresh* Ds below 64 KiB (0.66 and 0.61) for the same reason. `recycle_wal` gives the record WAL that too: against the prefilled Dsp, JRP is 1.05 / 1.10 / 1.02 / 1.43.
- `recycle_wal` is `false` by default (db.rs:3938, read). Whether the suite's arms set it was not checked.

## What was built (`e5.c`, from E5's, with E5's shapes unchanged)

Every candidate follows E5's A conventions:
- **Layout:** design.md §3 stored offsets (`data_off`, `tagpage_off`), and the 4 KiB header written at create and unsynced (`pf_header`, e5.c:596).
- **Sealing:** AES-256-GCM over every page, tag page, record and frame, with the nonces and AADs each design gives.
- **Write calls:** B's call structure (`run_commit`: whole 1 MiB pieces at mark time with a writeback hint each, then the rest), then one fdatasync.
- **Write-through:** `w` shapes write through by `pwrite` into a memfd, and `0` shapes have none.

| shape | what | code |
|---|---|---|
| `Ow`, `O0` | ADJ cases 0-5 with M = 8 and the model's fixes. A1: case 1 falls to case 4 when a home of H is in P. A2: synced_home = min(Q ∪ {T}) of the record registered at the sync's issue. A3: case 2 needs a contiguous H. A4: a carry that meets P falls to case 4. Records are self-sufficient: L, synced_home, F, the chain head, the open tags, the pending images and the tail. 8 ring slots of 2 pages; the carry is placed at T + m̂ + 2. | e5.c:902 `adj_write`, 824, 1039 |
| `Tw`, `T0` | ADJ with M = 0: case 4 for every m ≥ 1 (homes, then a self-sufficient record in a ring slot) | same |
| `Rw`, `R0` | JRP, one RP per call (design-free §5): ring fallocated at create (4 KiB-1 MiB), TAIL frames byte-packed past the pins (P1/P3; P2 is empty with one thread), a homing as one `pwritev` of `[pages][RP]` at slot 256 + H + dead, T_d 64 KiB, T_h 128 KiB, hints translated through the dead table | e5.c:1077, 1136, 1256 |
| `Ds`, `Dsp` | today's WAL commit under a cipher: `seal_into` (vault.rs:1801-1815, read) as `len u32 \| nonce12 (prefix8\|count) \| ct \| tag16`, AAD id\|at\|len, `write_all` (db.rs:4448-4451, read), `sync_data`, `drop_synced` per MiB (db.rs:4469-4478, read); header 68 B (db.rs:4134-4135, 4200-4212, read); `p` = over a prefilled file | e5.c:1342 |
| `J`, `Jp` | ADJ §10 Journal: `len u32 \| seq u64 \| attempt u24 \| ct \| tag16`, nonce 0x13\|seq\|attempt, AAD file_id\|clear part\|**previous record's tag** (the model's fix) | e5.c:1342 |
| `S`, `Sp` | design-free §4 Stream: `len u32 \| nonce12 \| ct \| tag16` under K_rec derived from K_f, AAD "pfs"\|store_id\|file_id\|offset\|len\|**previous tag** (the model's fix), `pwrite` at the layer's own end. The header is 72 B: §4's fields do not fit 64 with a tag. | e5.c:1281, 1342 |
| `Ap0` | E5's Ap without write-through: the paged Log over a prefilled file, i.e. a design.md WAL under `recycle_wal` | E5's code |

Re-run in the same sittings: E5's A, Aw, A0, B, D, Cf and Cp, unchanged.

**Not built:**
- JRP's multi-RP call with CONT RPs. It is unreachable at these sizes: no call exceeds 1024 pages, which the probe asserts.
- JF.
- Any buffered policy, sync in flight, recovery or finish.

**Verified before timing.** `verify.py` reads each kept stored file the way the candidate's recovery reads it after a clean stop and requires the plaintext the probe handed down, byte for byte. It ran for O0, Ow, T0, R0, Rw, Ds, Dsp, J, Jp, S and Sp at all four sizes, at 7 and 300 commits: 88 of 88 OK. It finds records in the ring and in the data area (ADJ), walks the tag chain, follows the RP chain and the frames past the last RP (JRP), and decodes the records (WAL). It is not a crash test.

**A gap in ADJ's tag chain, found by the verifier.** design-adjacent §3.1 and §6 give each tag page's AAD the previous page's MAC, and the record names `chain_g` and its MAC. Nothing stores a tag page's ctr, which its nonce needs, and nothing stores an older page's MAC, which its AAD needs. So the chain cannot be walked past its newest page.

The probe fixes it at no device cost (e5.c:794):
- each tag page holds 128 × 24 B entries and the previous page's ctr, sealed as 4080 B with its own tag in its last 16 B;
- the record carries the head's ctr.

The same applies to E5's A: its CKPT carries group MACs without tag-page versions (e5.c:467-470). That does not move a device cost either.

## Method (E5's)

- **Runs:** one process per run, on fresh files, 1,000 commits. Sizes 100, 4096, 13300 and 133000 B. 20 shapes, rotated each rep, with the rotation offset per sitting so the 30 reps cover all 20 orders.
- **Sittings:** three of 10 reps (s1 11:44-11:53, s2 11:54-12:03, s3 12:03-12:13 UTC), 2,400 runs and 0 failures.
- **Window:** `sync()` and 300 ms before it. The file's create is inside it. `/sys/block/vda/stat` is read around it: write ios and sectors, flushes, and discard ios (field 12, new here) and discard sectors (field 14).
- **Ratios** are paired per rep (same sitting, rep and size) and pooled, with the count of pairs where the first shape is slower and the median of each sitting.
- **The discard mode** (E5 skeptic): a run with ≥ 0.3 discards a commit is "in mode". Same-mode pairs are in `raw/pooled.txt`.
- **Traces:** `block_rq_issue` through tracefs over 300-commit runs of every shape and size. tracefs was mounted for the run and unmounted after (`mount` shows none now). Files were kept until tracing stopped, so the unlink's discards are not in the counts.
- **strace:** `strace -c -f` over 200-commit runs.
- **Controls:** E5's jext4 method, ext4 in an image behind a direct-io loop device, 300 commits × 3 reps, on three filesystems:
  - `jext4` (journal, no discard);
  - `njnd` (no journal, no discard);
  - `njd` (no journal, discard: `/`'s options).

  Counts are the loop device's. Timings are indicative only.

## Results on vda

### Absolute, µs per commit

Median of run medians, 30 runs each. "mean" is the median of the runs' mean per commit, where it differs materially.

| shape | 100 B | 4 KiB | 13.3 KB | 133 KB |
|---|---|---|---|---|
| B (plain run) | 148 (mean 159) | 200 (209) | 226 (238) | 365 (378) |
| D (today's cipher run) | 154 | 199 | 227 | 412 (435) |
| A / Aw / A0 | 116 / 116 / 110 | 237 / 242 / 237 | 276 / 274 / 260 | 557 / 508 / 464 |
| Ap0 (A0 prefilled) | 113 | 125 | 148 | 353 |
| Ow / O0 | 115 / 115 | 195 / 193 (mean 212) | 272 / 276 | 540 / 488 |
| Tw / T0 | 116 / 115 | 287 / 265 | 306 / 296 | 534 / 491 |
| Rw / R0 | 108 / 112 | 127 / 131 (mean 168 / 172; p90 268 / 274) | 147 / 126 (mean 201 / 182; p90 404 / 380) | 556 / 474 (mean 590 / 495) |
| Cf / Cp (plain WAL) | 157 / 109 | 189 / 113 | 227 / 136 | 372 / 331 |
| Ds / Dsp | 156 / 108 | 188 / 116 | 213 / 131 | 384 / 327 |
| J / Jp | 155 / 108 | 190 / 120 | 218 / 128 | 393 / 347 |
| S / Sp | 151 / 102 | 193 / 115 | 227 / 132 | 399 / 347 |

### Paired ratios

Median [min-max], first slower in k/30. Per-sitting medians are given for the decision rules.

**The run (decision 1's ground):**

| pair | 100 B | 4 KiB | 13.3 KB | 133 KB |
|---|---|---|---|---|
| R0/B | 0.73 [0.47-1.07] 1/30 | 0.67 [0.44-1.13] 2/30 (s 0.64 / 0.68 / 0.65) | 0.59 [0.43-0.86] 0/30 (s 0.56 / 0.65 / 0.53) | 1.25 [1.04-1.63] 30/30 (s 1.26 / 1.21 / 1.27) |
| R0/A0 | 0.97 | 0.54 [0.35-0.80] 0/30 | 0.51 [0.37-0.72] 0/30 | 1.02 [0.79-1.44] 17/30 (s 1.04 / 1.06 / 0.97) |
| R0/D | 0.74 | 0.62 | 0.54 | 1.10 [0.85-1.45] 25/30 |
| Rw/Aw | 1.00 | 0.52 | 0.53 | 1.06 |
| Rw/D | 0.73 | 0.63 | 0.62 | 1.30 [0.84-1.90] 27/30 |
| O0/A0 | 1.02 | 0.82 [0.56-1.11] 4/30 (s **0.901** / 0.877 / 0.737) | 1.07 [0.71-1.47] 17/30 (s 1.070 / 1.072 / 1.020) | 1.03 [0.87-1.49] 21/30 |
| Ow/Aw | 0.99 | 0.81 (s 0.846 / 0.781 / 0.753) | 1.00 (s 0.998 / 0.911 / 1.046) | 1.04 |
| O0/B | 0.77 | 0.95 [0.59-1.43] 13/30 (mean 1.02) | 1.27 [0.85-1.65] 28/30 | 1.33 [1.06-1.56] 30/30 |
| T0/A0 | 1.05 [0.57-1.80] 19/30 | 1.14 [0.82-1.47] 26/30 | 1.15 [0.83-1.79] 24/30 | 1.07 [0.75-1.30] 20/30 |
| Tw/Aw | 1.03 | 1.15 | 1.10 | 1.06 |
| T0/B | 0.76 | 1.43 | 1.37 | 1.34 |
| A0/B (E5: A0 at 4 KiB 1.20) | 0.75 | 1.25 | 1.16 | 1.27 |
| A/B (E5: 0.69 / 1.29 / 1.39 / 1.46) | 0.77 | 1.23 | 1.20 | 1.52 |
| D/B | 1.02 | 0.98 | 1.06 | 1.15 |
| A0/D, Aw/D | 0.73, 0.73 | 1.22, 1.26 | 1.15, 1.15 | 1.10, 1.22 |

**The WAL (decision 2):**

| pair | 100 B | 4 KiB | 13.3 KB | 133 KB |
|---|---|---|---|---|
| Ds/Cf | 1.00 [0.57-1.34] 14/30 | 1.01 | 0.93 | 1.01 |
| Dsp/Cp | 0.96 | 1.00 | 0.94 | 1.00 |
| J/Ds, S/Ds | 1.02, 0.99 | 1.00, 1.04 | 1.03, 1.05 | 1.02, 1.03 |
| Jp/Dsp, Sp/Dsp | 1.00, 1.00 | 1.05, 1.01 | 1.00, 0.99 | 1.06, 1.06 |
| S/Cf, Sp/Cp | 0.97, 0.97 | 1.02, 1.01 | 1.00, 0.95 | 1.03, 1.07 |
| A0/Ds (paged Log WAL, fresh) | 0.74 2/30 | 1.26 24/30 | 1.21 27/30 | 1.20 30/30 |
| A0/J (s1 / s2 / s3) | 0.73 | 1.24 / 1.16 / 1.37 | 1.25 / 1.07 / 1.17 | 1.13 / 1.19 / 1.20 |
| Ap0/Dsp | 0.99 | 1.13 22/30 | 1.12 22/30 | 1.07 22/30 |
| Ap0/Jp (s1 / s2 / s3) | 1.03 | 0.95 / 1.09 / 1.03 | 1.13 / 1.05 / 1.16 | 0.91 / 1.07 / 1.06 |
| R0/Ds | 0.71 | 0.66 [0.40-1.07] 1/30 | 0.61 [0.47-0.80] 0/30 | 1.25 [0.96-1.50] 28/30 |
| R0/Dsp | 1.05 | 1.10 27/30 | 1.02 17/30 | 1.43 30/30 |
| O0/Ds, T0/Ds | 0.76, 0.74 | 0.95, 1.37 | 1.29, 1.39 | 1.27, 1.27 |

**Write-through by pwrite** (w/0), at 100 B / 4 KiB / 13.3 KB / 133 KB:
- Aw/A0: 0.99 / 1.02 / 1.04 / 1.09
- Ow/O0: 0.99 / 1.00 / 1.00 / 1.07
- Tw/T0: 1.02 / 1.03 / 1.01 / 1.08
- Rw/R0: 1.03 / 0.97 / 1.13 / 1.17 (+84 µs median at 133 KB)

**Device sectors a commit against B,** at 100 B / 4 KiB / 13.3 KB / 133 KB:
- R: 0.55 / 1.09 / 1.41 / 1.04
- O: 0.59 / 1.13 / 1.56 / 1.03
- T: 0.59 / 1.11 / 1.07 / 1.02
- A: 0.58 / 1.18 / 1.11 / 1.03
- Ds against Cf: 1.00 at every size.

**Allocated bytes a commit at 133 KB:** B 133,030; A0 135,107; O and T 134,095; R 138,760. R's extra is its RP slots, 3.5% of the homed pages ("dead"), and the fallocated ring.

### Block requests per commit

From tracefs, 300 commits: probe data / probe meta (inode and extent writes) / kworker discards. The flush is 1.01 in every shape.

| shape | 100 B | 4 KiB | 13.3 KB | 133 KB |
|---|---|---|---|---|
| B | 1.00 / 1.01 / 0 | 1.01 / 1.01 / 0 | 1.02 / 1.01 / 0.94 (in mode that run) | 1.13 / 1.01 / 0 |
| A0 | 1.04 / 0.11 / 0 | 2.01 / 1.01 / 0.76 | 2.03 / 1.01 / 0.94 | 2.38 / 1.01 / 1.03 |
| O0 | 1.00 / 0.08 / 0.02 | **1.01 / 0.55** / 0.09 | **1.05 / 1.79** / 0.13 | 2.49 / 1.02 / 1.03 |
| T0 | 1.04 / 0.08 / 0 | 2.01 / 1.01 / 0.87 | 2.04 / 1.01 / 0.91 | 2.48 / 1.08 / 0.95 |
| R0 | 1.00 / 0.08 / 0 | **1.01** / 0.87 (ring lap 1) / 0 | **1.00 / 0.41** / 0.28 | **1.16** / 1.01 / 0.98 |
| Ds, J, S | 1.00 / 1.01 / 0 | 1.01 / 1.01 / 0 | 1.01 / 1.01 / 0 to 0.94 | 1.01 / 1.01 / 0 |
| Dsp, Jp, Sp | 1.00 / 0.05 / 0 | 1.01 / 0.05 / 0 | 1.01 / 0.05 / 0 | 1.01 / 0.12-0.16 / 0 |

O0 at 13.3 KB writes a second metadata block at every sync (block 32005832, 237 times in 300). R0's ring is fallocated, so lap 1 converts unwritten blocks at each sync. Lap 1 lasts:
- about 240 commits at 4 KiB;
- about 77 at 13.3 KB;
- never ends within 1,000 commits at 100 B.

The 300-commit trace at 4 KiB is therefore mostly lap 1. Over the timed 1,000 commits R0's ios a commit are 2.31 at 4 KiB and 2.22 at 13.3 KB, against B's 3.00.

Syscalls a commit (the probe's counters agree with `strace -c`):

| shape | 4 KiB | 13.3 KB | 133 KB |
|---|---|---|---|
| R0 | 0.97 `pwrite` + 0.03 `pwritev` | 0.91 + 0.10 | 1.00 `pwritev` + 0.12 `pwrite` |
| O0 | 1.00 `pwritev` | 1.01 + 0.04 | 1.18 + 1.32 |
| A0 and T0 | 2 | 2.05 | 2.5 |

Every shape makes one `fdatasync` a commit.

### Case and counter mix (median a run, 1,000 commits)

- **O, 4 KiB:** flush and carry alternate exactly (c2 501, c3 499), with 0.50 carried pages and 1.83 record pages in the data area a commit.
- **O, 13.3 KB:** c2 503, c3 495, c4 11 (4 non-contiguous H, 7 carries meeting P), with 1.61 carried pages and 2.95 record pages a commit.
- **O, 133 KB:** c4 1,060, with c2 59 at the rests after a piece boundary.
- **O, 100 B:** c0 969.
- **T:** a ring record of 1.07-1.46 pages a commit.
- **R:**

| | homings a commit | pages a homing | frames a commit |
|---|---|---|---|
| 4 KiB | 0.031 | 32 | 0.97 |
| 13.3 KB | 0.100 | 32.5 | 0.91 |
| 133 KB | 1.003 | 32.4 | 0.123 (the parts under 64 KiB at a piece boundary) |
| 100 B | none in 1,000 commits | | |

- **A:** the TAIL frame carries 2,034-2,053 B a commit at ≥ 4 KiB, as E5 found.

### Where R's time goes at 133 KB

Write part, then sync part, median µs:

| shape | write part | sync part | sync part in the discard mode | sync part out of it |
|---|---|---|---|---|
| R0 | 67 | 401 | 419 (n = 20) | 378 (n = 10) |
| B | 33 | 329 | | 329 (never in mode) |
| A0 | 58 | 406 | | |

R removes 1.2 data requests a commit against A0, and the time does not move.
- **CPU:** sealing and staging 133 KB cost +34 µs of write part over B.
- **This mount:** the discard mode costs about +40 µs of sync part in R (20/30 runs).
- **Without the discard mode:** +49 µs remains against B (same-mode R0/B 1.20 [1.04-1.45] 10/10).

The njnd control (no discard) reads R0 at 4.14 ios a commit against B's 3.14. Not attributed.

### The discard mode

Runs at ≥ 0.3 discards a commit, of 30:

| size | A0 | O0 | T0 | R0 | B | Ds |
|---|---|---|---|---|---|---|
| 4 KiB | 30 | 25 | 25 | 0 | 7 | 6 |
| 13.3 KB | 26 | 0 | 24 | 0 | 5 | 2 |
| 133 KB | 19 | 24 | 21 | 20 | 0 | 1 |

`filefrag` over kept 300-commit files gives A0, T0 and O0 exactly 4 extents (header, ring, tag page, data), R0 2 at 4 KiB and 6 at 133 KB, and B 2 or 5. A file at the inode's in-place capacity of 4 extents flipping its tree depth each commit would allocate and free a block a commit. That is a hypothesis and is not verified.

## Controls (300 commits × 3 reps on a direct-io loop: indicative only)

| pair | njnd (no journal, no discard) | jext4 (journal) | njd (no journal, discard) |
|---|---|---|---|
| R0/B at 4 KiB / 13.3 KB / 133 KB | 1.06 / 0.61 / 1.21 | 0.86 / 0.43 / 1.20 | 0.98 / 0.51 / 1.01 (B in the discard mode 3/3, R0 1/3) |
| R0/A0 at 4 KiB / 13.3 KB / 133 KB | 0.95 / 0.78 / 1.08 | 0.88 / 0.45 / 1.12 | 1.04 / 0.39 / 1.00 |
| O0/A0 at 4 KiB / 13.3 KB | 0.98 / 1.14 | 0.79 / 1.14 | 0.84 / 1.07 |
| T0/A0 at 4 KiB / 13.3 KB | 1.01 / 1.04 | 1.02 / 1.12 | 1.26 / 1.01 |
| A0/Ds at 100 B / 4 KiB / 13.3 KB / 133 KB | 0.61 / 1.00 / 0.78 / 1.18 | 0.38 / 1.06 / 0.97 / 1.02 | 0.82 / 0.96 / 0.99 / 1.22 |
| S/Cf, all sizes | 1.00-1.04 | 0.93-1.18 | 0.95-1.08 |

The 4 KiB R cells are ring lap 1, so they are R's worst case.

What the controls show:
- JRP's win at 13.3 KB holds on every filesystem tried.
- Its loss at 133 KB holds on both filesystems without discard (1.21 and 1.20). On njd it is level (1.01), with B in the discard mode in all three runs and R0 in one.
- O's loss at 13.3 KB holds on every one.
- At 100 B on jext4, every paged shape is about 0.3x B, because B and Cf pay a journal commit for growth. Dsp and Cp are level with them there.

## What the measurements decide, by the designs' own rules

**design-adjacent §11:**
- O0 against A0 is below 0.9 in every sitting nowhere. At 4 KiB the sittings read 0.901 / 0.877 / 0.737; at 13.3 KB they read 1.07 / 1.07 / 1.02. So cases 2 and 3 are dropped, and the Log keeps two writes.
- T is not within noise of A: T0/A0 is 1.05-1.15. So A's delta frames and CKPTs stay.
- J against A0 (fresh) is below 1 in every sitting at 4 KiB, 13.3 KB and 133 KB, so (b) holds against A.
- Jp against Ap0 is not below 1 in every sitting at 4 KiB or 133 KB (level), and is 0.89x at 13.3 KB.

**design-free §9:**

| prediction | measured | |
|---|---|---|
| data requests ≤ 1.1 a commit | 1.00 / 1.01 / 1.00 / 1.16 | missed at 133 KB |
| inode writes ≤ 0.15 after lap 1 | not isolated; 0.31 and 0.22 a commit over 1,000 commits including lap 1, at 4 KiB and 13.3 KB | not verified |
| J/B ≤ 0.85 at 4 KiB and 13.3 KB | median 0.67 and 0.59 | holds; the mean is 0.86 at 4 KiB, just over |
| J/B ≤ 1.15 at 133 KB | 1.25 | killed |
| J/A0 ≤ 0.90 at 133 KB | 1.02 | killed |
| J/D ≤ 0.92 at 133 KB | 1.10 | killed |
| S/Cf within 1.05 | 0.97 / 1.02 / 1.00 / 1.03 | holds; Sp/Cp at 133 KB is 1.07 |

The fallback rule fires, which leads to JF.

**What follows for the owner's two choices:**
- **The run temp.** At the load's 133 KB batch, A0, R0, O0 and T0 sit within 1.02-1.07x of each other. They cost 1.25-1.34x the plain run and about 1.10x today's double write (1.22-1.30x with pwrite write-through). The envelope at the close, which they exist to remove, is not in this window.
  - Journal-first homing (JRP's journal path, or JF) is the only shape that beats B at small synced commits: 4 KiB and 13.3 KB.
  - Whether the interleaved layout is worth its reader table is answered no at 133 KB by its own rule.
- **The WAL.** The record per commit costs the plain WAL's time whether the engine seals it (b) or a door class does (J or S): they are level within ±6%. A paged Log WAL is dearer at 4 KiB and up, except on fresh files, where its preallocated ring buys what `recycle_wal` buys the record WAL.

## Not measured, and limits

- **Policies and states:** the buffered policy, syncs in flight (the helper's, the close's presync), the T_d/T_h and M sweeps, JF, JRP's CONT split, finish/FINAL and recovery cost were not measured.
- **The read path was not measured:** the dead table, the loss of 1 MiB alignment and its folios, the pager.
- **The probe's WAL shapes use the run's call structure:** a 24-byte marker against a 21-byte commit frame, and the first commit's 4,288-byte prefix. As in E5, that is negligible.
- **R's first lap of the ring.** It pays a conversion at each sync. The 1,000-commit runs at 4 KiB are a quarter lap 1, and the controls' 300 commits at 4 KiB almost all of it.
- **The discard mode's mechanism is not verified,** and neither are the R0 / njnd 4.14 ios and the +49 µs out-of-mode sync part at 133 KB.
- **The controls are a loop device over vda,** 3 reps of 300 commits each. Their counts are reliable and their timings indicative only.
- **The device is this guest's.** Prices move with the filesystem, so the decision is taken on the one the store ships for.

## Files

| file | what |
|---|---|
| `e5.c`, `e5` | the probe. `e5-orig.c` is E5's source as copied. Build: `gcc -O2 -o e5 e5.c -lcrypto`. |
| `verify.py` | the stored-file check (`E5_KEEP=1 E5_KEYS=… E5_PLAIN=…`) |
| `run.sh` | the sittings: `run.sh OUT REPS SITTING` |
| `tracerun.sh` | the traces (tracefs mounted and unmounted) and `strace -c` |
| `ctl.sh` | the loop-device controls |
| `analyze.py`, `extra.py` | the tables. `tracecount.py`, `paired.py`, `pooled.py` and `summarize.py` are E5's, copied. |
| `raw/s1.txt`, `raw/s2.txt`, `raw/s3.txt` | the runs |
| `raw/pooled.txt`, `raw/extra.txt`, `raw/s1-summary.txt` | the summaries |
| `raw/trace/tracecount.txt`, `raw/trace/trace-*.txt` | the traces |
| `raw/strace/c-*.txt` | the strace counts |
| `raw/ctl-{jext4,njnd,njd}.txt`, `raw/ctl-{jext4,njnd,njd}-summary.txt` | the controls |
| `raw/lat/s*-*-r{1,2}.lat` | per-commit ns: total, write part, sync part |
| `raw/idle-before-s1.txt` | idle disk activity before sitting 1 |

Reproduce:

    gcc -O2 -o e5 e5.c -lcrypto
    ./run.sh $PWD/raw/sN.txt 10 N
    python3 -I analyze.py raw/s*.txt
    python3 -I extra.py raw/s*.txt

`tmp/` held the run files, and is removed.
