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

ids=$(grep -oE '"id": "[A-Z]+[0-9]*\.[0-9]+"' claims.json | sed 's/.*"id": "//; s/"//')
cited=$(grep -rhoE '\b([FCW][0-9]+|EXT)\.[0-9]+\b' \
          src web bench tests --include='*.rs' --include='*.mjs' 2>/dev/null | sort -u)

missing=
for c in $cited; do
  found=no
  for i in $ids; do
    [ "$c" = "$i" ] && { found=yes; break; }
  done
  [ "$found" = no ] && missing="$missing $c"
done

if [ -n "$missing" ]; then
  echo "cited in the source but not registered in claims.json:"
  for m in $missing; do
    echo "  $m"
    grep -rn "\\b$m\\b" src web bench tests --include='*.rs' --include='*.mjs' 2>/dev/null \
      | sed 's/^/      /' | cut -c1-120
  done
  echo
  echo "A citation is only useful while it resolves. Either register the claim,"
  echo "or attribute the measurement to its experiment by name instead."
  exit 1
fi
echo "every claim id cited in the source resolves"
