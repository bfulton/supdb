# zprobe: what it asked, what it predicted, what it found

One host: a Firecracker guest, 4 vCPUs, a 1 GiB cgroup v1 memory group per run, the probe's files dropped from the
page cache before each run, one thread, 1,000,000 uniform reads a pass. The cache the pages share is about 957 MB at a
ballast of 100 MB and falls with the ballast. Every number is one sitting unless two are named.

## The thesis run (10M keys, ballast 100 MB)

Registered before it ran:

- raw pages and a 190 MB file touched once a read, the engine's shape: 0.40-0.46 faults a read in passes 1-3; outside
  0.35-0.52 the probe is not a model of the engine and nothing below counts
- raw pages with no hash: 0.30-0.36 faults a read
- page-filling LZ4 groups, 68 B a key, about 0.68 GB, no hash: pass 0 (the cold fill) 0.12-0.22, passes 1-3 at most 0.05,
  150-330 thousand reads a second
- refuted if the groups' passes 1-3 exceed 0.15 faults or fall under 82 thousand reads a second (LMDB with readahead off
  read 79-82 thousand at 0.27 faults in the suite's ooc row), or the calibration fails

Found:

| layout | pass 0 | passes 1-3 |
|---|---|---|
| raw + 190 MB hash file | 0.532 faults, 46k reads/s | 0.414-0.423 faults, 54-58k reads/s |
| raw, no hash | 0.327 faults, 74k/s | 0.182 faults, 113-117k/s |
| LZ4 groups, no hash | 0.166 faults, 97k/s | 0.000 faults, 262-280k/s (3.6-3.8 us a read) |

The calibration held (the engine's cell reads 0.41-0.46 and 54k). The raw layout without the hash missed low: the
probe's records are 117 B a key where the engine's are 126-130, and the hash is out of the cache entirely. The groups
were not refuted.

## The margin sweep (stage 1a of the design; ballast 100-400 MB, two sittings, the second in reverse order)

The groups' file is 680 MB with a 3.3 MB fence. Registered: faults 0.00-0.01 up to ballast 350 and 0.03-0.05 at 400, refuted
over 0.02 at 250; with an 80 MB file touched once a read (standing for the ordered index's heads) 0.00-0.01 up to 250, about
0.01 at 300, 0.08 +- 0.03 at 350, 0.16 at 400, refuted over 0.05 at 200; the hash file 97-100% resident after a raw run, refuted
under 90%.

| ballast, MB | no heads: pass 0 | passes 1-3 (both sittings) | with the 80 MB file: pass 0 | passes 1-3 (both sittings) |
|---|---|---|---|---|
| 100 | 0.166 | 0.000, 0.000 | 0.186 | 0.000, 0.210 (reruns: 0.000 three times) |
| 150 | 0.166 | 0.000, 0.000 | 0.186 | 0.000, 0.000 |
| 200 | 0.166 | 0.000, 0.000 | 0.186 | 0.000, 0.000 |
| 250 | 0.166 | 0.000, 0.000 | 0.186 | 0.000, 0.000 |
| 300 | 0.166 | 0.000, 0.000 | 0.194 | 0.022, 0.022 |
| 350 | 0.166 | 0.000, 0.000 | 0.246 | 0.106, 0.106 |
| 400 | 0.182 | 0.048, 0.048 | 0.298 | 0.196, 0.196 |

Reads a second in passes 1-3 sat at 250-277 thousand where the faults were zero (230-246 thousand with the extra file),
159 and 151 thousand at the 0.048. None of the ranges was refuted. One cell, the second sitting's ballast 100 with the extra
file, read 0.210 faults and 145 thousand; three reruns of it read 0.000 and 214-238 thousand, so it is a host event and not a
position effect. The hash file was 91.0% resident after the raw-with-hash run (44,241 of 48,640 pages), under the 97-100%
predicted and over the 90% that would have refuted it.

The knee is sharp. With no extra file the groups held zero faults at ballast 350, where the cache is about 707 MB and the
file 96% of it, and read 0.048 at ballast 400, about 657 MB and 103%. With the 80 MB file they held zero to ballast 250,
where the two together are 89% of the cache.

Command lines: `build $D 10000000 190`, then for each ballast `drop.py` over the three files and
`BALLAST_MB=b sh bound.sh 1073741824 zprobe read $D 10000000 z H 1000000 4` with H 0 and, on a directory whose `hash.bin` is its
first 80 MB, 1; the raw run was `... raw 1 ...` at ballast 100. The suite's ooc rows for the same cell are in
`docs/engine.md`.
