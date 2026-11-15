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
# Three rules, then:
#
#   1. Every `run:` block parses as bash.
#   2. No `run:` block contains a `${{ }}` expression. GitHub substitutes
#      those into the script as text before the shell sees them, so a value
#      carrying a quote ends the string it landed in and the rest of it runs.
#      Pass values through `env:` and read them as variables.
#   3. No job runs on `ubuntu-latest`. The label is whichever image GitHub
#      says it is that week: it moved to Ubuntu 26.04 between two runs of one
#      afternoon, where `scripts/libclang.sh` finds no libclang after
#      installing it and `qemu-user-static` is no longer a package, and three
#      jobs of `ci` failed in their set-up on a tree that had not changed.
#      `ci` was given its image by name that day and the five workflows that
#      run by hand or after a merge were left with the label, to fail the
#      same way on the day somebody needed one. A Linux job names its image,
#      and moving to a newer one is a change made in the workflow, with the
#      scripts it needs. A comment may say the label; nothing else may.
set -eu

cd "$(dirname "$0")/.."

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

status=0
total=0
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
echo "workflows: every run block parses, none interpolates ($total blocks); every Linux job names its image"
