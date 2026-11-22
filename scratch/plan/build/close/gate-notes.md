# close: full gate on HEAD 31b33339
Step: default gate running. Log: close/gate.log (EXIT line). TMPDIR close/tmp/gate.
- default gate: EXIT 0 (gate.log), 09:41-10:00 UTC. lists vs ea635218: EXIT 0 (lists-base.log).
- seal-red 14 s, seal-red-red 43 s, ~100 MB disk peak each (seal-red*.log).
- removed bench/target/checked (5.5G build cache) before the count for disk.
- cost: running, COST_BASE=7c62153e, log close/cost.log, COST_DIR close/tmp/cost/w.
- cost: EXIT 1 RED by function as expected, 3227 s, min avail 4.6G; totals all under upstream; vs M1 r3 only seal std::fs::OpenOptions::_open 804->944 and work abandon_look 5548->5662 (in slack).
- cross-arm: red 2 of 5 parallel runs, 'QEMU internal SIGSEGV' (qemu 8.2.2's own crash); green 3 of 5, and green with --test-threads=1 (530 passed, 639 s). machine: exit 2 (Linux host). Done; tree clean.
