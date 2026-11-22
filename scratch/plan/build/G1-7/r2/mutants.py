# Round 2's mutants. Each: (id, description, [(file, old, new)], [commands]).
# Applied one at a time; every command run in order, each status kept; the
# verdict is the LAST command's status (nonzero: killed). Files put back
# from the copy taken before the edit and checked by sha256 against it.
import subprocess, sys, hashlib, os
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = "/home/user/supdb"
RES = os.path.join(HERE, "results.txt")
TMP = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-7"
ENV = dict(os.environ, TMPDIR=TMP)
SRC = "cargo test --profile checked --test source -- seal"
CLIPPY = "cargo clippy --profile checked --lib -- -D warnings"
ORD = "use std::path::Path;\n"
ZF = "use std::sync::Arc;\n"
BL = "/// A section of the file: lent by the source"
DB = "use std::cell::UnsafeCell;\n"
BODY = '        let body = b"the fence key-00750 and a value nobody may read".repeat(300);\n'
PAGED = "            ptr: pager.base(Base(())),\n            lend: 0,\n"
M = [
 ("E1", "ordindex hands a token out by a qualified path; db.rs borrows a mapping whole by type with it",
  [("src/ordindex.rs", ORD, ORD + "#[allow(dead_code)]\npub(crate) fn hand() -> lend::Token {\n    <lend::Token>::new()\n}\n"),
   ("src/db.rs", DB, DB + "#[allow(dead_code)]\nfn sneak(m: &crate::bytes::MmapBytes) -> usize {\n    crate::bytes::MmapBytes::lent(m, crate::ordindex::hand()).len()\n}\n")],
  [CLIPPY, SRC]),
 ("E2", "zfile: an unlisted reader by type with a qualified token",
  [("src/db/zfile.rs", ZF, ZF + "#[allow(dead_code)]\nfn another(m: &MmapBytes) -> usize {\n    MmapBytes::lent(m, <lend::Token>::new()).len()\n}\n")],
  [CLIPPY, SRC]),
 ("E3", "blob: a renamed import of the token and lend_at through a function value",
  [("src/blob.rs", BL, "use lend::Token as Tk;\n#[allow(dead_code)]\nfn lend_any<B: Bytes>(src: &B) -> Option<&[u8]> {\n    let f = <B as Bytes>::lend_at;\n    f(src, 0, 1, Tk::new())\n}\n\n" + BL)],
  [CLIPPY, SRC]),
 ("E4", "tests/blob.rs: lend_at with a token made by mem::zeroed",
  [("tests/blob.rs", "use supdb::{Blob, BlobOptions, SegmentOptions};\n", "use supdb::{Blob, BlobOptions, SegmentOptions};\n\n#[allow(dead_code)]\nfn conjured<B: Bytes>(b: &B) -> Option<&[u8]> {\n    b.lend_at(0, 1, unsafe { std::mem::zeroed() })\n}\n")],
  ["cargo clippy --profile checked --test blob -- -D warnings", SRC]),
 ("E5", "the vault fixture zeroed at every page start, and a paged mapping lending whole",
  [("src/db/vault.rs", BODY, BODY.replace("let body", "let mut body") + "        for i in (0..body.len()).step_by(crate::bytes::os_page_size()) {\n            body[i] = 0;\n        }\n"),
   ("src/bytes.rs", PAGED, "            ptr: pager.base(Base(())),\n            lend: pager.size(),\n")],
  ["cargo test --profile checked --lib -- a_kept_file_lives_in_memory_and_reaches_the_directory_as_an_envelope"]),
 ("E5b", "a paged mapping lending whole, the fixture as it is",
  [("src/bytes.rs", PAGED, "            ptr: pager.base(Base(())),\n            lend: pager.size(),\n")],
  ["cargo test --profile checked --lib -- a_kept_file_lives_in_memory_and_reaches_the_directory_as_an_envelope"]),
 ("E5c", "the vault fixture zeroed at every page start, the paged mapping as it is (the premise alone)",
  [("src/db/vault.rs", BODY, BODY.replace("let body", "let mut body") + "        for i in (0..body.len()).step_by(crate::bytes::os_page_size()) {\n            body[i] = 0;\n        }\n")],
  ["cargo test --profile checked --lib -- a_kept_file_lives_in_memory_and_reaches_the_directory_as_an_envelope"]),
 ("T1", "MmapBytes::touch: at += page / 2",
  [("src/bytes.rs", "            at += page;\n        }\n        sum\n", "            at += page / 2;\n        }\n        sum\n")],
  ["cargo test --profile checked --lib -- a_touch_reads_each_page a_warm_reads_the_cold_file_once"]),
 ("W1", "warm_file touches only the span's first page",
  [("src/db/zfile.rs", "sum = sum.wrapping_add(map.touch(off as u64, span));", "sum = sum.wrapping_add(map.touch(off as u64, span.min(page)));")],
  ["cargo test --profile checked --lib -- a_warm_reads_the_cold_file_once"]),
 ("D1", "records_by_descriptor: the Pieces arm answers the region's place",
  [("src/db.rs", "            crate::blob::Records::Pieces(..) => None,\n", "            crate::blob::Records::Pieces(..) => raw.record_region_place().map(|(at, len)| (at, len as u64)),\n")],
  ["cargo test --profile checked --no-fail-fast --test zpages --test cipher -- cipher"]),
 ("B1", "Blob::records: the pieces asked before the whole lend",
  [("src/blob.rs", "        if let Some((sec, idx)) = self.flat_lent() {\n            return idx.regions(sec).map(|(r, d)| Records::Whole(r, d));\n        }\n        let (sec, idx) = self.pieces()?;\n        idx.regions(sec).map(|(r, d)| Records::Pieces(r, d))\n",
    "        if let Some((sec, idx)) = self.pieces() {\n            return idx.regions(sec).map(|(r, d)| Records::Pieces(r, d));\n        }\n        let (sec, idx) = self.flat_lent()?;\n        idx.regions(sec).map(|(r, d)| Records::Whole(r, d))\n")],
  ["cargo test --profile checked --no-fail-fast --lib -- zfile", "cargo test --profile checked --no-fail-fast --test zpages -- cipher drop warm"]),
 ("S1", "seal-red-red.sh: case_'s words check deleted",
  [("scripts/seal-red-red.sh", '''  printf '%s\\n' "$out" | grep -q "$2" || {
    printf '%s\\n' "$out" | tail -20
    fail "$1: seal-red.sh failed without saying \\"$2\\""
  }
''', "")],
  ["sh scripts/seal-red-red.sh"]),
 ("O1", "seal_names: a `use` item's `lend` not counted (in_use dropped)",
  [("tests/source.rs", '"lend" if post == "::" || in_use || (n > 0 && ws[n - 1].1 == "mod") => toks += 1,', '"lend" if post == "::" || (n > 0 && ws[n - 1].1 == "mod") => toks += 1,')],
  [SRC]),
 ("O2", "seal_check: a listed call's token names not counted against its calls",
  [("tests/source.rs", "            if made != calls.len() || toks != calls.len() {", "            if made != calls.len() {")],
  [SRC]),
 ("O3", "outside_sources: the suite not read",
  [("tests/source.rs", '    for d in ["tests", "examples", "bench"] {', '    for d in ["tests", "examples"] {')],
  [SRC]),
 ("O4", "seal_names: `lent` reached by a path (`::lent`) not counted",
  [("tests/source.rs", '"lent" if !def && (path || (dot && (post.starts_with(\'(\') || post == "::"))) => {', '"lent" if !def && dot && (post.starts_with(\'(\') || post == "::") => {')],
  [SRC]),
 ("O6", "seal_names: the stride form's re-export's name not counted",
  [("tests/source.rs", '"LendToken" | "OrdLendToken" | "StrideLendToken" => toks += 1,', '"LendToken" | "OrdLendToken" => toks += 1,')],
  [SRC]),
 ("O5", "seal_names: a re-export's name not counted",
  [("tests/source.rs", '"LendToken" | "OrdLendToken" | "StrideLendToken" => toks += 1,', '"LendToken" | "StrideLendToken" => toks += 1,')],
  [SRC]),
]
only = set(sys.argv[1:])
def sha(p):
    return hashlib.sha256(open(p, "rb").read()).hexdigest()
for mid, desc, edits, cmds in M:
    if only and mid not in only:
        continue
    files = sorted({f for f, _, _ in edits})
    orig = {f: open(os.path.join(ROOT, f), "rb").read() for f in files}
    sums = {f: hashlib.sha256(orig[f]).hexdigest() for f in files}
    ok = True
    for f, old, new in edits:
        p = os.path.join(ROOT, f)
        t = open(p).read()
        if t.count(old) != 1:
            print(mid, "edit does not apply once:", f, t.count(old)); ok = False; break
        open(p, "w").write(t.replace(old, new, 1))
    statuses = []
    log = os.path.join(HERE, "mut", mid + ".log")
    if ok:
        with open(log, "w") as lf:
            for c in cmds:
                lf.write("$ " + c + "\n"); lf.flush()
                r = subprocess.run(c, shell=True, cwd=ROOT, env=ENV, stdout=lf, stderr=subprocess.STDOUT)
                lf.write("STATUS %d\n" % r.returncode); lf.flush()
                statuses.append(r.returncode)
    for f in files:
        open(os.path.join(ROOT, f), "wb").write(orig[f])
        assert sha(os.path.join(ROOT, f)) == sums[f], f
    verdict = "NOT-APPLIED" if not ok else ("killed" if statuses[-1] != 0 else "SURVIVED")
    with open(RES, "a") as r:
        r.write("%s\t%s\t%s\t%s\n" % (mid, verdict, statuses, desc))
    print(mid, verdict, statuses, flush=True)
print("EXIT-RUNNER")
