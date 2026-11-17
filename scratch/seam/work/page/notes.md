# E9 ("page"): results, written as the runs land

Tree 3df85d22, clean before every timed run (sit.sh writes `git status --porcelain` into each `*-head.txt`).
W = this directory; O = ../once. Tools: sit.sh (O's, with BIN from env, storesize.py beside the sampler,
the busy check also matching bench-page|bench-p4080|target/release/bench, and a `pagerlayout` grep after each
sitting), sampler.py/phasemem.py (O's, unchanged), report.py (O's), storesize.py (new: apparent/allocated
bytes by kind every 5 s; resident.py every 30 s over a clear store).

Deviation (all runs): sit.sh runs two sittings (A,B then B,A); for a self-pair that is four fresh-process
readings, not the two the plan counted. Kept: it costs 3.5 min a bound and gives four readings.

Environment note: dmesg shows `idle-reclaim (85): drop_caches: 1` periodically (host side, between runs);
each sitting's dmesg window is saved in pairs/*.dmesg.

## A. H1: the clear store under reduced bounds (bench-once, supdb-nozpages,supdb-nozpages)

Commands (from W/pairs):
  BIN=$O/bench-once sh $W/sit.sh h1-473 supdb-nozpages supdb-nozpages 5000000 1 --bound 495976448 --passes read --ops-div 1
  BIN=$O/bench-once sh $W/sit.sh h1-328 supdb-nozpages supdb-nozpages 5000000 1 --bound 343932928 --passes read --ops-div 1
    (sitting 2 was killed by MY tool timeout at 590 s mid-read; its partial files are in pairs/aborted/. Not a result.
     Sitting 1's two readings agree within 2% in faults a read, so by the plan's rule no second sitting was run.)
  BIN=$O/bench-once sh $W/sit.sh h1-575 supdb-nozpages supdb-nozpages 5000000 1 --bound 602931200 --passes read --ops-div 1
All rc 0, no OOM, no leftover group. Run in the background from here on (nohup), waited on by pgrep.

S, measured (storesize.py, every clear store, stable through the read pass): the 19 partitions are
715,847,504 apparent bytes = **682.7 MiB** (allocated 715.9 MB); the ordered indexes 40,001,292 bytes = 38.1 MiB
(not read by point reads: resident.py reads them at 0 MiB in every read pass). Predicted 640-690: inside.
So the line's derived 660 is 3% under the measured 683.

C measured = resident.py's `par` resident MiB over the store's directory (mincore), read pass, steady value
(each reading named by store i=0/1). faults = `read majfaults_per_op`.

| bound MiB | sitting/reading | C (par resident MiB) | cache-shmem 2nd half (sampler, includes text) | faults a read | reads/s | read_us_per_io |
|---|---|---|---|---|---|---|
| 328 | 1/i0 | 271 | 279 (group) | 0.748 | 32,657 | 31.1 |
| 328 | 1/i1 | 278 | | 0.763 | 31,474 | 31.7 |
| 473 | 1/i0 | 417 | 417 | 0.468 | 47,693 | 33.0 |
| 473 | 1/i1 | 417 | | 0.468 | 48,216 | 32.9 |
| 473 | 2/i0 | 416 | 413 | 0.476 | 48,177 | 32.2 |
| 473 | 2/i1 | 413 | | 0.470 | 48,695 | 32.4 |
| 575 | 1/i0 | 526 | 518 | 0.277 | 75,757 | 34.2 |
| 575 | 1/i1 | 517 | | 0.262 | 80,975 | 33.2 |
| 575 | 2/i0 | 519 | 527 | 0.260 | 83,201 | 32.7 |
| 575 | 2/i1 | 527 | | 0.275 | 74,877 | 34.4 |
(The sampler's cache-shmem is one figure a sitting, the group's; within a sitting the two readings are different
processes in different windows -- report.py joins by arm name, which is the same for both, so the per-reading
sampler figure is not separated. resident.py's figure is per store and is the one compared.)

RssAnon 36-48 MiB in every read pass; failcnt rises 1.3-3.8M a pass (continuous reclaim, as at 640).

### H1 against the registered predictions and the refutation band
| point | cipher measured (E8) | clear measured here | difference | band (max(15%, 0.04)) | verdict |
|---|---|---|---|---|---|
| C ~ 275 | 0.684 at held 273-278 | 0.748-0.763 at 271-278 | +0.064..+0.079 (+9..+12%) | 0.103 | inside: H1 holds |
| C ~ 415-420 | 0.428 at held 417-431 (~0.43 at 415) | 0.468-0.476 at 413-417 | +0.04..+0.05 (+9..+11%) | 0.064 | inside: H1 holds |
| C ~ 520 (no cipher point yet) | -- | 0.260-0.277 at 517-527 | predicted 0.21-0.29 | | inside the prediction |
Predicted rates: 473 -> 50-60k (measured 47.7-48.7k, just under); 328 -> 33-40k (31.5-32.7k, just under);
575 -> 80-95k (74.9-83.2k, low end). Predicted faults: 0.36-0.49 (0.468-0.476 in), 0.58-0.79 (0.748-0.763 in),
0.21-0.29 (0.260-0.277 in).

Reading: the clear store at the same cached bytes takes 9-12% MORE faults a read than the cipher store takes
misses, at both comparison points, inside the band on the "clear higher" side: the kernel's LRU is no better
per MiB than the pager's megabyte clock on uniform reads; the "uncounted factor of two" in once.md is not there.
The single line does not fit with one k (derived): k = m/(1 - C/682.7) is 1.27 / 1.20 / 1.14 for clear at
C = 275 / 415 / 522, and 1.15 / 1.12 for the cipher at 275 / 421. So the curve is convex in C, not linear;
the cipher's points sit ~9% under the clear curve. For H2's predictions use the clear curve interpolated
(conservative) or the cipher's own k ~1.12-1.15 with S = 682.7 (derived):
  held 480 -> 1.12-1.15 x (1-480/682.7) = 0.33-0.34; 520 -> 0.27-0.27; 550 -> 0.22-0.22 (cipher k);
  clear curve at 480/520/550 interpolated -> ~0.37 / ~0.28 / ~0.22.
These are restated beside the plan's 0.32 / 0.25 / 0.20 before any H2 run is read: the plan's values are
5-10% optimistic at 480-520 (derived).

Side result (derived): at C ~415 the clear store reads 48.2k/s = 20.7 us a read at 0.47 faults x ~32.6 us
device = 15.3 us, leaving ~5.4 us non-device; b432 (E8) read 20.4 us at 0.422 faults x ~33 = 13.9, leaving
~6.5 us non-device a read, i.e. ~1.1 us more than clear spread over 0.428 misses = ~2.6 us a miss above the
clear store's own fault handling (derived, cross-session).

## B. The scratch patch (page.patch), tests, binaries

Deviation from §2.2 (method, not content): instead of `git apply once.patch` and stripping by hand, the patch
was written as `W/edits.py` -- exact-match string replacements, each asserted to match once -- run over the tree.
It carries only what §2.2 says to keep (the dynamic budget `new_with(limit, dyn_pct)` with the 100 ms recompute
from `zfile::read_budget()`, `hard`/`swamp` derived from the current limit via `unit_bytes`, `limit_lo/hi`,
SUPDB_PAGER_TRACE's `pagertrace` line without direct/runchunks/nowait), and none of the O_DIRECT path. The
acceptance grep over the diff's added lines names only `nonce` (substring "once"). It was dry-run first on copies
in W/stage while H1 timed, so the tree stayed clean during H1.
Content as §2.3-2.8: envelope v2 (`Header.apart`, `new_apart`, VERSION_APART=2, `tag_off`, `stored_end`,
`chunk_plain_len` pub(crate), `encrypt_tags_apart[_with_id]` writing data in MiB pieces then the tag region;
`Encrypted::open_with` and `decrypt` refuse v2 "an envelope with its tags apart is a store's file, read through its
cache"); pager `PAGE_LAYOUT`, CHUNK, Back::open refusing shift/layout mismatch, `fill` reading ciphertext at
stored_off and the tag at tag_off through the mapping, `resident`/`stored_pages` by `stored_end`, a `pagerlayout`
line once a process under SUPDB_PAGER_TRACE; vault `write_envelope_at` -> `encrypt_tags_apart` under PAGE_LAYOUT,
`scratch_dynamic`; engines arms x432/x480/x520/x550 (fixed) and xdyn94/xdyn85 with `cipher_limit_lo/hi` rows;
run.rs `SUPDB_MIX_ONLY` (a repeated letter -> `ycsb-C#2`, printed as a `mixagain` line, not pushed to the row).
Tests edited (CHUNK instead of 4080) and three new: crypt `scratch_a_header_with_its_tags_apart_round_trips`,
`scratch_an_envelope_with_its_tags_apart_is_refused_by_the_public_reader`, pager
`a_changed_tag_in_the_tag_region_fails_its_chunk` (chunk 3's tag, and the file's last byte = last chunk's tag).

Tests (checked profile, TMPDIR=W/tmp, each after --list showed its tests; outputs in W/tests/):
| run | result |
|---|---|
| --lib crypt (27 incl. the 2 new) | pass |
| --lib db::pager (11 incl. the new) --test-threads=4 | pass |
| --lib db::vault (12) | pass |
| --test cipher (24) | pass (the known flake did not show) |
| --test crypt (7 + 1 ignored, as on the clean tree) -- v1 unchanged | pass |
| --test dirkey (2 + 4 ignored: need scripts/fscrypt.sh) | pass |
| --test zpages cipher (5) | pass |
| fuzz_range SUPDB_PAGER_GUARD=1 ZFUZZ_CIPHER=1 ZFUZZ_CIPHERCACHE=16384 seeds 0-200 | pass, 85 s |
| fuzz_range_under_a_cipher_with_the_compact_form | pass |
| busy: same fuzz, seeds 200-600 as 4 processes x 100, own TMPDIRs | 4/4 pass, 58-61 s each |
| node web/crypt.test.mjs | 5413 checks passed |
| PAGE_LAYOUT=false: --lib db::pager (11), --test cipher (24) | pass |
No failure of either kind; nothing to triage.

Binaries: bench-page (PAGE_LAYOUT=true), bench-p4080 (false); page-4080.patch differs from page.patch in that one
line (diff checked). Tree reverted with `git checkout --` of the five files: `git status --porcelain` empty, HEAD
3df85d22, before any timed run below.

Sanity (W/sanity):
1. bench-page `supdb-cipherxdyn94,supdb-nozpages` 100k, read,scan, unbounded: rc 0, `pagerlayout chunk 4096 apart
   true unit 1048576`, cipher_fills 0.
2. As planned (`supdb-cipherxdyn94,supdb-cipherx480` 1M under 256 MiB, load,read): rc 1, x480 KILLED -- a fixed
   480 MiB budget under a 256 MiB bound; a flaw of the plan's sanity line, not of the patch (kept in
   sanity/x480killed/). Rerun as `supdb-cipherxdyn94,supdb-cipherxdyn85`: bench-page rc 0, fills 27,815/34,062,
   evicted units 184/236, no poison or error; bench-p4080 rc 0, `pagerlayout chunk 4080 apart false unit 1044480`,
   fills 27,737/35,391. Note: cipher_limit_hi read 299-332 MB under a 256 MiB bound (read_budget's reading at some
   moment of the load; not chased).

## C. The read pass, 5M keys under 640 MiB (--bound 671088640 --passes read --ops-div 1)

Commands (W/chain1.sh, run from W/pairs, one at a time, tree clean checked first):
  BIN=$W/bench-p4080 sh $W/sit.sh r0 supdb-cipher supdb-nozpages 5000000 1 $P
  BIN=$W/bench-page  sh $W/sit.sh {xdyn94,x520,x480} supdb-cipher{..} supdb-nozpages 5000000 1 $P
  BIN=$W/bench-page  sh $W/sit.sh pdef supdb-cipher supdb-nozpages 5000000 1 $P
  BIN=$W/bench-page  sh $W/sit.sh {x550,xdyn85,x432} supdb-cipher{..} supdb-nozpages 5000000 1 $P
Every sitting rc 0; every dmesg window empty (no OOM, no drop_caches); no leftover group; every sitting's
`pagerlayout` line names its binary's layout (4080/apart false for r0, 4096/apart true for the rest).
Sitting 1 = cipher arm first, sitting 2 = clear first. Full table: W/tab-read.txt (summary.py).

Columns: misses = cipher_fills / 5M; majflt = the cipher arm's `read majfaults_per_op`; held = pagertrace median over
the read pass's second half; shmem = sampler (group) median over the second half / max over the pass; shmem/held =
sampler shmem joined to pagertrace held within 1 s, median; cache-shmem = page cache that is not shmem, 2nd half
median; usage med/max over the pass; limit = pagertrace median (counts' lo..hi for the whole process).

| arm (bin) | k | reads/s | clear r/s | xclear | misses | majflt | held | shmem med/max | shmem/held | cache-shmem | usage med/max | RssAnon | limit (lo..hi) | peak | breaths | failcnt+ |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| supdb-cipher (p4080) | 1 | 44,170 | 142,223 | 0.311 | 0.684 | 0.443 | 274 | 338/349 | 1.228 | 233 | 635/640 | 47 | 280 | 314 | 0 | 2.19M |
| supdb-cipher (p4080) | 2 | 43,194 | 154,017 | 0.280 | 0.683 | 0.425 | 274 | 337/352 | 1.229 | 244 | 634/640 | 38 | 280 | 314 | 0 | 2.11M |
| supdb-cipher (page) | 1 | 53,332 | 149,438 | 0.357 | 0.685 | 0.363 | 273 | 274/280 | 1.001 | 309 | 637/640 | 38 | 280 | 314 | 0 | 1.77M |
| supdb-cipher (page) | 2 | 53,195 | 153,889 | 0.346 | 0.684 | 0.360 | 273 | 275/280 | 1.008 | 310 | 638/640 | 38 | 280 | 314 | 0 | 1.76M |
| x432 | 1 | 52,262 | 155,163 | 0.337 | 0.429 | 0.398 | 426 | 422/432 | 0.995 | 151 | 638/640 | 46 | 432 | 456 | 0 | 1.98M |
| x432 | 2 | 52,799 | 137,976 | 0.383 | 0.430 | 0.397 | 423 | 422/432 | 0.999 | 160 | 640/640 | 38 | 432 | 456 | 0 | 1.98M |
| xdyn85 | 1 | 55,545 | 150,266 | 0.370 | 0.397 | 0.376 | 440 | 441/450 | 1.004 | 141 | 640/640 | 38 | 450 (280..475) | 472 | 0 | 1.87M |
| xdyn85 | 2 | 54,852 | 153,499 | 0.357 | 0.399 | 0.379 | 441 | 437/450 | 0.996 | 142 | 640/640 | 38 | 450 (280..475) | 472 | 0 | 1.88M |
| x480 | 1 | 61,850 | 148,627 | 0.416 | 0.344 | 0.336 | 470 | 469/480 | 0.999 | 106 | 640/640 | 46 | 480 | 502 | 0 | 1.70M |
| x480 | 2 | 63,869 | 134,824 | 0.474 | 0.343 | 0.334 | 469 | 472/480 | 0.998 | 103 | 640/640 | 47 | 480 | 502 | 0 | 1.68M |
| xdyn94 | 1 | 66,194 | 146,540 | 0.452 | 0.314 | 0.309 | 485 | 485/498 | 1.003 | 94 | 640/640 | 38 | 498 (280..526) | 535 | 0 | 1.56M |
| xdyn94 | 2 | 62,510 | 139,988 | 0.447 | 0.314 | 0.309 | 490 | 484/498 | 0.994 | 93 | 635/640 | 38 | 498 (280..526) | 535 | 0 | 1.55M |
| x520 | 1 | 72,908 | 151,880 | 0.480 | 0.273 | 0.272 | 510 | 505/520 | 0.988 | 63 | 637/640 | 46 | 520 | 539 | 0 | 1.37M |
| x520 | 2 | 74,936 | 151,387 | 0.495 | 0.275 | 0.273 | 508 | 512/520 | 1.009 | 74 | 640/640 | 38 | 520 | 539 | 0 | 1.35M |
| x550 | 1 | 87,630 | 140,628 | 0.623 | 0.219 | 0.220 | 540 | 541/550 | 0.996 | 37 | 640/640 | 44 | 550 | 577 | 0 | 1.09M |
| x550 | 2 | 82,719 | 147,258 | 0.562 | 0.223 | 0.224 | 542 | 541/550 | 0.991 | 35 | 639/640 | 46 | 550 | 577 | 0 | 1.13M |
oom_kill 0 in every sitting. The clear arm took 0.138-0.158 faults a read throughout (its usual 0.14).

Mechanism checks:
- shmem/held 0.988-1.009 for every whole-page arm, 1.228-1.229 for the 4080 control: the straddle is gone, and
  the 1.22-1.27x of E8 was the straddle (now shown: one constant moved it, all else equal).
- The tags are read: every whole-page fill reads its tag at tag_off through the mapping (the only path; the unit
  test that flips a tag-region byte fails its chunk). The envelopes' sizes are the same formula in both layouts
  (stored_len unchanged), checked by checked_header at every open.
- The arms differ in budget as intended: limit medians 280 / 432 / 450 (dyn85) / 480 / 498 (dyn94) / 520 / 550.
- No text thrash: majflt <= misses + 0.01 in every arm (majflt is 0.53x of misses for the default budget, where
  309 MiB of ciphertext stays in the page cache, and 0.92-1.0x from x432 up).
- breaths 0 everywhere; peak / limit: 1.12 (default 314/280), 1.06 (x432), 1.05 (xdyn85 vs 450), 1.05 (x480),
  1.07 (xdyn94 535 vs 498 median; 1.02 vs its hi 526), 1.04 (x520), 1.05 (x550).
- usage median 635-640 in every arm, the control included: the plan's BEST criterion "usage median under 630"
  excludes every arm (mapping-read arms sit at the limit by design, the page cache filling what shmem leaves).
  Not applied; see BEST below.

Per read (derived: us/read = 1e6/rate; device = majflt x read_us_per_io of the arm; non-device = the rest;
per miss = (non-device - 2.4) / misses):
| arm | us/read | device | non-device | non-device a miss | all a miss |
|---|---|---|---|---|---|
| supdb-cipher p4080 | 22.6-23.2 | 13.9-14.0 | 8.8-9.1 | 9.3-9.9 | 29.6-30.4 |
| supdb-cipher page | 18.8 | 11.7-12.1 | 6.6-7.1 | 6.2-6.9 | 23.9-24.0 |
| x432 | 18.9-19.1 | 12.6-12.9 | 6.3 | 9.0-9.1 | 38.5-39.0 |
| xdyn85 | 18.0-18.2 | 12.2-12.3 | 5.8-5.9 | 8.7-8.8 | 39.3-39.7 |
| x480 | 15.7-16.2 | 10.5-10.9 | 5.2-5.3 | 8.2-8.4 | 38.7-40.0 |
| xdyn94 | 15.1-16.0 | 10.0-10.7 | 5.2-5.3 | 8.8-9.3 | 40.5-43.3 |
| x520 | 13.3-13.7 | 8.9-9.0 | 4.4-4.7 | 7.3-8.4 | 39.8-41.5 |
| x550 | 11.4-12.1 | 7.2-7.6 | 4.2-4.5 | 8.3-9.2 | 41.1-43.4 |
The "non-device a miss" mixes in the misses served from cached ciphertext (no device wait), so it is a ceiling.

### Predictions (§4, registered) beside results
Run 4 (control, p4080): 40-45k -> 43.2-44.2k IN; 0.28-0.33x -> 0.280-0.311 IN; misses 0.684+-0.01 -> 0.683-0.684 IN;
shmem/held 1.21-1.26 -> 1.228-1.229 IN. The patch did not move the 4080 path.

| arm | held pred / meas | misses pred / meas | shmem/held pred / meas | reads/s pred / meas | xclear pred / meas |
|---|---|---|---|---|---|
| default | 265-285 / 273 IN | 0.66-0.71 / 0.684-0.685 IN | 0.99-1.03 / 1.001-1.008 IN | 41-47k / 53.2-53.3k ABOVE | 0.28-0.33 / 0.346-0.357 ABOVE |
| x432 | 425-435 / 423-426 edge | 0.42-0.44 / 0.429-0.430 IN | IN (0.995-0.999) | 50-56k / 52.3-52.8k IN | 0.34-0.39 / 0.337-0.383 IN |
| x480 | 470-482 / 469-470 edge | 0.30-0.34 / 0.343-0.344 just over | IN | 63-72k / 61.9-63.9k low edge | 0.42-0.50 / 0.416-0.474 IN-ish |
| x520 | 510-522 / 508-510 edge | 0.23-0.27 / 0.273-0.275 just over | IN | 76-88k / 72.9-74.9k UNDER | 0.52-0.61 / 0.480-0.495 UNDER |
| x550 | 540-552 / 540-542 IN | 0.18-0.22 / 0.219-0.223 edge | IN | 90-105k / 82.7-87.6k UNDER | 0.62-0.72 / 0.562-0.623 mostly under; survived (60% pred) |
| xdyn94 | 485-510 / 485-490 IN | 0.27-0.31 / 0.314 just over | IN | 68-78k / 62.5-66.2k UNDER | 0.47-0.55 / 0.447-0.452 UNDER |
| xdyn85 | 440-460 / 440-441 IN | 0.36-0.40 / 0.397-0.399 IN | IN | 56-63k / 54.9-55.5k just under | 0.39-0.45 / 0.357-0.370 UNDER |
"majflt 0.96-1.00 x misses" -> 0.92-1.00 from x432 up (x432 0.93, xdyn85 0.95), 0.53 for the default: the
prediction failed for the smaller budgets, where ciphertext still has page cache. "majflt <= misses + 0.01" IN
everywhere. "breaths 0" IN. "peak <= limit x 1.07" IN except the default (1.12) and xdyn94 against its median
(1.074; 1.02 against its hi).
Against the restated line (cipher k ~1.12-1.15, S = 682.7, derived in A): 480 -> 0.33-0.34 (x480 held 470 ->
0.343: on it); 520 -> 0.27 (x520 at 509 -> 0.274: on it); 550 -> 0.22 (x550 at 541 -> 0.221: on it). Re-fitted
k = misses / (1 - held/682.7): 1.15 (273), 1.12 (425), 1.12 (440), 1.10 (470), 1.10 (487), 1.08 (509), 1.07 (541)
-- one smooth curve through the 4080 points of E8 and the whole-page points here: the misses are a function of
the plaintext held, whatever the layout.
The rates are under the plan's because the plan's per-miss cost (40+-1.5 us all-in, "33.5 device + 6-7.5") held
(38.5-43.4 all-in, measured), but its misses at each held were 5-10% optimistic (the 660 line); the miss
predictions restated after H1 (0.33-0.34 / 0.27 / 0.22) are where the misses landed.

### Kill lines (§5)
- "Every whole-page arm holding >= 500 MiB reads under 0.45x in both sittings": x520 0.480/0.495 and x550
  0.623/0.562 -> NOT met. (xdyn94 held 485-490, under 500, read 0.447-0.452.)
- "shmem/held does not fall to <= 1.06": it fell to 0.99-1.01 -> NOT met.
- "misses at held >= 480 more than 15% above k(1 - held/S)": on the line -> NOT met.
H2 as stated is not refuted. The reading's mechanism holds: shmem = held under whole pages, and the misses follow
the plaintext held on one curve.
- "Worth it": needs an arm surviving run 6 at >= 0.55x in both sittings of its read cell. Read-only, only x550
  (0.562-0.623) reaches 0.55, and it is a fixed budget; xdyn94 reads 0.447-0.452. Whether it is "worth it" or
  "between" is decided by run 6.
- Layout alone at today's budget (default, same misses 0.684): 53.2-53.3k against the control's 43.2-44.2k in this
  session = 1.21-1.23x, from fewer device faults (0.36 vs 0.43-0.44 a read: 309 MiB of ciphertext page cache where
  the straddle's shmem left 233-244) and less non-device time a miss (6.2-6.9 vs 9.3-9.9 us, derived).
- Layout at b432's held (x432, 0.429 misses): 52.3-52.8k against E8's b432 48.3-49.8k (cross-session, not paired):
  1.05-1.09x.

BEST: the plan's usage-median criterion excludes all arms (above), so BEST is read on xclear alone among arms not
killed and without text thrash: x550 (0.56-0.62). Per the plan, run 6 uses the dynamic xdyn94 regardless; x550's
lead over it (1.24-1.38x by rate, cross-pairing) is the price of a budget meant to survive the load.

## D. Run 6: load,read (ordered load, read pass, shuffled load; each in a fresh process), bench-page

  BIN=$W/bench-page sh $W/sit.sh lr-xdyn94 supdb-cipherxdyn94 supdb-nozpages 5000000 0.5 --bound 671088640 --passes load,read --ops-div 1
  BIN=$W/bench-page sh $W/sit.sh lr-xdyn85 supdb-cipherxdyn85 supdb-nozpages 5000000 0.5 --bound 671088640 --passes load,read --ops-div 1
    (xdyn85 run because xdyn94 was killed in one sitting, as the plan says)

**xdyn94 was OOM-killed in sitting 2, in the shuffled load** (rc 1, "supdb-cipherxdyn94 KILLED under the bound
after read; the rep's pair is lost"). dmesg:
  `Memory cgroup out of memory: Killed process 13763 (bench-page) total-vm:2000104kB, anon-rss:195284kB, file-rss:13296kB, shmem-rss:363468kB`
Its last pagertrace lines in the shuffled load: held 203-372 MiB against limits 377-422 (the dynamic budget
following anon 153-178 MiB down); sampler at the kill: usage 640, shmem 350-406, RssAnon 172-178. Sitting 1
survived. (Order, as the phase marks show it: `bench ab` with --reps 1 runs the second-named arm first, so
sitting 1 (`--arms cipher,clear`) ran the clear arm first and sitting 2 ran the cipher arm first; the order was
swapped between sittings as intended, and "sitting 1 = cipher first" in section C's header is wrong in the
same way: there sitting 1 ran the clear arm first. Nothing else depends on it.)
xdyn85 survived both sittings.

Caveat on the `count` rows in a load,read pairing: they are not the read pass's (cipher_limit_lo 549-550 MiB,
hi 931-1081 MiB, peak 856-1005 MiB, fills 1.73M: values no process under the 640 MiB group could hold as
plaintext). Not chased; for these runs the read pass's misses come from the pagertrace `fills` within the read
window (scaled by trace count, derived) and the rate from the read phase marks.

| arm | k | read reads/s (clear) | xclear | misses (trace) | majflt | held | shmem/held | ordered load ops/s (clear) | ratio | shuffled load ops/s (clear) | ratio | sync ordered / shuffled ms (clear) | device bytes/byte (clear) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| xdyn94 | 1 | 60,358 (131,846) | 0.458 | 0.315 | 0.308 | 488 | 1.007 | 772,617 (1,273,066) | 0.607 | 240,334 (344,812) | 0.697 | 131 / 11,221 (63 / 5,112) | 2.50 (1.34) |
| xdyn94 | 2 | 61,383 (phase marks; pair lost) | -- | 0.316 | -- | -- | -- | -- | -- | KILLED | -- | -- | -- |
| xdyn85 | 1 | 50,093 (129,599) | 0.387 | 0.408 | 0.381 | 438 | 0.991 | 845,575 (1,092,770) | 0.774 | 219,599 (317,411) | 0.692 | 97 / 13,275 (57 / 4,946) | 2.50 (1.34) |
| xdyn85 | 2 | 55,963 (145,779) | 0.384 | 0.401 | 0.374 | 438 | 1.003 | 737,436 (1,298,674) | 0.568 | 230,471 (342,024) | 0.674 | 104 / 13,365 (47 / 5,411) | 2.50 (1.34) |

Memory by phase (phasemem.py, sampler at 0.5 s):
| arm/k | phase | usage med/max | shmem med/max | page cache non-shmem med | RssAnon med/max |
|---|---|---|---|---|---|
| xdyn94/1 | ordered load | 617/640 | 342/508 | 98 | 37/41 |
| xdyn94/1 | read | 640/640 | 487/497 | 94 | 39/39 |
| xdyn94/1 | shuffled load | 615/640 | 358/434 | 68 | 170/207 |
| xdyn94/2 | ordered load | 618/640 | 370/509 | 108 | 38/43 |
| xdyn94/2 | read | 640/640 | 487/498 | 95 | 38/38 |
| xdyn94/2 | shuffled load | killed at ~10 s in: shmem 253-406, RssAnon 127-178 in the last 2 s | | | |
| xdyn85/1 | ordered load | 608/640 | 319/461 | 136 | 38/51 |
| xdyn85/1 | read | 640/640 | 435/446 | 141 | 44/44 |
| xdyn85/1 | shuffled load | 627/640 | 352/391 | 96 | 152/192 |
| xdyn85/2 | ordered load | 623/640 | 361/449 | 145 | 38/44 |
| xdyn85/2 | read | 639/640 | 440/450 | 142 | 38/38 |
| xdyn85/2 | shuffled load | 622/640 | 315/376 | 92 | 198/238 |
(the clear arm: shuffled load RssAnon 114-172 max, page cache 448-453 med.)
Shuffled-load limit (pagertrace): xdyn94 336-422 MiB (pred 370-395: about), xdyn85 lower.

### Predictions for run 6 beside results
| quantity | prediction (xdyn94) | result |
|---|---|---|
| survives both sittings | 60% | xdyn94 killed in 1 of 2 (shuffled load); xdyn85 survived 2 of 2 |
| shuffled-load limit | 370-395 MiB | 336-422 (xdyn94/1 trace) |
| shmem max in the load | 420-470 | 434 (xdyn94/1); 391/376 (xdyn85) |
| read pass | as 5a, 0.47-0.55x | xdyn94 0.458 (s1); xdyn85 0.384-0.387 (as its read-only 0.357-0.370) |
| ordered load | 0.61-0.74x | xdyn94 0.607; xdyn85 0.774 / 0.568 (unresolved: clear's own load moved 1.09M-1.30M) |
| shuffled load | 0.55-0.80x | 0.697 (xdyn94); 0.692 / 0.674 (xdyn85) |
| device_bytes_per_byte | within 1% of today's | 2.5001 (today's per CLAUDE.md "2.5 against 1.3"; clear 1.339) -- same |
Closing syncs: the shuffled load's sync (drain + merges) 11.2-13.4 s against the clear store's 4.9-5.4 s.

### Kill line: "worth it" or "between"
No arm that survives load,read reads 0.55x of clear: the survivor (xdyn85) reads 0.384-0.387; xdyn94 reads
0.458 where it survives and is killed in the shuffled load in one sitting of two; the only arm over 0.55 read-only
(x550, 0.56-0.62) is a fixed budget that the plan already expected the load to kill (not run in load,read).
Verdict: **between** -- the whole-page layout is necessary (it moved the default budget 1.21-1.23x and made shmem =
held) but not sufficient; the budget's owner decides the rest. The term that moved: shmem/held (1.23 -> 1.00),
and with it the plaintext held per MiB of shmem; misses follow held on the one curve; non-device us a miss
(derived) ~7.3-9.3 against the predicted 6-7.5 (40+-1.5 all-in held: 38.5-43.4); what is left is headroom: in
the read pass the whole-page arms leave 35-160 MiB of page cache that is not shmem at usage 640, and the
shuffled load's anon (150-240 MiB) is what the dynamic budget has to give way to.

## E. Run 7: the scan pass (xdyn85, the arm that survived run 6), bench-page

  BIN=$W/bench-page sh $W/sit.sh scan-xdyn85 supdb-cipherxdyn85 supdb-nozpages 5000000 1 --bound 671088640 --passes scan --ops-div 1
Both sittings rc 0, dmesg empty. The pass is short (clear 2.5 s, cipher 4.1 s after its ordered load).
| k | cipher entries/s | clear entries/s | xclear | cipher majflt/scan | clear majflt/scan | cipher fills (process) |
|---|---|---|---|---|---|---|
| 1 | 1,231,221 | 1,968,211 | 0.626 | 1.568 | 0.982 | 85,586 |
| 2 | 1,224,073 | 2,100,250 | 0.583 | 1.529 | 0.920 | 84,088 |
Prediction 0.55-0.95x: IN (low end). Against E8's R8a 0.40-0.47x for today's cipher (cross-session, not paired).

## F. Run 8: ycsb-C twice (SUPDB_MIX_ONLY=CC, set for both arms through EXTRA), Zipf, unscrambled

  EXTRA=SUPDB_MIX_ONLY=CC BIN=$W/bench-page  sh $W/sit.sh mixc-page  supdb-cipherxdyn85 supdb-nozpages 5000000 1 --bound 671088640 --passes mixes --ops-div 1
  EXTRA=SUPDB_MIX_ONLY=CC BIN=$W/bench-p4080 sh $W/sit.sh mixc-4080  supdb-cipher      supdb-nozpages 5000000 1 --bound 671088640 --passes mixes --ops-div 1
  EXTRA=SUPDB_MIX_ONLY=CC BIN=$W/bench-p4080 sh $W/sit.sh mixc-clear supdb-nozpages    supdb-nozpages 5000000 1 --bound 519045120 --passes mixes --ops-div 1
Deviation: BEST for 8a is xdyn85 (the run-6 survivor, as the plan says for 7 and 8a); 8c's bound is
held(xdyn85 ~440) + 55 = 495 MiB (519045120). All six sittings rc 0, dmesg empty.
Each C is ~0.85-0.89M ops; C#2 lasts 1-3 s, so its misses come from 1-2 pagertrace intervals inside the window
(derived, coarse); its rate and device faults are the harness's (`mixagain` line, `ycsb-C#2` rows).

| arm (bin) | k | C ops/s (clear) | C#2 ops/s (clear) | C#2 xclear | C#2 majflt/op (clear) | C#2 misses/op (trace, derived) | held in C#2 |
|---|---|---|---|---|---|---|---|
| xdyn85 (page) | 1 | 212,906 (254,477) | 312,875 (834,659) | 0.375 | 0.0438 (0.0155) | ~0.047 | 434-435 (limit 443) |
| xdyn85 (page) | 2 | 251,570 (197,450) | 369,626 (812,526) | 0.455 | 0.0152 (0.0371) | ~0.038 | 425-449 (limit 450) |
| supdb-cipher (p4080) | 1 | 171,372 (221,258) | 501,993 (827,047) | 0.607 | 0.0225 (0.0154) | ~0.10 | 271-278 (limit 279) |
| supdb-cipher (p4080) | 2 | 248,283 (148,314) | 446,199 (797,730) | 0.559 | 0.0155 (0.0238) | ~0.079 | 273-278 (limit 279) |
| clear @495 MiB | 1 | 198,481 / 200,159 | 838,439 (pair 1-0) | -- | 0.0163 / 0.0160 | -- | par resident 413-418 MiB |
| clear @495 MiB | 2 | 193,328 / 180,081 | 786,489 (pair) | -- | 0.0163 / 0.0160 | -- | |
(clear-arm entries in sittings 2 of 8a/8b are the clear arm that ran first and took the cold misses of its own
process; its C#2 majflt 0.0155-0.037 is the harness's.)

Predictions: BEST second-C misses 0.01-0.05 -> ~0.038-0.047 IN; BEST xclear 0.55-0.95 -> 0.375-0.455 UNDER;
today's cipher misses 0.04-0.10 -> ~0.079-0.10 IN; its xclear 0.35-0.70 -> 0.559-0.607 IN; clear at matched C
0.008-0.04 -> 0.016 IN.
Skew kill line (plan §5): BEST's second-C misses (~0.04) exceed the matched clear store's faults (0.016) by
~2.4-2.9x (> 1.5x), while its uniform misses sat inside H1's band -> by the plan's line, "megabyte FIFO costs
under skew". Caveat: the misses are from 1-2 one-second trace intervals (derived).
**Unexplained, measured:** under skew the whole-page dynamic arm halved the misses of today's 4080 default
(~0.04 vs ~0.08-0.10) and read SLOWER (313-370k vs 446-502k ops/s, 0.67-0.83x), with device faults a read the
same in sitting 2 (0.0152 vs 0.0155) and double in sitting 1 (0.0438 vs 0.0225). The 4080 default's extra misses
are served from its larger ciphertext page cache (it leaves ~240 MiB of page cache, xdyn85 ~140), which covers
part of sitting 1 (~0.7 us of a 1.2 us/op gap, derived) and nothing of sitting 2 (0.46 us/op). Not attributed:
the two arms differ in budget (443-450 vs 279) and dynamic recompute as well as layout, the windows are 1-3 s,
and one reading a sitting. A paired x280-whole-pages vs p4080-default run under skew would separate layout from
budget; not run.

## G. Verdict

- **H1 holds.** At the same plaintext held / page cache, the clear store takes 9-12% MORE device faults a read
  than the cipher store takes misses (0.748-0.763 vs 0.684 at C ~275; 0.468-0.476 vs 0.428 at C ~415), inside the
  band, on the "clear higher" side. The pager's megabyte clock is no worse per MiB than the kernel's LRU on
  uniform reads; the "uncounted factor of two" of once.md does not exist. S measured: 682.7 MiB of partitions
  (+38.1 MiB of ordered indexes, unread by point reads); the curve is convex (k = 1.27 -> 1.14 clear, 1.15 -> 1.07
  cipher, across C = 273 -> 541), not the single line.
- **H2's mechanism holds; its rates were 5-15% optimistic.** Whole pages put shmem = held (0.99-1.01, against
  1.228-1.229 for the 4080 control in the same session); the misses follow held on the one curve the 4080 points
  are on; held tracks the budget. At the default budget the layout alone reads 1.21-1.23x of today
  (53.2-53.3k vs 43.2-44.2k, 0.346-0.357x of clear). With budgets that take more of the bound: x480 0.42-0.47x,
  xdyn94 0.45x, x520 0.48-0.50x, x550 0.56-0.62x (read-only). The plan's per-miss cost held (38.5-43.4 us
  all-in); its misses were 5-10% low at each held because its 660 line was below the measured curve.
- **Not "worth it" by the plan's line; "between".** The only budgets that reach 0.55x are fixed ones near the
  bound's edge. The dynamic xdyn94 (94% of read_budget) reads 0.45-0.46x and was OOM-killed in the shuffled load
  in 1 of 2 sittings; xdyn85 survives load,read in both and reads 0.38x (0.384-0.387), ordered load 0.57-0.77x,
  shuffled 0.67-0.69x, scan 0.58-0.63x. What is left is the budget and headroom under shmem, which the kernel
  cannot reclaim: the read pass leaves 35-160 MiB of non-shmem page cache, the shuffled load needs 150-240 MiB of
  anon beside the plaintext, and the kill came with shmem 350-406 + anon 172-178 at usage 640.
- **Skew (single 1-3 s windows): unresolved and against expectation:** whole pages at xdyn85 halved the misses
  of today's default and read slower than it; budget and layout are confounded there.

Not run / not measured: a second sitting of h1-328 (my tool timeout killed it; the first's two readings agree
within 2%); x550 under load,read (fixed budget, not in the plan's run 6); a paired whole-pages default against a
whole-pages budget under skew; any profile of a miss; threaded reads; page-granular eviction.
Tree: `git status --porcelain` empty, HEAD 3df85d22, bench/target/release/bench rebuilt from the clean tree at the
end with nothing running. Disk 5.5 GB free. W/tmp empty.
