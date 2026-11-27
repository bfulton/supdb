# Size of the history

Measured at `origin/main` = `b3a043dc` (tree `7f58d776`), over the 49 refs fetched into the checkout: `origin/main`, nine `bank/quick-*` branches, two `claude/*` leftovers and 37 pull request heads. The branch of the session that ran the audit is left out. No tag exists. [S2]

- Bytes are an object's own size. MB is 10^6 bytes.
- "Packed" is an object's entry in this checkout's packs, the smallest where more than one pack holds it. A packed sum over part of the refs is an estimate: deltas cross the edge of the part.
- An object is filed under the first path `git rev-list --objects` meets it at.
- A label in brackets names the command under "Commands" that prints the figure. In those commands `origin/main` is `b3a043dc`.

## The plan's figures, re-measured

The plan's figures were taken at `8025423` (#32). Main is five pull requests later.

| The plan states | At `b3a043dc` | |
|---|---|---|
| One squash body is 339 KB (#27) | #27 is 339,159 bytes still. Two are larger: #35 at 345,381 and #36 at 344,967. Four bodies are over 256 KiB and eight over 64 KiB; the ninth is 42,265 bytes. Main's 32 messages are 1,842,844 bytes together. [M2, M3] | stale |
| #32 changes 120 lines in two files and carries 94 KB | Holds: 120 insertions and 1 deletion in 2 files, 94,579 bytes. The wider case is now #36: 147 insertions and 80 deletions in 6 files under 344,967 bytes, 278 of its 279 items being items of #35. [M2, M4, M6] | holds, no longer the extreme |
| Four of the seven newest open with the same text | The seven newest are #31 to #37, and two of them (#31, #32) open with the text #29 and #30 open with. Counted by item instead: main's messages list 1,551 squash items, and 653 repeat an item of an older main commit (#36: 278 of 279; #32: 88 of 89; #31: 82 of 88; #35: 126 of 280; #30: 40 of 82). [M5, M6] | stale |
| 599 `Co-Authored-By` and 599 `Claude-Session` lines, seven distinct session links | That count is of the seven newest commits at `8025423`. The seven newest now carry 790 and 790, with 9 distinct links. All of main carries 1,542 and 1,543, with 15 distinct links (1,581 co-author lines when the 39 spelled `Co-authored-by` are counted). All refs carry 3,554 and 3,555, with 18 distinct links. [M7, M8] | stale |
| 51 MB of the history's file versions are banked rows | The plan's figure is 53,523,449 bytes (51.0 MiB) in 112 rows. Main's history now holds 130 rows, 67,067,767 bytes (64.0 MiB), 8,876,320 packed. Every one is in the current tree: no row was ever rewritten. [D2] | stale |
| Nothing under `scratch/` is on main | Holds: no path under `scratch/` in main's history. [E1] | holds |
| Two `claude/*` leftovers | Holds: `claude/e-recode-auto` and `claude/wal-ahead-beside-wip`. [M9] | holds |
| Five unmerged `bank/quick-*` | Nine exist and all nine are unmerged: each is one commit ahead of main, on nine different bases (#28 to #36). [M9] | stale |

Main is 32 commits: one initial commit and 31 squash commits, #1 to #20 and #27 to #37 [M1]. The local `main` ref stands at `e40d6283` (#36), one behind `origin/main`, so a command has to say `origin/main` [S2].

## (a) Packs and objects

| | |
|---|---:|
| Packs | 3 |
| Entries in packs | 33,269 |
| Distinct objects | 32,503 |
| Objects held in more than one pack | 724 |
| Pack files, bytes | 12,409,929 + 77,333,465 + 19,636,872 = 109,380,266 |
| `size-pack` (packs and their indexes) | 105.20 MiB |

[A1, A3, A4] The pack files are 31.7 MB larger than the smallest-copy sum of their distinct objects, 77,651,678 bytes. Every object in the packs is reachable from the 49 refs.

| Type | Objects | Bytes | Packed |
|---|---:|---:|---:|
| blob | 22,294 | 2,776,522,646 | 74,110,335 |
| tree | 8,186 | 7,437,594 | 1,130,074 |
| commit | 2,023 | 5,555,268 | 2,411,269 |
| all | 32,503 | 2,789,515,508 | 77,651,678 |

[A2]

## (b) The 40 largest blobs

37 of the 40 are versions of `src/db.rs`, a file of 3,433,010 bytes in the current tree. The other three are run logs under `scratch/`. One of the 40 is in main's tree, three more are in main's history, and 36 are reachable from pull request heads alone. [B, B2]

| # | Path | Bytes | Packed | Introduced by | Pull heads holding that commit | Reach |
|---:|---|---:|---:|---|---|---|
| 1 | `scratch/wave2/zrecode2hot8-io.samples` | 7,611,360 | 891,893 | `84cc340e`, 2026-10-27 | #6 | pull refs only |
| 2 | `src/db.rs` | 3,433,010 | 902,820 | `054bf219`, 2026-11-27 | #35, #36 | main tree (#35) |
| 3 | `src/db.rs` | 3,432,309 | 1,118,445 | `24cfc0ff`, 2026-11-27 | #35, #36 | pull refs only |
| 4 | `src/db.rs` | 3,432,309 | 191 | `92fa6744`, 2026-11-27 | #35, #36 | pull refs only |
| 5 | `src/db.rs` | 3,431,884 | 207 | `5b81772d` merge, 2026-11-27 | #35, #36 | pull refs only |
| 6 | `src/db.rs` | 3,091,619 | 20,360 | `48aba372` merge, 2026-11-27 | #33 | main history (#33) |
| 7 | `src/db.rs` | 3,082,028 | 20,728 | `3b419ea9`, 2026-11-27 | #33 | pull refs only |
| 8 | `src/db.rs` | 3,080,954 | 11,554 | `bfd38186`, 2026-11-27 | #35, #36 | pull refs only |
| 9 | `src/db.rs` | 3,080,663 | 1,027 | `413ca075`, 2026-11-26 | #35, #36 | pull refs only |
| 10 | `src/db.rs` | 3,080,640 | 37,322 | `e5777539` merge, 2026-11-26 | #33 | pull refs only |
| 11 | `src/db.rs` | 3,080,031 | 225 | `1c88fe88`, 2026-11-26 | #35, #36 | pull refs only |
| 12 | `src/db.rs` | 3,077,857 | 9,227 | `ee4bf6ef`, 2026-11-26 | #35, #36 | pull refs only |
| 13 | `src/db.rs` | 3,059,460 | 3,186 | `0478889f`, 2026-11-25 | #35, #36 | pull refs only |
| 14 | `src/db.rs` | 3,055,543 | 1,100 | `ff9483b4`, 2026-11-26 | #35, #36 | pull refs only |
| 15 | `src/db.rs` | 3,053,818 | 1,264 | `acbaa317`, 2026-11-26 | #35, #36 | pull refs only |
| 16 | `src/db.rs` | 3,053,189 | 609 | `31398c6d`, 2026-11-25 | #35, #36 | pull refs only |
| 17 | `src/db.rs` | 3,052,829 | 999,956 | `46e13bf4`, 2026-11-25 | #35, #36 | pull refs only |
| 18 | `src/db.rs` | 3,052,329 | 1,293 | `c04f46d6`, 2026-11-25 | #35, #36 | pull refs only |
| 19 | `src/db.rs` | 3,049,945 | 41,666 | `b4be61df` merge, 2026-11-25 | #35, #36 | pull refs only |
| 20 | `src/db.rs` | 2,981,513 | 783,375 | `f8670780`, 2026-11-26 | #31, #32 | main history (#31) |
| 21 | `src/db.rs` | 2,981,176 | 130 | `d67c35d1`, 2026-11-26 | #31, #32 | pull refs only |
| 22 | `src/db.rs` | 2,981,011 | 201 | `77d5371d`, 2026-11-26 | #31, #32 | pull refs only |
| 23 | `src/db.rs` | 2,980,550 | 3,792 | `e8900661`, 2026-11-26 | #31, #32 | pull refs only |
| 24 | `src/db.rs` | 2,971,922 | 861 | `dd83d582`, 2026-11-26 | #30, #31, #32 | main history (#30) |
| 25 | `src/db.rs` | 2,971,622 | 6,309 | `3f4f23f2`, 2026-11-26 | #30, #31, #32 | pull refs only |
| 26 | `src/db.rs` | 2,954,175 | 1,318 | `3f413cd8`, 2026-11-26 | #30, #31, #32 | pull refs only |
| 27 | `src/db.rs` | 2,951,780 | 13,563 | `2c0a1b1b`, 2026-11-26 | #30, #31, #32 | pull refs only |
| 28 | `src/db.rs` | 2,938,537 | 403 | `db56d808`, 2026-11-26 | #30, #31, #32 | pull refs only |
| 29 | `src/db.rs` | 2,924,629 | 4,647 | `eee0834e`, 2026-11-24 | #35, #36 | pull refs only |
| 30 | `src/db.rs` | 2,924,004 | 277 | `c746ecdd` merge, 2026-11-24 | #35, #36 | pull refs only |
| 31 | `src/db.rs` | 2,921,674 | 276 | `5201f654` merge, 2026-11-24 | #35, #36 | pull refs only |
| 32 | `src/db.rs` | 2,915,441 | 14,127 | `d3ea4fe8` merge, 2026-11-24 | #35, #36 | pull refs only |
| 33 | `src/db.rs` | 2,912,758 | 2,842 | `8e235df2`, 2026-11-26 | #30, #31, #32 | pull refs only |
| 34 | `src/db.rs` | 2,912,718 | 140 | `10b7f4dd`, 2026-11-26 | #30, #31, #32 | pull refs only |
| 35 | `src/db.rs` | 2,906,833 | 1,450 | `dad89740`, 2026-11-26 | #30, #31, #32 | pull refs only |
| 36 | `src/db.rs` | 2,903,820 | 5,805 | `9fb49d9f`, 2026-11-25 | #30, #31, #32 | pull refs only |
| 37 | `src/db.rs` | 2,888,981 | 59,788 | `c75da235`, 2026-11-25 | #30, #31, #32 | pull refs only |
| 38 | `scratch/wave2/zlazyparseauto5m-ab-1.err` | 2,885,854 | 285,342 | `a80b13d7`, 2026-10-29 | #6 | pull refs only |
| 39 | `scratch/wave2/zlazyparseauto5m-ab-2.err` | 2,883,648 | 294,350 | `a80b13d7`, 2026-10-29 | #6 | pull refs only |
| 40 | `src/db.rs` | 2,877,605 | 887 | `1a3c2e64`, 2026-11-24 | #35, #36 | pull refs only |

"Introduced by" is the oldest commit in which the blob appears, by commit date; `merge` marks a merge commit [B2]. The next column lists the pull request heads that commit is reachable from [B4]. The four blobs on main reached it through the squash commit named in the last column [B5].

Versions of one file are most of the uncompressed history. [B3, B3m]

| Path | Versions, all refs | Bytes | Packed | Versions on main | Bytes | Packed |
|---|---:|---:|---:|---:|---:|---:|
| `src/db.rs` | 698 | 1,165,383,287 | 7,241,613 | 23 | 42,530,462 | 1,813,495 |
| `docs/engine.md` | 476 | 229,663,947 | 2,407,211 | 21 | 12,634,502 | 984,832 |
| `bench/src/engines.rs` | 439 | 223,316,131 | 1,461,863 | 18 | 10,827,140 | 475,247 |
| `CLAUDE.md` | 544 | 152,995,705 | 2,028,045 | 22 | 7,284,225 | 672,390 |
| `tests/db.rs` | 253 | 125,940,187 | 1,344,440 | 15 | 9,353,549 | 433,341 |
| `tests/zpages.rs` | 207 | 105,728,693 | 1,569,372 | 15 | 9,322,037 | 553,503 |

`src/db.rs` alone is 42% of all blob bytes and 9% of all packed bytes.

## (c) File versions by directory

"main" is every blob reachable from `origin/main`. "Other refs only" is every blob reachable from another ref and not from main. [C1]

| Directory | main: blobs | bytes | packed | other refs only: blobs | bytes | packed |
|---|---:|---:|---:|---:|---:|---:|
| `scratch/` | 0 | 0 | 0 | 2,426 | 289,522,464 | 30,533,069 |
| `bench/` | 350 | 84,079,762 | 10,199,728 | 1,135 | 284,617,886 | 5,130,789 |
| `src/` | 124 | 56,532,233 | 3,211,912 | 1,622 | 1,294,420,688 | 7,720,756 |
| `docs/` | 3,006 | 28,279,572 | 2,378,766 | 10,520 | 273,277,185 | 5,914,711 |
| `tests/` | 112 | 23,315,064 | 1,718,202 | 895 | 259,760,851 | 3,136,809 |
| `(root)` | 105 | 8,400,213 | 980,091 | 905 | 166,940,682 | 1,822,459 |
| `results/` | 170 | 1,412,230 | 351,167 | 473 | 2,710,920 | 557,318 |
| `scripts/` | 40 | 268,105 | 84,204 | 89 | 891,074 | 87,430 |
| `.github/` | 30 | 174,064 | 47,564 | 123 | 687,982 | 55,170 |
| `web/` | 28 | 246,172 | 61,078 | 56 | 510,705 | 33,622 |
| `figures/` | 19 | 105,415 | 21,062 | 38 | 212,136 | 24,784 |
| `experiments/` | 11 | 51,132 | 16,930 | 8 | 38,477 | 4,099 |
| `examples/` | 3 | 25,758 | 9,046 | 5 | 41,467 | 9,296 |
| `.cargo/` | 0 | 0 | 0 | 1 | 409 | 273 |
| total | 3,998 | 202,889,720 | 19,079,750 | 18,296 | 2,573,632,926 | 55,030,585 |

`results/`, `figures/` and `.cargo/` are not in the current tree.

Second level. `dir/*` is the files directly in `dir`. [C2]

| Directory | main: blobs | bytes | packed | other refs only: blobs | bytes | packed |
|---|---:|---:|---:|---:|---:|---:|
| `bench/*` | 45 | 1,472,964 | 167,016 | 284 | 11,744,269 | 177,661 |
| `bench/.cargo/` | 1 | 3,897 | 1,517 | 1 | 3,361 | 1,381 |
| `bench/aws/` | 6 | 24,695 | 11,250 | 6 | 19,740 | 2,005 |
| `bench/examples/` | 43 | 450,899 | 130,535 | 48 | 700,250 | 43,177 |
| `bench/external/` | 7 | 458,692 | 48,331 | 82 | 5,300,034 | 47,127 |
| `bench/runs/` | 130 | 67,067,767 | 8,876,320 | 58 | 28,357,376 | 3,649,408 |
| `bench/scripts/` | 28 | 197,836 | 46,611 | 24 | 151,905 | 14,718 |
| `bench/shim/` | 2 | 11,579 | 4,431 | 0 | 0 | 0 |
| `bench/src/` | 75 | 14,222,660 | 875,103 | 628 | 238,319,615 | 1,192,301 |
| `bench/tests/` | 7 | 53,749 | 13,938 | 3 | 15,928 | 2,903 |
| `bench/tools/` | 1 | 6,350 | 2,386 | 1 | 5,408 | 108 |
| `bench/vendor/` | 5 | 108,674 | 22,290 | 0 | 0 | 0 |
| `docs/*` | 33 | 13,877,018 | 1,226,351 | 517 | 218,380,946 | 1,434,404 |
| `docs/figures/` | 2,973 | 14,402,554 | 1,152,415 | 10,003 | 54,896,239 | 4,480,307 |
| `scratch/*` | 0 | 0 | 0 | 40 | 1,691,223 | 101,835 |
| `scratch/charts/` | 0 | 0 | 0 | 2 | 51,518 | 987 |
| `scratch/design/` | 0 | 0 | 0 | 2 | 575,548 | 111,934 |
| `scratch/enc/` | 0 | 0 | 0 | 54 | 1,070,059 | 137,767 |
| `scratch/hunt/` | 0 | 0 | 0 | 23 | 294,520 | 88,344 |
| `scratch/hunt2/` | 0 | 0 | 0 | 8 | 36,494 | 10,606 |
| `scratch/onefile/` | 0 | 0 | 0 | 34 | 158,996 | 44,415 |
| `scratch/ooc/` | 0 | 0 | 0 | 12 | 266,724 | 18,520 |
| `scratch/plan/` | 0 | 0 | 0 | 707 | 4,987,082 | 1,043,577 |
| `scratch/reap/` | 0 | 0 | 0 | 7 | 184,158 | 20,866 |
| `scratch/seam/` | 0 | 0 | 0 | 242 | 2,208,571 | 753,520 |
| `scratch/stage0/` | 0 | 0 | 0 | 79 | 2,069,658 | 594,004 |
| `scratch/wave1/` | 0 | 0 | 0 | 4 | 39,765 | 13,474 |
| `scratch/wave2/` | 0 | 0 | 0 | 1,212 | 275,888,148 | 27,593,220 |
| `src/*` | 76 | 45,695,672 | 2,126,664 | 1,105 | 1,158,960,313 | 6,109,074 |
| `src/bench/` | 10 | 165,403 | 44,527 | 21 | 315,198 | 11,923 |
| `src/bin/` | 10 | 1,154,412 | 136,222 | 176 | 30,628,963 | 391,788 |
| `src/db/` | 28 | 9,516,746 | 904,499 | 320 | 104,516,214 | 1,207,971 |
| `tests/*` | 101 | 23,102,751 | 1,680,434 | 821 | 257,566,823 | 3,080,435 |
| `tests/common/` | 5 | 62,000 | 12,175 | 21 | 222,812 | 12,131 |
| `tests/data/` | 5 | 121,553 | 17,953 | 53 | 1,971,216 | 44,243 |
| `tests/lex/` | 1 | 28,760 | 7,640 | 0 | 0 | 0 |

## (d) `bench/runs`

Rows in the current tree. [D1]

| Scale directory | Row files | Bytes |
|---|---:|---:|
| `quick` | 86 | 38,497,827 |
| `ooc` | 34 | 22,386,720 |
| `full` | 8 | 4,598,794 |
| `killed/ooc` | 2 | 1,584,426 |
| all | 130 | 67,067,767 |

Row versions in history. [D2]

| Reachable from | Row blobs | Bytes | Packed |
|---|---:|---:|---:|
| `8025423` (the plan's point) | 112 | 53,523,449 | 7,130,584 |
| `origin/main` | 130 | 67,067,767 | 8,876,320 |
| all refs | 188 | 95,425,143 | 12,525,728 |

The rows are 72% of the current tree's bytes and 45% of main's packed bytes. The 58 row blobs off main are 27 quick rows on the nine `bank/quick-*` branches and on no other ref (17,888,453 bytes, 2,914,821 packed), and 31 under pull request heads [D4, G5].

A row's file name ends in the commit it was measured at. Of the 130 names on main, 2 name a commit on main, 126 name a commit that is under pull request heads and not on main, and 2 name a commit no fetched ref holds (`5227305`, `14f8cc6`). The 27 rows on the `bank/quick-*` branches all name main's commits, #28 to #36, three each. [D3]

## (e) `scratch/`

Nothing under `scratch/` is in main's history [E1], and no ref has it in its tip tree. Three pull request heads hold it in their history. [E2, E3]

| Held by | Blobs | Bytes | Packed | Files at the tip |
|---|---:|---:|---:|---:|
| pull request #6 | 1,274 | 277,292,068 | 27,849,158 | 0 |
| pull requests #35 and #36 (the same set) | 1,153 | 12,230,396 | 2,683,920 | 0 |
| all refs | 2,426 | 289,522,464 | 30,533,069 | 0 |

`scratch/wave2/` is 27,593,220 of the packed bytes [C2], and 24,337,401 of that is 346 `.err` run logs [E4]. `scratch/` is 39% of all packed bytes in the checkout and the largest directory in it.

## (f) The current tree

| | |
|---|---:|
| Tracked files | 2,630 |
| Bytes | 92,948,487 |
| Distinct blobs | 2,172 |
| Bytes of distinct blobs | 91,029,029 |
| Packed, estimate | 12,112,316 |

[F1, F2] All files are regular: 2,617 at mode 100644, 13 at 100755, no symlink, no submodule, no `.gitattributes` [F8].

| Top level | Files | Bytes |
|---|---:|---:|
| `bench/` | 220 | 69,745,857 |
| `docs/` | 2,278 | 12,011,624 |
| `src/` | 21 | 6,519,973 |
| `tests/` | 37 | 4,133,080 |
| `scripts/` | 29 | 221,250 |
| root files | 10 | 100,526 |
| `web/` | 11 | 92,447 |
| `experiments/` | 11 | 51,132 |
| `.github/` | 10 | 46,840 |
| `examples/` | 3 | 25,758 |

[F5]

Files over 1,000,000 bytes: `src/db.rs` (3,433,010), `bench/src/engines.rs` (1,100,439), `docs/engine.md` (1,033,568) [F3]. No row file reaches that size; the largest is 872,130 bytes [F9].

Binary by content: one file, `tests/data/crypt-v1.bin`, 5,344 bytes [F4].

Generated files and fixtures that are committed [F6, F7, F10]:

| What | Files | Bytes |
|---|---:|---:|
| Rows, `bench/runs/**/*.json` | 130 | 67,067,767 |
| Figures, `docs/figures/` (2,271 SVG, 3 HTML; 1,817 distinct blobs) | 2,274 | 10,515,967 |
| Test data, `tests/data/` (four lists and the envelope on record) | 5 | 121,553 |
| `Cargo.lock`, four manifests | 4 | 34,247 |

Rows and figures are 2,404 of the 2,630 files and 83% of the bytes.

## (g) What a clone costs

| What is fetched | Refs | Commits | Objects | Bytes | Packed |
|---|---:|---:|---:|---:|---:|
| `main` alone | 1 | 32 | 4,349 | 205,116,470 | 19,664,736 |
| every branch, which is what a plain `git clone` takes | 12 | 278 | 13,513 | 731,568,765 | 27,181,503 |
| every branch and every pull request head | 49 | 2,023 | 32,503 | 2,789,515,508 | 77,651,678 |

[G1, H1] The first line is measured, not estimated: one of the three packs here is main at #36 and nothing else, 4,342 objects in 19,636,872 bytes [A5]. The other two lines are estimates.

Where the 77.7 MB lies:

| Reachable from | Objects | Packed | Share |
|---|---:|---:|---:|
| `main` | 4,349 | 19,664,736 | 25% |
| other branches, not main | 9,164 | 7,516,767 | 10% |
| pull request heads only | 18,990 | 50,470,175 | 65% |

Main's 19.7 MB is 19,079,750 of blobs, 138,735 of trees and 446,251 of commits [G4]. The rows are 8,876,320 of it.

The branches beyond main: the nine `bank/quick-*` hold 2,343 objects and 4,166,435 packed bytes, and their 2,244 blobs are 27 rows and 2,217 figure files [G5]; the two `claude/*` leftovers hold 7,313 objects and 5,681,955 packed bytes [G6]. 492 objects are in both sets.

The pull-only 50.5 MB is 48,023,140 of blobs, 718,598 of trees and 1,728,437 of commits [G2]. By directory: `scratch/` 30,533,069, `src/` 7,239,272, `tests/` 2,941,979, `docs/` 2,715,943, `bench/` 2,078,395, root files 1,746,330 [G3].

Pull request heads that hold more than 1 MB beyond main. The sets overlap, so the column does not add up. [G7]

| Pull request | Commits not on main | Objects not on main | Packed |
|---|---:|---:|---:|
| #6 | 691 | 6,610 | 34,551,432 |
| #35 | 630 | 12,741 | 18,256,017 |
| #36 | 632 | 12,627 | 15,524,841 |
| #27 | 361 | 10,302 | 9,961,808 |
| #26 | 353 | 10,171 | 9,749,293 |
| #25 | 384 | 9,966 | 9,504,001 |
| #23 | 344 | 9,597 | 8,895,340 |
| #22 | 339 | 9,435 | 8,460,527 |
| #33 | 49 | 2,490 | 5,034,897 |
| #21 | 177 | 6,331 | 4,696,362 |
| #31 | 90 | 949 | 3,579,493 |
| #30 | 83 | 1,063 | 3,313,881 |
| #1 | 329 | 3,209 | 2,724,986 |
| #32 | 92 | 945 | 2,232,820 |
| #29 | 40 | 461 | 1,859,298 |

## (h) Extremes

| | |
|---|---|
| Largest commit message | #35, `09cadec0`: 345,381 bytes. Then #36 (344,967), #27 (339,159), #6 (276,792). All four are on main. [M2, H3] |
| Commit bytes | Main's 32 commit objects are 1,875,181 bytes. All 2,023 are 5,555,268. Off main the median commit object is 1,652 bytes and the largest 30,834. [H4] |
| Widest tree, all refs | `scratch/wave2`, 979 entries, pull request heads only. [H5] |
| Widest tree on main | A machine's directory under `docs/figures/ooc/`, 280 entries. [H6] |
| Deepest path, all refs | 8 components, under `scratch/seam/`. [H7] |
| Deepest path, main's tree | 5 components, 2,274 files at that depth. The longest path is 143 characters. [H7, H8] |
| Merge commits | 62 over all refs, none on main. [H1] |

Commits by where they are reachable from [H1]:

| Class | Commits |
|---|---:|
| `main` | 32 |
| pull request heads only | 1,745 |
| a branch and a pull request head, not main | 236 |
| a branch only | 10 |
| all | 2,023 |

The ten on a branch only are the nine `bank/quick-*` commits and the one WIP commit at the tip of `claude/wal-ahead-beside-wip` [M10]. Every commit of `claude/e-recode-auto` is under a pull request head already [M10].

31 of main's 32 commits are also reachable from pull request heads, and the same 31 from the `bank/quick-*` branches. `b3a043dc` (#37) is the one commit on main alone [H2, H10].

30 of the 31 squash commits have the tree of their pull request's head, byte for byte. #15 is the one that differs [M11]. Six pull requests, #21 to #26, have no squash commit on main; 45 commits are reachable from their heads alone [H9].

## What bears on the three options

1. Size does not separate keeping history from re-messaging main. A re-message keeps every tree and blob. Main's commit objects are 446,251 packed bytes of main's 19,664,736, about 2%.
2. A re-message leaves the old commits in a plain clone unless the `bank/quick-*` branches go too. All of main's commits but #37 are reachable from `bank/quick-46050828509`, and from pull request heads.
3. No rewrite of main touches 65% of the packed bytes. They are reachable from pull request heads alone, and `scratch/` under #6, #35 and #36 is three fifths of that. A plain clone never fetches them.
4. A plain clone takes about 27.2 MB, and 7.5 MB of it is the eleven stale branches. Without them a plain clone is main's 19.7 MB. The 27 quick rows on the bank branches are on no other ref.
5. Rows name commits main does not hold. 126 of the 130 row files on main name commits that only pull request heads keep; a fresh repository has nothing for those names to resolve to, and this repository resolves them only while the host keeps the pull request heads. Only 2 rows on main, and the 27 on the bank branches, name main's own commits, which a re-message would change.
6. The floor for a fresh repository is the current tree: about 12.1 MB packed, 8.9 MB of it rows. Against a clone of main alone that saves about 7.6 MB.
7. A re-message can check itself against the pull request heads: 30 of the 31 squash trees equal their pull request's head tree.

## Not measured

- No repack was run. The size of a freshly packed clone of all refs is estimated, not measured.
- What the host stores beyond these refs: merge refs of pull requests, unreachable objects, forks.
- Pull request titles and descriptions, which a re-message would take its text from, are not in the repository.
- File contents were not read. Credentials and private data are another track's.
- Citations of commit hashes in the docs were not counted here, only the hashes in row file names.

## Commands

Run from `/home/user/supdb`. All are read-only. The setup block comes first; the rest can be run in any order after it.

```sh
# S: setup. $R is every fetched ref but this session's own branch, $B the branches among them, $P the pull heads; $D/all holds one line an object: id, type, bytes, packed bytes, path
R=$(git for-each-ref --format='%(refname)' refs/remotes | grep -v supdb-pretag-coordination)
B=$(git for-each-ref --format='%(refname)' refs/remotes/origin | grep -v supdb-pretag-coordination)
P=$(git for-each-ref --format='%(refname)' refs/remotes/pull)
D=$(mktemp -d)
git rev-list --objects $R > $D/paths
cut -c1-40 $D/paths | git cat-file --batch-check='%(objectname) %(objecttype) %(objectsize)' > $D/size
for i in .git/objects/pack/*.idx; do git verify-pack -v $i; done | awk '$2~/^(commit|tree|blob)$/{if(!($1 in d)||$4<d[$1])d[$1]=$4} END{for(k in d)print k,d[k]}' > $D/packed
awk 'FNR==1{f++} f==1{p[$1]=$2;next} f==2{o[$1]=$2" "$3;next} {print $1,o[$1],p[$1],substr($0,42)}' $D/packed $D/size $D/paths > $D/all
git rev-list --objects origin/main | cut -c1-40 > $D/main
git rev-list --objects $B | cut -c1-40 > $D/heads
git ls-tree -r origin/main | awk '{print $3}' | sort -u > $D/tree

# S2: refs in $R, $B, $P; tags; local main and origin/main
echo "$(echo "$R" | wc -l) $(echo "$B" | wc -l) $(echo "$P" | wc -l) $(git for-each-ref refs/tags | wc -l) $(git for-each-ref --format='%(objectname:short=8)' refs/heads/main refs/remotes/origin/main | tr '\n' ' ')"
# A1
git count-objects -vH | tr '\n' ' '; echo
# A2: type count bytes packed
awk '{n[$2]++;s[$2]+=$3;d[$2]+=$4} END{for(t in n)print t,n[t],s[t],d[t]}' $D/all | sort
# A3: pack files
ls -l .git/objects/pack/*.pack | awk '{print $5}' | tr '\n' ' '; echo
# A4: distinct objects, objects held twice
for i in .git/objects/pack/*.idx; do git verify-pack -v $i; done | awk '$2~/^(commit|tree|blob)$/{c[$1]++} END{for(k in c){n++; if(c[k]>1)d++} print n,d}'
# A5: pack a14cee06 against main at #36
git verify-pack -v .git/objects/pack/pack-a14cee06*.idx | awk '$2~/^(commit|tree|blob)$/{print $1}' | sort > $D/pk; git rev-list --objects e40d6283 | cut -c1-40 | sort > $D/m36; echo "$(wc -l < $D/pk) in pack, $(wc -l < $D/m36) reachable from e40d6283, $(comm -3 $D/pk $D/m36 | wc -l) differ"

# B: top 40 blobs: bytes packed class blob path
awk 'FNR==1{f++} f==1{t[$1]=1;next} f==2{m[$1]=1;next} f==3{h[$1]=1;next} $2=="blob"{print $3,$4,(($1 in t)?"tree":(($1 in m)?"main-history":(($1 in h)?"branch":"pull-only"))),$1,$5}' $D/tree $D/main $D/heads $D/all | sort -k1,1nr -k4,4 | head -40 > $D/top40; awk '{print $1,$2,$3,substr($4,1,10),$5}' $D/top40
# B2: introducing commit of each
while read z p c s f; do echo "${s:0:10} $(git log $R --reverse -m --find-object=$s --format='%h %cs' | head -1)"; done < $D/top40
# B4: refs that hold a commit B2 names (here the first)
git for-each-ref --contains 84cc340e --format='%(refname:short)' refs/remotes | tr '\n' ' '; echo
# B5: the squash commit that put a blob on main (here the current src/db.rs)
git log origin/main --reverse --find-object=$(git rev-parse origin/main:src/db.rs) --format='%h %s' | head -1 | grep -o '^[0-9a-f]* \|(#[0-9]*)$' | tr -d '\n'; echo
# B3: paths by version bytes: bytes versions packed path
awk '$2=="blob"{n[$5]++;s[$5]+=$3;d[$5]+=$4} END{for(p in n)print s[p],n[p],d[p],p}' $D/all | sort -rn | head -10
# B3m: same, reachable from main
awk 'NR==FNR{m[$1]=1;next}($1 in m)&&$2=="blob"{n[$5]++;s[$5]+=$3;d[$5]+=$4} END{for(p in n)print s[p],n[p],d[p],p}' $D/main $D/all | sort -rn | head -8

# C1: class dir blobs bytes packed
awk 'NR==FNR{m[$1]=1;next} $2=="blob"{n=split($5,a,"/");k=(n>1?a[1]"/":"(root)");c=($1 in m)?"main":"other";N[c" "k]++;Z[c" "k]+=$3;P[c" "k]+=$4} END{for(k in N)print k,N[k],Z[k],P[k]}' $D/main $D/all | sort -k1,1 -k4,4nr
# C2: second level
awk 'NR==FNR{m[$1]=1;next} $2=="blob"{n=split($5,a,"/");if(a[1]!~/^(bench|docs|src|tests|scratch)$/)next;k=a[1]"/"(n>2?a[2]"/":"*");c=($1 in m)?"main":"other";N[c" "k]++;Z[c" "k]+=$3;P[c" "k]+=$4} END{for(k in N)print k,N[k],Z[k],P[k]}' $D/main $D/all | sort -k1,1 -k2,2

# D1: rows in tree by scale: dir files bytes
git ls-tree -r -l origin/main -- bench/runs | awk '{split($5,a,"/");n[a[3]]++;s[a[3]]+=$4} END{for(k in n)print k,n[k],s[k]}' | sort
# D2: row versions in history: ref blobs bytes packed
for r in 8025423 origin/main; do git rev-list --objects $r | cut -c1-40 > $D/x; echo "$r $(awk 'NR==FNR{m[$1]=1;next}($1 in m)&&$2=="blob"&&$5~/^bench\/runs\//{n++;s+=$3;d+=$4} END{print n,s,d}' $D/x $D/all)"; done; echo "all-refs $(awk '$2=="blob"&&$5~/^bench\/runs\//{n++;s+=$3;d+=$4} END{print n,s,d}' $D/all)"
# D3: row file names by where the commit they name resolves
git rev-list origin/main > $D/mc; git rev-list $P > $D/pc; git rev-list $R > $D/ac; git ls-tree -r --name-only origin/main -- bench/runs | sed 's/.*-\([0-9a-f]*\)\.json$/\1/' | while read h; do if grep -q "^$h" $D/mc; then echo main; elif grep -q "^$h" $D/pc; then echo pull-heads; elif grep -q "^$h" $D/ac; then echo branch-only; else echo absent; fi; done | sort | uniq -c | tr '\n' ' '; echo

# E1: scratch in main history
git rev-list --objects origin/main | awk '{print $2}' | grep -c '^scratch\(/\|$\)'
# E2: scratch by ref: ref blobs bytes packed tipfiles
for r in $R; do git rev-list --objects $r | cut -c1-40 > $D/x; echo "${r#refs/remotes/} $(awk 'NR==FNR{m[$1]=1;next}($1 in m)&&$2=="blob"&&$5~/^scratch\//{n++;s+=$3;d+=$4} END{print n+0,s+0,d+0}' $D/x $D/all) $(git ls-tree -r $r -- scratch | wc -l)"; done | awk '$2>0||$5>0'
# E3: scratch all refs
awk '$2=="blob"&&$5~/^scratch\//{n++;s+=$3;d+=$4} END{print n,s,d}' $D/all
# E4: scratch/wave2 by extension: packed files bytes ext
awk '$2=="blob"&&$5~/^scratch\/wave2\//{e=$5;sub(/.*\//,"",e);if(e~/\./)sub(/.*\./,".",e);else e="(none)";n[e]++;s[e]+=$3;d[e]+=$4} END{for(k in n)print d[k],n[k],s[k],k}' $D/all | sort -rn | head -3

# F1: tree files bytes
git ls-tree -r -l origin/main | awk '{n++;s+=$4} END{print n,s}'
# F2: tree distinct blobs bytes packed
awk 'NR==FNR{t[$1]=1;next}($1 in t){n++;s+=$3;d+=$4} END{print n,s,d}' $D/tree $D/all
# F3: over 1,000,000 bytes
git ls-tree -r -l origin/main | awk '$4>1000000{print $4,$5}' | sort -rn
# F4: binary by content
git ls-tree -r --name-only origin/main | sort > $D/names; git grep -I --name-only -e '' origin/main | sed 's/^origin\/main://' | sort | comm -23 $D/names -
# F5: by top level: bytes files dir
git ls-tree -r -l origin/main | awk '{n=split($5,a,"/");k=(n>1?a[1]"/":"(root)");c[k]++;s[k]+=$4} END{for(k in c)print s[k],c[k],k}' | sort -rn
# F6: generated: rows, figures, fixtures, locks: files bytes
for p in bench/runs docs/figures tests/data; do echo "$p $(git ls-tree -r -l origin/main -- $p | awk '{n++;s+=$4} END{print n,s}')"; done; echo "Cargo.lock $(git ls-tree -r -l origin/main | awk '$5~/Cargo\.lock$/{n++;s+=$4} END{print n,s}')"
# F9: largest row file
git ls-tree -r -l origin/main -- bench/runs | sort -k4,4nr | head -1 | awk '{print $4,$5}'
# F10: figure files by extension
git ls-tree -r --name-only origin/main -- docs/figures | sed 's/.*\.//' | sort | uniq -c | tr '\n' ' '; echo
# F7: figures distinct blobs
git ls-tree -r origin/main -- docs/figures | awk '{print $3}' | sort -u | wc -l
# F8: modes, symlinks, attributes
git ls-tree -r origin/main | awk '{print $1}' | sort | uniq -c | tr '\n' ' '; git ls-tree -r --name-only origin/main | grep -c gitattributes

# G1: set objects bytes packed
for f in main heads; do awk -v f=$f 'NR==FNR{m[$1]=1;next}($1 in m){n++;s+=$3;d+=$4} END{print f,n,s,d}' $D/$f $D/all; done; awk '{n++;s+=$3;d+=$4} END{print "all",n,s,d}' $D/all
# G2: not in branches (pull refs only) by type
awk 'NR==FNR{m[$1]=1;next}!($1 in m){n[$2]++;s[$2]+=$3;d[$2]+=$4} END{for(t in n)print t,n[t],s[t],d[t]}' $D/heads $D/all | sort
# G3: pull-only blobs by top level: packed blobs bytes dir
awk 'NR==FNR{m[$1]=1;next}!($1 in m)&&$2=="blob"{n=split($5,a,"/");k=(n>1?a[1]"/":"(root)");N[k]++;Z[k]+=$3;P[k]+=$4} END{for(k in N)print P[k],N[k],Z[k],k}' $D/heads $D/all | sort -rn | head -6
# G4: main by type
awk 'NR==FNR{m[$1]=1;next}($1 in m){n[$2]++;s[$2]+=$3;d[$2]+=$4} END{for(t in n)print t,n[t],s[t],d[t]}' $D/main $D/all | sort
# G5: bank branches beyond main: objects bytes packed; blobs; rows, their bytes and packed; figure files
git rev-list --objects $(echo "$B" | grep /bank/) ^origin/main | cut -c1-40 > $D/x; awk 'NR==FNR{m[$1]=1;next}($1 in m){n++;s+=$3;d+=$4;if($2=="blob"){bn++;if($5~/^bench\/runs\//){rn++;rs+=$3;rd+=$4};if($5~/^docs\/figures\//)fn++}} END{print n,s,d,"blobs",bn,"rows",rn,rs,rd,"figures",fn}' $D/x $D/all
# G6: claude leftovers beyond main: objects bytes packed
git rev-list --objects origin/claude/e-recode-auto origin/claude/wal-ahead-beside-wip ^origin/main | cut -c1-40 > $D/x; awk 'NR==FNR{m[$1]=1;next}($1 in m){n++;s+=$3;d+=$4} END{print n,s,d}' $D/x $D/all

# H1: commits: all main pull-not-main branches-not-main branches-not-main-not-pull pull-only merges
echo "$(git rev-list --count $R) $(git rev-list --count origin/main) $(git rev-list --count $P ^origin/main) $(git rev-list --count $B ^origin/main) $(git rev-list --count $B ^origin/main --not $P) $(git rev-list --count $P --not $B) $(git rev-list --count --merges $R)"
# H2: main commits on no other ref
git rev-list origin/main --not $P $(echo "$B" | grep -v 'origin/main$') | cut -c1-8
# H3: largest commit objects
awk '$2=="commit"{print $3,substr($1,1,8)}' $D/all | sort -rn | head -4 | tr '\n' ' '; echo
# H4: commit bytes: main, all; median and max off main
awk 'NR==FNR{m[$1]=1;next}$2=="commit"{if($1 in m)a+=$3;else print $3; t+=$3} END{print "main",a,"all",t > "/dev/stderr"}' $D/main $D/all 2>&1 >$D/offmain | cat; sort -n $D/offmain | awk '{a[NR]=$1} END{print a[int((NR+1)/2)],a[NR],NR}'
# H5: widest tree: entries path (largest tree object)
awk '$2=="tree"{print $3,$1,$5}' $D/all | sort -rn | head -1 | while read z s p; do echo "$(git ls-tree $s | wc -l) $p"; done
# H6: widest tree on main
awk 'NR==FNR{m[$1]=1;next}($1 in m)&&$2=="tree"{print $3,$1,$5}' $D/main $D/all | sort -rn | head -1 | while read z s p; do echo "$(git ls-tree $s | wc -l) $p"; done
# H7: deepest path: all refs, main tree (components, files at that depth)
awk '$5!=""{n=split($5,a,"/");if(n>m){m=n;p=$5}} END{print m,p}' $D/all; git ls-tree -r --name-only origin/main | awk '{n=split($0,a,"/");c[n]++;if(n>m)m=n} END{print m,c[m]}'
# H8: longest path in main tree (characters)
git ls-tree -r --name-only origin/main | awk '{if(length($0)>m)m=length($0)} END{print m}'

# M1: main commits, with PR number
git rev-list --count origin/main; git log --format=%s origin/main | grep -c '(#[0-9]*)$'
# M2: message bytes per main commit, top 9
git rev-list origin/main | while read c; do echo "$(git log -1 --format=%B $c | wc -c) $(git log -1 --format=%s $c | grep -o '(#[0-9]*)$')"; done | sort -rn | head -9 | tr '\n' ' '; echo
# M3: message bytes on main
git log --format=%B origin/main | wc -c
# M4: diffstats
for c in 8025423 e40d6283 09cadec0 7650d890; do echo "$c$(git diff --shortstat $c^ $c)"; done
# M5: first body line of the nine newest
git rev-list -9 origin/main | while read c; do echo "$(git log -1 --format=%b $c | head -1 | md5sum | cut -c1-8) $(git log -1 --format=%s $c | grep -o '(#[0-9]*)$')"; done | tr '\n' ' '; echo
# M6: squash items repeated from an older main commit: commit items repeated
git rev-list --reverse origin/main | while read c; do git log -1 --format=%b $c | grep '^\* ' | sed "s/^/$c /"; done | awk '{c=$1;$1="";n[c]++;N++;if(($0 in f)&&f[$0]!=c){r[c]++;T++};if(!($0 in f))f[$0]=c} END{for(c in n)if(r[c]>0)print substr(c,1,8),n[c],r[c];print "total",N,T}' | sort
# M7: trailers on main: Co-Authored-By, Claude-Session, distinct links, co-author lines in any case
echo "$(git log --format=%B origin/main | grep -c '^Co-Authored-By:') $(git log --format=%B origin/main | grep -c '^Claude-Session:') $(git log --format=%B origin/main | grep -o 'claude\.ai/code/session_[A-Za-z0-9]*' | sort -u | wc -l) $(git log --format=%B origin/main | grep -ci '^co-authored-by:')"
# M8: same over seven newest at 8025423, seven newest at main, all refs
for a in "-7 8025423" "-7 origin/main" "$R"; do echo "$(git log $a --format=%B | grep -c '^Co-Authored-By:') $(git log $a --format=%B | grep -c '^Claude-Session:') $(git log $a --format=%B | grep -o 'claude\.ai/code/session_[A-Za-z0-9]*' | sort -u | wc -l)"; done
# M9: branches against main: name ahead behind
for r in $(echo "$B" | grep -v 'origin/main$'); do echo "${r#refs/remotes/origin/} $(git rev-list --count origin/main..$r) $(git rev-list --count $r..origin/main)"; done
# M10: claude leftovers: commits in no pull ref
for r in origin/claude/e-recode-auto origin/claude/wal-ahead-beside-wip; do echo "$r $(git rev-list --count $r ^origin/main --not $P)"; done
# M11: squash trees equal to their pull head
git log --format='%T %s' origin/main | while read t s; do n=$(echo "$s" | grep -o '(#[0-9]*)$' | tr -d '(#)'); if [ -n "$n" ]; then if [ "$(git rev-parse refs/remotes/pull/$n^{tree})" = "$t" ]; then echo same; else echo "differs#$n"; fi; fi; done | sort | uniq -c | tr '\n' ' '; echo
# G7: pull heads beyond main: number commits objects packed
for r in $P; do git rev-list --objects $r ^origin/main | cut -c1-40 > $D/x; echo "${r##*/} $(git rev-list --count $r ^origin/main) $(awk 'NR==FNR{m[$1]=1;next}($1 in m){n++;d+=$4} END{print n+0,d+0}' $D/x $D/all)"; done | sort -n
# H9: commits held only by pull heads #21-#26, which have no squash on main
M=$(for n in $(git log --format=%s origin/main | grep -o '(#[0-9]*)$' | tr -d '(#)'); do echo refs/remotes/pull/$n; done); git rev-list --count $(for n in 21 22 23 24 25 26; do echo refs/remotes/pull/$n; done) --not origin/main $M
# H10: main commits not under a pull head; not under a bank/quick-* branch
echo "$(git rev-list origin/main --not $P | cut -c1-8) $(git rev-list origin/main --not $(echo "$B" | grep /bank/) | cut -c1-8)"
# D4: row blobs off main, by where they are held
git rev-list --objects $P | cut -c1-40 > $D/x; awk 'FNR==1{f++} f==1{m[$1]=1;next} f==2{p[$1]=1;next} $2=="blob"&&$5~/^bench\/runs\//&&!($1 in m){n[($1 in p)?"pull-heads":"branches-only"]++} END{for(k in n)print k,n[k]}' $D/main $D/x $D/all
```
