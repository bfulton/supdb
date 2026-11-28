#!/bin/sh
# The differential fuzz past the gate's ranges, one process per block of
# seeds, each block in a TMPDIR of its own under a timeout, so that a hang
# is a result and not a stall. Every block's summary line is kept, so the
# mechanism counts (chunks decrypted, one-file counts) are read beside the
# verdict.
#
#   sh release/findings/hostile-input/fuzz-long.sh MODE FROM TO [BLOCK] [JOBS]
#
# MODE: plain | cipher (ZFUZZ_CIPHER=1) | onefile (ZFUZZ_ONEFILE=1) |
#       cipher-test (the fuzz_range_under_a_cipher* tests) | optvary
#       (ZFUZZ_OPTVARY=1 ZFUZZ_CACHE=all) | concurrent (concurrent_range)
# Output: $OUT (default ./fuzz-MODE-FROM-TO.log); exit 1 if any block failed
# or did not finish.
set -eu
cd "$(dirname "$0")/../../.."
mode=$1 from=$2 to=$3 block=${4:-50} jobs=${5:-3}
out=${OUT:-$PWD/release/findings/hostile-input/out/fuzz-$mode-$from-$to.log}
mkdir -p "$(dirname "$out")"
bin=$(cargo test --profile checked --test fuzz --no-run 2>&1 |
  sed -n 's/.*Executable .*(\(.*\))/\1/p')
[ -x "$bin" ] || { echo "no fuzz binary" >&2; exit 2; }
"$bin" --list | grep -q '^fuzz_range: test' || { echo "fuzz_range not listed" >&2; exit 2; }
base=${TMPDIR:-/tmp}/supdb-fuzzlong-$$
mkdir -p "$base"
base=$(cd "$base" && pwd -P)
case $mode in
  plain) envs="" test=fuzz_range ;;
  cipher) envs="ZFUZZ_CIPHER=1" test=fuzz_range ;;
  onefile) envs="ZFUZZ_ONEFILE=1" test=fuzz_range ;;
  cipher-test) envs="" test=fuzz_range_under_a_cipher_that_does_not_fit_its_cache ;;
  optvary) envs="ZFUZZ_OPTVARY=1 ZFUZZ_CACHE=all" test=fuzz_range ;;
  onefile-optvary) envs="ZFUZZ_ONEFILE=1 ZFUZZ_OPTVARY=1 ZFUZZ_CACHE=all" test=fuzz_range ;;
  concurrent) envs="" test=concurrent_range ;;
  *) echo "unknown mode $mode" >&2; exit 2 ;;
esac
: > "$out"
s=$from
blocks=""
while [ "$s" -lt "$to" ]; do
  e=$((s + block)); [ "$e" -gt "$to" ] && e=$to
  blocks="$blocks $s:$e"
  s=$e
done
echo "$blocks" | tr ' ' '\n' | grep . | xargs -P "$jobs" -I{} sh -c '
  r={}; a=${r%:*}; b=${r#*:}
  d='"$base"'/$a; mkdir -p "$d"
  if env TMPDIR="$d" '"$envs"' ZFUZZ_FROM=$a ZFUZZ_TO=$b RUST_BACKTRACE=1 \
      timeout 1800 '"$bin"' '"$test"' --exact --nocapture > "$d.log" 2>&1; then
    v=ok
  else
    v="FAIL($?)"
  fi
  printf "%s %s..%s %s\n" "$v" "$a" "$b" "$(grep -m1 "^seeds " "$d.log" | cut -c1-400)" >> '"$out"'
  case $v in ok) rm -rf "$d" "$d.log" ;; esac
'
fails=$(grep -c -v '^ok ' "$out" || true)
echo "$mode $from..$to: $(grep -c '^ok ' "$out") blocks ok, $fails failed; log $out"
[ "$fails" -eq 0 ] || { echo "failed blocks' logs under $base" >&2; exit 1; }
rm -rf "$base"
