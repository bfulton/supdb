# Apple Silicon, via localmost

Two `ext-kv --profile full --engines supdb,lmdb` runs on a self-hosted M-series
Mac, taken to answer one question: **does this host hold still?**

It does. Ratio drift between the two runs:

| | run 1 | run 2 | drift |
|---|---|---|---|
| load | 8.966x | 8.517x | -5.0% |
| read | 2.315x | 2.344x | +1.2% |
| scan | 1.031x | 1.058x | +2.7% |

Absolutes moved 0.8-6.0%. On the x86 cloud VM that produced everything in
`results/`, EXT.1 read 0.866x, 0.891x, 0.892x, 1.010x, 1.154x, 1.043x and
1.331x across seven equally rigorous runs with the code unchanged between
several of them, and LMDB's own load figure ranged from 508,205 to 1,034,797.

The Mac managed that **while busy**: loadavg was 4.21 for run 1 and 6.76 rising
to 9.61 during run 2. A loaded laptop is an order of magnitude steadier than an
idle shared VM, which is not what I expected and is the whole finding.

## Not citable, for two separate reasons

**Architecture.** These are aarch64 with 128-byte cache lines and 16 KiB pages.
Every claim in `claims.json` measured on x86 stays x86's. Nothing here
contradicts or replaces it.

**LMDB's load number is a platform artifact, not an engine result.** It loads
at 165,385 ops/s here against 508k-1,035k on Linux. LMDB commits durably on
every batch and Supdb does not (`durable_commit: false`), so this axis measures
what macOS does with fsync on APFS more than it measures either engine. The
8.97x is not a win and must not be quoted as one. Read `scan`, which reaches
parity at 1.03-1.06x where the same code on Linux gives 0.65x, and `read`,
where 2.3x is larger than Linux's 1.1-1.6x but at least measures the same
thing on both.

What these files are for is the *spread*, not the values.
