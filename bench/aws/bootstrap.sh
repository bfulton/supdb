#!/bin/bash
# Runs ON the benchmark instance. Prepares a measurement-grade environment,
# builds, runs every suite, and leaves results in /home/ubuntu/out.
#
# Everything here is the reproducibility hygiene from docs/profiling.md. It is
# applied rather than assumed, because `Env::warnings()` will otherwise record
# that the numbers came from a machine that was not quiet.
set -eux
REF="${1:-main}"
REPO="${2:-https://github.com/bfulton/supdb}"

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq build-essential git valgrind linux-tools-common \
  "linux-tools-$(uname -r)" cpupower-gui linux-cloud-tools-common || true
apt-get install -y -qq "linux-tools-$(uname -r)" || echo "note: exact perf build unavailable"

# --- measurement hygiene -----------------------------------------------------
# Frequency drift, a sibling hyperthread, or ASLR moving allocations between
# runs are all indistinguishable from a code change in the output.
cpupower frequency-set -g performance 2>/dev/null || \
  for c in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do echo performance > "$c" 2>/dev/null || true; done
echo 0 > /sys/devices/system/cpu/cpufreq/boost 2>/dev/null || true
echo off > /sys/devices/system/cpu/smt/control 2>/dev/null || true
echo 0 > /proc/sys/kernel/randomize_va_space
echo -1 > /proc/sys/kernel/perf_event_paranoid
echo madvise > /sys/kernel/mm/transparent_hugepage/enabled
swapoff -a || true

# --- confirm the PMU is actually reachable -----------------------------------
# This is the entire reason for using a .metal instance. If it is not, say so
# loudly rather than producing a suite of results with a silent hole in them.
if perf stat -e cycles true 2>&1 | grep -q "<not supported>"; then
  echo "WARNING: no hardware PMU on this instance -- use a .metal size" | tee /home/ubuntu/PMU-MISSING
else
  echo "PMU OK" > /home/ubuntu/PMU-OK
fi

su - ubuntu -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal"
su - ubuntu -c "
set -eux
source \$HOME/.cargo/env
git clone --depth 50 '$REPO' supdb && cd supdb && git checkout '$REF'
cargo build --release --workspace
mkdir -p ~/out

# Pin to one core and disable ASLR for the layout-sensitive measurements.
RUN='taskset -c 2 setarch -R'

\$RUN ./target/release/internal all --profile full   --out ~/out 2>&1 | tee ~/out/internal.log
\$RUN ./target/release/external all --profile full   --out ~/out 2>&1 | tee ~/out/external.log
\$RUN ./target/release/correctness all --profile full --out ~/out 2>&1 | tee ~/out/correctness.log
\$RUN ./target/release/indexlab --profile full       --out ~/out 2>&1 | tee ~/out/indexlab.log
\$RUN ./target/release/indexlab trace --keys 10000000 2>&1 | tee ~/out/trace.log
bench/profile.sh 2>&1 | tee ~/out/cachegrind.log

# Hardware counters, if this instance has them. The events that matter for the
# index work: where the misses are, and whether the TLB is the reason.
if [ -f ~/PMU-OK ]; then
  for L in heap-hash hash+flat hash+flatfixed hash+paged; do
    perf stat -e cycles,instructions,cache-references,cache-misses,\
LLC-load-misses,dTLB-load-misses,iTLB-load-misses,branch-misses \
      ./target/release/indexlab probe --layout \"\$L\" --keys 10000000 --lookups 500000 \
      2>&1 | tee -a ~/out/perf-counters.log
  done
fi
./target/release/verify --profile full --results ~/out 2>&1 | tee ~/out/verify.log || true
tar czf ~/results.tgz -C ~ out
touch ~/DONE
"
