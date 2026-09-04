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

## Cost, and not being surprised by it

Three independent stops, because one is not enough and the weakest of them was
the original design:

1. **A watchdog on the instance.** `bootstrap.sh` runs `shutdown -h +240` as its
   very first action, before it installs anything, and the instance launches
   with `--instance-initiated-shutdown-behavior terminate`. The instance ends
   itself after four hours no matter what — whether the run finished, whether
   ssh ever connected, whether the launching machine still exists. Override with
   `MAX_MINUTES=90`.
2. **A spot ceiling.** `MAX_PRICE` (default `1.00`) caps the hourly rate, so a
   spot price spike cannot quietly bill at on-demand rates.
3. **A reaper.** `bench/aws/reap.sh --list` sweeps every region for anything
   tagged `supdb-bench`; without `--list` it terminates them. It should always
   find nothing.

**What was wrong before:** teardown was a shell `trap` in the launching process.
That is not a guarantee. If the launching machine dies mid-run the trap never
fires and the instance bills until somebody notices — and the machine this was
developed on restarts on its own. The bootstrap now runs from user-data so the
watchdog is armed at boot rather than pushed over ssh, and nothing about
termination depends on the launcher surviving.

## Account-level caps, which are worth more than any script

A script you trust is worse than a limit that cannot be exceeded.

- **Service Quotas** are the only true hard stop. Set *Running On-Demand
  Standard instances* to a low vCPU count (say 200, enough for one `.metal`) and
  you cannot launch a second one by accident, script bug or otherwise.
- **AWS Budgets alert; Budget Actions stop.** A plain budget only emails you. A
  *Budget Action* can attach a deny policy at a threshold. Only the second is a
  cap.
- **A scoped IAM user** for this work: allow `ec2:RunInstances` only in one
  region, only with `ec2:InstanceType` in an explicit list, and require the
  `supdb-bench` tag. Then the credential cannot launch anything expensive even
  if the script is wrong.
- **A separate sub-account** under Organizations with its own small budget, if
  you want the blast radius bounded by construction.

## The cheap ladder — and most of it is free

`.metal` buys exactly one thing: hardware performance counters. Everything else
runs anywhere, and the single most important open question cannot be answered on
AWS at all.

| what you want | where | cost |
|---|---|---|
| ARM correctness, weak memory ordering | GitHub Actions `ubuntu-24.04-arm` (already in CI) | free |
| ARM build + logic | local cross-compile + qemu (already wired) | free |
| simulated cache misses | cachegrind, any machine | free |
| **128-byte lines / 16 KiB pages** | **your Mac — no AWS instance has this** | free |
| ARM server timings | `c7g.large` ≈ $0.07/hr, `c7g.xlarge` ≈ $0.15/hr | cents |
| real PMU counters, `toplev`, `perf c2c` | `c7g.metal` / `c6i.metal`, spot | ~$0.60–1.80/hr |

A full sweep is about an hour. On spot that is roughly a dollar or two per
architecture, once.

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

## A least-privilege identity

`iam/bench-policy.json` is the smallest policy that runs everything here, and
`iam/setup.sh` applies it and then removes whatever else is attached.

Run it yourself with admin credentials; it does not need an agent. The order is
deliberate — the scoped policy is created **and verified with the IAM policy
simulator** before anything is detached, so a mistake is caught while you still
have the privileges to fix it. If the simulation disagrees with intent, the
script stops and leaves admin in place.

```sh
bench/aws/iam/setup.sh supdb-bench-user
```

What the policy allows: EC2 `Describe*`, `RunInstances` restricted to an
explicit instance-type list *and* requiring the `supdb-bench` tag,
`CreateTags` only as part of a launch, and `TerminateInstances` only on
instances already carrying that tag.

What it denies outright, regardless of anything else attached: all of `iam:*`
and `sts:AssumeRole`, so the credential cannot grant itself more; security
group and key pair creation, so it cannot open network access to what it
launches; and everything outside that EC2 subset via a `NotAction` deny, so a
future service cannot be reached by default.

Recovery, if the reduced policy turns out to be wrong:

```sh
aws iam attach-user-policy --user-name supdb-bench-user \
  --policy-arn arn:aws:iam::aws:policy/AdministratorAccess
```
