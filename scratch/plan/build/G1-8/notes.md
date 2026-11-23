# G1-8 docs: notes

## Repair round 2 (09:16-)
No file of the repository changed this round: the reviewers' ten extra
mutants were run, and where one would survive the red input grew a check
instead of the docs a word. The tree equals keep/ by sha256 (keep/forms.md
and its sum added: the scratchpad contract is a mutant target now).
- docs_red.py (round 1's copy kept as docs_red.r1.py) gains: check 3 holds
  every file the stage added (cost-diff.py, cost-hot.txt, cost-red.sh,
  ratchet.py, raises.txt, ratchet-red.sh, seal-red-red.sh among them) to a
  Layout row's path cell and to the tree, and every path a scripts row names
  to a file that exists; check 6 holds "no copy is made" to the code
  (`OrdIndex::mapped_heads` filters out a paged mapping, `Seg::own_heads`
  copies only from it); check 7 holds the methods the Paging doc names to
  `ensure` and `piece_from` and to the trait's declarations; 12 (item c)
  resident_share's "a paged mapping always answers", "a kernel that will not
  answer" confined to a mapped file, and MmapBytes's paged arm being
  `Some(p.resident(off, len))`; 13 the wall paragraph clause by clause, each
  held to the tests/source.rs test that holds the gate's behaviour
  (`a_coupling_added_to_a_form_fails_as_growth`,
  `a_coupling_removed_fails_until_the_file_is_lowered`,
  `a_coupling_moved_between_functions_passes`) and the traded wording
  refused; 14 every test name forms.md's "As built at G1" cites is a `fn` of
  tests/ (of the named file where it says `tests/x.rs::`), with a floor of
  twenty names parsed (CLAUDE.md's are held by check 1, which a rename to a
  name nothing defines already fails); 15 ModeNote's doc in the present tense
  (item e); 16 verify_row_by_piece's "every byte of the section" (item b).
- Red: before.r2.log, docs_red.py over f0349e51, 62 findings, EXIT 1;
  after.r2.docs.log, over the tree, green, EXIT 0. Check 14 reads the
  scratchpad's forms.md in both, not the commit's (forms.md is not in the
  repository).
- Mutants: mutants4.log is every mutant of rounds 0-2 run again (45 killed;
  M5 and E4b survive as recorded in round 1, neither a false statement),
  results in mutants.results; mutants3.log the extra list's first run.
- Checks: source3.log (tests/source.rs, cargo fmt --check, `SUPDB_BASE=f0349e51`
  lists in lists-r2.log), read by its "EXIT done" line. Compiled code: none
  moved in either repair round (the repository's files are round 1's by
  sha256), so no count, no wasm run, engine_changed false.

Began at HEAD f0349e51 (tree clean). Base of the run ea635218.

## Repair round 1 (09:05-)
Step: the checks after the round (source2.log: tests/source.rs, cargo fmt
--check, the lint group, `SUPDB_BASE=f0349e51` lists, the doctests; read by its
EXIT line). The finished copy of every changed file is keep/ (SHA256SUMS,
ordindex.rs added only as a mutant's restore point, unchanged from HEAD);
round 0's copy is keep.r0/, its mutant results mutants.r0.results.
- Blocking: CLAUDE.md's door sentence in the cipher section sent the reader
  "below" for `scripts/door/run.sh`, which is described above it, under
  "Running the checks". It names that section now. docs_red.py check 9 holds
  every "(`P`, below|above|under "S")" of CLAUDE.md to where P is (a later
  line, an earlier line, a `## S` that names it), and the door sentence to the
  paragraph that describes the run; mutant R0 (round 0's CLAUDE.md) is red on
  it. Check 9 found one more reference over the tree: "(`scripts/cost-diff.py`,
  below)" at the cost paragraph, written by G1-3, where the comparator is
  described below by what it is ("The comparator reads canonical names") and
  not by name. That is true, and the check takes those words for the name (a
  table of one entry in check 9), rather than the sentence being rewritten.
- Encryption::Cipher's doc rewrapped (one line was 117 columns); text unchanged.
- docs_red.py checks 7-11 added for the extra mutants (round 0's copy kept as
  docs_red.r0.py): 7 Bytes::authenticated and Paging's docs, held to the code
  as well (MmapBytes::authenticated is `map().is_none()`; OrdIndex's open asks
  `piecewise` and not `authenticated`), 8 verify_index and read_ahead (and
  verify_row_by_piece names no read-ahead), 9 the cross-references, 10 the
  Cipher doc's block form, 11 the lib.rs groups by structure (each group of
  compile_fail examples followed at once by one that compiles, or continuing,
  by prose that starts lower case, a group whose compiling example stands just
  before that prose; and the section parsed as at least five groups, since the
  first version of the reader broke at the section's first line and read no
  group, which E8 and O3 surviving showed).
- No compiled code moved this round either (the comment-stripped diff of src
  and bench/src prints nothing): no count, no wasm run.

## Round 0 step
Done (09:01 UTC). Green: source1.log (78 of tests/source.rs), lint.log (the lint
group, door and seal red inputs included, EXIT 0), doc.log, lists-default.log and
lists-f0349e51.log (EXIT 0 both), after.log (whole suite --no-fail-fast, EXIT 0,
no failure), cargo fmt --check clean. Mutants: mutants.md. Tree equals keep/ by
sha256. Private TMPDIR removed. No count run (no compiled code moved). Logs in this directory: source1.log (tests/source.rs after every
src edit, EXIT 0), lint.log, doc.log, lists-*.log, after.log (whole suite,
--no-fail-fast). before.log / docs_red.py are the red input; mutants.py and
mutants.results the mutants. The finished copy of every file changed is
keep/ (SHA256SUMS); keep/forms.md.orig is the scratchpad contract before
this unit's edit.

## What differs from the plan's tree (682b2c09)
- The plan's section for G1-8 is three bullets; the launcher's since.txt list
  (a)-(g) is the unit's scope and stands where they differ.
- forms.md's R12 references (zfile.rs:5199-5270, db.rs:14936, 13062/13074) were
  of tree 3df85d22 and name code that moved and, since G1-6, code that is gone
  (`db::resident_share`). The scratchpad copy now names functions:
  `zfile::read_budget`, `zfile::thread_major_faults`, `Bytes::resident_share`,
  `db::thread_minflt` / `thread_majflt`.
- CLAUDE.md said "No module carries a blanket lint allowance" while six door
  modules carry `#![allow(clippy::disallowed_methods, clippy::disallowed_types)]`
  (vault, bytes, pager, crypt, dirkey, blocks); and named a test that G1-2
  deleted (`the_engine_names_no_file_but_through_the_door`). The cost paragraph
  already points at scripts/cost.sh (G1-3 wrote it); the lists paragraph (T18)
  and the seal paragraphs (G1-7) are current and were left. Nothing in CLAUDE.md
  described the wall or the door's clippy run: one paragraph added under the
  lint group, with the failure that moved the door from text to symbol (forms.md
  G-door: the text test read db.rs alone).

## What changed (docs only)
- CLAUDE.md: Layout rows for tests/source.rs, scripts/door/, the cost scripts,
  the ratchet scripts with base.sh and raises.txt, the seal's red scripts; the
  allowance sentence names the six door modules and why; the door paragraph
  under the cipher section says the door is held by symbol; a paragraph on the
  wall and the door; the merged entries named for what they count (land probe:
  "280 instructions under it an operation of the probe, a key rewritten";
  "A note in one cell": `Reader::scan` by function, the point read probe's total).
- src/blob.rs: BlobOptions::read_ahead (only a section held whole uses it),
  BlobOptions::verify_index (whole / by the piece / authenticated / sparse),
  verify_row_by_piece ("every byte of the section").
- src/bytes.rs: Bytes::authenticated says what Blob::open and OrdIndex::open each
  do; resident_share's None (a paged mapping always answers); lend_at's "it
  always was" (history) gone; Paging's contract (an implementor answers ensure
  and piece_from only for bytes it has checked; a stand-in that checks nothing
  leaves them checked by nobody).
- src/lib.rs: the read-path section's history sentence gone; the twin sentence
  made true (groups, each followed or preceded by the call that compiles). No
  example changed, no verdict changed.
- src/db.rs: ModeNote's doc in the present tense; Encryption::Cipher, heads_owned
  and zpages_stride say what a store under a cipher does.
- bench/src/engines.rs: with_policy's comment no longer claims a struct update
  copies Options beside one it leaves in place.
- README.md: the stride form and the heads' copy under a cipher.
- ROADMAP.md: two entries, "The stride form under a cipher" and "The heads'
  copy under a cipher", each with what would make it work. The controller's
  entry already stands ("An envelope whose chunks are whole pages", done when the
  controller is on under a cipher).
- docs/zpages-design.md: the header says the accessor it calls
  `Blob::record_regions` is `Blob::records` now (the document is a registered
  design kept as it was; its body is not rewritten).
- Scratchpad forms.md: R12's references by name; section "As built at G1".

## (f) checked against the code, and what no test holds
- zpages_under_cipher / cipher_controller (src/db.rs): ZPages::Auto -> Off unless
  the option is set. Held by tests/zpages.rs
  the_controller_leaves_a_store_under_a_cipher_raw_unless_it_is_asked.
- The stride form under a cipher: the build of a kept file (SegSrc::Kept in
  src/db/zfile.rs) answers StrideAsk::Only with NotStride(StrideNo::Kept) and
  OrBlock with the block form, stride_no Some(Kept), counted in
  ZCounts::stride_declines; the attach's readers (attach_stride, the probe at
  zfile.rs's FileFacts, check_stride) take MmapBytes::lent, empty for a paged
  mapping. FOR THE LAUNCHER: no test holds any of this. tests/zpages.rs's stride
  test asserts kept == 0 in the clear; no cipher test reads stride_declines. The
  docs say it from the code, not as tested. A test of today's behaviour is owed
  first (ROADMAP.md, "The stride form under a cipher").
- The heads' copy under a cipher: Seg::own_heads asks OrdIndex::mapped_heads,
  None for a paged mapping, so no copy. FOR THE LAUNCHER: no test holds it; and
  own_heads calls `ord.willneed_heads_all()` before that None, so under a cipher
  with the compact form on (ZPages::Always, or Auto with zpages_under_cipher),
  every z_heads_look asks the kernel for every left-raw partition's heads'
  ciphertext and copies nothing, again at each look (`made` never counts it, so
  HEADS_A_LOOK does not bound it). Not a default path under a cipher. Not fixed
  here (no compiled code moves in this unit); said in ROADMAP's entry.

## (g) No compiled code changed
`git diff -U0 -- src bench/src` has no added or removed line that is not a
comment (the grep in the step-5 log of this session printed none). The wasm
module's size and the count are unchanged by construction; neither was run.

## Red input
docs_red.py (in this directory): the docs' statements held to the tree. before.log
is it over commit f0349e51 (25 findings, exit 1); over the working tree it reads
green. Written after the edits had begun (it reads the commit, so the red is the
commit's), which is said here: the edits were not made against it.

## What it cannot see
docs_red.py holds the items it names and no others: it reads names, not
meaning. A doc sentence that is false in other words passes it. It is a
scratch input, not a gate in the tree.

## Departures from the plan
- The red input is a scratch script, not a gate in the tree: the plan says
  "Red input. None (docs)", and a docs gate in tests/source.rs would be a list
  of its own for the ratchet, which no unit section asks for.
- docs/zpages-design.md gets a header note, not a rewrite of its body, since the
  document is kept as registered (its own header says so).

## Not verified
- The two cipher behaviours above, by any test (none exists).
- That Paging's implementors other than the pager check what they lend: InMemory
  in src/ordindex.rs's tests checks nothing (the doc says so of a stand-in).
