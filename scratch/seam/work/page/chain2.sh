#!/bin/bash
W=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/page
cd /home/user/supdb; [ -z "$(git status --porcelain)" ] || { echo DIRTY; exit 1; }
cd $W/pairs
BIN=$W/bench-page sh $W/sit.sh scan-xdyn85 supdb-cipherxdyn85 supdb-nozpages 5000000 1 --bound 671088640 --passes scan --ops-div 1
EXTRA=SUPDB_MIX_ONLY=CC BIN=$W/bench-page sh $W/sit.sh mixc-page supdb-cipherxdyn85 supdb-nozpages 5000000 1 --bound 671088640 --passes mixes --ops-div 1
EXTRA=SUPDB_MIX_ONLY=CC BIN=$W/bench-p4080 sh $W/sit.sh mixc-4080 supdb-cipher supdb-nozpages 5000000 1 --bound 671088640 --passes mixes --ops-div 1
EXTRA=SUPDB_MIX_ONLY=CC BIN=$W/bench-p4080 sh $W/sit.sh mixc-clear supdb-nozpages supdb-nozpages 5000000 1 --bound 519045120 --passes mixes --ops-div 1
echo CHAIN2 DONE
