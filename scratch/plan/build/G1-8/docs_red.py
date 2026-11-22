#!/usr/bin/env python3
"""G1-8's red input: the docs' statements about the gates and the read path,
held to the tree they describe. Run as `docs_red.py REPO [REV]`: over the
working tree, or over the files of commit REV (git show), so the same check
reads the commit before the unit and the unit's tree. Exit 1 and a FAIL line
per finding; `docs: green` and exit 0 otherwise.

What it holds (each a statement this unit found false or missing):
1. Every backticked snake_case name of five words or more in CLAUDE.md
   names a fn, a field or a const somewhere in the crate's or the suite's
   Rust sources (a test CLAUDE.md cites that no longer exists is a pointer
   to nothing: `the_engine_names_no_file_but_through_the_door`).
2. CLAUDE.md does not say no module carries a blanket lint allowance while
   a module of src carries one at its top; and every such module is named
   in CLAUDE.md's sentence about the allowance.
3. The Layout table names each gate the checks run: tests/source.rs,
   scripts/door/, scripts/cost.sh, scripts/ratchet.sh, scripts/base.sh,
   scripts/seal-red.sh.
4. src/lib.rs's section on what the read path does not hand out carries no
   history ("before it was sealed") and does not say every example has a
   twin that differs only in the call.
5. A doc that names `record_regions` (gone from the tree) names what it is
   now, `Blob::records`.
6. The docs of `Encryption::Cipher`, `heads_owned` and `zpages_stride`, and
   README.md, say what a store under a cipher does with the stride form and
   the heads' copy; ROADMAP.md has an entry for each.
"""
import re
import subprocess
import sys

repo = sys.argv[1]
rev = sys.argv[2] if len(sys.argv) > 2 else None


def files():
    if rev:
        out = subprocess.run(["git", "-C", repo, "ls-tree", "-r", "--name-only", rev],
                             capture_output=True, text=True, check=True).stdout
        return [l for l in out.splitlines() if l]
    out = subprocess.run(["git", "-C", repo, "ls-files"], capture_output=True, text=True,
                         check=True).stdout
    return [l for l in out.splitlines() if l]


def read(path):
    if rev:
        r = subprocess.run(["git", "-C", repo, "show", f"{rev}:{path}"], capture_output=True)
        if r.returncode != 0:
            return None
        return r.stdout.decode("utf-8", "replace")
    try:
        return open(f"{repo}/{path}", encoding="utf-8", errors="replace").read()
    except FileNotFoundError:
        return None


fails = []
all_files = files()
rs = [f for f in all_files if f.endswith(".rs") and not f.startswith("scratch/")]
names = set()
allowed_mods = []
for f in rs:
    t = read(f) or ""
    names.update(re.findall(r"\bfn\s+([a-z_][a-z0-9_]*)", t))
    names.update(re.findall(r"^\s*(?:pub(?:\([^)]*\))?\s+)?([a-z_][a-z0-9_]*)\s*:", t, re.M))
    names.update(n.lower() for n in re.findall(r"\bconst\s+([A-Za-z_][A-Za-z0-9_]*)", t))
    if f.startswith("src/") and re.search(r"^#!\[allow\(", t, re.M):
        allowed_mods.append(f.rsplit("/", 1)[1][:-3])

claude = read("CLAUDE.md") or ""
for m in sorted(set(re.findall(r"`([a-z][a-z0-9_]*(?:_[a-z0-9]+){4,})`", claude))):
    if m not in names:
        fails.append(f"1 CLAUDE.md names `{m}`, which no Rust source defines")

flat = re.sub(r"\s+", " ", claude)
if allowed_mods and "No module carries a blanket lint allowance" in flat:
    fails.append(f"2 CLAUDE.md says no module carries a blanket allowance; {allowed_mods} do")
m = re.search(r"([^.]*carry a blanket allowance[^.]*\.[^.]*\.)", flat)
for mod in allowed_mods:
    if not m or f"`{mod}`" not in m.group(1):
        fails.append(f"2 CLAUDE.md's allowance sentence does not name `{mod}`")

layout = claude.split("## Layout", 1)[-1].split("\n## ", 1)[0]
STAGE = ["tests/source.rs", "scripts/door/", "scripts/cost.sh", "scripts/cost-diff.py",
         "scripts/cost-hot.txt", "scripts/cost-red.sh", "scripts/ratchet.sh",
         "scripts/ratchet.py", "scripts/raises.txt", "scripts/ratchet-red.sh",
         "scripts/base.sh", "scripts/seal-red.sh", "scripts/seal-red-red.sh"]
for g in STAGE:
    if not any(f == g or f.startswith(g) for f in all_files):
        fails.append(f"3 the stage's file {g} is not in the tree")
for row in [l for l in layout.splitlines() if l.startswith("| `scripts/") or l.startswith("| `tests/source")]:
    cell = row.split("|")[1]
    first = re.findall(r"`([^`]+)`", cell)
    if not first:
        continue
    d = first[0].rsplit("/", 1)[0] + "/" if "/" in first[0] else ""
    for n in first:
        path = n if "/" in n else d + n
        if not any(f == path or f.startswith(path) for f in all_files):
            fails.append(f"3 the Layout row names {path}, which is not in the tree")
cells = " ".join(r.split("|")[1] for r in layout.splitlines() if r.startswith("| `"))
for g in STAGE:
    base = g.rsplit("/", 1)[-1] if not g.endswith("/") else g
    if f"`{g}`" not in cells and f"`{base}`" not in cells:
        fails.append(f"3 the Layout table does not name {g} in a row's path cell")
for g in ["tests/source.rs", "scripts/door/", "scripts/cost.sh", "scripts/ratchet.sh",
          "scripts/base.sh", "scripts/seal-red.sh"]:
    if g.rsplit("/", 1)[-1] not in layout and g not in layout:
        fails.append(f"3 the Layout table does not name {g}")
    elif f"`{g}`" not in layout and f"`{g.rsplit('/', 1)[-1]}`" not in layout:
        fails.append(f"3 the Layout table does not name {g} as a path")

lib = re.sub(r"\s*//!\s*", " ", read("src/lib.rs") or "")
if "before it was sealed" in lib:
    fails.append("4 src/lib.rs's read-path section carries history: 'before it was sealed'")
if "has a twin beside it that differs only" in lib:
    fails.append("4 src/lib.rs says every example has a twin that differs only in the call")

for f in all_files:
    if not f.endswith(".md") or f.startswith("scratch/"):
        continue
    t = read(f) or ""
    if "record_regions" in t and "Blob::records" not in t.replace("Blob::record_regions", ""):
        fails.append(f"5 {f} names record_regions and not what it is now, Blob::records")

db = re.sub(r"\s*///\s*", " ", read("src/db.rs") or "")
for what, field, word in [
    ("Encryption::Cipher", "Cipher(vault::StoreCipher)", "stride form is not built"),
    ("Encryption::Cipher", "Cipher(vault::StoreCipher)", "heads' copy is not made"),
    ("heads_owned", "pub heads_owned: bool", "Under a cipher no copy is made"),
    ("zpages_stride", "pub zpages_stride: ZStride", "Under a cipher the stride form is not built"),
]:
    at = db.find(field)
    doc = db[max(0, at - 2500):at] if at >= 0 else ""
    if word not in doc:
        fails.append(f"6 the doc of {what} does not say: {word}")
readme = re.sub(r"\s+", " ", read("README.md") or "")
for word in ["the stride form is not built", "heads (`Options::heads_owned`) is not made"]:
    if word not in readme:
        fails.append(f"6 README.md does not say: {word}")
ordt = read("src/ordindex.rs") or ""
mh = ordt[ordt.find("fn mapped_heads"):]
mh = mh[:mh.find("\n    }\n")]
if ".filter(|_| !self.map.piecewise())" not in mh:
    fails.append("6 OrdIndex::mapped_heads no longer answers None for a paged mapping; 'no copy is made' is unchecked")
dbt = read("src/db.rs") or ""
oh = dbt[dbt.find("fn own_heads"):]
oh = oh[:oh.find("\n    }\n")]
if "let Some(src) = self.ord.mapped_heads() else {" not in oh:
    fails.append("6 Seg::own_heads no longer copies only from mapped_heads; 'no copy is made' is unchecked")
roadmap = read("ROADMAP.md") or ""
for title in ["The stride form under a cipher.", "The heads' copy under a cipher."]:
    if title not in roadmap:
        fails.append(f"6 ROADMAP.md has no entry '{title}'")

# 7. Bytes::authenticated and Paging, against the code.
bytes_rs = read("src/bytes.rs") or ""
bflat = re.sub(r"\s*///\s*", " ", bytes_rs)
at = bflat.find("fn authenticated(&self) -> bool {\n        false")
adoc = bflat[max(0, bflat.rfind("}", 0, at)):at] if at >= 0 else ""
for word in ["`Blob::open` reads this", "(`OrdIndex::open`)", "which answers yes"]:
    if word not in adoc:
        fails.append(f"7 Bytes::authenticated's doc does not say: {word}")
if "A reader skips the checks of its own" in adoc:
    fails.append("7 Bytes::authenticated's doc says 'a reader' where the readers differ")
if not re.search(r"fn authenticated\(&self\) -> bool \{\s*self\.map\(\)\.is_none\(\)", bytes_rs):
    fails.append("7 MmapBytes::authenticated is not map().is_none(): 'which answers yes' unchecked")
ord_rs = read("src/ordindex.rs") or ""
over = ord_rs[ord_rs.find("fn over(map: MmapBytes"):]
over = over[:over.find("\n    }\n")]
if "piecewise()" not in over or "authenticated" in over:
    fails.append("7 OrdIndex::open no longer skips on piecewise alone; its doc in bytes.rs is stale")
pt = bflat.find("pub(crate) trait Paging")
pdoc = bflat[max(0, pt - 1600):pt] if pt >= 0 else ""
for word in ["A paged mapping answers `Bytes::authenticated` with yes",
             "only for bytes it has checked", "checked by nobody"]:
    if word not in pdoc:
        fails.append(f"7 the Paging trait's doc does not say: {word}")
ptrait = bytes_rs[bytes_rs.find("pub(crate) trait Paging"):]
ptrait = ptrait[:ptrait.find("\n}\n")]
pm = re.search(r"implementor answers ((?:`\w+`(?:, | and )?)+) only for bytes it has checked", pdoc)
if not pm:
    fails.append("7 the Paging trait's doc does not name the methods an implementor answers only for checked bytes")
else:
    named = re.findall(r"`(\w+)`", pm.group(1))
    if sorted(named) != ["ensure", "piece_from"]:
        fails.append(f"7 the Paging trait's doc names {named} as the lending methods; they are ensure and piece_from")
    for n in named:
        if f"fn {n}(" not in ptrait:
            fails.append(f"7 the Paging trait's doc names `{n}`, which Paging does not declare")

# 8. BlobOptions::verify_index and read_ahead.
blob_rs = read("src/blob.rs") or ""
oflat = re.sub(r"\s*///\s*", " ", blob_rs)
def field_doc(field):
    ls = blob_rs.splitlines()
    at = next((k for k, l in enumerate(ls) if l.strip() == field + ","), None)
    if at is None:
        return ""
    k = at
    while k > 0 and ls[k - 1].strip().startswith("///"):
        k -= 1
    return " ".join(l.strip()[3:].strip() for l in ls[k:at])
vdoc = field_doc("pub verify_index: bool")
if "every piece at a resident open" in vdoc:
    fails.append("8 BlobOptions::verify_index still says 'every piece at a resident open'")
for word in ["(`Bytes::authenticated`), where the row is not read", "a section held by the piece",
             "a sparse reader verifies each piece as it is first used"]:
    if word not in vdoc:
        fails.append(f"8 BlobOptions::verify_index's doc does not say: {word}")
rdoc = field_doc("pub read_ahead: usize")
if "Only a section held whole is walked this way" not in rdoc:
    fails.append("8 BlobOptions::read_ahead's doc does not say only a section held whole uses it")
vr = blob_rs.find("fn verify_row_by_piece")
vbody = blob_rs[vr:blob_rs.find("\n}\n", vr)] if vr >= 0 else ""
if vr < 0 or "read_ahead" in vbody or "open_ahead" in vbody:
    fails.append("8 verify_row_by_piece uses the read-ahead window its doc says it does not")

# 9. CLAUDE.md's cross-references point where the thing is.
lines = claude.splitlines()
sections = {}
cur = None
for i, l in enumerate(lines):
    if l.startswith("## "):
        cur = l[3:].strip()
        sections[cur] = []
    if cur:
        sections[cur].append(l)
flat_lines = []
for i, l in enumerate(lines):
    flat_lines.append(l)
joined = "\n".join(lines)
for m in re.finditer(r"\(`([^`]+)`,\s+(below|above|under\s+\"([^\"]+)\")\)", joined):
    p, how, sec = m.group(1), m.group(2), m.group(3)
    before, after = joined[:m.start()], joined[m.end():]
    # A file described below by what it is rather than by its name: the
    # words that describe it stand in for the name.
    described = {"scripts/cost-diff.py": "The comparator reads canonical names"}
    if how == "below" and f"`{p}`" not in after and described.get(p, "\0") not in after:
        fails.append(f"9 CLAUDE.md sends the reader below for `{p}`, named nowhere below")
    elif how == "above" and f"`{p}`" not in before:
        fails.append(f"9 CLAUDE.md sends the reader above for `{p}`, named nowhere above")
    elif sec is not None:
        body = "\n".join(sections.get(sec, []))
        if sec not in sections:
            fails.append(f"9 CLAUDE.md sends the reader to a section \"{sec}\" it does not have")
        elif f"`{p}`" not in body:
            fails.append(f"9 CLAUDE.md's section \"{sec}\" does not name `{p}`")
door = re.search(r"linted for\s+the calls the door stands for, by symbol \(`scripts/door/run\.sh`,\s+"
                 r"(below|above|under\s+\"[^\"]+\")\)", joined)
if not door:
    fails.append("9 CLAUDE.md's door sentence in the cipher section points nowhere for scripts/door/run.sh")
else:
    desc = joined.find("`scripts/door/run.sh`, config")
    how = door.group(1)
    if desc < 0:
        fails.append("9 CLAUDE.md describes scripts/door/run.sh nowhere")
    elif how == "below" and desc < door.start():
        fails.append("9 CLAUDE.md's door sentence says below; the run is described above it")
    elif how == "above" and desc > door.start():
        fails.append("9 CLAUDE.md's door sentence says above; the run is described below it")
    elif how.startswith("under"):
        sec = how.split('"')[1]
        body = "\n".join(sections.get(sec, []))
        if "`scripts/door/run.sh`, config" not in body:
            fails.append(f"9 CLAUDE.md's door sentence sends the reader to \"{sec}\", which does not describe the run")

# 10. Encryption::Cipher: the block form.
at = db.find("Cipher(vault::StoreCipher)")
cdoc = db[max(0, at - 2500):at] if at >= 0 else ""
if "built and served in the block form (`Options::zpages_stride`)" not in cdoc:
    fails.append("10 the doc of Encryption::Cipher does not say the partition is served in the block form")

# 11. src/lib.rs's examples are in the groups its sentence says.
librs = read("src/lib.rs") or ""
sec = librs.split("//! ## What the read path does not hand out", 1)[-1]
items = []  # ("cf"|"ok", None) or ("prose", first word)
inblock = False
prose = []
for l in sec.splitlines()[1:]:
    if not l.startswith("//!"):
        break
    s = l[3:].strip()
    if s.startswith("```"):
        if not inblock:
            if prose:
                items.append(("prose", " ".join(prose)))
                prose = []
            items.append(("cf" if "compile_fail" in s else "ok", None))
            inblock = True
        else:
            inblock = False
        continue
    if not inblock and s:
        prose.append(s)
i = 0
while i < len(items):
    if items[i][0] != "cf":
        i += 1
        continue
    j = i
    while j < len(items) and items[j][0] == "cf":
        j += 1
    followed = j < len(items) and items[j][0] == "ok"
    cont = (i >= 2 and items[i - 1][0] == "prose" and items[i - 1][1][:1].islower()
            and items[i - 2][0] == "ok")
    if not (followed or cont):
        fails.append(f"11 src/lib.rs: the compile_fail group at item {i} has no compiling example beside it")
    i = j
groups = sum(1 for k, it in enumerate(items) if it[0] == "cf" and (k == 0 or items[k - 1][0] != "cf"))
if groups < 5:
    fails.append(f"11 src/lib.rs: the read-path section parsed as {groups} groups of compile_fail examples; the reader of it is broken")

# 12. (c) Bytes::resident_share: None only where the source cannot say,
# and a paged mapping always answers -- held to MmapBytes's arm.
rs_at = bflat.find("fn resident_share(&self, _off: u64, _len: usize) -> Option<f64> {")
rdoc12 = bflat[max(0, bflat.rfind("}", 0, rs_at)):rs_at] if rs_at >= 0 else ""
if "a paged mapping always answers" not in rdoc12:
    fails.append("12 Bytes::resident_share's doc does not say a paged mapping always answers")
for bad in ["paged mapping may answer None", "paged mapping never answers", "paged mapping cannot say"]:
    if bad in rdoc12:
        fails.append(f"12 Bytes::resident_share's doc says '{bad}'")
if not re.search(r"for a mapped file, a\s+kernel that will not answer", rdoc12):
    fails.append("12 Bytes::resident_share's doc does not confine 'a kernel that will not answer' to a mapped file")
mi = bytes_rs.find("fn resident_share(&self, off: u64, len: usize) -> Option<f64> {")
mbody = bytes_rs[mi:bytes_rs.find("\n    }\n", mi)] if mi >= 0 else ""
if not re.search(r"let Own::Paged\(p\) = &self\.own else \{.*\};\s*Some\(p\.resident\(off, len\)\)\s*$",
                 mbody, re.S):
    fails.append("12 MmapBytes::resident_share's paged arm is not Some(p.resident(..)); 'always answers' is unchecked")

# 13. CLAUDE.md's wall paragraph, clause by clause, each held to the test
# that holds the gate's behaviour.
src_t = read("tests/source.rs") or ""
for phrase, test in [
    ("a total above the file's is growth and red", "a_coupling_added_to_a_form_fails_as_growth"),
    ("one below it is red until the file is lowered in the same change",
     "a_coupling_removed_fails_until_the_file_is_lowered"),
    ("a coupling moved between functions changes no total", "a_coupling_moved_between_functions_passes"),
]:
    if phrase not in flat:
        fails.append(f"13 CLAUDE.md's wall paragraph does not say: {phrase}")
    if f"fn {test}(" not in src_t:
        fails.append(f"13 tests/source.rs has no {test}, which holds: {phrase}")
for bad in ["a total below the file's is growth", "one above it is red until the file is lowered",
            "a total above the file's passes"]:
    if bad in flat:
        fails.append(f"13 CLAUDE.md's wall paragraph says: {bad}")

# 14. Every test name the docs cite is a fn of the tests: CLAUDE.md's
# backticked names that the suite's tests define are held by check 1;
# forms.md's section "As built at G1" is held here, a name prefixed by
# its file to a fn of that file.
import os
forms = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "seam", "forms.md")
forms_t = open(forms).read() if os.path.exists(forms) else ""
built = forms_t.split("## As built at G1", 1)
if len(built) < 2:
    fails.append("14 forms.md has no section 'As built at G1'")
else:
    sec14 = built[1].split("\n## ", 1)[0]
    tests_rs = {f: (read(f) or "") for f in rs if f.startswith("tests/")}
    alltests = "\n".join(tests_rs.values())
    cited = re.findall(r"`(?:(tests/[a-z_]+\.rs)::)?([a-z][a-z0-9]*(?:_[a-z0-9]+){4,})`", sec14)
    if len(cited) < 20:
        fails.append(f"14 forms.md's As built section parsed {len(cited)} test names; the reader of it is broken")
    for f, n in cited:
        hay = tests_rs.get(f, "") if f else alltests
        if not re.search(rf"\bfn\s+{n}\s*\(", hay):
            fails.append(f"14 forms.md cites `{(f + '::') if f else ''}{n}`, which no test file defines")
for n in sorted(set(re.findall(r"`([a-z][a-z0-9]*(?:_[a-z0-9]+){4,})`", claude))):
    pass  # check 1

# 15. (e) ModeNote's doc in the present tense.
mn = db.find("struct ModeNote {")
mdoc = db[max(0, db.rfind("}", 0, mn)):mn] if mn >= 0 else ""
if "a note held whole in one cell is copied in and out" not in mdoc:
    fails.append("15 ModeNote's doc does not say what a note held whole in one cell costs, in the present")
if re.search(r"\bwas copied\b|\bwas held\b|\bused to\b", mdoc):
    fails.append("15 ModeNote's doc carries history")

# 16. (b) verify_row_by_piece's doc: every byte of the section, not "whole".
vrp = blob_rs.find("fn verify_row_by_piece")
vdoc16 = re.sub(r"\s*///\s*", " ", blob_rs[max(0, blob_rs.rfind("\n}\n", 0, vrp)):vrp]) if vrp >= 0 else ""
if "ask for every byte of the section at every open" not in vdoc16:
    fails.append("16 verify_row_by_piece's doc does not say it would ask for every byte of the section")
if "section whole" in vdoc16:
    fails.append("16 verify_row_by_piece's doc says 'the section whole' where it means every byte of it")

for f in fails:
    print("FAIL", f)
if fails:
    print(f"docs: red ({len(fails)})")
    sys.exit(1)
print("docs: green")
