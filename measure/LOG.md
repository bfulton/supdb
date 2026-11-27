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
