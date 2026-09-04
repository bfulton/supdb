#!/bin/sh
# Every claim id cited in the source must resolve to a claim.
#
# Comments here argue from measurements and cite them by id, which is only
# useful while the id resolves. Retiring an experiment leaves the prose behind
# citing something a reader cannot look up -- and nothing noticed, because
# `verify` reads claims.json and results/ and never reads the source. Thirteen
# such citations had accumulated, several of them in the engine's own module
# doc, all naming claims that retired with the engine they described.
#
# The numbers in that prose are worth keeping; the dead ids are not. Attribute
# to the experiment by name instead -- `f38 priced it at 90ns a segment` --
# which stays true after the experiment is gone.
#
# Ids in the plan files (R-numbers, the registered asks) are a different
# namespace and are not checked here; they resolve to `*-plan.md`.
set -eu
cd "$(dirname "$0")/.."

# `find -exec +` rather than `grep -r --include`: this project is measured on
# Apple Silicon as well as x86, so a check a contributor cannot run on a Mac
# is a check that runs in one place. `-exec +` also does the right thing with
# no matching files, where `xargs grep` would read stdin and hang.
# `docs` and `README.md` are in here because prose is where the drift lives.
# The gate was source-only and the documents had accumulated thirty-nine dead
# citations, most of them naming claims that retired with the engine they
# described -- which is the exact fault this script was written for, in the
# files a reader is most likely to be holding.
#
# The plan files are deliberately out. They are working notes, their R-numbers
# are a different namespace resolving to `*-plan.md`, and a plan that recorded
# a prediction under an id the run later renamed is a true record of what was
# predicted rather than a broken pointer.
scan() {
  find src web bench tests docs README.md \
    \( -name '*.rs' -o -name '*.mjs' -o -name '*.md' \) -type f \
    -exec grep "$@" {} + 2>/dev/null
}

# No `\b`: it is a GNU extension rather than POSIX ERE, and a grep that does
# not know it matches nothing -- which would leave `cited` empty, skip the
# loop below and print success. A gate that reports a verdict it has not
# earned is the shape of every other gate failure in this repository, so the
# boundaries are done with the portable trick instead: pull in any identifier
# characters either side of a candidate, then require the whole extraction to
# be exactly an id. `XF12.3` and `F12.3a` extract whole and are rejected;
# `F12.3` extracts alone and is kept.
id_re='(EXT|[FCW][0-9]+)\.[0-9]+'
ids=$(grep -oE '"id": "[A-Z]+[0-9]*\.[0-9]+"' claims.json | sed 's/.*"id": "//; s/"//')
cited=$(scan -hoE "[A-Za-z0-9_]*${id_re}[A-Za-z0-9_]*" \
          | grep -xE "$id_re" | sort -u || true)

# And the guard the comment above argues for. Every one of these files cites
# claims; finding none means the search broke, not that the source went
# quiet.
if [ -z "$ids" ]; then
  echo "no claim ids parsed out of claims.json -- the gate cannot run"
  exit 1
fi
if [ -z "$cited" ]; then
  echo "no claim ids found cited in src, web, bench, tests or docs -- the gate cannot"
  echo "have passed, since these files are full of them. Check the extraction."
  exit 1
fi

missing=
for c in $cited; do
  found=no
  for i in $ids; do
    [ "$c" = "$i" ] && { found=yes; break; }
  done
  [ "$found" = no ] && missing="$missing $c"
done

if [ -n "$missing" ]; then
  echo "cited in the source or docs but not registered in claims.json:"
  for m in $missing; do
    echo "  $m"
    scan -nF "$m" | sed 's/^/      /' | cut -c1-120 || true
  done
  echo
  echo "A citation is only useful while it resolves. Either register the claim,"
  echo "or attribute the measurement to its experiment by name instead."
  exit 1
fi
echo "every claim id cited in the source resolves"
