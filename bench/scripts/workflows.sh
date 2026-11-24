#!/bin/sh
# Check the shell inside the repository's .github/workflows/*.yml, which
# live a level up from this directory.
#
#   sh scripts/workflows.sh
#
# A workflow's `run:` block is a shell script that nothing compiles and, for
# the three self-hosted workflows, nothing runs either -- `quiet-bench` and
# `runner-smoke` are dispatch-only and `apple-silicon`'s sweep needs a Mac
# that is usually not attached. Both pickup watchdogs reached this repository
# with a block of an older draft left pasted after their `exit 1`: a stray
# `;;`, a second `case`, a second `done`. Neither had ever been able to
# start, and the only thing that would ever have said so was a dispatch on a
# day the runner was down -- the day the watchdog is the thing you need.
#
# Five rules, then:
#
#   1. Every `run:` block parses as bash.
#   2. No `run:` block contains a `${{ }}` expression. GitHub substitutes
#      those into the script as text before the shell sees them, so a value
#      carrying a quote ends the string it landed in and the rest of it runs.
#      Pass values through `env:` and read them as variables.
#   3. No job runs on `ubuntu-latest`. The label is whichever image GitHub
#      says it is that week: it moved to Ubuntu 26.04 between two runs of one
#      afternoon, where `scripts/libclang.sh` found no libclang after
#      installing it (it looked in the highest LLVM directory alone, and
#      asks each now) and `qemu-user-static` is no longer a package, and three
#      jobs of `ci` failed in their set-up on a tree that had not changed.
#      `ci` was given its image by name that day and the five workflows that
#      run by hand or after a merge were left with the label, to fail the
#      same way on the day somebody needed one. A Linux job names its image,
#      and moving to a newer one is a change made in the workflow, with the
#      scripts it needs. A comment may say the label; nothing else may.
#   4. A cache's target directory is where cargo builds. `rust-cache` takes
#      `workspace -> target` with the target a path inside the workspace,
#      and every workflow said `bench -> bench/target`, which names
#      bench/bench/target: no such directory, so the step restored nothing,
#      saved a cache with none of the suite's build in it, and reported
#      both as done, while RocksDB was built from its sources twice in every
#      job. A target is held to sitting directly in a directory with a
#      Cargo.toml, which is what a workspace's own target directory does.
#   5. The suite's build alone is one cache. Every job whose cache step
#      names `bench -> target` and nothing else builds the same
#      dependencies, `cargo build --release --bin bench`, and under the
#      action's default key, the job's name, each kept its own copy: one
#      round of the workflows that run by hand saved that build four times,
#      the repository passed GitHub's ten gigabytes, and what was evicted,
#      by last read, included two caches a pull request was using. Such a
#      step carries `shared-key: bench`, so the first of those jobs to run
#      saves the cache and the rest restore it. No other step carries it:
#      a job that restores a key whole saves nothing, so what only one job
#      builds beside the binary would never be cached under it.
set -eu

cd "$(dirname "$0")/.."

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

status=0
total=0
maps=0
shared=0
for wf in ../.github/workflows/*.yml; do
  rm -rf "$work"/blocks
  mkdir -p "$work"/blocks

  # Extract every `run:` value: a block scalar, dedented by the indent of
  # its own first line so that a heredoc body survives the trip, or a
  # one-line command, which the first version of this script skipped --
  # and most of the steps here are one-liners.
  awk -v out="$work/blocks" '
    /^[[:space:]]*(- )?run:[[:space:]]*\|/ {
      match($0, /[^ ]/); keyind = RSTART - 1
      n += 1; file = sprintf("%s/%03d.sh", out, n)
      printf "" > file
      lineof[n] = NR
      inblock = 1; blockind = -1
      next
    }
    /^[[:space:]]*(- )?run:[[:space:]]*[^|>[:space:]]/ {
      inblock = 0
      n += 1; file = sprintf("%s/%03d.sh", out, n)
      line = $0; sub(/^[[:space:]]*(- )?run:[[:space:]]*/, "", line)
      print line > file
      lineof[n] = NR
      next
    }
    inblock {
      if ($0 ~ /^[[:space:]]*$/) { print "" >> file; next }
      match($0, /[^ ]/); ind = RSTART - 1
      if (blockind < 0) {
        if (ind <= keyind) { inblock = 0 } else { blockind = ind }
      }
      if (inblock && ind >= blockind) { print substr($0, blockind + 1) >> file; next }
      inblock = 0
    }
    END { for (i = 1; i <= n; i++) printf "%03d %d\n", i, lineof[i] }
  ' "$wf" > "$work"/index

  while read -r id line; do
    total=$((total + 1))
    block="$work/blocks/$id.sh"
    if ! err=$(bash -n "$block" 2>&1); then
      echo "$wf:$line: run block does not parse as bash" >&2
      echo "$err" | sed "s|$block|  |" >&2
      status=1
    fi
    if grep -n '\${{' "$block" >/dev/null 2>&1; then
      echo "$wf:$line: run block interpolates a \${{ }} expression;" >&2
      echo "  pass the value through env: and read it as a variable" >&2
      grep -n '\${{' "$block" | sed 's/^/    /' >&2
      status=1
    fi
  done < "$work"/index

  # The fourth rule: each `workspace -> target` line names a directory
  # whose parent is a cargo workspace's root.
  grep -n -E -- '^[[:space:]]*(workspaces:[[:space:]]*)?[^[:space:]#]+ -> [^[:space:]]+[[:space:]]*$' "$wf" > "$work"/maps || true
  while IFS= read -r m; do
    line=${m%%:*}
    pair=$(printf '%s\n' "${m#*:}" | sed 's/^[[:space:]]*\(workspaces:\)\{0,1\}[[:space:]]*//')
    ws=$(printf '%s\n' "${pair%% -> *}" | sed 's/[[:space:]]*$//')
    target=$(printf '%s\n' "${pair#* -> }" | sed 's/[[:space:]]*$//')
    parent=$(dirname "../$ws/$target")
    maps=$((maps + 1))
    if [ ! -f "$parent/Cargo.toml" ]; then
      echo "$wf:$line: the cache's target is $ws/$target, and $ws/$(dirname "$target") is no workspace's root;" >&2
      echo "  the target of \`$ws -> $target\` is a path inside $ws" >&2
      status=1
    fi
  done < "$work"/maps

  # The fifth rule, over each rust-cache step: its lines run from `uses:`
  # to the next line indented no deeper than the step's own dash.
  if ! awk -v wf="$wf" -v count="$work/shared" '
    function close_step() {
      if (!instep) return
      instep = 0
      alone = (nmaps == 1 && map == "bench -> target")
      if (alone) n += 1
      if (alone && key != "bench") {
        printf "%s:%d: the step caches bench/target alone under a key of its own (%s);\n", wf, at, (key == "" ? "the job name" : key)
        printf "  give it `shared-key: bench`, the one cache every such job restores\n"
        bad = 1
      }
      if (!alone && key == "bench") {
        printf "%s:%d: the step takes `shared-key: bench` and does not cache bench/target alone;\n", wf, at
        printf "  a job that restores that key whole saves nothing, so what it builds beside the binary is never cached\n"
        bad = 1
      }
    }
    {
      blank = ($0 ~ /^[[:space:]]*$/)
      ind = 0
      if (!blank) { match($0, /[^ ]/); ind = RSTART - 1 }
    }
    instep && !blank && ind <= stepind { close_step() }
    /^[[:space:]]*(- )?uses:[[:space:]]*Swatinem\/rust-cache@/ {
      close_step()
      instep = 1; at = NR; nmaps = 0; map = ""; key = ""
      stepind = ($0 ~ /^[[:space:]]*- /) ? ind : ind - 2
      next
    }
    instep && !blank && $0 !~ /^[[:space:]]*#/ {
      line = $0
      sub(/^[[:space:]]*/, "", line); sub(/[[:space:]]*$/, "", line)
      if (line ~ / -> /) {
        sub(/^workspaces:[[:space:]]*/, "", line)
        nmaps += 1; map = line
      } else if (line ~ /^shared-key:/) {
        sub(/^shared-key:[[:space:]]*/, "", line)
        key = line
      }
    }
    END { close_step(); print n + 0 > count; exit bad }
  ' "$wf" >&2; then
    status=1
  fi
  shared=$((shared + $(cat "$work"/shared)))

  # The third rule, over the file's lines that are not comments.
  if floating=$(grep -n 'ubuntu-latest' "$wf" | grep -v '^[0-9]*:[[:space:]]*#'); then
    echo "$wf: a job runs on ubuntu-latest; name the image (ubuntu-24.04)" >&2
    echo "$floating" | sed 's/^/    /' >&2
    status=1
  fi
done

if [ "$status" -ne 0 ]; then
  exit 1
fi
if [ "$maps" -eq 0 ]; then
  echo "workflows: no \`workspace -> target\` line was found to check; the fourth rule ran over nothing" >&2
  exit 1
fi
if [ "$shared" -eq 0 ]; then
  echo "workflows: no cache step of bench/target alone was found to check; the fifth rule ran over nothing" >&2
  exit 1
fi
echo "workflows: every run block parses, none interpolates ($total blocks); every Linux job names its image; every cache's target is a workspace's ($maps); the suite's build alone is one cache ($shared steps)"
