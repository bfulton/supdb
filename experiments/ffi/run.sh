#!/bin/sh
# Build both halves and run them: the C harness loads the stores and reads
# them natively; bench.py reopens them from Python.
#   sh run.sh <root> <keys> <value_bytes> <reps>
set -e
cd "$(dirname "$0")"
ROOT=${1:?root}; KEYS=${2:-300000}; VLEN=${3:-100}; REPS=${4:-5}
# Cargo's own status decides, not the filter's: a failed build that left a
# library behind would otherwise link and measure stale engine code.
LOG=$(mktemp)
if ! (cd capi && cargo build --release) > "$LOG" 2>&1; then
  cat "$LOG"; rm -f "$LOG"; exit 1
fi
grep -E "^(warning|error)|Finished" "$LOG"
rm -f "$LOG"
LIB=$PWD/capi/target/release
cc -O2 -Wall -o harness harness.c -L"$LIB" -lsupdb_capi -llmdb -lrocksdb -Wl,-rpath,"$LIB"
PY=${PYTHON:-python3}
INC=$($PY -c 'import sysconfig;print(sysconfig.get_paths()["include"])')
SUF=$($PY -c 'import sysconfig;print(sysconfig.get_config_var("EXT_SUFFIX"))')
cc -O2 -Wall -shared -fPIC -I"$INC" -o "supdbpy$SUF" supdbpy.c -L"$LIB" -lsupdb_capi -Wl,-rpath,"$LIB"
rm -rf "$ROOT"; mkdir -p "$ROOT"
./harness "$ROOT" "$KEYS" "$VLEN" "$REPS" load
$PY bench.py "$ROOT" "$KEYS" "$VLEN" "$REPS"
