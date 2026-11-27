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
