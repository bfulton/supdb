# Working in this repository

Supdb is an embedded key-multivalue store, and a benchmark suite whose job is
to try to falsify the claims made about it. The two are meant to stay together.

## The rule that matters most

**A finding is not a number, it is a statement with a recorded expected state.**
`claims.json` holds every statement the project makes about the engine,
including the ones that currently fail. `verify` checks it against `results/`
and CI fails in **both** directions:

- a limitation that gets worse turns the build red;
- a limitation that gets **fixed** also turns the build red.

The second is not a mistake. Either the engine improved and the claim is stale,
or the experiment stopped testing anything. Both need a person to decide which.
So when you fix something, update `claims.json` in the same change — that edit
is the record that the fix was intentional.

## Before adding a benchmark

Four rules, enforced in `src/bench/` rather than remembered:

1. **Nothing is measured once.** Use `Trial`, which runs configurations
   interleaved. Report a median with an interquartile range.
2. **A difference is not a difference until it clears `stats::compare`** — a
   Mann-Whitney U test *and* a minimum effect size. Do not hand-roll a
   comparison; the gate exists because the original design document reported a
   13.9% difference as a win against its own stated 15% noise floor, and
   `stats.rs` carries that case as a regression test.
3. **A finding whose precondition was not met is `Finding::not_exercised`,
   never `holds`.** This has already caught three false greens: an out-of-core
   experiment that compared warm against cold inside a dataset too big to be
   warm; a multi-process experiment that ran 8 readers against a 64-slot table;
   and a crash experiment that blamed the engine for crashing before any
   checkpoint existed. If a run cannot reach the condition, say so.
4. **Throughput never travels alone.** Latency distribution, peak RSS, and
   device-level write bytes come with it. Write amplification is measured from
   `/proc/self/io`, never inferred from file size — they are different
   quantities.

## Profiles

`ci` runs in seconds and is **never citable**; it proves the experiments still
run. `dev` is minutes. `full` is the only profile a published claim may cite,
and every record carries which it was.

Never run two timing benchmarks concurrently. Four cores measuring each other
is not a measurement.

## Layout

| path | what |
|---|---|
| `src/` | the engine, vendored from the design artifact **verbatim** |
| `src/bench/` | the measurement substrate — stats, histogram, plotting, env capture |
| `src/bin/internal.rs` | falsification suite (f1–f7) |
| `src/bin/correctness.rs` | damaged files, model oracle, crash injection (c1–c3) |
| `bench/external/` | Supdb inside other projects' evaluations (redb, LMDB, sled) |
| `results/` | committed measurements — the source of truth |
| `figures/` | generated from `results/`, never drawn by hand |
| `docs/architecture-review.md` | why every experiment here exists |

The engine modules carry scoped `#[allow(clippy::all, dead_code)]`. They were
vendored byte-for-byte from the design artifact and have since been changed
only to fix specific defects, each described in `claims.json`. **Do not
reformat them** — the architecture review cites line numbers in commit
`101a4e7`, and `results/baseline/` holds the measurements taken against that
revision. Everything in `src/bench/`, `src/bin/` and `bench/external/` holds to
`-D warnings`.

## Measuring a change to the engine

**Never compare two separate runs.** It was tried here and it does not work:
between a pre-fix and a post-fix run of the same suite, the three *unchanged*
comparators in the external benchmark moved by +20% to +43%. Almost all of the
apparent improvement was the machine.

To measure the cost of a change, put both arms behind a runtime flag and run
them **interleaved in one process**, as `f8-checksums` does for
`Options::checksums`. Space is the exception — file size is immune to drift and
can be compared across runs.

And use `--profile full`. The same checksum cost measured at `dev` came out
"+3.0%, not significant"; at `full`, with the variance tight enough to resolve
it, it is +8.5% and unambiguous. An underpowered measurement is not a free
lunch, it is a measurement that could not see.

## Known-failing on purpose

Do not "fix" these casually; each is load-bearing evidence and each is
described in `claims.json`:

- `Store::create` truncates and there is no `Store::open` — a store cannot be
  reopened for writing.
- Reader open is super-linear in key count; the index is heap-resident per
  process, at ~131 bytes per key.
- Write throughput barely scales with writer threads.
- No usable point on the durability curve; `checkpoint` is O(key count).

If you fix one, the corresponding claim must change from `fails` to `holds` in
the same commit, and the review in `docs/` should be updated to say so.

Fixed so far, with reproducers kept in `tests/known_bugs.rs`: delete
resurrection, the double-free that handed one slot to three blocks, decoder
panics on damaged input, and silently-served corruption (now checksummed).
