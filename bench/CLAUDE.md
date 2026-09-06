# Working in this repository

This directory is the benchmark suite for the engine in the directory
above, and its own cargo workspace so the engine's builds never pay for the
comparators. `DESIGN.md` is the specification and is short; read it first. This file is notes to whoever
picks the work up next with no memory of it: the rules, and the failure that
produced each one.

## What the suite is

A time series. `bench run` measures every arm over a ladder of store sizes
and writes one row under `runs/<scale>/`. A row holds raw per-rep samples
and the machine fields as read, and nothing derived: median, error bars and
the machine class are computed when `runs/` is read, so a change to any of
them recomputes history instead of stranding it. A regression is a row
whose error bars lie entirely on the worse side of the last ten rows' in the
same class. There are no claims and no expected states.

The previous suite -- 183 claims in a JSON file, adjudicated by `verify`
across three profiles with pins, arch guards and a `needs` field -- lived in
the supdb-bench repository and is in its history. Its `because` prose is
real reasoning about the engine and is worth reading when a number
surprises you; an id like `EXT.23` or `W4.1` in an engine comment names one
of those claims. It was retired because a
gate that adjudicated timing comparisons on shared runners went red on an
engine head that had not changed, and because its largest run, at 100 MB,
never left the page cache.

## Rules that stay, and why

**Interleave the arms.** Every arm in one process, round-robin within a
rep. Blocked execution confounds a comparison with anything that drifts
across the run; two runs on instances of the same nominal machine have
moved untouched comparator arms by half.

**Never compare two separate runs.** The only comparison is within a row.
Across rows, a quantity is compared only to its own history in its own
class.

**Never run two timing benchmarks at once.** Four cores measuring each
other is not a measurement. This includes your own agents.

**Two rows is not a band.** The gate says "insufficient history" below
three prior rows rather than pretending.

**Nothing typed.** The window of ten is the one parameter and it is stated
once, in `DESIGN.md`. The moment a threshold appears in code, ask what
measured quantity it is standing in for.

**The checks are `scripts/check.sh`, and there is no second definition.**
Every gate this suite has broken has broken the same way: a check that was
not running, or one reporting a verdict it had not earned. The engine's
`scripts/check.sh` calls this one by group name and CI calls that.

## Shapes the bugs come in

**A gate can be red for a reason that is not the engine's.** EXT.47, a
"not resolvably slower" comparison gated on clearing a 5% minimum effect,
recorded 0.935x on a shared runner on a head that had passed the same job
eighty-five minutes earlier. Near-zero true effects and a fixed floor are
the shape. The design answers it with bands drawn from the series rather
than a typed floor, and with the arms as the dimension rather than the
ratio between two near-identical arms.

**A one-sided bound reads `holds` for a broken measurement.** F68.6 was
`ratio >= 0.90`, and a run where the ratio came out 8.5x -- on a store
where the mechanism says the policy can only lose -- recorded `holds`. A
row whose value is implausibly *good* is flagged, not passed.

**A number can arrive in the wrong type.** `J::u` once wrapped a `u64` into
an `i64` on the way to JSON; a wasm `u32::MAX` arrived in JavaScript as
`-1`. Rows are serde; there is no hand-rolled JSON to get this wrong in.

**A percentile at an exact boundary can round into the next bucket.**
`hist::percentile` does parts-per-million integer arithmetic for that
reason; `99.9 / 100.0` is not `0.999` in binary.

**A cache line size that is guessed is not a measurement.** The detector
once read a `/sys` path that does not exist on macOS and defaulted to 64
on a machine whose lines are 128. `cache_line_detected` records whether it
was read, and `apple-silicon.yml` fails if it was not.

**Per-record allocation in the harness is paid by every arm equally, and
so is invisible in every ratio.** `Batch` builds a batch without allocating
per record after that was found to cost as much as an engine's whole commit
path.

**A workflow that never runs can be syntactically invalid for months.**
Both self-hosted pickup watchdogs arrived with a block of an older draft
pasted after their `exit 1`. `scripts/workflows.sh` parses every `run:`
block and rejects any `${{ }}` inside one; it runs in `lint`.

## One repository

The suite was a separate repository carrying the engine as a submodule for
as long as it was large: claims, results, plan files, browser tests and a
launcher. Once it was ten files and a time series, the second repository
cost more than it guarded -- a paired pull request for every change that
touched both, a submodule pointer and a branch override so CI tested the
right engine, and the engine's release profile silently ignored under the
other workspace. Now the pull request's own commit is the engine under test
and the row's one `sha` names both.

Building RocksDB runs bindgen, which needs libclang, and every workflow
calls `scripts/libclang.sh` for it -- one definition. The first CI run that
built RocksDB at all failed on both Linux (only a versioned `.so` on the
image) and macOS (dyld could not find `@rpath/libclang.dylib` at run time);
naming the directory fixed Linux and not macOS, because SIP strips `DYLD_*`
before a workflow step's shell starts, so on macOS the script adds an rpath
to `RUSTFLAGS`. Cargo takes profiles from the root of the workspace being
built, which here is this directory, so the engine's release profile is
repeated in `Cargo.toml` and cargo warns on every build that it is ignoring
the engine's -- the warning is expected, the repetition is what keeps the
measured engine the shipped one.
