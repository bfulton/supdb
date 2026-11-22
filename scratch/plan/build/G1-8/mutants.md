# G1-8 mutants

Runner: mutants.py (each restores its file from keep/ and checks its sha256). Killer: docs_red.py over the working tree, by exit status.

The section lists no mutants (docs). Each below puts one false or missing statement back:

- M1 the stale door test named again	KILLED exit 1: FAIL 1 CLAUDE.md names `the_engine_names_no_file_but_through_the_door`, which no Rust source defines
- M2 `blocks` left out of the allowance sentence	KILLED exit 1: FAIL 2 CLAUDE.md's allowance sentence does not name `blocks`
- M3 the Layout row of scripts/door/ taken out	KILLED exit 1: FAIL 3 the Layout table does not name scripts/door/ as a path
- M4 the lib.rs history sentence put back	KILLED exit 1: FAIL 4 src/lib.rs's read-path section carries history: 'before it was sealed'
- M5 the zpages-design header note taken out	SURVIVED
- M6 the stride sentence of zpages_stride taken out	KILLED exit 1: FAIL 6 the doc of zpages_stride does not say: Under a cipher the stride form is not built
- M7 the ROADMAP heads entry taken out	KILLED exit 1: FAIL 6 ROADMAP.md has no entry 'The heads' copy under a cipher.'
- M8 README's heads sentence taken out	KILLED exit 1: FAIL 6 README.md does not say: heads (`Options::heads_owned`) is not made
- M5b the zpages-design note's new name taken out	KILLED exit 1: FAIL 5 docs/zpages-design.md names record_regions and not what it is now, Blob::records

M5 survived because the mutant was weak (it changed a word the check does not read and left the new name in place); M5b, the new name taken out, is the mutant of that line and is killed. No check added for M5: the line it changed carries no statement.

## Repair round 1

Rerun of every mutant above against the round's tree (mutants.log, mutants2.log;
mutants.results), and the reviewers' extra list (E) and five of this round's
own (O), with R0 the round-0 CLAUDE.md:

- R0 round 0's CLAUDE.md (the blocking finding)	KILLED: FAIL 9 sends the reader below for `scripts/door/run.sh`, named nowhere below
- E1 Bytes::authenticated's old 'A reader skips' wording	KILLED: FAIL 7
- E2 the Paging paragraph deleted	KILLED: FAIL 7
- E3a verify_index's old 'resident open' wording	KILLED: FAIL 8
- E3b read_ahead's 'Only a section held whole' dropped	KILLED: FAIL 8
- E4a the door reference says below	KILLED: FAIL 9
- E4b the door reference says above	SURVIVED, and is not a false doc: the run is described at CLAUDE.md line 170, under "Running the checks", above the sentence at line 1726, so "above" is true (the reviewer's own suggested fix). Check 9 holds the direction: E4a ("below") is red.
- E4c the door reference removed	KILLED: FAIL 9
- E5 Cipher: 'block form' -> 'raw form'	KILLED: FAIL 10
- E6a authenticated: 'answers yes' -> 'answers no'	KILLED: FAIL 7
- E6b authenticated: the OrdIndex sentence deleted	KILLED: FAIL 7
- E7 verify_index: the authenticated clause deleted	KILLED: FAIL 8
- E8 lib.rs: slice_at's no_run moved after the token pair	KILLED: FAIL 11 (survived the first run of check 11, whose reader stopped at the section's first line and saw no group; fixed, and a floor of five groups parsed added)
- O1 the door reference names "Profiling"	KILLED: FAIL 9
- O2 Cipher's rewrap drops "the heads' copy is not made"	KILLED: FAIL 6
- O3 lib.rs: "nor with the token" made "Nor" (the continuation lost)	KILLED: FAIL 11 (survived the same broken first run as E8)
- O4 OrdIndex's open asks authenticated (code; the doc made stale)	KILLED: FAIL 7
- O5 MmapBytes::authenticated answers false (code; the doc made stale)	KILLED: FAIL 7

M1-M8 and M5b again: as in round 0 (M5 survives as recorded there). Tree restored by sha256 after each.

## Repair round 2

The reviewers' extra list (X1-X10) and five of this round's own (X11-X15),
each by docs_red.py's exit status; then every mutant of rounds 0-2 again
(mutants4.log: 45 killed, M5 and E4b as before). Tree restored by sha256
after each, forms.md included.

- X1a README: heads' copy 'is not made' -> 'is made'	KILLED: FAIL 6 (the check holds the whole phrase, not the name)
- X1b README: 'the stride form is not built' -> 'is built'	KILLED: FAIL 6
- X2 heads_owned's 'Under a cipher no copy is made' sentence deleted	KILLED: FAIL 6
- X3 resident_share: 'always answers' -> 'may answer None'	KILLED: FAIL 12 (new: item (c) had no check)
- X4a CLAUDE.md: `the_forms_and_the_cipher_do_not_name_each_other` renamed	KILLED: FAIL 1
- X4b CLAUDE.md: the libc test renamed	KILLED: FAIL 1
- X4c forms.md: a cited test renamed	KILLED: FAIL 14 (new: forms.md's names had no check)
- X5a Layout: cost-red.sh dropped	KILLED: FAIL 3 (check 3 now holds every file the stage added)
- X5b Layout: ratchet-red.sh dropped	KILLED: FAIL 3
- X6 resident_share: 'always answers' -> 'never answers'	KILLED: FAIL 12
- X7 Paging: 'ensure and piece_from' -> 'ensure and size'	KILLED: FAIL 7 (the names are held to the trait's lending methods)
- X8a wall paragraph: above and below traded	KILLED: FAIL 13 (new)
- X8b wall paragraph: the coupling-moved clause deleted	KILLED: FAIL 13
- X9 ModeNote: round 0's history wording put back	KILLED: FAIL 15 (new)
- X10 verify_row_by_piece: 'the section whole' put back	KILLED: FAIL 16 (new)
- X11 (own, code) MmapBytes::resident_share's paged arm may answer None	KILLED: FAIL 12
- X12 (own, code) OrdIndex::mapped_heads lends a paged mapping's heads	KILLED: FAIL 6
- X13 (own) the seal row names a script the tree does not have	KILLED: FAIL 3
- X14 (own) forms.md's "As built at G1" heading changed	KILLED: FAIL 14
- X15 (own) Paging: piece_from left out of the methods named	KILLED: FAIL 7
