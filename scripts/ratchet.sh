#!/bin/sh
# The lists that only go down (tests/data's, as scripts/ratchet.py's
# registry names them), held against the commit the change is made on.
#
#   sh scripts/ratchet.sh          # the base scripts/base.sh decides
#   sh scripts/ratchet.sh BASE     # against BASE's merge-base with HEAD
#
# Run in the repository to check: its working tree is the tree. Exits 0
# green, 1 where a list grew past what scripts/raises.txt declares (or a
# list or a declaration is wrong in the ways scripts/ratchet.py names), 2
# where no base could be decided or a file does not parse. The verdict is
# the last line: `ratchet: green` or `ratchet: RED`, every raise let
# through printed above it.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(git rev-parse --show-toplevel) || { echo "ratchet: not in a git repository"; echo "ratchet: RED"; exit 2; }
if [ $# -gt 0 ]; then
  base=$1
else
  base=$(sh "$here/base.sh") || { echo "ratchet: no base (above)"; echo "ratchet: RED"; exit 2; }
fi
python3 "$here/ratchet.py" "$root" "$base"
