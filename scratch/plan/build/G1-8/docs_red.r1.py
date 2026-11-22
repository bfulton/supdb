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

for f in fails:
    print("FAIL", f)
if fails:
    print(f"docs: red ({len(fails)})")
    sys.exit(1)
print("docs: green")
