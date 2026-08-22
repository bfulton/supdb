# Running the suites on AWS

The local machine is a Firecracker guest with no PMU, so hardware counters are
unavailable at any privilege level. AWS `.metal` sizes are the reliable way to
get them.

```sh
export KEY_NAME=my-keypair SECURITY_GROUP=sg-0123... AWS_REGION=us-east-1
bench/aws/run.sh c7g.metal            # ARM64, Graviton3
bench/aws/run.sh c6i.metal            # x86-64, Ice Lake
```

It launches a spot instance, applies the measurement hygiene from
`docs/profiling.md`, builds, runs every suite plus cachegrind and `perf stat`,
copies the results into `results/aws-<type>-<timestamp>/`, and terminates.
`KEEP=1` leaves it up; `ON_DEMAND=1` skips spot.

## Which instance

**Only `.metal` sizes expose the PMU.** Virtualised Nitro instances report
every hardware event as `<not supported>` — the same wall we hit locally. If
you do not need counters, any instance will do and is far cheaper.

| type | arch | line | page | notes |
|---|---|---|---|---|
| `c6i.metal` | x86-64 Ice Lake | 64 B | 4 KiB | closest to the current results |
| `c7i.metal-24xl` | x86-64 Sapphire Rapids | 64 B | 4 KiB | newest x86, best TMA support |
| `c7g.metal` | ARM64 Graviton3 | 64 B | 4 KiB | ARM *server* |
| `c8g.metal-24xl` | ARM64 Graviton4 | 64 B | 4 KiB | newest Graviton |

Roughly $2–6/hour on demand, ~70% less on spot. A full sweep is about an hour,
so a complete cross-architecture run costs a few dollars.

## "ARM" is not one target

This matters for the tuning work and is easy to get wrong.

Graviton has **64-byte cache lines and 4 KiB pages** — the same geometry as
x86. Apple Silicon has **128-byte lines and 16 KiB pages**. They are both
ARM64 and they are different machines for every question this project has been
asking: distinct-lines-per-lookup halves on Apple Silicon, and TLB reach is
four times better before huge pages enter the discussion.

So Graviton tells you about ARM *servers*. It does not tell you about a
developer's laptop. If both matter, both have to be measured, and the tuning
constants have to be derived at runtime rather than compiled in.

## What perf gives you there that nothing gives you here

- `perf stat` — real cache and dTLB miss counts, rather than a simulation.
- `perf mem` / `perf c2c` — which data structure missed, and false sharing
  between writer threads. `c2c` is the direct instrument for the appender-lock
  convoy in `f6-threads`.
- `toplev` (from `pmu-tools`) — Top-Down analysis. On x86 this is the highest
  value tool available: it would have identified the varint decode as core-bound
  in one command, rather than the three rounds of hypothesis-and-control it
  actually took.
