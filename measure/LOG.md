# Measurement log at C0

C0 is `b3a043dc` (tree `7f58d776`). Runs from a detached worktree of C0 at
`/home/user/c0` (nothing tracked changed; `git status --porcelain
--untracked-files=no` empty); rows, logs and each run's TMPDIR under
`/home/user/base`. Times are UTC.

Host class (`bench machine`): `x86_64/intel-r-xeon-r-processor-2.10ghz/4c/16g/hypervisor`,
kernel 6.18.44-fc-v880, 16,473,316 kB. The run sheet's 4.5 read a 2.30GHz
host; this container is the 2.10GHz class.

Binary stamp (`bench stamp`): `b3a043dcad52f64b69af6590cafbeb94beb55d98`, no `-dirty`,
rustc 1.100.0. Built with `cd bench && cargo build --release --bin bench`
(cargo reported 8m 09s).

## 0. Set-up and smokes

- `sh scripts/deps.sh`: `all deps ready: rust node libclang faster`; FASTER
  fetched at the pin (PIN = COMMIT = 332ea7da).
- `sh scripts/ooc.sh --prove-red`, 2026-11-27T23:09:17Z:
  `ooc.sh: the gate fired on a bound of 67108864 bytes (exit 1), as it must`
  (`measure/smoke-red.err`).
- `./target/release/bench run --scale quick --top 10000 --bottom 10000 --reps 1 --out /home/user/base/smoke`,
  2026-11-27T23:09:21Z to 23:09:41Z: every arm opened once, row
  `measure/smoke-quick-20261127T230922Z-b3a043d.json` (a smoke, not a row
  of the series).

Rows are kept at the run sheet's `$BASE/rows/...` paths (`BASE=/home/user/base`)
and copied here under `measure/rows/` at the same relative path.

## 1. Quick row, numbered keys

- Command: `cd /home/user/c0 && TMPDIR=/home/user/base/tmp/quick sh scripts/check.sh quick`
- Start 2026-11-27T23:10:15Z, row written 23:32:07Z, verdict line
  `all checks passed: quick` at 23:32:14Z (about 22 minutes, build included).
  No kill.
- Row: `measure/rows/quick/20261127T231015Z-b3a043d.json` (15 arms, to 300,000 keys, 5 reps).
- Gate (`measure/logs/quick-gate.txt`): 10 prior rows of this class.
  `host: OUT OF BAND` — the mem-floor read below the window, so quantities
  that move with the machine get no verdict; 3 of 2228 quantities
  no-verdict (supdb-heldnoframes recover-shuffled open_ms at 30k, 100k, 300k),
  37 flagged better, none of the engine's regressed.

## 2. Encrypted roster, quick row

- Build: `sh scripts/deps.sh openssl` (`all deps ready: openssl`), then
  `cd bench && cargo build --release --no-default-features --features enc --target-dir target-enc --bin bench`
  (cargo reported 8m 22s); stamp `b3a043dc…`, clean.
- Command: `cd /home/user/c0/bench && TMPDIR=/home/user/base/tmp/enc ./target-enc/release/bench run --scale quick --arms enc --out /home/user/base/rows-enc`
- Start 2026-11-27T23:41:08Z, row written 2026-11-28T00:09:01Z (about 28 minutes). No kill.
- Row: `measure/rows-enc/quick/20261127T234108Z-b3a043d.json` (to 300,000 keys).
- Read: `python3 scripts/encrow.py <row>` → `measure/logs/encrow.txt`
  (supdb-cipher against lmdb3-enc: ahead 111, behind 17, unresolved 16;
  against rocksdb-tuned-enc: ahead 114, behind 9, unresolved 21; the rest in the file).

## 3. Bounded row, 1.25 GiB, numbered keys

- Command: `cd /home/user/c0/bench && TMPDIR=/home/user/base/tmp/ooc-1280-num sh scripts/ooc.sh 1342177280 --out /home/user/base/rows`
- Start 2026-11-28T00:09:38Z, verdict `ooc.sh: bounded run under 1342177280 bytes banked`
  by 03:12:53Z; the run's own clock 10,981 s (3 h 03 m), one sitting. No kill.
- Row: `measure/rows/ooc/20261128T000943Z-b3a043d.json` (10 arms, rungs 3M and 10M, 3 reps).
- Gate `bench gate <row> --runs runs` (exit 1; `measure/logs/ooc-1280-num-gate.txt`):
  10 prior rows of class `.../b1280m`, floors within the window (8 of 8);
  7 of 3078 quantities worse than every row in the window, all lag-burst and
  scan-lag cells of supdb-nozpages, supdb-nocache and supdb-noadvice, plus
  rocksdb-tuned load device_bytes_per_byte at 10M (a comparator below its
  window); no band yet for 520.

## 4. Bounded row, 1 GiB, numbered keys

- Command: `cd /home/user/c0/bench && TMPDIR=/home/user/base/tmp/ooc-1024-num sh scripts/ooc.sh 1073741824 --out /home/user/base/rows`
- Start 2026-11-28T03:13:17Z, verdict `ooc.sh: bounded run under 1073741824 bytes banked`
  by 07:25:26Z; the run's own clock 15,117 s (4 h 12 m), one sitting. No kill.
- Row: `measure/rows/ooc/20261128T031319Z-b3a043d.json` (10 arms, rungs 3M and 10M, 3 reps).
- Gate (exit 0; `measure/logs/ooc-1024-num-gate.txt`): 4 prior rows of class
  `.../b1024m`; `host: OUT OF BAND` on wal-floor (median 1,092,387 ops/s
  against the window's [1,328,102, 1,934,464]); 19 of 3131 no-verdict,
  nothing the engine's own, 18 flagged; 8 comparator cells below their
  window (lmdb-nordahead, rocksdb-tuned-lz4); no band yet for 1770.

## 5. Bounded row, 1.25 GiB, ids

- Command: `cd /home/user/c0/bench && TMPDIR=/home/user/base/tmp/ooc-1280-ids sh scripts/ooc.sh 1342177280 --keys ids --out /home/user/base/rows`
- Start 2026-11-28T07:25:44Z, verdict `ooc.sh: bounded run under 1342177280 bytes banked`
  by 10:57:01Z; the run's own clock 12,666 s (3 h 31 m), one sitting. No kill.
- Row: `measure/rows/ooc/20261128T072548Z-b3a043d.json` (10 arms, rungs 3M and 10M, 3 reps).
- Gate (exit 0; `measure/logs/ooc-1280-ids-gate.txt`): class `.../b1280m/kids`,
  1 prior row: no band yet for any of 3134 quantities.

## 6. Bounded row, 1 GiB, ids — cut by a container restart, not resumed

- Command: `cd /home/user/c0/bench && TMPDIR=/home/user/base/tmp/ooc-1024-ids sh scripts/ooc.sh 1073741824 --keys ids --out /home/user/base/rows`
- Start 2026-11-28T10:57:17Z. The session's worker and container were
  restarted about 11:00Z (uptime 0 at 11:00:24Z), in rep 1 of the 3M rung,
  after 5 whole passes (the last line the run printed is at 121 s). Not a kill under the bound.
- The container came back on another host class:
  `x86_64/intel-r-xeon-r-processor-2.30ghz/4c/16g/hypervisor` (every row
  above is of the 2.10GHz class). A resume on another class is refused, so
  it was not attempted; that decision is the coordinator's.
- Ledger kept: `/home/user/base/rows/inflight/20261128T105721Z-b3a043d`
  (copied to `measure/rows/inflight/`; resume with
  `sh scripts/ooc.sh 1073741824 --resume <ledger>` on a 2.10GHz host).
- Not run: this row, the full rows (numbered, ids), the noise floor.

## Second series: the 2.30GHz class

From 2026-11-28T11:01Z the coordinator directed: carry on here on
`x86_64/intel-r-xeon-r-processor-2.30ghz/4c/16g/hypervisor` as a second
series, in the order quick (numbered, the bridge between classes), noise
floor, full rows (numbered, then ids), then the ooc 1 GiB ids row from the
start; the cut ledger stays in `inflight/` as a record and is not used.
No 2.10GHz row is compared with a 2.30GHz row as a verdict. Phase 6 pairs
the kept C0 binaries against the last commit's on whatever host it gets.
Same C0 binaries (`/home/user/c0/bench/target`, stamp `b3a043dc`, clean).
