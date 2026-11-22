#!/bin/sh
# Put the finished copies back, each checked by its sum.
K=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T26/keep/v3
cp $K/ordindex.rs /home/user/supdb/src/ordindex.rs && cp $K/db.rs /home/user/supdb/src/db.rs && cp $K/vault.rs /home/user/supdb/src/db/vault.rs && \
 (cd /home/user/supdb/src && sha256sum ordindex.rs db.rs db/vault.rs | sed 's#db/vault.rs#vault.rs#' | diff - $K/SHA256SUMS) && echo RESTORED-OK
