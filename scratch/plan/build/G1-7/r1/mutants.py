# Round 1's mutants. Each: (id, description, [(file, old, new)], killer).
# Applied one at a time to the working tree, the killer run, the verdict
# appended to results.txt at once, every file put back from keep/r1 and
# checked by its sha256 before the next.
import subprocess, sys, hashlib, os, shutil
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = "/home/user/supdb"
KEEP = os.path.join(HERE, "..", "keep", "r1")
RES = os.path.join(HERE, "results.txt")
TMP = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-7"
SEAL = "sh scripts/seal-red.sh"
SRC = "cargo test --profile checked --test source -- seal"
LENT = [
    ("src/ordindex.rs", "map.lent(lend::Token::new())", "map.lent()"),
    ("src/ordindex.rs", ".lent(lend::Token::new())\n            .get(HEADER", ".lent()\n            .get(HEADER"),
    ("src/db/zfile.rs", "        let bytes: &[u8] = map.lent(lend::Token::new());\n        let (trailer", "        let bytes: &[u8] = map.lent();\n        let (trailer"),
    ("src/db/zfile.rs", "self.map.lent(lend::Token::new())", "self.map.lent()"),
    ("src/db/zfile.rs", "    let bytes: &[u8] = map.lent(lend::Token::new());\n    let Ok(body)", "    let bytes: &[u8] = map.lent();\n    let Ok(body)"),
    ("src/db/zfile.rs", "StrideBody::open(map.lent(lend::Token::new()))", "StrideBody::open(map.lent())"),
    ("src/bytes.rs", "pub(crate) fn lent<T: whole::Reader>(&self, _: T) -> &[u8] {", "pub(crate) fn lent(&self) -> &[u8] {"),
]
ZF_ANCHOR = "use std::sync::Arc;\n"
M = [
 ("R1", "lent without its token (the plan's mutant): the signature and every caller", LENT, SEAL),
 ("R2", "ordindex's Token::new widened to pub(crate)",
  [("src/ordindex.rs", "pub(in crate::ordindex) fn new()", "pub(crate) fn new()")], SEAL),
 ("R2s", "ordindex's Token::new widened to pub(crate), the source test",
  [("src/ordindex.rs", "pub(in crate::ordindex) fn new()", "pub(crate) fn new()")], SRC),
 ("R3", "zfile's Token::new widened to pub(crate)",
  [("src/db/zfile.rs", "pub(in crate::db::zfile) fn new()", "pub(crate) fn new()")], SEAL),
 ("R4", "the readers' trait's module made pub(crate)",
  [("src/bytes.rs", "mod whole {\n    pub trait Reader {}", "pub(crate) mod whole {\n    pub trait Reader {}")], SEAL),
 ("X1", "a call of lend_at in zfile with a token made by unsafe code",
  [("src/db/zfile.rs", ZF_ANCHOR, ZF_ANCHOR + "#[allow(dead_code)]\nfn conjured(m: &MmapBytes) -> Option<&[u8]> {\n    m.lend_at(0, 1, unsafe { std::mem::zeroed() })\n}\n")], SRC),
 ("X1b", "the same with transmute",
  [("src/db/zfile.rs", ZF_ANCHOR, ZF_ANCHOR + "#[allow(dead_code)]\nfn conjured(m: &MmapBytes) -> Option<&[u8]> {\n    m.lend_at(0, 1, unsafe { core::mem::transmute(()) })\n}\n")], SRC),
 ("X2", "blob hands a token out (a function), and zfile calls lend_at with it",
  [("src/blob.rs", "/// A section of the file: lent by the source", "pub(crate) fn token() -> lend::Token {\n    lend::Token::new()\n}\n\n/// A section of the file: lent by the source"),
   ("src/db/zfile.rs", ZF_ANCHOR, ZF_ANCHOR + "#[allow(dead_code)]\nfn handed(m: &MmapBytes) -> Option<&[u8]> {\n    m.lend_at(0, 1, crate::blob::token())\n}\n")], SRC),
 ("X2b", "blob gives the token Default, and zfile calls lend_at with Default::default()",
  [("src/blob.rs", "/// A section of the file: lent by the source", "impl Default for lend::Token {\n    fn default() -> Self {\n        lend::Token::new()\n    }\n}\n\n/// A section of the file: lent by the source"),
   ("src/db/zfile.rs", ZF_ANCHOR, ZF_ANCHOR + "#[allow(dead_code)]\nfn handed(m: &MmapBytes) -> Option<&[u8]> {\n    m.lend_at(0, 1, Default::default())\n}\n")], SRC),
 ("X2c", "blob gives the token Default (impl), seal-red",
  [("src/blob.rs", "/// A section of the file: lent by the source", "impl Default for lend::Token {\n    fn default() -> Self {\n        lend::Token::new()\n    }\n}\n\n/// A section of the file: lent by the source")], SEAL),
 ("X3", "a new reader of a mapping as it lies in zfile, with its token",
  [("src/db/zfile.rs", ZF_ANCHOR, ZF_ANCHOR + "#[allow(dead_code)]\nfn another(m: &MmapBytes) -> Option<usize> {\n    let all = m.lent(lend::Token::new());\n    if all.is_empty() {\n        return None;\n    }\n    Some(all.len())\n}\n")], SRC),
 ("X4", "M5 (touch bounded by the file's length) with the vault test's second assertion taken out: its first alone",
  [("src/bytes.rs", "        let end = off.saturating_add(len as u64).min(self.lend);\n        let mut sum = 0u64;", "        let end = off.saturating_add(len as u64).min(self.len);\n        let mut sum = 0u64;"),
   ("src/db/vault.rs", '''            assert_eq!(
                m.touch(0, body.len()),
                0,
                "a paged mapping is not touched, its pages present or not"
            );
''', "")],
  "cargo test --profile checked --lib -- a_kept_file_lives_in_memory_and_reaches_the_directory_as_an_envelope"),
 ("X5", "derive(Clone, Copy, Default) on blob's Token, seal-red",
  [("src/blob.rs", "pub(crate) mod lend {\n    pub struct Token(());", "pub(crate) mod lend {\n    #[derive(Clone, Copy, Default)]\n    pub struct Token(());")], SEAL),
 ("X5d", "derive(Clone, Copy, Default) on blob's Token, the doctests",
  [("src/blob.rs", "pub(crate) mod lend {\n    pub struct Token(());", "pub(crate) mod lend {\n    #[derive(Clone, Copy, Default)]\n    pub struct Token(());")], "cargo test --profile checked --doc"),
 ("X5s", "derive(Clone, Copy, Default) on blob's Token, the source test",
  [("src/blob.rs", "pub(crate) mod lend {\n    pub struct Token(());", "pub(crate) mod lend {\n    #[derive(Clone, Copy, Default)]\n    pub struct Token(());")], SRC),
 ("X6", "derive(Default) on bytes::Base, seal-red",
  [("src/bytes.rs", "pub(crate) struct Base(());", "#[derive(Default)]\npub(crate) struct Base(());")], SEAL),
 ("X6b", "impl Base { pub(crate) fn new() }",
  [("src/bytes.rs", "pub(crate) struct Base(());", "pub(crate) struct Base(());\n#[cfg(not(target_family = \"wasm\"))]\nimpl Base {\n    #[allow(dead_code)]\n    pub(crate) fn new() -> Base {\n        Base(())\n    }\n}")], SRC),
 ("X7", "records_by_descriptor: the Whole arm answering None",
  [("src/db.rs", "            crate::blob::Records::Whole(..) => {\n                raw.record_region_place().map(|(at, len)| (at, len as u64))\n            }", "            crate::blob::Records::Whole(..) => None,")],
  "cargo test --profile checked --no-fail-fast --test zpages --lib -- drop"),
 ("X8a", "touch steps from off + page (the first page of each span untouched)",
  [("src/bytes.rs", "        let mut at = off;\n        while at < end {", "        let mut at = off + page;\n        while at < end {")],
  "cargo test --profile checked --lib -- a_touch_reads_each_page a_warm_reads_the_cold_file_once"),
 ("X8b", "touch clamped to lend less a page",
  [("src/bytes.rs", "        let end = off.saturating_add(len as u64).min(self.lend);\n        let mut sum = 0u64;", "        let end = off.saturating_add(len as u64).min(self.lend.saturating_sub(page));\n        let mut sum = 0u64;")],
  "cargo test --profile checked --lib -- a_touch_reads_each_page a_warm_reads_the_cold_file_once"),
 ("X9a", "seal-red.sh: the home phase's status check taken out",
  [("scripts/seal-red.sh", '''if [ "$status" != 0 ]; then
  printf '%s\\n' "$out"
  fail "a line does not compile in the module allowed to make its call (cargo check answered $status)"
fi''', "")], "sh scripts/seal-red-red.sh"),
 ("X9b", "seal-red.sh: the count of lines placed abroad not compared",
  [("scripts/seal-red.sh", ''' || [ "$abroad" != "$listed" ]''', "")], "sh scripts/seal-red-red.sh"),
 ("O1", "blob's token admitted as a reader of a mapping whole",
  [("src/bytes.rs", "impl whole::Reader for StrideLendToken {}", "impl whole::Reader for StrideLendToken {}\n#[cfg(not(target_family = \"wasm\"))]\nimpl whole::Reader for LendToken {}")], SRC),
 ("O2", "a blanket impl of the readers' trait (any value lends a mapping whole)",
  [("src/bytes.rs", "impl whole::Reader for StrideLendToken {}", "impl whole::Reader for StrideLendToken {}\n#[cfg(not(target_family = \"wasm\"))]\nimpl whole::Reader for () {}")], SEAL),
 ("O2s", "a blanket impl of the readers' trait, the source test",
  [("src/bytes.rs", "impl whole::Reader for StrideLendToken {}", "impl whole::Reader for StrideLendToken {}\n#[cfg(not(target_family = \"wasm\"))]\nimpl<T> whole::Reader for (T,) {}")], SRC),
 ("O3", "zfile's token with a public field",
  [("src/db/zfile.rs", "pub(crate) mod lend {\n    pub struct Token(());", "pub(crate) mod lend {\n    pub struct Token(pub ());")], SEAL),
 ("O4", "the source test's check of the calls' scopes taken out",
  [("tests/source.rs", '''                    Some((_, scopes)) if !scopes.contains(&scope.as_str()) => errs.push(format!(
                        "{f} ({scope}): `{t}`: a caller of `{call}` that is not listed (SEAL_CALLS)"
                    )),
''', "")], SRC),
 ("O5", "the source test's check of a token made apart from its call taken out",
  [("tests/source.rs", '''            if makes > spent && !def {''', '''            if makes > spent && !def && false {''')], SRC),
]

def sha(p):
    return hashlib.sha256(open(p, "rb").read()).hexdigest()

env = dict(os.environ, TMPDIR=TMP)
only = sys.argv[1:]
for mid, desc, edits, cmd in M:
    if only and mid not in only:
        continue
    files = sorted({f for f, _, _ in edits})
    ok = True
    for f, old, new in edits:
        p = os.path.join(ROOT, f)
        s = open(p).read()
        if s.count(old) != 1:
            ok = False
            open(RES, "a").write(f"{mid}\t{desc}\tNOT APPLIED ({f}: old text found {s.count(old)} times)\n")
            break
        open(p, "w").write(s.replace(old, new))
    if ok:
        log = os.path.join(HERE, "mut", mid + ".log")
        os.makedirs(os.path.dirname(log), exist_ok=True)
        r = subprocess.run(cmd, shell=True, cwd=ROOT, env=env, stdout=open(log, "w"), stderr=subprocess.STDOUT)
        verdict = "KILLED" if r.returncode != 0 else "SURVIVED"
    back = True
    for f in files:
        p = os.path.join(ROOT, f)
        shutil.copy(os.path.join(KEEP, f), p)
        back = back and sha(p) == sha(os.path.join(KEEP, f))
    if ok:
        open(RES, "a").write(f"{mid}\t{desc}\t{verdict} (exit {r.returncode}) by: {cmd}\tput back: {'sum ok' if back else 'SUM MISMATCH'}\n")
    if not back:
        sys.exit(3)
    shutil.rmtree(TMP, ignore_errors=True)
    os.makedirs(TMP, exist_ok=True)
