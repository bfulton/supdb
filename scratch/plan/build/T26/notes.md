# T26 merge-gainers: notes

Base (SUPDB_BASE, every count): 3c2b64d2d0b8b4505e2d29e1a3c5a1a1b06cf015. HEAD at start 2e7de0ee
(scratch/ only on top, checked: git diff --name-only base HEAD names only scratch/).

## Step now
1. listings at the tree (listing/base.*.lst; listing/instr.py lays callgrind --dump-instr to
   objdump + addr2line -i). Original files kept: keep/orig/ with SHA256SUMS.

## What in the tree differs from the plan (682b2c09)
This change has no plan section (brief.txt is its section). The brief's numbers (close/cost.log)
are against a tree of upstream's this work does not build; per the owner, counts here are against
the base commit only.

## Findings so far (merge probe, 200,000 ops; listings under listing/)
- floor = listing/floor.py (cipher arms as text out); floor2 = iterA.py's ordindex part (head's lim
  test deleted outright, top reads the field).
- whole_key_at: base 60,744,515; floor (assert shape) 61,322,932; floor2 (no test) 58,566,072 =
  close/cost.log's left column exactly. So the +10.9 an op was the cipher's two arms: head's lim
  test (cmp/jae, 689k calls) and top's is_empty test (110k), with the register shuffles they made.
- seal_table::{closure#4}: base 16,698,736; floor 15,698,864 = left column (15,698,832). The four
  door calls (remove_file, len, rename, persist) in the closure made LLVM keep the captured table
  in a stack slot reloaded 8 times a key (mov 0x8(%rsp); mov 0xc38(..)) where the floor kept it in
  r12. As_path (iterA) took 400k; lazy persist (iterB) nothing in the loop; binding `mem` once as
  &MemTable (iterC) 14,299,492.
- Db::append: base 34,001,535; floor 34,001,319 (cipher's logged() arm out). Not the cipher's.
- Emitter::enter: base 14,400,234; floor 13,800,228 = left column. The door's remove_file(&tmp)
  took the address of the per-key PathBuf clone (3 stores a key). Taking out the clone: 10,200,198.

## The change (keep/v1/, SHA256SUMS; keep/v1/change.diff)
- src/ordindex.rs: whole_key_at's plain half tests `rank >= lim` once (lim is n for a mapped file,
  zero for one read by the piece) and reads top_eager / the heads in memory (head_in_memory, the
  unchecked read under that bound); everything else tail-calls whole_key_at_by_piece (#[cold],
  #[inline(never)]), which is the old body (n bound, top(), head()). key_from_head shared.
  head() now calls head_in_memory after its own lim test (same code). New unit test
  a_whole_key_by_the_piece_is_the_mapped_ones (red-first: before.log).
- src/db.rs Emitter::enter: no PathBuf clone a rank; the temp name borrowed in the branch that
  opens a piece.
- src/db.rs seal closure: `let mem: &MemTable = &mem;` before the walk; tmp.as_path() for the
  door's remove_file, len, rename.
- src/db/vault.rs persist: takes the pairs as a closure, called only under a cipher; the four
  callers (seal, the closing run's seal, compact_run, tier_walk) no longer build the Vec of names
  for a plain store. Test call sites updated.
- Db::append: no change (floor = base).
- join_seal: no change (see below).

## Gate state
- after.log = suite.log + suite2.log. suite.log: whole suite --no-fail-fast, EXIT 101 with leak
  (1) and zpages (4) failing on "one form file mapped: []" -- my TMPDIR had a `..` in it, the
  case CLAUDE.md names (the maps match by the resolved path). suite2.log: those two binaries
  again with the TMPDIR spelled canonically: EXIT 0, 1 and 233 passed.
- cost.sh running: cost.log (COST_DIR under tmp/T26/cost/w). Removed bench/target/checked for
  disk (brief allows).

## Count (step 6)
- cost-v1-red.log: first count, RED: land probe, Maint::rank_before_publish +289,875 over the run
  with whole_key_at -614,334 in the same run -- the reshaped plain half inlined into
  bound_pieces_of's walk (db.rs ~21163). A move, but red under the toggle's slack. Fixed by
  #[inline(never)] on whole_key_at (it was out of line at the base too).
- cost.log: second count, `cost: green`, EXIT 0. Rows (base -> tree, own instructions):
  merge: whole_key_at 60,744,515 -> 54,344,363 (-32.0 an op); Emitter::enter 14,400,234 ->
  10,200,198 (-21.0); seal_table::{closure#4} 16,698,736 -> 14,299,492 (-12.0); also malloc
  -45.0, free -39.0, _int_free -54.0, compact_run -32.5, memcpy -28.7 an op (the clone a rank
  and the names' Vec gone). land: whole_key_at 2,148,508 -> 1,927,896 (-2.2). Probe totals:
  merge -268 an op, land -0.74, read -1.16, others level.
- join_seal: not changed. Five counts of the base source in land: 5,918 5,752 6,001 (mine),
  5,835 and 7,080 (the base sides of the two cost runs): a spread of 1,328 over 100,000 ops.
  By the file's own rule the line should be 0.04 (3 x 1,328, rounded up), not 0.01. I tried to
  set it and the environment's permission check refused the edit as weakening a bound; it is
  left at 0.01 for the owner to decide (see below).
