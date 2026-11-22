#!/usr/bin/env python3
"""G1-8's mutants: each puts one false or missing statement back into the
unit's docs and requires docs_red.py red on it, then restores the file from
keep/ and checks its sha256. Verdicts go to mutants.results one a line as
they come."""
import hashlib
import os
import subprocess
import sys

REPO = "/home/user/supdb"
HERE = os.path.dirname(os.path.abspath(__file__))
KEEP = os.path.join(HERE, "keep")
RES = os.path.join(HERE, "mutants.results")
sums = {}
for line in open(os.path.join(KEEP, "SHA256SUMS")):
    h, p = line.split(None, 1)
    sums[p.strip()[2:]] = h

FORMS_PATH = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/forms.md"
MUTANTS = [
    ("M1 the stale door test named again", "CLAUDE.md",
     "by symbol (`scripts/door/run.sh`, under\n\"Running the checks\"),",
     "by symbol (`the_engine_names_no_file_but_through_the_door`),"),
    ("M2 `blocks` left out of the allowance sentence", "CLAUDE.md",
     "`crypt`, `dirkey` and `blocks`, the",
     "`crypt` and `dirkey`, the"),
    ("M3 the Layout row of scripts/door/ taken out", "CLAUDE.md",
     "| `scripts/door/` | the door by symbol",
     "| `scripts/doorway/` | the door by symbol"),
    ("M4 the lib.rs history sentence put back", "src/lib.rs",
     "//! from outside the crate, and a caller asks instead:",
     "//! from outside as it was before it was sealed, and a caller asks:"),
    ("M5 the zpages-design header note taken out", "docs/zpages-design.md",
     "> session that wrote them. The accessor this design calls",
     "> session that wrote them. The accessor this design names"),
    ("M5b the zpages-design note's new name taken out", "docs/zpages-design.md",
     "> `Blob::record_regions` is the crate's own `Blob::records` in the tree,",
     "> `Blob::record_regions` is gone from the tree,"),
    ("M6 the stride sentence of zpages_stride taken out", "src/db.rs",
     "(`zfile::ZCursor`). Under a cipher the stride form is not built: it",
     "(`zfile::ZCursor`). The stride form: it"),
    ("M7 the ROADMAP heads entry taken out", "ROADMAP.md",
     "- [ ] **The heads' copy under a cipher.**",
     "- [ ] **The heads copy under a cipher.**"),
    ("M8 README's heads sentence taken out", "README.md",
     "(`Options::heads_owned`) is not made.",
     "(`Options::heads_owned`) is not used."),
]

LIB_NR = """//! ```no_run
//! fn lent<B: supdb::Bytes>(b: &B) -> Option<&[u8]> {
//!     b.slice_at(0, 1)
//! }
//! ```
"""
LIB_TOK_END = """//!     b.lend_at(0, 1, Default::default())
//! }
//! ```
"""
LIB_CUR = open(os.path.join(KEEP, "src/lib.rs")).read()
LIB_MOVED = LIB_CUR.replace(LIB_NR, "", 1).replace(LIB_TOK_END, LIB_TOK_END + LIB_NR, 1)
AUTH_NEW = """    /// lies in -- before it is lent. `Blob::open` reads this: over a source
    /// that lends by the piece it skips the key section's checksum row
    /// where the answer is yes, and verifies the row a piece at a time
    /// where it is no. The ordered index (`OrdIndex::open`) skips its
    /// checksum over any source that lends by the piece without asking
    /// this, since its one such source is a paged mapping (`vault::map`),
    /// which answers yes. `false` is always a correct answer.
"""
AUTH_OLD = """    /// lies in -- before it is lent. A reader skips the checks of its own
    /// that such a source makes redundant where making them would ask for
    /// a section whole (`Blob::open`'s key section row, over a source that
    /// lends by the piece), and makes them over any other source. `false`
    /// is always a correct answer.
"""
PAGING = """///
/// A paged mapping answers `Bytes::authenticated` with yes, and the readers
/// over it skip the checks of their own that would ask for a section whole
/// (the key section's checksum row, the ordered index's checksum). So an
/// implementor answers `ensure` and `piece_from` only for bytes it has
/// checked: the pager does, by each chunk's tag. A stand-in that checks
/// nothing, as a test's does, leaves those bytes checked by nobody.
"""
VI_NEW = """    /// Verify the key index's checksum row when the section carries one.
    /// A section held whole is verified every piece at the open; a section
    /// held by the piece, every piece at the open, each asked for on its
    /// own, except over a source that says it has checked every byte it
    /// lends (`Bytes::authenticated`), where the row is not read; a sparse
    /// reader verifies each piece as it is first used. On by default for
    /// the same reason.
"""
VI_OLD = """    /// Verify the key index's checksum row when the section carries one:
    /// every piece at a resident open, each piece as it is first used in a
    /// sparse reader. On by default for the same reason.
"""
RA = """    /// the asks made are `Blob::open_ahead`. Only a section held whole is
    /// walked this way: a section held by the piece is verified through its
    /// source's asks, one piece at a time, and this window is not used.
"""
DOOR = """the calls the door stands for, by symbol (`scripts/door/run.sh`, under
"Running the checks"), since the first only fires where a test under a
cipher reaches the call."""
MUTANTS += [
    ("R0 the round-0 tree's CLAUDE.md (the blocking finding)", "CLAUDE.md",
     open(os.path.join(KEEP, "CLAUDE.md")).read(), open(os.path.join(HERE, "keep.r0", "CLAUDE.md")).read()),
    ("E1 Bytes::authenticated's old 'A reader skips' wording", "src/bytes.rs", AUTH_NEW, AUTH_OLD),
    ("E2 the Paging paragraph deleted", "src/bytes.rs", PAGING, ""),
    ("E3a verify_index's old 'resident open' wording", "src/blob.rs", VI_NEW, VI_OLD),
    ("E3b read_ahead's 'Only a section held whole' sentence dropped", "src/blob.rs", RA,
     "    /// the asks made are `Blob::open_ahead`.\n"),
    ("E4a the door cross-reference says below", "CLAUDE.md", DOOR,
     DOOR.replace('under\n"Running the checks")', "below)")),
    ("E4b the door cross-reference says above", "CLAUDE.md", DOOR,
     DOOR.replace('under\n"Running the checks")', "above)")),
    ("E4c the door cross-reference removed", "CLAUDE.md", DOOR,
     DOOR.replace(' (`scripts/door/run.sh`, under\n"Running the checks")', "")),
    ("E5 Cipher: 'served in the block form' -> 'raw form'", "src/db.rs",
     "and served in the block form (`Options::zpages_stride`)", "and served in the raw form (`Options::zpages_stride`)"),
    ("E6a authenticated: 'which answers yes' -> 'which answers no'", "src/bytes.rs",
     "    /// which answers yes. `false`", "    /// which answers no. `false`"),
    ("E6b authenticated: the OrdIndex sentence deleted", "src/bytes.rs", AUTH_NEW,
     AUTH_NEW.split(" The ordered index")[0] + " `false` is always a correct answer.\n"),
    ("E7 verify_index: the authenticated clause deleted", "src/blob.rs",
     """    /// own, except over a source that says it has checked every byte it
    /// lends (`Bytes::authenticated`), where the row is not read; a sparse
""", """    /// own; a sparse
"""),
    ("E8 lib.rs: the slice_at no_run moved after the token pair", "src/lib.rs", LIB_CUR, LIB_MOVED),
    ("O1 door cross-reference names a section that does not describe the run", "CLAUDE.md", DOOR,
     DOOR.replace('"Running the checks"', '"Profiling"')),
    ("O2 Cipher's rewrap drops 'the heads' copy is not made'", "src/db.rs",
     "    /// heads' copy is not made (`Options::heads_owned`). `Db::create`",
     "    /// heads' copy (`Options::heads_owned`). `Db::create`"),
    ("O3 lib.rs: the token group's prose no longer continues ('Nor')", "src/lib.rs",
     "//! nor with the token it takes", "//! Nor with the token it takes"),
    ("O4 OrdIndex::open asks authenticated (the doc's claim made stale)", "src/ordindex.rs",
     "        let paged = map.piecewise();\n", "        let paged = map.piecewise() && map.authenticated();\n"),
    ("O5 MmapBytes::authenticated answers false (the doc's 'yes' made stale)", "src/bytes.rs",
     "    fn authenticated(&self) -> bool {\n        self.map().is_none()\n",
     "    fn authenticated(&self) -> bool {\n        false\n"),
]

MUTANTS += [
    ("X1a README: the heads' copy 'is not made' -> 'is made'", "README.md",
     "(`Options::heads_owned`) is not made.", "(`Options::heads_owned`) is made."),
    ("X1b README: 'the stride form is not built' -> 'is built'", "README.md",
     "the stride form is not built, a", "the stride form is built, a"),
    ("X2 heads_owned: the 'Under a cipher no copy is made' sentence deleted", "src/db.rs",
     """    /// Under a cipher no copy is made: the copy is taken from the mapping
    /// as it lies, and a store under a cipher reads its index by the piece
    /// through its own cache (`OrdIndex::mapped_heads`).
""", ""),
    ("X3 resident_share: 'always answers' -> 'may answer None'", "src/bytes.rs",
     "a paged mapping always answers. A", "a paged mapping may answer None. A"),
    ("X4a CLAUDE.md: the wall test renamed to one that does not exist", "CLAUDE.md",
     "(`the_forms_and_the_cipher_do_not_name_each_other`)", "(`the_forms_and_the_cipher_never_name_each_other`)"),
    ("X4b CLAUDE.md: the libc test renamed to one that does not exist", "CLAUDE.md",
     "(`every_libc_call_that_takes_a_string_is_barred_or_said_to_touch_no_file`)",
     "(`every_libc_call_taking_a_string_is_barred_or_said_to_touch_no_file`)"),
    ("X4c forms.md: a cited test renamed to one that does not exist", "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/forms.md",
     "`a_seals_token_made_another_way_fails`", "`a_seals_token_made_some_other_way_fails`"),
    ("X5a Layout: cost-red.sh dropped from the cost row", "CLAUDE.md",
     "`cost-hot.txt`, `cost-red.sh` | what", "`cost-hot.txt` | what"),
    ("X5b Layout: ratchet-red.sh dropped from the lists row", "CLAUDE.md",
     "`raises.txt`, `ratchet-red.sh`, `base.sh` |", "`raises.txt`, `base.sh` |"),
    ("X6 resident_share: 'always answers' -> 'never answers'", "src/bytes.rs",
     "a paged mapping always answers. A", "a paged mapping never answers. A"),
    ("X7 Paging: 'ensure and piece_from' -> 'ensure and size'", "src/bytes.rs",
     "implementor answers `ensure` and `piece_from` only", "implementor answers `ensure` and `size` only"),
    ("X8a wall paragraph: above and below traded", "CLAUDE.md",
     """a total above the file's is growth and red, one
below it is red until the file is lowered""", """a total below the file's is growth and red, one
above it is red until the file is lowered"""),
    ("X8b wall paragraph: the coupling-moved clause deleted", "CLAUDE.md",
     """in the same change, and a
coupling moved between functions changes no total.""", """in the same change."""),
    ("X9 ModeNote: round 0's history wording put back", "src/db.rs",
     """/// a field, since every read of the writer's counts one of them and looks
/// at `quiet_out`: a note held whole in one cell is copied in and out whole
/// at each, a load and a store a word of it.""", """/// a field: every read of the writer's counts one of them and looks at
/// `quiet_out`, and a note held whole in one cell was copied in and out
/// whole at each, a load and a store a word of it."""),
    ("X10 verify_row_by_piece: 'the section whole' put back", "src/blob.rs",
     """since verifying would ask for every
/// byte of the section at every open""", """since verifying would ask for the
/// section whole at every open"""),
    ("X11 MmapBytes::resident_share's paged arm answers None (code; the doc made stale)", "src/bytes.rs",
     "        Some(p.resident(off, len))\n    }\n    fn advise_dontneed(&self) {",
     "        p.size().checked_sub(0).map(|_| p.resident(off, len)).filter(|s| *s > 0.0)\n    }\n    fn advise_dontneed(&self) {"),
    ("X12 OrdIndex::mapped_heads lends a paged mapping's heads (code; the doc made stale)", "src/ordindex.rs",
     "            .filter(|_| !self.map.piecewise())\n", "\n"),
]

MUTANTS += [
    ("X13 Layout: the seal row names a script the tree does not have", "CLAUDE.md",
     "| `scripts/seal-red.sh`, `seal-red-red.sh` |", "| `scripts/seal-red.sh`, `seal-red2.sh` |"),
    ("X14 forms.md: the As built section's heading changed", FORMS_PATH,
     "## As built at G1\n", "## As landed at G1\n"),
    ("X15 Paging: piece_from left out of the methods named", "src/bytes.rs",
     "implementor answers `ensure` and `piece_from` only", "implementor answers `ensure` only"),
]


def sha(p):
    return hashlib.sha256(open(p, "rb").read()).hexdigest()


FORMS = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/forms.md"


def where(f):
    return f if f.startswith("/") else os.path.join(REPO, f)


def kept(f):
    return "forms.md" if f == FORMS else f


def restore(f):
    dst = where(f)
    open(dst, "wb").write(open(os.path.join(KEEP, kept(f)), "rb").read())
    assert sha(dst) == sums[kept(f)], f"{f} not restored"


done = set()
if os.path.exists(RES):
    done = {l.split("\t")[0] for l in open(RES)}
for name, f, old, new in MUTANTS:
    if name in done:
        continue
    path = where(f)
    t = open(path).read()
    if t.count(old) != 1 or old == new:
        verdict = f"NOT APPLIED ({t.count(old)} matches)"
    else:
        open(path, "w").write(t.replace(old, new))
        r = subprocess.run([sys.executable, os.path.join(HERE, "docs_red.py"), REPO],
                           capture_output=True, text=True)
        fail = [l for l in r.stdout.splitlines() if l.startswith("FAIL")]
        verdict = f"KILLED exit {r.returncode}: {fail[0] if fail else ''}" if r.returncode else "SURVIVED"
    restore(f)
    with open(RES, "a") as out:
        out.write(f"{name}\t{verdict}\n")
    print(name, verdict)
for f in sums:
    assert sha(FORMS if f == "forms.md" else os.path.join(REPO, f)) == sums[f], f
print("tree restored")
