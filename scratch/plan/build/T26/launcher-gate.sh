#!/bin/sh
# The launcher's own gate over the builder's finished copy (keep/v3), after the run was stopped for quota.
cd /home/user/supdb
T=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T26g
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T26g
mkdir -p $T; export TMPDIR=$T
sh scripts/check.sh build test stride lint; echo "EXIT1 $?"
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T26g
mkdir -p $T
SUPDB_BASE=3c2b64d2d0b8b4505e2d29e1a3c5a1a1b06cf015 sh scripts/check.sh lists; echo "EXIT2 $?"
sh scripts/check.sh wasm; echo "EXIT3 $?"
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T26g
echo "EXIT done"
