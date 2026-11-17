# open-order notes (in progress)
base HEAD: see base-head
step: 1 reading
step 2 (started): design settled (see below); writing tests into tests/durable.rs section "open-order" + seam Shared::open_synced / Db::open_synced_files.
Design: open order = read slots (+torn flag) -> pending skip -> choose -> list -> CHECKS (no m: refuse if par-/pcs- (plain) or segment envelopes (cipher); F1 every name present; fallback(torn or skipped, plain): replay first, F2 unnamed intact U with end_seq > covered needs replay next >= end_seq, F3 unnamed U no temp id>=next_id first/last key held by named/temp-backed/replayed) -> then all mutations (vault sweep, make_slots, orphan sweep, pending fsyncs+dir before durable slot, tmp recovery, wal cut/removals).
step 2: section appended to tests/durable.rs (copy at section.rs)
step 3: fix applied (fix.py, fix2.py); building
step 2 done: before.log = 9 of 11 fail at their contract assertions (2 guards pass: windows, zeroed control). See before.log.
step 3 done: fix in; after-mine.log all 11 pass.
step 4: neighbours running via neigh.sh -> neigh-<name>.log, status in neigh.status
step 4: src edits (fell_back = slot_failed; docs), 'inputs' window case + ord record assertion added; neighbours restarted (neigh.sh)
step 4b: FIXED COPIES = keep/*.fixed (src_db.rs.fixed, src_db_vault.rs.fixed, tests_durable.rs.fixed, CLAUDE.md.fixed, ROADMAP.md.fixed), sha256 in keep/sha256.txt. If src/db.rs differs from keep/src_db.rs.fixed and mut-current.txt names a mutant, a mutant is in the tree: copy the fixed one back.
step 4c: neighbours on the fixed tree: neigh.sh -> neigh-*.log, neigh.status
step 4d: fixed copies re-saved after O7-variant test + doc rewrap (keep/sha256.txt)
step 5 prepared: mut.py (results appended per mutant to mut-results.jsonl, logs m-<id>.log/.diff, mut-current.txt names a mutant in the tree). Will run after neighbours finish.
