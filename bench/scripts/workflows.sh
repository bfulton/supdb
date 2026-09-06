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
# Two rules, then:
#
#   1. Every `run:` block parses as bash.
#   2. No `run:` block contains a `${{ }}` expression. GitHub substitutes
#      those into the script as text before the shell sees them, so a value
#      carrying a quote ends the string it landed in and the rest of it runs.
#      Pass values through `env:` and read them as variables.
set -eu

cd "$(dirname "$0")/.."

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

status=0
for wf in ../.github/workflows/*.yml; do
  rm -rf "$work"/blocks
  mkdir -p "$work"/blocks

  # Extract each block scalar under a `run:` key, dedented by the indent of
  # its own first line so that a heredoc body survives the trip.
  awk -v out="$work/blocks" '
    /^[[:space:]]*(- )?run:[[:space:]]*\|/ {
      match($0, /[^ ]/); keyind = RSTART - 1
      n += 1; file = sprintf("%s/%03d.sh", out, n)
      printf "" > file
      lineof[n] = NR
      inblock = 1; blockind = -1
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
done

if [ "$status" -ne 0 ]; then
  exit 1
fi
echo "workflows: every run block parses, none interpolates"
