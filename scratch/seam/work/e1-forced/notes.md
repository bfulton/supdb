# E1: the compact form's upper bound in the clear, forced

Label: e1-forced. Tree 3df85d22, clean (`git status --short` empty). Bench binary
bench/target/release/bench built 2026-11-16 06:27, newer than the last commit touching
src/ or bench/src (2026-11-15 22:51); no .rs under src/ or bench/src newer than it.
Machine idle (ps) before every run; one timed thing at a time.

## 1. Existing answers

- bench/runs/ooc/ (three rows, 47aee03, 2b6122c, 5bf1f88): no forced arm in any row. The
  5bf1f88 row has `supdb-autozpages` (controller) beside `supdb` at 10M under 1.25 GiB
  (mem_limit_bytes 1342177280): reads 132.7k-146.5k/s against 68.2k-73.1k, faults 0.060-0.063
  against 0.229-0.230 a read. Not forced, and what `supdb` was at that commit is not verified.
- docs/engine.md:6934-6960: forced against raw at 10M under 1 GiB, `pair.sh supdb-nocache
  supdb-nocachezpages 10000000 --reps 12 --bound 1073741824 --passes load,read,read-threaded
  --ops-div 10` (at a commit when `supdb-nocache` was raw): 4.4-6.4x on one thread, faults
  0.000 against 0.45-0.47, 12 of 12 pairs in both sittings. The form fit its budget there.
- No forced-vs-raw pairing at 5M under 640 MiB found in CLAUDE.md, docs/engine.md or
  docs/zpages-design.md (grep of "forced", "nocachezpages", "5000000").

## 2. The arms (what the code says)

- `supdb-nocachezpages` = `Policy { block_cache: false, zpages: true }`
  (bench/src/engines.rs:5577-5586) -> `ZPages::Always` (engines.rs:7064-7065).
- `supdb-nocache` = `Policy { block_cache: false }` (engines.rs:5532-5540) -> zpages is the
  engine's default (engines.rs:7071), which is `ZPages::Auto` (src/db.rs:3875). So
  `supdb-nocache` is the controller's arm, NOT raw. The task's suggested pair is forced vs Auto.
- `supdb-nozpages` = `Policy { zpages_off: true }` (engines.rs:6505-6513) -> `ZPages::Off`
  (engines.rs:7068-7069), block cache on (the default).
- No arm has block cache off and `ZPages::Off`, and no arm has block cache on (unbudgeted) and
  `ZPages::Always` (dispatch list engines.rs:10060-10400; naming engines.rs:8399-8442). No
  environment variable sets zpages. So no pair differs in the form alone.
- Primary pair used: `supdb-nocachezpages` against `supdb-nozpages`. They differ in
  - `zpages` Always vs Off (the question);
  - `block_cache` off vs on, which maps only to `Options::scan_block_cache`
    (engines.rs:6848). The read pass makes no scan; in every raw sitting `cache_bytes_at_end`,
    `forms_held_at_end` and `blk_by_engine/reader/upkeep` are 0, so the block cache built and
    held nothing. Whether its being on moves a point read's instructions: not verified.
  - the forced arm's `sync` also calls `Db::zpages_settle` (engines.rs:8930-8932), which waits
    for the segment work; the raw arm's flush returns earlier (engines.rs:5567-5576).
- Secondary pair: `supdb-nocachezpages` against `supdb-nocache` (forced vs controller, both with
  the block cache off), to see what `Auto` does in the clear at this rung.
- Forms standing from the start: `zpages_refusal` (bench/src/run.rs:2060) refuses a forced arm
  after the load unless every partition has a form file (run.rs:2082-2097, called at
  run.rs:2338), and after the reads if over 1% went raw (run.rs:2110). Every forced sitting
  passed both, and the counters say so directly (section 3).

## 3. Runs

All: `cd bench && TMPDIR=$W/tmp OUT=$W/pairs[/<dir>] sh scripts/pair.sh SUPDB_ZPAGES_TRACE=1
<A> <B> 5000000 --bound <BYTES> --passes read --reps 1 --ops-div 1` (5M uniform point reads a
sitting, each pass in a process of its own). W = this directory.

| run | dir | A | B | bound | sittings |
|---|---|---|---|---|---|
| P1 | pairs/ | supdb-nocachezpages | supdb-nozpages | 671088640 (640 MiB) | 2 |
| P1 r2 | pairs/r2 | same | same | 640 MiB | 2 |
| P1 r3 | pairs/r3 | same | same | 640 MiB | 2 |
| tight | pairs/b448 | same | same | 469762048 (448 MiB) | 2 |
| tight | pairs/b384 | same | same | 402653184 (384 MiB) | 2 |
| P2 | pairs/p2 | supdb-nocachezpages | supdb-nocache | 640 MiB | 2 |

Reduction: summ.py (per-file counters), table.py (medians). Each sitting 55-60 s at 640 MiB,
~2.3 min at 448, ~3.3 min at 384.

### 640 MiB, forced against raw (6 sittings, 3 in each order)

| sitting | first | forced reads/s | raw reads/s | forced/raw | forced flt/read | raw flt/read | p99 forced / raw (us) |
|---|---|---|---|---|---|---|---|
| P1-1 | forced | 383,055 | 149,329 | 2.565 | 0.0000 | 0.1364 | 4.7 / 62.7 |
| P1-2 | raw | 429,535 | 149,605 | 2.871 | 0.0000 | 0.1379 | 4.4 / 63.7 |
| r2-1 | forced | 445,426 | 147,857 | 3.013 | 0.0000 | 0.1397 | 4.1 / 63.7 |
| r2-2 | raw | 447,377 | 145,019 | 3.085 | 0.0000 | 0.1395 | 4.3 / 64.5 |
| r3-1 | forced | 401,606 | 126,100 | 3.185 | 0.0000 | 0.1557 | 4.8 / 73.2 |
| r3-2 | raw | 390,007 | 149,683 | 2.606 | 0.0000 | 0.1369 | 5.5 / 63.2 |

Median forced/raw 2.94x (range 2.57-3.19); forced 416k/s (383-447k), raw 148.6k/s (126-150k);
faults a read forced 0.0000 in all six, raw 0.139 (0.136-0.156); bytes read from the device a
read forced 0, raw 558-638. No order effect visible (forced first: 2.57, 3.01, 3.19; raw first:
2.87, 3.09, 2.61).

The forced arm in every sitting: 19 builds started, 19 finished, 0 cancelled, 0 failed, all
inside the load's sync (`zbuild` lines t=3.8-5.1 s after the process's `zepoch`; build majflt 0;
391.6 MB of form files, 93,333 pages); 19 form files at the end, 0 damaged; `zpage_reads`
5,000,000, `zpage_rawserves` 0, `zpage_fallbacks` 0; 4,938,971 decodes and 61,029 memo hits;
raw records dropped (`zpage_raw_dropped` 22-23). So the form stood whole before the first read.
The raw arm: `zpage_reads` 0 (the refusal for an unforced arm reading a form file, run.rs:2118,
did not fire).

### Tighter bounds, forced against raw (2 sittings each, one in each order)

| bound | sitting | forced reads/s | raw reads/s | forced/raw | forced flt/read | raw flt/read |
|---|---|---|---|---|---|---|
| 448 MiB | 1 (forced first) | 344,621 | 43,633 | 7.898 | 0.0170 | 0.5216 |
| 448 MiB | 2 (raw first) | 298,019 | 45,393 | 6.565 | 0.0175 | 0.5165 |
| 384 MiB | 1 (forced first) | 93,529 | 36,239 | 2.581 | 0.1743 | 0.6470 |
| 384 MiB | 2 (raw first) | 98,988 | 36,802 | 2.690 | 0.1693 | 0.6460 |

Same form (19 files, 391.6 MB, all built in the load, 5M reads through it, 0 raw serves) at
both bounds. No pass was killed.

### 640 MiB, forced against the controller (`supdb-nocache`, Auto)

| sitting | first | forced reads/s | auto reads/s | forced/auto | auto flt/read | auto reads via form |
|---|---|---|---|---|---|---|
| 1 | forced | 403,348 | 380,622 | 1.060 | 0.0048 | 4,980,624 |
| 2 | auto | 476,007 | 436,108 | 1.091 | 0.0049 | 4,982,882 |

In the clear the controller built all 19 form files ahead during the load (`zpage_prebuilt`
19, `zsample ... coding Some(Block)`, w_z 390,782,648, the same 391,266,304 bytes of form file
as the forced arm), installed once (`zpage_sel_installs` 1) and served 99.6% of the pass's
reads through the form. So `supdb-nocache` is not a raw arm at this rung.

## 4. What it says

- E1's kill is not met: in the clear, forced, out of core, the compact form is ahead of the raw
  form on point reads at every bound measured: 2.94x median (2.57-3.19, 6 sittings) at 640 MiB,
  6.6-7.9x at 448 MiB, 2.6-2.7x at 384 MiB.
- At 640 MiB the clear regime is: the form (391 MB, all block-coded) fits the page cache whole
  and reads 0.000 faults; the raw form (620 MB records + ~80 MB hash slots) is barely out of
  core at 0.139 faults a read. The forced arm's rate there is its decode cost (2.2-2.6 us a read,
  ~99% of reads a group decode), so 640 MiB measures the form's in-core cost against the raw
  form's mild faulting, not a regime in which both fault.
- Time follows faults closely in the clear, one line per form (fitted, not registered):
  raw t = 0.99 us + 41.4 us x faults (fit to 640 and 448; predicts 27.7 us at 384 against 27.4
  measured); form t = 2.41 us + 46.4 us x faults (fit to 640 and 384; predicts 3.21 at 448
  against 3.11 measured). The form wins wherever its faults a read are below
  (41.4 x raw faults - 1.4) / 46.4: below 0.36 when raw takes 0.44 (the cipher raw arm's figure).

## 5. Against the design

- Section 8's form row says the cipher pairing will read "the ratio the clear store shows in a
  forced pairing, within a tenth", and in the same row "about 1.4x of raw". E1 puts the clear
  ratio at 2.6-3.2x (median 2.94) at the same bound. The two halves of the row cannot both hold,
  and E1b's own arithmetic (form fills 0.40-0.45 a read, faults 0.26-0.30 against raw 0.44 under
  whole-page chunks) is at 1.4-1.7x, so "within a tenth of the clear ratio" is contradicted by the
  design's own model. The clear pairing at 640 MiB is not the cipher's regime:
  - in the clear the whole page-cache budget holds the form, 0.000 faults a read;
  - under the cipher the plaintext cache defaults to half the budget (`default_cache`,
    src/db/vault.rs:640-646: `(b.budget / 2).max(64 << 20)`), ~285-340 MB held in E1b's runA,
    less than the form, and the raw arm faults 0.44 a read there against 0.14 in the clear.
  The clear figure is an upper bound on what the cipher could read, not a prediction of it.
- The coding differs between the clear and the cipher at this rung (E1b's runA outputs,
  scratchpad/seam/work/e1b-straddle/runA/*.out and .err): under the cipher the controller built
  10-11 of 19 partitions raw-tailed (`zpage_builds_raw_tails` 10 and 11; `zsample` lines at 12
  and 16 partitions picked `RawTails`, the last at 19 picked `Block`), so its form files were
  500-507 MB; in the clear, forced and Auto alike, all 19 are block-coded, 391 MB. A cipher arm
  with the form forced does not exist in engines.rs (cipher arms: supdb-cipher, -cipher64,
  -cipherlog, -cipherauto, -ingestcipher, -ingestcipherlog, -ingestcipherauto). Inference, not
  measured: with E1b's held figure (~73.6k pages of form plaintext) and whole-page chunks, a
  391 MB block-coded form would fill ~0.23 a read (1 - 73.6k/95.5k) against 0.40-0.45 for the
  500-544 MB mixed forms, i.e. faults ~0.15 against raw 0.44; whether the controller under a
  cipher would pick Block is not verified.
- The kill clause (S5 drops its controller step if the form is not ahead in the clear) does not
  fire. The design's S3 pairing list names "a forced pair as E1" under the cipher; that needs a
  forced cipher arm, which does not exist yet.
- In the clear, at 640 MiB, the controller reaches the forced form's rate (1.06-1.09x forced
  over Auto), so the "both forms from the start" premise matters less in the clear than under
  the cipher, where `zpages_under_cipher` is off as shipped.

## 6. Caveats

- One rep a sitting, as the task's command; 6 sittings at 640 MiB, 2 at each other bound. The
  tighter-bound and P2 figures are two sittings each, so their spreads are two points.
- The primary pair differs in the block cache as well as the form; the raw arm's block cache
  held nothing in the pass (counters), but a per-read code-path effect is not verified.
- SUPDB_ZPAGES_TRACE=1 was set for both arms (both are supdb arms); it prints `zbuild`/`zsample`
  lines and its cost on the rates is not separately measured.
- The time-per-fault lines in section 4 are fits to 2 points each with a third as a check; the
  raw base (0.99 us) is fitted, not measured in core.
- Why the clear controller's builds ahead started with Block where the cipher's started with
  RawTails at 12 partitions: not verified.
