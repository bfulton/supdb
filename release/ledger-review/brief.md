# Options ledger: the judgment pass

Repository: /home/user/supdb (public bfulton/supdb). C0 = b3a043dc; the
checkout's HEAD differs from C0 only under release/. Phase 1 of the pre-tag
plan deletes options and dead code; this pass decides, option by option,
what phase 1 does. Nobody edits code in this pass.

Input: /home/user/supdb/release/options.csv (header row, then one row per
option; columns: struct, name, type, default, doc_hidden, doc_markers,
doc_lines, arms_in_doc, read_sites_src, read_files, write_sites_src,
bench_policy_field, bench_arms_setting, bench_arm_names, bench_env,
tests_setting, fuzz_drawn, docs_mentions, docs_files, flags, ...). The tool
that made it is release/tools/options_ledger.py; its header says what each
column counts. Read your rows with python3's csv module.

For each of your rows, read the option's rustdoc in the source (the struct
is in src/db.rs, src/blob.rs, src/db/vault.rs or src/crypt.rs; use
`grep -n` for the field, then `sed -n` a range), and as needed its read
sites, the arms that set it (bench/src/engines.rs: the `Policy` field it
maps from and the arms in `open`), and any mention in ROADMAP.md,
bench/DESIGN.md and docs/engine.md.

The suite's rosters (the arms every row runs) are these names; every other
arm is run only when someone names it:
  supdb supdb-cache256 supdb-cache256tofit supdb-cipher supdb-forms
  supdb-heldnoframes supdb-heldpart supdb-ingest supdb-ingestcipher
  supdb-noadvice supdb-nocache supdb-nosnap supdb-nozpages supdb-settle
  (and the comparators: lmdb*, rocksdb*, faster, enc)

Categories, exactly one per row:
- keep-stable: a knob a user of the library sets for their own reasons
  (durability, encryption, sizes, budgets, the forms a store may use) and
  that the docs present as such.
- keep-experimental: prices a shipped default and is still needed: a
  roster arm sets it, or an open ROADMAP.md item, or a bench/DESIGN.md
  measurement still to be repeated, names it or its arm.
- inline: the default is decided (the shipped default is the measured
  winner, or the off position is a known wrong answer: "a fix for a wrong
  answer is not an arm") and no roster arm and no open roadmap item needs
  the other shape. Phase 1 removes the option, keeps the default's
  behaviour, and removes the other branch and the arms that set it.
- delete-refuted: the option's non-default shape was tried and refuted
  (its rustdoc says so, or a cited pairing does), the default is the shape
  that ships, and no roster arm or open roadmap item needs the refuted
  shape. Phase 1 removes the option, the refuted branch and its arms.
- delete-unread: nothing sets it anywhere (the `never_set` flag). Phase 1
  removes it as `inline` would.
- test-seam: only tests set it (`tests_only`), it exists to hold a state a
  test needs. Phase 1 keeps it hidden, or moves it behind cfg(test) where
  only unit tests in src/ use it.
- escalate: you cannot classify it from the evidence; say exactly what is
  missing.

Rules that bind the judgment (from the repository's CLAUDE.md): the
comparison arms in Options are not dead code, since the suite prices a new
shape against the old one in one process, so check bench/src/engines.rs
before calling one removable; an arm kept for pricing keeps that shape's
other settings; an option sound only under another option's value is
checked where the store opens. Where removing an option would also remove
an arm that a test in bench/ or tests/ names, say so in the note.

When in doubt between keep-experimental and inline or delete, choose
keep-experimental and say why in the note; phase 1 can always delete more
later, and a wrong deletion removes an experiment.

Output: a CSV with exactly these columns and one line per row of your
slice, in the input's order: struct,name,category,decision,note
- decision: one short imperative ("keep", "keep hidden", "inline at true:
  drop the off branch and arms supdb-x, supdb-y", "delete with arm
  supdb-z", "move behind cfg(test)", "escalate").
- note: "[cluster] reason with evidence", where cluster is a short family
  name shared by options phase 1 would remove in one commit (for example
  upkeep, zpages-build, wal-ahead, seal, scan-snapshot), and the evidence
  is the rustdoc phrase, the roster arm, or the roadmap item, by name.
Quote fields that contain commas. Write it to the path your task gives.

Rules for you: read only; never run cargo, rustc, a test or a benchmark;
git read-only (show, grep, log); create no file in /home/user/supdb; write
only your output CSV and, if you need them, scratch files in your own
directory. Then return a summary: counts by category, the escalations with
their questions, and the clusters you named.
